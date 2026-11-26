use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::ast::*;
use crate::error::{RSharpError, Result};

/// Raw source of `rsharp-prelude` (single source of truth for embedded standalone builds).
const RAW_PRELUDE_RS: &str = include_str!("../../rsharp-prelude/src/lib.rs");

/// Controls how the R# standard prelude is emitted in generated Rust.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PreludeMode {
    /// Embed `mod rsharp_prelude` directly in the generated `.rs` file for standalone `rustc` builds.
    #[default]
    Embedded,
    /// Emit `use rsharp_prelude::*;` for Cargo crates depending on `rsharp-prelude`.
    CrateImport,
    /// Emit pure Rust with no prelude import or module.
    None,
}

/// Options controlling R# -> Rust transpilation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TranspileOptions {
    pub prelude_mode: PreludeMode,
}

/// Transpiles an R# `FileAst` to idiomatic Rust 2024 source code.
pub fn generate_rust(file: &FileAst, options: TranspileOptions) -> Result<String> {
    let mut codegen = RustCodegen::new(options);
    codegen.emit_file(file);
    if let Some(err) = codegen.first_err.borrow_mut().take() {
        return Err(err);
    }
    Ok(codegen.out)
}

#[derive(Debug, Clone, Default)]
struct VarInfo {
    struct_path: Option<String>,
    tuple_fields: Option<HashMap<String, usize>>,
}

struct RustCodegen {
    options: TranspileOptions,
    out: String,
    indent: usize,
    known_paths: HashSet<String>,
    known_types: HashSet<String>,
    known_values: HashSet<String>,
    enum_variants: HashMap<String, Vec<String>>,
    shadowed_expr_aliases: HashSet<String>,
    scopes: Vec<HashMap<String, VarInfo>>,
    current_self_type: Option<String>,
    in_const_fn: bool,
    first_err: Rc<RefCell<Option<RSharpError>>>,
}

impl RustCodegen {
    fn new(options: TranspileOptions) -> Self {
        let mut known_paths = HashSet::new();
        for name in [
            "std",
            "core",
            "alloc",
            "crate",
            "super",
            "Self",
            "Vec",
            "List",
            "VecDeque",
            "Queue",
            "HashMap",
            "Dictionary",
            "HashSet",
            "BTreeMap",
            "SortedDictionary",
            "BTreeSet",
            "SortedSet",
            "LinkedList",
            "BinaryHeap",
            "PriorityQueue",
            "String",
            "string",
            "Option",
            "Result",
            "Box",
            "Rc",
            "Arc",
            "Mutex",
            "RwLock",
            "Pin",
            "Cell",
            "RefCell",
            "Some",
            "None",
            "Ok",
            "Err",
        ] {
            known_paths.insert(name.to_string());
        }
        Self {
            options,
            out: String::new(),
            indent: 0,
            known_paths,
            known_types: HashSet::new(),
            known_values: HashSet::new(),
            enum_variants: HashMap::new(),
            shadowed_expr_aliases: HashSet::new(),
            scopes: vec![HashMap::new()],
            current_self_type: None,
            in_const_fn: false,
            first_err: Rc::new(RefCell::new(None)),
        }
    }

    fn record_err(&self, err: RSharpError) {
        self.first_err.borrow_mut().get_or_insert(err);
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn declare_var(&mut self, name: &str, ty: &Type) {
        let info = var_info_from_type(ty);
        let idx = self.scopes.len() - 1;
        self.scopes[idx].insert(name.to_string(), info);
    }

    fn lookup_var(&self, name: &str) -> Option<&VarInfo> {
        for scope in self.scopes.iter().rev() {
            if let Some(info) = scope.get(name) {
                return Some(info);
            }
        }
        None
    }

    fn is_local_var(&self, name: &str) -> bool {
        self.lookup_var(name).is_some()
    }

    fn write_indent(&mut self) {
        for _ in 0..self.indent {
            self.out.push_str("    ");
        }
    }

    fn line(&mut self, s: &str) {
        self.write_indent();
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn register_items(&mut self, items: &[Item]) {
        for item in items {
            match &item.kind {
                ItemKind::Struct(s) => {
                    self.known_paths.insert(s.name.clone());
                    self.known_types.insert(s.name.clone());
                }
                ItemKind::Enum(e) => {
                    self.known_paths.insert(e.name.clone());
                    self.known_types.insert(e.name.clone());
                    self.enum_variants.insert(
                        e.name.clone(),
                        e.variants.iter().map(|v| v.name.clone()).collect(),
                    );
                }
                ItemKind::Union(u) => {
                    self.known_paths.insert(u.name.clone());
                    self.known_types.insert(u.name.clone());
                }
                ItemKind::Trait(t) => {
                    self.known_paths.insert(t.name.clone());
                    self.known_types.insert(t.name.clone());
                }
                ItemKind::Alias(a) => {
                    self.known_paths.insert(a.name.clone());
                    self.known_types.insert(a.name.clone());
                }
                ItemKind::Fn(f) => {
                    self.shadowed_expr_aliases.insert(f.name.clone());
                }
                ItemKind::Const(c) => {
                    self.known_values.insert(c.name.clone());
                }
                ItemKind::Static(s) => {
                    self.known_values.insert(s.name.clone());
                }
                ItemKind::ExternCrate(ec) => {
                    let key = ec.alias.clone().unwrap_or_else(|| ec.name.clone());
                    if key != "_" {
                        self.known_paths.insert(key);
                    }
                }
                ItemKind::Namespace(ns) => {
                    self.known_paths.insert(ns.path[0].clone());
                }
                ItemKind::Using(u) => {
                    register_using_tree(&u.tree, &mut self.known_paths);
                    let mut shadowed = HashSet::new();
                    register_shadowed_using_aliases(&u.tree, &self.enum_variants, &mut shadowed);
                    for s in shadowed {
                        if is_std_collection_alias(&s) {
                            self.known_types.insert(s.clone());
                        }
                        self.shadowed_expr_aliases.insert(s);
                    }
                }
                _ => {}
            }
        }
    }

    fn emit_file(&mut self, file: &FileAst) {
        self.register_items(&file.items);

        if let Some(sb) = &file.shebang {
            self.line(sb);
        }
        self.emit_inner_docs_and_attrs(&file.inner_docs, &file.inner_attrs);

        match self.options.prelude_mode {
            PreludeMode::Embedded => {
                self.line(
                    "#![allow(non_snake_case, non_camel_case_types, unused_imports, unused_parens)]",
                );
                self.emit_embedded_prelude();
                self.line("use rsharp_prelude::*;");
                self.out.push('\n');
            }
            PreludeMode::CrateImport => {
                self.line(
                    "#![allow(non_snake_case, non_camel_case_types, unused_imports, unused_parens)]",
                );
                self.line("use rsharp_prelude::*;");
                self.out.push('\n');
            }
            PreludeMode::None => {
                if file
                    .inner_attrs
                    .iter()
                    .any(|a| matches!(a.name.as_str(), "Deny" | "deny"))
                {
                    self.line("#![allow(unused_parens, unused_braces, unreachable_code)]");
                }
            }
        }

        let mut idx = 0;
        let mut file_ns: Option<Vec<String>> = None;
        if let Some(first) = file.items.first()
            && first.vis == Visibility::Private
            && first.attrs.is_empty()
            && first.docs.is_empty()
            && let ItemKind::Namespace(ns) = &first.kind
            && ns.items.is_none()
            && ns.path.len() > 1
        {
            file_ns = Some(ns.path.clone());
            idx = 1;
        }

        if let Some(ref ns_path) = file_ns {
            for seg in ns_path {
                self.line(&format!("pub mod {} {{", fmt_rust_ident(seg)));
                self.indent += 1;
                if self.options.prelude_mode != PreludeMode::None {
                    self.line("use super::*;");
                }
            }
        }

        for item in &file.items[idx..] {
            self.emit_item(item);
        }

        if let Some(ref ns_path) = file_ns {
            for _ in ns_path {
                self.indent -= 1;
                self.line("}");
            }
        }
    }

    fn emit_embedded_prelude(&mut self) {
        let body = RAW_PRELUDE_RS
            .split("#[cfg(test)]")
            .next()
            .unwrap_or(RAW_PRELUDE_RS);
        self.line("#[allow(dead_code, non_snake_case, unused_imports)]");
        self.line("mod rsharp_prelude {");
        for raw_line in body.lines() {
            let trimmed = raw_line.trim_start();
            if trimmed.starts_with("//!") || trimmed.starts_with("#![") {
                continue;
            }
            if raw_line.is_empty() {
                self.out.push('\n');
            } else {
                self.out.push_str("    ");
                self.out.push_str(raw_line);
                self.out.push('\n');
            }
        }
        self.line("}");
    }

    fn emit_inner_docs_and_attrs(&mut self, docs: &[String], attrs: &[Attribute]) {
        for d in docs {
            if d.is_empty() {
                self.line("//!");
            } else {
                self.line(&format!("//! {}", d));
            }
        }
        for a in attrs {
            let mut inner_a = a.clone();
            inner_a.inner = true;
            self.line(&format_attr(&inner_a));
        }
    }

    fn emit_docs_and_attrs(&mut self, docs: &[String], attrs: &[Attribute]) {
        for d in docs {
            if d.is_empty() {
                self.line("///");
            } else {
                self.line(&format!("/// {}", d));
            }
        }
        for a in attrs {
            self.line(&format_attr(a));
        }
    }

    fn emit_item(&mut self, item: &Item) {
        self.emit_docs_and_attrs(&item.docs, &item.attrs);
        let vis = format_vis(&item.vis);
        match &item.kind {
            ItemKind::Namespace(ns) => {
                if let Some(items) = &ns.items {
                    for seg in &ns.path {
                        self.line(&format!("{}mod {} {{", vis, fmt_rust_ident(seg)));
                        self.indent += 1;
                    }
                    self.emit_inner_docs_and_attrs(&ns.inner_docs, &ns.inner_attrs);
                    self.register_items(items);
                    for sub in items {
                        self.emit_item(sub);
                    }
                    for _ in &ns.path {
                        self.indent -= 1;
                        self.line("}");
                    }
                } else {
                    let name = ns.path.join("_");
                    self.line(&format!("{}mod {};", vis, fmt_rust_ident(&name)));
                }
            }
            ItemKind::Using(u) => {
                self.line(&format!("{}use {};", vis, format_using_tree(&u.tree)));
            }
            ItemKind::ExternCrate(ec) => {
                let as_clause = ec
                    .alias
                    .as_ref()
                    .map(|a| format!(" as {}", fmt_rust_ident(a)))
                    .unwrap_or_default();
                self.line(&format!(
                    "{}extern crate {}{};",
                    vis,
                    fmt_rust_ident(&ec.name),
                    as_clause
                ));
            }
            ItemKind::Struct(s) => self.emit_struct(&vis, s),
            ItemKind::Enum(e) => self.emit_enum(&vis, e),
            ItemKind::Union(u) => self.emit_union(&vis, u),
            ItemKind::Trait(t) => self.emit_trait(&vis, t),
            ItemKind::Impl(im) => self.emit_impl(im),
            ItemKind::Alias(a) => {
                let gens = self.fmt_generics(&a.generics, true);
                let wh = self.fmt_where(&a.where_clause);
                let target = self.fmt_type(&a.target);
                self.line(&format!(
                    "{}type {}{}{} = {};",
                    vis,
                    fmt_rust_ident(&a.name),
                    gens,
                    wh,
                    target
                ));
            }
            ItemKind::Const(c) => {
                let ty = self.fmt_type(&c.ty);
                let val = self.fmt_expr(&c.value);
                self.line(&format!(
                    "{}const {}: {} = {};",
                    vis,
                    fmt_rust_ident(&c.name),
                    ty,
                    val
                ));
            }
            ItemKind::Static(s) => {
                let safety = if s.is_safe {
                    "safe "
                } else if s.is_unsafe {
                    "unsafe "
                } else {
                    ""
                };
                let mut_kw = if s.is_mut { "mut " } else { "" };
                let ty = self.fmt_type(&s.ty);
                let name = fmt_rust_ident(&s.name);
                match &s.value {
                    Some(v) => {
                        let val = self.fmt_expr(v);
                        self.line(&format!(
                            "{}{}static {}{}: {} = {};",
                            vis, safety, mut_kw, name, ty, val
                        ));
                    }
                    None => {
                        self.line(&format!(
                            "{}{}static {}{}: {};",
                            vis, safety, mut_kw, name, ty
                        ));
                    }
                }
            }
            ItemKind::Fn(f) => self.emit_fn(&vis, f),
            ItemKind::ExternBlock(eb) => {
                let abi = eb
                    .abi
                    .as_ref()
                    .map(|a| format!(" \"{}\"", a))
                    .unwrap_or_default();
                let _ = eb.is_unsafe;
                self.line(&format!("unsafe extern{} {{", abi));
                self.indent += 1;
                self.emit_inner_docs_and_attrs(&eb.inner_docs, &eb.inner_attrs);
                for sub in &eb.items {
                    self.emit_item(sub);
                }
                self.indent -= 1;
                self.line("}");
            }
            ItemKind::MacroDecl(m) => {
                let name = fmt_rust_ident(&m.name);
                match m.delim {
                    MacroDelim::Brace => {
                        self.line(&format!("macro_rules! {} {{ {} }}", name, m.body))
                    }
                    MacroDelim::Paren => {
                        self.line(&format!("macro_rules! {} ( {} );", name, m.body))
                    }
                    MacroDelim::Bracket => {
                        self.line(&format!("macro_rules! {} [ {} ];", name, m.body))
                    }
                }
            }
            ItemKind::MacroCall(mc) => {
                let s = format_macro_call(mc);
                if mc.delim == MacroDelim::Brace {
                    self.line(&s);
                } else {
                    self.line(&format!("{};", s));
                }
            }
            ItemKind::InlineRust(raw) => {
                for l in raw.trim().lines() {
                    self.line(l);
                }
            }
        }
    }

    fn emit_struct(&mut self, vis: &str, s: &StructDecl) {
        let gens = self.fmt_generics(&s.generics, true);
        let wh = self.fmt_where(&s.where_clause);
        let sname = fmt_rust_ident(&s.name);
        match &s.kind {
            StructKind::Unit => {
                self.line(&format!("{}struct {}{}{};", vis, sname, gens, wh));
            }
            StructKind::Tuple(fields) => {
                let mut fstrs = Vec::new();
                for f in fields {
                    let mut p = String::new();
                    for a in &f.attrs {
                        p.push_str(&format_attr(a));
                        p.push(' ');
                    }
                    p.push_str(&format_vis(&f.vis));
                    p.push_str(&self.fmt_type(&f.ty));
                    fstrs.push(p);
                }
                self.line(&format!(
                    "{}struct {}{}({}){};",
                    vis,
                    sname,
                    gens,
                    fstrs.join(", "),
                    wh
                ));
            }
            StructKind::Named(fields) => {
                self.line(&format!("{}struct {}{}{} {{", vis, sname, gens, wh));
                self.indent += 1;
                for f in fields {
                    self.emit_docs_and_attrs(&f.docs, &f.attrs);
                    let fvis = format_vis(&f.vis);
                    let fty = self.fmt_type(&f.ty);
                    self.line(&format!("{}{}: {},", fvis, fmt_rust_ident(&f.name), fty));
                }
                self.indent -= 1;
                self.line("}");
            }
        }

        if !s.methods.is_empty() {
            let impl_gens = self.fmt_generics(&s.generics, false);
            let gen_args = format_generic_args_from_params(&s.generics);
            self.line(&format!("impl{} {}{}{} {{", impl_gens, sname, gen_args, wh));
            let prev_self = self.current_self_type.replace(s.name.clone());
            self.indent += 1;
            for m in &s.methods {
                self.emit_item(m);
            }
            self.indent -= 1;
            self.current_self_type = prev_self;
            self.line("}");
        }
    }

    fn emit_enum(&mut self, vis: &str, e: &EnumDecl) {
        let gens = self.fmt_generics(&e.generics, true);
        let wh = self.fmt_where(&e.where_clause);
        let ename = fmt_rust_ident(&e.name);
        self.line(&format!("{}enum {}{}{} {{", vis, ename, gens, wh));
        self.indent += 1;
        for v in &e.variants {
            self.emit_docs_and_attrs(&v.docs, &v.attrs);
            let vname = fmt_rust_ident(&v.name);
            let disc = match &v.discriminant {
                Some(ex) => format!(" = {}", self.fmt_expr(ex)),
                None => String::new(),
            };
            match &v.kind {
                VariantKind::Unit => {
                    self.line(&format!("{}{},", vname, disc));
                }
                VariantKind::Tuple(fields) => {
                    let mut tys = Vec::new();
                    for f in fields {
                        let mut p = String::new();
                        for a in &f.attrs {
                            p.push_str(&format_attr(a));
                            p.push(' ');
                        }
                        p.push_str(&self.fmt_type(&f.ty));
                        tys.push(p);
                    }
                    self.line(&format!("{}({}){},", vname, tys.join(", "), disc));
                }
                VariantKind::Named(fields) => {
                    self.line(&format!("{} {{", vname));
                    self.indent += 1;
                    for f in fields {
                        self.emit_docs_and_attrs(&f.docs, &f.attrs);
                        let fty = self.fmt_type(&f.ty);
                        self.line(&format!("{}: {},", fmt_rust_ident(&f.name), fty));
                    }
                    self.indent -= 1;
                    self.line(&format!("}}{},", disc));
                }
            }
        }
        self.indent -= 1;
        self.line("}");

        if !e.methods.is_empty() {
            let impl_gens = self.fmt_generics(&e.generics, false);
            let gen_args = format_generic_args_from_params(&e.generics);
            self.line(&format!("impl{} {}{}{} {{", impl_gens, ename, gen_args, wh));
            let prev_self = self.current_self_type.replace(e.name.clone());
            self.indent += 1;
            for m in &e.methods {
                self.emit_item(m);
            }
            self.indent -= 1;
            self.current_self_type = prev_self;
            self.line("}");
        }
    }

    fn emit_union(&mut self, vis: &str, u: &UnionDecl) {
        let gens = self.fmt_generics(&u.generics, true);
        let wh = self.fmt_where(&u.where_clause);
        let uname = fmt_rust_ident(&u.name);
        self.line(&format!("{}union {}{}{} {{", vis, uname, gens, wh));
        self.indent += 1;
        for f in &u.fields {
            self.emit_docs_and_attrs(&f.docs, &f.attrs);
            let fvis = format_vis(&f.vis);
            let fty = self.fmt_type(&f.ty);
            self.line(&format!("{}{}: {},", fvis, fmt_rust_ident(&f.name), fty));
        }
        self.indent -= 1;
        self.line("}");

        if !u.methods.is_empty() {
            let impl_gens = self.fmt_generics(&u.generics, false);
            let gen_args = format_generic_args_from_params(&u.generics);
            self.line(&format!("impl{} {}{}{} {{", impl_gens, uname, gen_args, wh));
            let prev_self = self.current_self_type.replace(u.name.clone());
            self.indent += 1;
            for m in &u.methods {
                self.emit_item(m);
            }
            self.indent -= 1;
            self.current_self_type = prev_self;
            self.line("}");
        }
    }

    fn emit_trait(&mut self, vis: &str, t: &TraitDecl) {
        let unsf = if t.is_unsafe { "unsafe " } else { "" };
        let gens = self.fmt_generics(&t.generics, true);
        let supers = if t.supertraits.is_empty() {
            String::new()
        } else {
            let bs: Vec<_> = t.supertraits.iter().map(|b| self.fmt_bound(b)).collect();
            format!(": {}", bs.join(" + "))
        };
        let wh = self.fmt_where(&t.where_clause);
        self.line(&format!(
            "{}{}trait {}{}{}{} {{",
            vis,
            unsf,
            fmt_rust_ident(&t.name),
            gens,
            supers,
            wh
        ));
        self.indent += 1;
        self.emit_inner_docs_and_attrs(&t.inner_docs, &t.inner_attrs);
        for ti in &t.items {
            match ti {
                TraitItem::Fn {
                    docs, attrs, decl, ..
                } => {
                    self.emit_docs_and_attrs(docs, attrs);
                    self.emit_fn("", decl);
                }
                TraitItem::AssocType {
                    docs,
                    attrs,
                    name,
                    generics,
                    bounds,
                    where_clause,
                    default,
                } => {
                    self.emit_docs_and_attrs(docs, attrs);
                    let ag = self.fmt_generics(generics, true);
                    let b = if bounds.is_empty() {
                        String::new()
                    } else {
                        let bs: Vec<_> = bounds.iter().map(|b| self.fmt_bound(b)).collect();
                        format!(": {}", bs.join(" + "))
                    };
                    let d = match default {
                        Some(ty) => format!(" = {}", self.fmt_type(ty)),
                        None => String::new(),
                    };
                    let aw = self.fmt_where(where_clause);
                    self.line(&format!(
                        "type {}{}{}{}{};",
                        fmt_rust_ident(name),
                        ag,
                        b,
                        d,
                        aw
                    ));
                }
                TraitItem::AssocConst {
                    docs,
                    attrs,
                    ty,
                    name,
                    default,
                } => {
                    self.emit_docs_and_attrs(docs, attrs);
                    let ty_s = self.fmt_type(ty);
                    let d = match default {
                        Some(ex) => format!(" = {}", self.fmt_expr(ex)),
                        None => String::new(),
                    };
                    self.line(&format!("const {}: {}{};", fmt_rust_ident(name), ty_s, d));
                }
                TraitItem::MacroCall { attrs, call: mc } => {
                    for a in attrs {
                        self.line(&format_attr(a));
                    }
                    let s = format_macro_call(mc);
                    if mc.delim == MacroDelim::Brace {
                        self.line(&s);
                    } else {
                        self.line(&format!("{};", s));
                    }
                }
            }
        }
        self.indent -= 1;
        self.line("}");
    }

    fn emit_impl(&mut self, im: &ImplDecl) {
        let unsf = if im.is_unsafe { "unsafe " } else { "" };
        let gens = self.fmt_generics(&im.generics, false);
        let target = self.fmt_type(&im.target_ty);
        let wh = self.fmt_where(&im.where_clause);
        let header = if let Some(tp) = &im.trait_path {
            format!(
                "{}impl{} {} for {}{} {{",
                unsf,
                gens,
                self.fmt_type_path(tp),
                target,
                wh
            )
        } else {
            format!("{}impl{} {}{} {{", unsf, gens, target, wh)
        };
        let prev_self = self.current_self_type.clone();
        self.current_self_type = type_to_struct_name(&im.target_ty);
        self.line(&header);
        self.indent += 1;
        self.emit_inner_docs_and_attrs(&im.inner_docs, &im.inner_attrs);
        for item in &im.items {
            match item {
                ImplItem::Fn {
                    docs,
                    attrs,
                    vis,
                    decl,
                } => {
                    self.emit_docs_and_attrs(docs, attrs);
                    let fvis = if im.trait_path.is_some() {
                        String::new()
                    } else {
                        format_vis(vis)
                    };
                    self.emit_fn(&fvis, decl);
                }
                ImplItem::AssocType {
                    attrs,
                    name,
                    generics,
                    where_clause,
                    ty,
                } => {
                    for a in attrs {
                        self.line(&format_attr(a));
                    }
                    let ag = self.fmt_generics(generics, true);
                    let t = self.fmt_type(ty);
                    let aw = self.fmt_where(where_clause);
                    self.line(&format!(
                        "type {}{} = {}{};",
                        fmt_rust_ident(name),
                        ag,
                        t,
                        aw
                    ));
                }
                ImplItem::AssocConst(it) => self.emit_item(it),
                ImplItem::MacroCall { attrs, call: mc } => {
                    for a in attrs {
                        self.line(&format_attr(a));
                    }
                    let s = format_macro_call(mc);
                    if mc.delim == MacroDelim::Brace {
                        self.line(&s);
                    } else {
                        self.line(&format!("{};", s));
                    }
                }
            }
        }
        self.indent -= 1;
        self.current_self_type = prev_self;
        self.line("}");
    }

    fn emit_fn(&mut self, vis: &str, f: &FnDecl) {
        let mut sig = String::new();
        sig.push_str(vis);
        if f.is_const {
            sig.push_str("const ");
        }
        if f.is_async {
            sig.push_str("async ");
        }
        if f.is_safe {
            sig.push_str("safe ");
        }
        if f.is_unsafe {
            sig.push_str("unsafe ");
        }
        if let Some(abi) = &f.extern_abi {
            sig.push_str(&format!("extern \"{}\" ", abi));
        }
        sig.push_str("fn ");
        sig.push_str(&fmt_rust_ident(&f.name));
        sig.push_str(&self.fmt_generics(&f.generics, false));
        sig.push('(');

        self.push_scope();
        let mut pstrs = Vec::new();
        for p in &f.params {
            pstrs.push(self.fmt_param(p));
        }
        if let Some(vp) = &f.variadic {
            let mut vs = String::new();
            for a in &vp.attrs {
                vs.push_str(&format_attr(a));
                vs.push(' ');
            }
            if let Some(name) = &vp.name {
                self.declare_var(name, &Type::Void);
                let mk = if vp.is_mut { "mut " } else { "" };
                vs.push_str(&format!("{}{}: ...", mk, fmt_rust_ident(name)));
            } else {
                vs.push_str("...");
            }
            pstrs.push(vs);
        }
        sig.push_str(&pstrs.join(", "));
        sig.push(')');

        let effective_ret = unwrap_async_task_type(f.is_async, &f.return_ty);
        if !matches!(effective_ret, Type::Void) {
            sig.push_str(" -> ");
            sig.push_str(&self.fmt_type(effective_ret));
        }
        sig.push_str(&self.fmt_where(&f.where_clause));

        if let Some(body) = &f.body {
            self.line(&format!("{} {{", sig));
            let prev_const = self.in_const_fn;
            self.in_const_fn = f.is_const;
            self.indent += 1;
            self.emit_block_stmts(body);
            self.indent -= 1;
            self.in_const_fn = prev_const;
            self.line("}");
        } else {
            self.line(&format!("{};", sig));
        }
        self.pop_scope();
    }

    fn fmt_param(&mut self, p: &Param) -> String {
        match p {
            Param::Receiver { attrs, kind: rk } => {
                self.declare_var("this", &Type::Void);
                let mut prefix = String::new();
                for a in attrs {
                    prefix.push_str(&format_attr(a));
                    prefix.push(' ');
                }
                let body = match rk {
                    ReceiverKind::Read(life) => {
                        let l = life
                            .as_ref()
                            .map(|n| format!("{} ", fmt_rust_lifetime(n)))
                            .unwrap_or_default();
                        format!("&{}self", l)
                    }
                    ReceiverKind::Write(life) => {
                        let l = life
                            .as_ref()
                            .map(|n| format!("{} ", fmt_rust_lifetime(n)))
                            .unwrap_or_default();
                        format!("&{}mut self", l)
                    }
                    ReceiverKind::Take { is_mut } => {
                        if *is_mut {
                            "mut self".to_string()
                        } else {
                            "self".to_string()
                        }
                    }
                    ReceiverKind::Typed { is_mut, ty } => {
                        let mk = if *is_mut { "mut " } else { "" };
                        format!("{}self: {}", mk, self.fmt_type(ty))
                    }
                };
                format!("{}{}", prefix, body)
            }
            Param::Normal {
                attrs,
                is_mut,
                ty,
                name,
            } => {
                self.declare_var(name, ty);
                let mut prefix = String::new();
                for a in attrs {
                    prefix.push_str(&format_attr(a));
                    prefix.push(' ');
                }
                let m = if *is_mut { "mut " } else { "" };
                format!(
                    "{}{}{}: {}",
                    prefix,
                    m,
                    fmt_rust_ident(name),
                    self.fmt_type(ty)
                )
            }
            Param::Pattern {
                attrs,
                pat,
                explicit_ty,
            } => {
                self.register_pattern_vars(pat);
                let mut prefix = String::new();
                for a in attrs {
                    prefix.push_str(&format_attr(a));
                    prefix.push(' ');
                }
                let pat_s = self.fmt_pattern(pat);
                let Some(ty) = explicit_ty
                    .clone()
                    .or_else(|| infer_param_pattern_type(pat))
                else {
                    self.record_err(RSharpError::codegen(
                        "Could not determine parameter type from pattern",
                    ));
                    return format!("{}{}: _", prefix, pat_s);
                };
                format!("{}{}: {}", prefix, pat_s, self.fmt_type(&ty))
            }
        }
    }

    fn emit_value_block_stmts(&mut self, block: &Block) {
        self.emit_value_block_stmts_with_prefix(block, &[]);
    }

    fn emit_value_block_stmts_with_prefix(&mut self, block: &Block, prefix_lines: &[String]) {
        self.push_scope();
        let prev_shadowed = self.shadowed_expr_aliases.clone();
        self.emit_inner_docs_and_attrs(&block.inner_docs, &block.inner_attrs);
        for line in prefix_lines {
            self.line(line);
        }
        if let Some((last, init)) = block.stmts.split_last()
            && let StmtKind::Return(Some(tail)) = &last.kind
        {
            for st in init {
                self.emit_stmt(st);
            }
            for a in &last.attrs {
                self.line(&format_attr(a));
            }
            let t = self.fmt_expr(tail);
            self.line(&t);
        } else {
            for st in &block.stmts {
                self.emit_stmt(st);
            }
        }
        self.shadowed_expr_aliases = prev_shadowed;
        self.pop_scope();
    }

    fn emit_block_stmts(&mut self, block: &Block) {
        self.push_scope();
        let prev_shadowed = self.shadowed_expr_aliases.clone();
        self.emit_inner_docs_and_attrs(&block.inner_docs, &block.inner_attrs);
        for stmt in &block.stmts {
            self.emit_stmt(stmt);
        }
        self.shadowed_expr_aliases = prev_shadowed;
        self.pop_scope();
    }

    fn emit_stmt(&mut self, stmt: &Stmt) {
        for a in &stmt.attrs {
            self.line(&format_attr(a));
        }
        match &stmt.kind {
            StmtKind::Local {
                is_mut,
                ty,
                name,
                init,
                ..
            } => {
                let m = if *is_mut { "mut " } else { "" };
                let ty_s = self.fmt_type(ty);
                let init_s = match init {
                    Some(expr) => {
                        let rhs = if is_list_type(ty) {
                            match expr {
                                Expr::ArrayLit(elems) => {
                                    let items: Vec<_> =
                                        elems.iter().map(|e| self.fmt_list_elem_expr(e)).collect();
                                    format!("vec![{}]", items.join(", "))
                                }
                                Expr::ArrayRepeat { value, count } => {
                                    format!(
                                        "vec![{}; {}]",
                                        self.fmt_expr(value),
                                        self.fmt_expr(count)
                                    )
                                }
                                _ => self.fmt_expr(expr),
                            }
                        } else {
                            self.fmt_expr(expr)
                        };
                        format!(" = {}", rhs)
                    }
                    None => String::new(),
                };
                self.declare_var(name, ty);
                let rname = fmt_rust_ident(name);
                // Rust forbids `let x: impl Trait = ...;` (E0562); emit the binding and enforce
                // the trait bounds at compile time via a zero-cost generic check function.
                if let Type::ImplTrait(bounds) = ty {
                    self.line(&format!("let {}{}{};", m, rname, init_s));
                    if let Some(chk) = self.fmt_impl_trait_check(&rname, bounds) {
                        self.line(&chk);
                    }
                } else {
                    self.line(&format!("let {}{}: {}{};", m, rname, ty_s, init_s));
                }
            }
            StmtKind::Destructure {
                pat,
                explicit_ty,
                init,
            } => {
                let pat_s = self.fmt_pattern(pat);
                let rhs = self.fmt_expr(init);
                if let Some(ty) = explicit_ty {
                    self.line(&format!("let {}: {} = {};", pat_s, self.fmt_type(ty), rhs));
                    self.register_pattern_vars(pat);
                } else {
                    self.line(&format!("let {} = {};", pat_s, rhs));
                    self.emit_pattern_type_checks(pat);
                }
            }
            StmtKind::DestructureAssign { pat, value } => {
                let pat_s = self.fmt_assign_pattern(pat);
                let rhs = self.fmt_expr(value);
                self.line(&format!("{} = {};", pat_s, rhs));
            }
            StmtKind::Discard(expr) => {
                let e = self.fmt_expr(expr);
                self.line(&format!("let _ = {};", e));
            }
            StmtKind::Item(item) => {
                self.register_items(std::slice::from_ref(item.as_ref()));
                self.emit_item(item);
            }
            StmtKind::Return(expr) => match expr {
                Some(e) => {
                    let s = self.fmt_expr(e);
                    self.line(&format!("return {};", s));
                }
                None => self.line("return;"),
            },
            StmtKind::Break { label, value } => {
                let mut line = String::from("break");
                if let Some(l) = label {
                    line.push_str(&format!(" {}", fmt_rust_lifetime(l)));
                }
                if let Some(v) = value {
                    line.push(' ');
                    line.push_str(&self.fmt_break_value_expr(label.is_some(), v));
                }
                line.push(';');
                self.line(&line);
            }
            StmtKind::Continue { label } => match label {
                Some(l) => self.line(&format!("continue {};", fmt_rust_lifetime(l))),
                None => self.line("continue;"),
            },
            StmtKind::If(if_stmt) => self.emit_if(if_stmt, false),
            StmtKind::Require {
                expr,
                pat,
                else_block,
            } => {
                let pat_s = self.fmt_pattern(pat);
                let raw_expr = self.fmt_expr(expr);
                let expr_s = if expr_needs_require_parens(expr) {
                    format!("({})", raw_expr)
                } else {
                    raw_expr
                };
                self.line(&format!("let {} = {} else {{", pat_s, expr_s));
                self.indent += 1;
                self.emit_block_stmts(else_block);
                self.indent -= 1;
                self.line("};");
                self.emit_pattern_type_checks(pat);
            }
            StmtKind::While { label, cond, body } => {
                let lbl = label
                    .as_ref()
                    .map(|l| format!("{}: ", fmt_rust_lifetime(l)))
                    .unwrap_or_default();
                self.emit_cond_block(&format!("{}while ", lbl), cond, body);
                self.line("}");
            }
            StmtKind::For {
                label,
                init,
                cond,
                step,
                body,
            } => {
                self.line("{");
                self.indent += 1;
                self.push_scope();
                if let Some(init_stmt) = init {
                    self.emit_stmt(init_stmt);
                }
                if step.is_some() {
                    self.line("let mut __rsharp_first = true;");
                }
                let lbl = label
                    .as_ref()
                    .map(|l| format!("{}: ", fmt_rust_lifetime(l)))
                    .unwrap_or_default();
                self.line(&format!("{}loop {{", lbl));
                self.indent += 1;
                if let Some(step_expr) = step {
                    let step_s = self.fmt_expr(step_expr);
                    self.line("if __rsharp_first {");
                    self.indent += 1;
                    self.line("__rsharp_first = false;");
                    self.indent -= 1;
                    self.line("} else {");
                    self.indent += 1;
                    self.line(&format!("{};", step_s));
                    self.indent -= 1;
                    self.line("}");
                }
                if let Some(cond_expr) = cond {
                    let c = self.fmt_expr(cond_expr);
                    self.line(&format!("if !({}) {{ break; }}", c));
                }
                for st in &body.stmts {
                    self.emit_stmt(st);
                }
                self.indent -= 1;
                self.line("}");
                self.pop_scope();
                self.indent -= 1;
                self.line("}");
            }
            StmtKind::Foreach {
                label,
                pat,
                iter,
                body,
            } => {
                let lbl = label
                    .as_ref()
                    .map(|l| format!("{}: ", fmt_rust_lifetime(l)))
                    .unwrap_or_default();
                let p = self.fmt_pattern(pat);
                let it = self.fmt_expr(iter);
                self.line(&format!("{}for {} in {} {{", lbl, p, it));
                self.indent += 1;
                self.push_scope();
                self.emit_pattern_type_checks(pat);
                for st in &body.stmts {
                    self.emit_stmt(st);
                }
                self.pop_scope();
                self.indent -= 1;
                self.line("}");
            }
            StmtKind::Loop { label, body } => {
                let lbl = label
                    .as_ref()
                    .map(|l| format!("{}: ", fmt_rust_lifetime(l)))
                    .unwrap_or_default();
                self.line(&format!("{}loop {{", lbl));
                self.indent += 1;
                self.emit_block_stmts(body);
                self.indent -= 1;
                self.line("}");
            }
            StmtKind::Match(m) => {
                let trailing_semi = m
                    .arms
                    .iter()
                    .any(|a| matches!(a.body, MatchArmBody::Expr(_)));
                self.emit_match(m, trailing_semi);
            }
            StmtKind::Block(b) => {
                self.line("{");
                self.indent += 1;
                self.emit_block_stmts(b);
                self.indent -= 1;
                self.line("}");
            }
            StmtKind::Empty => {
                self.line(";");
            }
            StmtKind::Unsafe(b) => {
                self.line("unsafe {");
                self.indent += 1;
                self.emit_block_stmts(b);
                self.indent -= 1;
                self.line("}");
            }
            StmtKind::InlineRust(raw) => {
                self.register_inline_rust_locals(raw);
                for l in raw.trim().lines() {
                    self.line(l);
                }
            }
            StmtKind::Expr(expr) => {
                let s = self.fmt_expr(expr);
                self.line(&format!("{};", s));
            }
        }
    }

    fn emit_cond_block(&mut self, prefix: &str, cond: &Condition, body: &Block) {
        match cond {
            Condition::Bool(e) => {
                let s = self.fmt_expr(e);
                self.line(&format!("{}{} {{", prefix, s));
                self.indent += 1;
                self.emit_block_stmts(body);
                self.indent -= 1;
            }
            Condition::IsPattern {
                expr,
                negated: false,
                pat,
            } => {
                let p = self.fmt_pattern(pat);
                let e = self.fmt_expr(expr);
                self.line(&format!("{}let {} = {} {{", prefix, p, e));
                self.indent += 1;
                self.push_scope();
                self.emit_pattern_type_checks(pat);
                for st in &body.stmts {
                    self.emit_stmt(st);
                }
                self.pop_scope();
                self.indent -= 1;
            }
            Condition::IsPattern {
                expr,
                negated: true,
                pat,
            } => {
                let p = self.fmt_pattern(pat);
                let e = self.fmt_expr(expr);
                self.line(&format!("{}!matches!({}, {}) {{", prefix, e, p));
                self.indent += 1;
                self.emit_block_stmts(body);
                self.indent -= 1;
            }
            Condition::LetChain(items) => {
                self.push_scope();
                let mut parts = Vec::new();
                let mut pats = Vec::new();
                for item in items {
                    match item {
                        ChainItem::IsPattern { expr, pat } => {
                            let p = self.fmt_pattern(pat);
                            let e = self.fmt_expr(expr);
                            self.register_pattern_vars(pat);
                            parts.push(format!("let {} = {}", p, e));
                            pats.push(pat);
                        }
                        ChainItem::Bool(e) => {
                            parts.push(self.fmt_expr(e));
                        }
                    }
                }
                self.line(&format!("{}{} {{", prefix, parts.join(" && ")));
                self.indent += 1;
                for pat in pats {
                    self.emit_pattern_type_checks(pat);
                }
                for st in &body.stmts {
                    self.emit_stmt(st);
                }
                self.indent -= 1;
                self.pop_scope();
            }
        }
    }

    fn emit_if(&mut self, if_stmt: &IfStmt, is_else_if: bool) {
        let prefix = if is_else_if { "else if " } else { "if " };
        self.emit_cond_block(prefix, &if_stmt.cond, &if_stmt.then_block);
        match &if_stmt.else_branch {
            Some(ElseBranch::ElseIf(next)) => {
                self.line("}");
                self.emit_if(next, true);
            }
            Some(ElseBranch::Else(eb)) => {
                self.line("} else {");
                self.indent += 1;
                self.emit_block_stmts(eb);
                self.indent -= 1;
                self.line("}");
            }
            None => self.line("}"),
        }
    }

    fn fmt_condition_expr<'a>(
        &self,
        cond: &'a Condition,
        sub: &mut RustCodegen,
        bound_pats: &mut Vec<&'a Pattern>,
    ) -> String {
        match cond {
            Condition::Bool(e) => sub.fmt_expr(e),
            Condition::IsPattern {
                expr,
                negated: false,
                pat,
            } => {
                let p = sub.fmt_pattern(pat);
                let e = sub.fmt_expr(expr);
                sub.register_pattern_vars(pat);
                bound_pats.push(pat);
                format!("let {} = {}", p, e)
            }
            Condition::IsPattern {
                expr,
                negated: true,
                pat,
            } => format!(
                "!matches!({}, {})",
                sub.fmt_expr(expr),
                sub.fmt_pattern(pat)
            ),
            Condition::LetChain(items) => {
                let mut parts = Vec::new();
                for item in items {
                    match item {
                        ChainItem::IsPattern { expr, pat } => {
                            let p = sub.fmt_pattern(pat);
                            let e = sub.fmt_expr(expr);
                            sub.register_pattern_vars(pat);
                            bound_pats.push(pat);
                            parts.push(format!("let {} = {}", p, e));
                        }
                        ChainItem::Bool(e) => {
                            parts.push(sub.fmt_expr(e));
                        }
                    }
                }
                parts.join(" && ")
            }
        }
    }

    fn emit_match(&mut self, m: &MatchExpr, trailing_semi: bool) {
        let scr = self.fmt_expr(&m.scrutinee);
        self.line(&format!("match {} {{", scr));
        self.indent += 1;
        for arm in &m.arms {
            for a in &arm.attrs {
                self.line(&format_attr(a));
            }
            let mut guard_sub = self.child_formatter();
            guard_sub.register_pattern_vars(&arm.pat);
            let mut guard_pats = Vec::new();
            let (p, g) = if let Pattern::Const(ce) = &arm.pat {
                let const_cond = format!("__rsharp_c == const {{ {} }}", self.fmt_expr(ce));
                let combined_guard = match &arm.guard {
                    Some(gc) => {
                        let gs = self.fmt_condition_expr(gc, &mut guard_sub, &mut guard_pats);
                        format!(" if ({}) && {}", const_cond, gs)
                    }
                    None => format!(" if {}", const_cond),
                };
                ("__rsharp_c".to_string(), combined_guard)
            } else {
                let p = self.fmt_pattern(&arm.pat);
                let g = match &arm.guard {
                    Some(gc) => {
                        let gs = self.fmt_condition_expr(gc, &mut guard_sub, &mut guard_pats);
                        format!(" if {}", gs)
                    }
                    None => String::new(),
                };
                (p, g)
            };
            match &arm.body {
                MatchArmBody::Block(b) => {
                    self.line(&format!("{}{} => {{", p, g));
                    self.indent += 1;
                    self.push_scope();
                    self.emit_inner_docs_and_attrs(&b.inner_docs, &b.inner_attrs);
                    self.emit_pattern_type_checks(&arm.pat);
                    for gp in &guard_pats {
                        self.emit_pattern_type_checks(gp);
                    }
                    for st in &b.stmts {
                        self.emit_stmt(st);
                    }
                    self.pop_scope();
                    self.indent -= 1;
                    self.line("}");
                }
                MatchArmBody::Expr(e) => {
                    let mut checks = self.collect_pattern_type_checks(&arm.pat);
                    for gp in &guard_pats {
                        checks.extend(self.collect_pattern_type_checks(gp));
                    }
                    self.push_scope();
                    self.register_pattern_vars(&arm.pat);
                    for gp in &guard_pats {
                        self.register_pattern_vars(gp);
                    }
                    let es = self.fmt_expr(e);
                    self.pop_scope();
                    if checks.is_empty() {
                        self.line(&format!("{}{} => {},", p, g, es));
                    } else {
                        self.line(&format!("{}{} => {{ {} {} }},", p, g, checks.join(" "), es));
                    }
                }
            }
        }
        self.indent -= 1;
        if trailing_semi {
            self.line("};");
        } else {
            self.line("}");
        }
    }

    fn register_inline_rust_locals(&mut self, raw: &str) {
        let trimmed = raw.trim();
        if !trimmed.starts_with("let ") {
            return;
        }
        if let Ok(syn::Stmt::Local(loc)) = syn::parse_str::<syn::Stmt>(trimmed) {
            let mut names = Vec::new();
            collect_syn_pat_idents(&loc.pat, &mut names);
            for name in names {
                self.declare_var(&name, &Type::Void);
            }
        }
    }

    fn register_pattern_vars(&mut self, pat: &Pattern) {
        let mut bindings = Vec::new();
        collect_pattern_bindings(pat, &mut bindings);
        for (name, ty) in bindings {
            self.declare_var(&name, ty.as_ref().unwrap_or(&Type::Void));
        }
    }

    fn fmt_impl_trait_check(&self, rname: &str, bounds: &[TypeBound]) -> Option<String> {
        if self.in_const_fn {
            return None;
        }
        let check_bounds: Vec<String> = bounds
            .iter()
            .filter(|b| {
                !matches!(b, TypeBound::PreciseCapture(_) | TypeBound::Lifetime(_))
                    && !bound_contains_never(b)
            })
            .map(|b| self.fmt_bound(b))
            .collect();
        if check_bounds.is_empty() {
            return None;
        }
        Some(format!(
            "{{ fn __rsharp_impl_check<__T: {}>(_: &__T) {{}} __rsharp_impl_check(&{}); }}",
            check_bounds.join(" + "),
            rname
        ))
    }

    fn collect_pattern_type_checks(&self, pat: &Pattern) -> Vec<String> {
        let mut bindings = Vec::new();
        collect_pattern_bindings(pat, &mut bindings);
        let mut checks = Vec::new();
        for (name, ty) in bindings {
            let Some(ty) = ty else {
                continue;
            };
            let rname = fmt_rust_ident(&name);
            if let Type::ImplTrait(bounds) = &ty {
                if let Some(chk) = self.fmt_impl_trait_check(&rname, bounds) {
                    checks.push(chk);
                }
            } else {
                let ty_s = self.fmt_type(&ty);
                checks.push(format!("let _: &{} = &{};", ty_s, rname));
            }
        }
        checks
    }

    fn emit_pattern_type_checks(&mut self, pat: &Pattern) {
        let checks = self.collect_pattern_type_checks(pat);
        self.register_pattern_vars(pat);
        for c in checks {
            self.line(&c);
        }
    }

    // ==================== TYPES & PATTERNS ====================

    fn fmt_type(&self, ty: &Type) -> String {
        match ty {
            Type::Void => "()".to_string(),
            Type::Never => "!".to_string(),
            Type::Dynamic => "Box<dyn std::any::Any>".to_string(),
            Type::Infer => "_".to_string(),
            Type::Path(p) => self.fmt_type_path(p),
            Type::QualifiedPath {
                self_ty,
                trait_path,
                segments,
            } => {
                let segs_s = self.fmt_type_path_segments(segments);
                match trait_path {
                    Some(tp) => format!(
                        "<{} as {}>::{}",
                        self.fmt_type(self_ty),
                        self.fmt_type_path(tp),
                        segs_s
                    ),
                    None => format!("<{}>::{}", self.fmt_type(self_ty), segs_s),
                }
            }
            Type::Grouped(inner) => format!("({})", self.fmt_type(inner)),
            Type::Read { life, inner } => {
                let l = life
                    .as_ref()
                    .map(|n| format!("{} ", fmt_rust_lifetime(n)))
                    .unwrap_or_default();
                format!("&{}{}", l, self.fmt_type(inner))
            }
            Type::Write { life, inner } => {
                let l = life
                    .as_ref()
                    .map(|n| format!("{} ", fmt_rust_lifetime(n)))
                    .unwrap_or_default();
                format!("&{}mut {}", l, self.fmt_type(inner))
            }
            Type::Slice(inner) => format!("[{}]", self.fmt_type(inner)),
            Type::ReadPtr(inner) => format!("*const {}", self.fmt_type(inner)),
            Type::WritePtr(inner) => format!("*mut {}", self.fmt_type(inner)),
            Type::Optional(inner) => format!("Option<{}>", self.fmt_type(inner)),
            Type::Array { elem, dims } => {
                let mut cur = self.fmt_type(elem);
                for d in dims.iter().rev() {
                    cur = format!("[{}; {}]", cur, self.fmt_expr(d));
                }
                cur
            }
            Type::Tuple(elems) => {
                let parts: Vec<_> = elems.iter().map(|e| self.fmt_type(&e.ty)).collect();
                if parts.len() == 1 {
                    format!("({},)", parts[0])
                } else {
                    format!("({})", parts.join(", "))
                }
            }
            Type::DynTrait(bounds) => {
                let bs: Vec<_> = bounds.iter().map(|b| self.fmt_bound(b)).collect();
                format!("dyn {}", bs.join(" + "))
            }
            Type::ImplTrait(bounds) => {
                let bs: Vec<_> = bounds.iter().map(|b| self.fmt_bound(b)).collect();
                format!("impl {}", bs.join(" + "))
            }
            Type::FnPtr {
                lifetimes,
                is_unsafe,
                extern_abi,
                params,
                variadic,
                ret,
            } => {
                let mut prefix = String::new();
                if !lifetimes.is_empty() {
                    let ls: Vec<String> = lifetimes.iter().map(|l| fmt_rust_lifetime(l)).collect();
                    prefix.push_str(&format!("for<{}> ", ls.join(", ")));
                }
                if *is_unsafe {
                    prefix.push_str("unsafe ");
                }
                if let Some(abi) = extern_abi {
                    prefix.push_str(&format!("extern \"{}\" ", abi));
                }
                let mut ps = Vec::new();
                for p in params {
                    let mut apfx = String::new();
                    for a in &p.attrs {
                        apfx.push_str(&format_attr(a));
                        apfx.push(' ');
                    }
                    let t = self.fmt_type(&p.ty);
                    if let Some(name) = &p.name {
                        ps.push(format!("{}{}: {}", apfx, fmt_rust_ident(name), t));
                    } else {
                        ps.push(format!("{}{}", apfx, t));
                    }
                }
                if let Some(vattrs) = variadic {
                    let mut vpfx = String::new();
                    for a in vattrs {
                        vpfx.push_str(&format_attr(a));
                        vpfx.push(' ');
                    }
                    ps.push(format!("{}...", vpfx));
                }
                let r = self.fmt_type(ret);
                if matches!(ret.as_ref(), Type::Void) {
                    format!("{}fn({})", prefix, ps.join(", "))
                } else {
                    format!("{}fn({}) -> {}", prefix, ps.join(", "), r)
                }
            }
            Type::Macro(mc) => format_macro_call(mc),
        }
    }

    fn fmt_type_path(&self, path: &TypePath) -> String {
        if !path.global
            && path.segments.len() == 1
            && matches!(path.segments[0].args, TypePathArgs::None)
            && let Some(prim) = map_primitive_alias(&path.segments[0].name)
        {
            return prim.to_string();
        }
        let joined = self.fmt_type_path_segments(&path.segments);
        if path.global {
            format!("::{}", joined)
        } else {
            joined
        }
    }

    fn fmt_type_path_segments(&self, segments: &[TypePathSegment]) -> String {
        let is_std_root = segments.first().is_some_and(|s| {
            matches!(s.name.as_str(), "std" | "core" | "alloc" | "rsharp_prelude")
        });
        let mut out = Vec::new();
        for (idx, seg) in segments.iter().enumerate() {
            let map_alias = (is_std_root && idx + 1 == segments.len())
                || (idx == 0 && segments.len() == 1 && !self.known_types.contains(&seg.name));
            let mapped = if map_alias {
                fmt_rust_ident(&map_std_type_alias(&seg.name))
            } else {
                fmt_rust_ident(&seg.name)
            };
            match &seg.args {
                TypePathArgs::None => out.push(mapped),
                TypePathArgs::Angle(args) => {
                    let ga: Vec<_> = args.iter().map(|a| self.fmt_generic_arg(a)).collect();
                    out.push(format!("{}<{}>", mapped, ga.join(", ")));
                }
                TypePathArgs::Parenthesized { params, ret } => {
                    let ps: Vec<_> = params.iter().map(|p| self.fmt_type(p)).collect();
                    let ret_s = match ret {
                        Some(r) if !matches!(r.as_ref(), Type::Void) => {
                            format!(" -> {}", self.fmt_type(r))
                        }
                        _ => String::new(),
                    };
                    out.push(format!("{}({}){}", mapped, ps.join(", "), ret_s));
                }
            }
        }
        out.join("::")
    }

    fn fmt_turbofish_segments(&self, segments: &[TypePathSegment]) -> String {
        let mut out = Vec::new();
        for seg in segments {
            let mapped = fmt_rust_ident(&seg.name);
            match &seg.args {
                TypePathArgs::Angle(args) if !args.is_empty() => {
                    let ga: Vec<_> = args.iter().map(|a| self.fmt_generic_arg(a)).collect();
                    out.push(format!("{}::<{}>", mapped, ga.join(", ")));
                }
                _ => out.push(mapped),
            }
        }
        out.join("::")
    }

    fn fmt_pat_path(&self, pp: &PatPath) -> String {
        let segs = self.fmt_turbofish_segments(&pp.segments);
        if let Some((self_ty, trait_path)) = &pp.qself {
            match trait_path {
                Some(tp) => format!(
                    "<{} as {}>::{}",
                    self.fmt_type(self_ty),
                    self.fmt_type_path(tp),
                    segs
                ),
                None => format!("<{}>::{}", self.fmt_type(self_ty), segs),
            }
        } else if pp.global {
            format!("::{}", segs)
        } else {
            segs
        }
    }

    fn fmt_generic_arg(&self, arg: &GenericArg) -> String {
        match arg {
            GenericArg::Lifetime(l) => fmt_rust_lifetime(l),
            GenericArg::Type(t) => self.fmt_type(t),
            GenericArg::AssocBinding { name, generics, ty } => {
                let gs = if generics.is_empty() {
                    String::new()
                } else {
                    let ga: Vec<_> = generics.iter().map(|a| self.fmt_generic_arg(a)).collect();
                    format!("<{}>", ga.join(", "))
                };
                format!("{}{} = {}", fmt_rust_ident(name), gs, self.fmt_type(ty))
            }
            GenericArg::AssocBound {
                name,
                generics,
                bounds,
            } => {
                let gs = if generics.is_empty() {
                    String::new()
                } else {
                    let ga: Vec<_> = generics.iter().map(|a| self.fmt_generic_arg(a)).collect();
                    format!("<{}>", ga.join(", "))
                };
                let bs: Vec<_> = bounds.iter().map(|b| self.fmt_bound(b)).collect();
                format!("{}{}: {}", fmt_rust_ident(name), gs, bs.join(" + "))
            }
            GenericArg::ConstExpr(e) => self.fmt_expr(e),
            GenericArg::ConstBlock(Expr::ConstBlock(ConstBody::Block(b))) => {
                let mut sub = self.child_formatter();
                sub.emit_value_block_stmts(b);
                format!("{{\n{}}}", sub.out)
            }
            GenericArg::ConstBlock(e) => format!("{{ {} }}", self.fmt_expr(e)),
        }
    }

    fn fmt_bound(&self, b: &TypeBound) -> String {
        match b {
            TypeBound::Trait(tp) => self.fmt_type_path(tp),
            TypeBound::Relaxed(tp) => format!("?{}", self.fmt_type_path(tp)),
            TypeBound::Forall {
                lifetimes,
                trait_path,
            } => {
                let ls: Vec<String> = lifetimes.iter().map(|l| fmt_rust_lifetime(l)).collect();
                format!("for<{}> {}", ls.join(", "), self.fmt_type_path(trait_path))
            }
            TypeBound::Paren(inner) => format!("({})", self.fmt_bound(inner)),
            TypeBound::Lifetime(l) => fmt_rust_lifetime(l),
            TypeBound::PreciseCapture(args) => {
                let parts: Vec<String> = args
                    .iter()
                    .map(|a| match a {
                        CaptureArg::Lifetime(l) => fmt_rust_lifetime(l),
                        CaptureArg::TypeOrConst(n) => fmt_rust_ident(n),
                    })
                    .collect();
                format!("use<{}>", parts.join(", "))
            }
        }
    }

    fn fmt_generics(&self, params: &[GenericParam], include_defaults: bool) -> String {
        if params.is_empty() {
            return String::new();
        }
        let mut parts = Vec::new();
        for p in params {
            let mut prefix = String::new();
            for a in &p.attrs {
                prefix.push_str(&format_attr(a));
                prefix.push(' ');
            }
            match &p.kind {
                GenericParamKind::Lifetime { name, outlives } => {
                    let mut s = format!("{}{}", prefix, fmt_rust_lifetime(name));
                    if !outlives.is_empty() {
                        let os: Vec<String> =
                            outlives.iter().map(|o| fmt_rust_lifetime(o)).collect();
                        s.push_str(&format!(": {}", os.join(" + ")));
                    }
                    parts.push(s);
                }
                GenericParamKind::Type {
                    name,
                    bounds,
                    default,
                } => {
                    let mut s = format!("{}{}", prefix, fmt_rust_ident(name));
                    if !bounds.is_empty() {
                        let bs: Vec<_> = bounds.iter().map(|b| self.fmt_bound(b)).collect();
                        s.push_str(&format!(": {}", bs.join(" + ")));
                    }
                    if include_defaults && let Some(def) = default {
                        s.push_str(&format!(" = {}", self.fmt_type(def)));
                    }
                    parts.push(s);
                }
                GenericParamKind::Const { ty, name, default } => {
                    let mut s = format!(
                        "{}const {}: {}",
                        prefix,
                        fmt_rust_ident(name),
                        self.fmt_type(ty)
                    );
                    if include_defaults && let Some(def) = default {
                        s.push_str(&format!(" = {}", self.fmt_expr(def)));
                    }
                    parts.push(s);
                }
            }
        }
        format!("<{}>", parts.join(", "))
    }

    fn fmt_where(&self, wc: &Option<WhereClause>) -> String {
        let Some(wc) = wc else {
            return String::new();
        };
        if wc.predicates.is_empty() {
            return String::new();
        }
        let mut preds = Vec::new();
        for p in &wc.predicates {
            match p {
                WherePredicate::Type {
                    lifetimes,
                    ty,
                    bounds,
                } => {
                    let forall = if lifetimes.is_empty() {
                        String::new()
                    } else {
                        let ls: Vec<String> =
                            lifetimes.iter().map(|l| fmt_rust_lifetime(l)).collect();
                        format!("for<{}> ", ls.join(", "))
                    };
                    let bs: Vec<_> = bounds.iter().map(|b| self.fmt_bound(b)).collect();
                    preds.push(format!(
                        "{}{}: {}",
                        forall,
                        self.fmt_type(ty),
                        bs.join(" + ")
                    ));
                }
                WherePredicate::Lifetime { life, outlives } => {
                    let os: Vec<String> = outlives.iter().map(|o| fmt_rust_lifetime(o)).collect();
                    preds.push(format!("{}: {}", fmt_rust_lifetime(life), os.join(" + ")));
                }
            }
        }
        format!(" where {}", preds.join(", "))
    }

    fn fmt_assign_pattern(&self, pat: &Pattern) -> String {
        match pat {
            Pattern::Path(pp)
                if pp.qself.is_none()
                    && !pp.global
                    && pp.segments.len() > 1
                    && (matches!(pp.segments[0].name.as_str(), "this" | "self")
                        || self.is_local_var(&pp.segments[0].name)) =>
            {
                let mut parts = Vec::with_capacity(pp.segments.len());
                for (idx, seg) in pp.segments.iter().enumerate() {
                    if idx == 0 && matches!(seg.name.as_str(), "this" | "self") {
                        parts.push("self".to_string());
                    } else {
                        parts.push(fmt_rust_ident(&seg.name));
                    }
                }
                parts.join(".")
            }
            Pattern::Tuple(ps) => {
                let items: Vec<_> = ps.iter().map(|p| self.fmt_assign_pattern(p)).collect();
                if items.len() == 1 {
                    format!("({},)", items[0])
                } else {
                    format!("({})", items.join(", "))
                }
            }
            Pattern::Slice(elems) => {
                let parts: Vec<_> = elems
                    .iter()
                    .map(|el| match el {
                        SlicePatElem::Pat(p) => self.fmt_assign_pattern(p),
                        SlicePatElem::Rest => "..".to_string(),
                        SlicePatElem::RestBind { name, .. } => {
                            format!("{} @ ..", fmt_rust_ident(name))
                        }
                    })
                    .collect();
                format!("[{}]", parts.join(", "))
            }
            Pattern::TupleStruct { path, fields } => {
                let fs: Vec<_> = fields.iter().map(|f| self.fmt_assign_pattern(f)).collect();
                format!("{}({})", self.fmt_pat_path(path), fs.join(", "))
            }
            Pattern::Struct { path, fields, rest } => {
                let mut parts = Vec::new();
                for f in fields {
                    let fname = fmt_rust_ident(&f.name);
                    match &f.pat {
                        Some(sub) => {
                            parts.push(format!("{}: {}", fname, self.fmt_assign_pattern(sub)))
                        }
                        None => parts.push(fname),
                    }
                }
                if *rest {
                    parts.push("..".to_string());
                }
                format!("{} {{ {} }}", self.fmt_pat_path(path), parts.join(", "))
            }
            Pattern::Paren(inner) => format!("({})", self.fmt_assign_pattern(inner)),
            other => self.fmt_pattern(other),
        }
    }

    fn fmt_pattern(&self, pat: &Pattern) -> String {
        match pat {
            Pattern::Wildcard => "_".to_string(),
            Pattern::Rest => "..".to_string(),
            Pattern::Lit(l) => fmt_literal(l),
            Pattern::Const(e) => format!("const {{ {} }}", self.fmt_expr(e)),
            Pattern::Range {
                start,
                end,
                inclusive,
            } => {
                let s = match start {
                    Some(e) => self.fmt_expr(e),
                    None => String::new(),
                };
                let e = match end {
                    Some(ex) => self.fmt_expr(ex),
                    None => String::new(),
                };
                let op = if *inclusive { "..=" } else { ".." };
                format!("{}{}{}", s, op, e)
            }
            Pattern::Binding { is_mut, name, .. } => {
                let rn = fmt_rust_ident(name);
                if *is_mut { format!("mut {}", rn) } else { rn }
            }
            Pattern::BorrowRef { mode, name, .. } => {
                let rn = fmt_rust_ident(name);
                match mode {
                    RefMode::Read => format!("ref {}", rn),
                    RefMode::Write => format!("ref mut {}", rn),
                }
            }
            Pattern::DerefPat { is_mut, inner } => {
                if *is_mut {
                    format!("&mut {}", self.fmt_pattern(inner))
                } else {
                    format!("&{}", self.fmt_pattern(inner))
                }
            }
            Pattern::Paren(inner) => format!("({})", self.fmt_pattern(inner)),
            Pattern::BindAt {
                ref_mode,
                is_mut,
                name,
                subpat,
                ..
            } => {
                let pfx = format_binding_prefix(*ref_mode, *is_mut);
                format!(
                    "{}{} @ {}",
                    pfx,
                    fmt_rust_ident(name),
                    self.fmt_pattern(subpat)
                )
            }
            Pattern::Tuple(ps) => {
                let items: Vec<_> = ps.iter().map(|p| self.fmt_pattern(p)).collect();
                if items.len() == 1 {
                    format!("({},)", items[0])
                } else {
                    format!("({})", items.join(", "))
                }
            }
            Pattern::Slice(elems) => {
                let mut parts = Vec::new();
                for el in elems {
                    match el {
                        SlicePatElem::Pat(p) => parts.push(self.fmt_pattern(p)),
                        SlicePatElem::Rest => parts.push("..".to_string()),
                        SlicePatElem::RestBind {
                            ref_mode,
                            is_mut,
                            name,
                            ..
                        } => {
                            let pfx = format_binding_prefix(*ref_mode, *is_mut);
                            parts.push(format!("{}{} @ ..", pfx, fmt_rust_ident(name)));
                        }
                    }
                }
                format!("[{}]", parts.join(", "))
            }
            Pattern::TupleStruct { path, fields } => {
                let fs: Vec<_> = fields.iter().map(|f| self.fmt_pattern(f)).collect();
                format!("{}({})", self.fmt_pat_path(path), fs.join(", "))
            }
            Pattern::Struct { path, fields, rest } => {
                let mut parts = Vec::new();
                for f in fields {
                    let mut apfx = String::new();
                    for a in &f.attrs {
                        apfx.push_str(&format_attr(a));
                        apfx.push(' ');
                    }
                    let fname = fmt_rust_ident(&f.name);
                    match &f.pat {
                        Some(sub) => {
                            parts.push(format!("{}{}: {}", apfx, fname, self.fmt_pattern(sub)))
                        }
                        None => {
                            let pfx = format_binding_prefix(f.ref_mode, f.is_mut);
                            parts.push(format!("{}{}{}", apfx, pfx, fname));
                        }
                    }
                }
                if *rest {
                    parts.push("..".to_string());
                }
                format!("{} {{ {} }}", self.fmt_pat_path(path), parts.join(", "))
            }
            Pattern::Path(p) => self.fmt_pat_path(p),
            Pattern::Or(alts) => {
                let items: Vec<_> = alts.iter().map(|p| self.fmt_pattern(p)).collect();
                items.join(" | ")
            }
            Pattern::Macro(mc) => format_macro_call(mc),
        }
    }

    // ==================== EXPRESSIONS ====================

    fn fmt_expr(&self, expr: &Expr) -> String {
        match expr {
            Expr::Lit(l) => fmt_literal(l),
            Expr::Infer => "_".to_string(),
            Expr::Attributed { attrs, expr } => {
                let mut prefix = String::new();
                for a in attrs {
                    prefix.push_str(&format_attr(a));
                    prefix.push(' ');
                }
                // Emit as a statement-attributed block so attributes work on stable Rust without `#![feature(stmt_expr_attributes)]`.
                format!("{{ {}{} }}", prefix, self.fmt_expr(expr))
            }
            Expr::InterpString(parts) => {
                let mut fmt_str = String::new();
                let mut args = Vec::new();
                for part in parts {
                    match part {
                        InterpExprPart::Literal(s) => fmt_str.push_str(s),
                        InterpExprPart::Expr { expr, format } => {
                            if let Some(f) = format {
                                fmt_str.push_str(&format!("{{:{}}}", f));
                            } else {
                                fmt_str.push_str("{}");
                            }
                            args.push(self.fmt_expr(expr));
                        }
                    }
                }
                if args.is_empty() {
                    format!("format!(\"{}\")", fmt_str)
                } else {
                    format!("format!(\"{}\", {})", fmt_str, args.join(", "))
                }
            }
            Expr::Ident(name) => {
                if self.is_local_var(name)
                    || self.shadowed_expr_aliases.contains(name)
                    || self.known_types.contains(name)
                {
                    fmt_rust_ident(name)
                } else {
                    fmt_rust_ident(&map_std_type_alias(name))
                }
            }
            Expr::This => "self".to_string(),
            Expr::SelfType => "Self".to_string(),
            Expr::Path { global, segments } => {
                let mut parts = Vec::new();
                for (idx, seg) in segments.iter().enumerate() {
                    let mapped = if !*global
                        && idx == 0
                        && segments.len() == 1
                        && !self.known_types.contains(&seg.name)
                        && !self.shadowed_expr_aliases.contains(&seg.name)
                    {
                        fmt_rust_ident(&map_std_type_alias(&seg.name))
                    } else {
                        fmt_rust_ident(&seg.name)
                    };
                    if seg.type_args.is_empty() {
                        parts.push(mapped);
                    } else {
                        let args: Vec<_> = seg
                            .type_args
                            .iter()
                            .map(|a| self.fmt_generic_arg(a))
                            .collect();
                        parts.push(format!("{}::<{}>", mapped, args.join(", ")));
                    }
                }
                let joined = parts.join("::");
                if *global {
                    format!("::{}", joined)
                } else {
                    joined
                }
            }
            Expr::QualifiedMember {
                self_ty,
                trait_path,
                segments,
            } => {
                let mut seg_strs = Vec::new();
                for seg in segments {
                    let mapped = fmt_rust_ident(&seg.name);
                    if seg.type_args.is_empty() {
                        seg_strs.push(mapped);
                    } else {
                        let ta: Vec<_> = seg
                            .type_args
                            .iter()
                            .map(|a| self.fmt_generic_arg(a))
                            .collect();
                        seg_strs.push(format!("{}::<{}>", mapped, ta.join(", ")));
                    }
                }
                let tail = seg_strs.join("::");
                if let Some(tp) = trait_path {
                    // If `self_ty` is a simple identifier that is a local variable, emit a cast + member call
                    if segments.len() == 1
                        && let Type::Path(stp) = self_ty.as_ref()
                        && stp.segments.len() == 1
                        && matches!(stp.segments[0].args, TypePathArgs::None)
                        && self.is_local_var(&stp.segments[0].name)
                    {
                        return format!(
                            "({} as {}).{}",
                            fmt_rust_ident(&stp.segments[0].name),
                            self.fmt_type_path(tp),
                            tail
                        );
                    }
                    format!(
                        "<{} as {}>::{}",
                        self.fmt_type(self_ty),
                        self.fmt_type_path(tp),
                        tail
                    )
                } else {
                    format!("<{}>::{}", self.fmt_type(self_ty), tail)
                }
            }
            Expr::Unary { op, expr } => {
                let inner = self.fmt_expr(expr);
                match op {
                    UnaryOp::Neg => format!("-{}", inner),
                    UnaryOp::Not | UnaryOp::BitNot => format!("!{}", inner),
                }
            }
            Expr::Binary { op, lhs, rhs } => {
                let l = if matches!(lhs.as_ref(), Expr::Range { .. }) {
                    format!("({})", self.fmt_expr(lhs))
                } else {
                    self.fmt_expr(lhs)
                };
                let r = if matches!(rhs.as_ref(), Expr::Range { .. }) {
                    format!("({})", self.fmt_expr(rhs))
                } else {
                    self.fmt_expr(rhs)
                };
                let sym = match op {
                    BinaryOp::Add => "+",
                    BinaryOp::Sub => "-",
                    BinaryOp::Mul => "*",
                    BinaryOp::Div => "/",
                    BinaryOp::Rem => "%",
                    BinaryOp::Eq => "==",
                    BinaryOp::Ne => "!=",
                    BinaryOp::Lt => "<",
                    BinaryOp::Le => "<=",
                    BinaryOp::Gt => ">",
                    BinaryOp::Ge => ">=",
                    BinaryOp::And => "&&",
                    BinaryOp::Or => "||",
                    BinaryOp::BitAnd => "&",
                    BinaryOp::BitOr => "|",
                    BinaryOp::BitXor => "^",
                    BinaryOp::Shl => "<<",
                    BinaryOp::Shr => ">>",
                };
                format!("({} {} {})", l, sym, r)
            }
            Expr::Assign { op, lhs, rhs } => {
                let l = self.fmt_expr(lhs);
                let r = self.fmt_expr(rhs);
                let sym = match op {
                    AssignOp::Assign => "=",
                    AssignOp::AddAssign => "+=",
                    AssignOp::SubAssign => "-=",
                    AssignOp::MulAssign => "*=",
                    AssignOp::DivAssign => "/=",
                    AssignOp::RemAssign => "%=",
                    AssignOp::BitAndAssign => "&=",
                    AssignOp::BitOrAssign => "|=",
                    AssignOp::BitXorAssign => "^=",
                    AssignOp::ShlAssign => "<<=",
                    AssignOp::ShrAssign => ">>=",
                };
                format!("{} {} {}", l, sym, r)
            }
            Expr::Step { inc, target } => {
                let t = self.fmt_expr(target);
                if *inc {
                    format!("{} += 1", t)
                } else {
                    format!("{} -= 1", t)
                }
            }
            Expr::Cast { expr, ty } => {
                format!("({} as {})", self.fmt_expr(expr), self.fmt_type(ty))
            }
            Expr::Ternary {
                cond,
                then_expr,
                else_expr,
            } => {
                format!(
                    "if {} {{ {} }} else {{ {} }}",
                    self.fmt_expr(cond),
                    self.fmt_expr(then_expr),
                    self.fmt_expr(else_expr)
                )
            }
            Expr::Coalesce { lhs, rhs } => {
                format!(
                    "({}).unwrap_or_else(|| {})",
                    self.fmt_expr(lhs),
                    self.fmt_expr(rhs)
                )
            }
            Expr::Try(inner) => format!("({})?", self.fmt_expr(inner)),
            Expr::Await(inner) => format!("({}).await", self.fmt_expr(inner)),
            Expr::Read(inner) | Expr::ReadSlice(inner) => format!("&{}", self.fmt_expr(inner)),
            Expr::Write(inner) | Expr::WriteSlice(inner) => {
                format!("&mut {}", self.fmt_expr(inner))
            }
            Expr::RawReadPtr(inner) => format!("&raw const {}", self.fmt_expr(inner)),
            Expr::RawWritePtr(inner) => format!("&raw mut {}", self.fmt_expr(inner)),
            Expr::Take(inner) => self.fmt_expr(inner),
            Expr::Deref(inner) => format!("*{}", self.fmt_expr(inner)),
            Expr::Range {
                start,
                end,
                inclusive,
            } => {
                let s = match start {
                    Some(e) => self.fmt_expr(e),
                    None => String::new(),
                };
                let e = match end {
                    Some(ex) => self.fmt_expr(ex),
                    None => String::new(),
                };
                let op = if *inclusive { "..=" } else { ".." };
                format!("{}{}{}", s, op, e)
            }
            Expr::Call {
                callee,
                type_args,
                args,
            } => {
                if let Expr::Ident(name) = callee.as_ref()
                    && name == "panic"
                    && type_args.is_empty()
                {
                    if args.len() == 1 {
                        return match &args[0] {
                            Expr::Lit(Literal::Str(s)) => format!("panic!(\"{}\")", s),
                            other => format!("panic!(\"{{}}\", {})", self.fmt_expr(other)),
                        };
                    }
                    let a: Vec<_> = args.iter().map(|e| self.fmt_expr(e)).collect();
                    return format!("panic!({})", a.join(", "));
                }
                let c = self.fmt_expr(callee);
                let targs = if type_args.is_empty() {
                    String::new()
                } else {
                    let ta: Vec<_> = type_args.iter().map(|a| self.fmt_generic_arg(a)).collect();
                    format!("::<{}>", ta.join(", "))
                };
                let a: Vec<_> = args.iter().map(|e| self.fmt_list_elem_expr(e)).collect();
                format!("{}{}({})", c, targs, a.join(", "))
            }
            Expr::Group(inner) => format!("({})", self.fmt_expr(inner)),
            Expr::Member {
                object,
                field,
                type_args,
            } => {
                if type_args.is_empty()
                    && let Expr::Ident(var_name) = object.as_ref()
                    && let Some(info) = self.lookup_var(var_name)
                    && let Some(map) = &info.tuple_fields
                    && let Some(idx) = map.get(field)
                {
                    return format!("{}.{}", fmt_rust_ident(var_name), idx);
                }
                let is_type_path_seg = !type_args.is_empty()
                    && field.chars().next().is_some_and(|c| c.is_uppercase())
                    && !self.is_local_var_root(object);
                let sep = if self.is_path_expr(object) || is_type_path_seg {
                    "::"
                } else {
                    "."
                };
                let obj = self.fmt_postfix_object(object);
                let mapped_field = if sep == "::"
                    && is_std_collections_root(object)
                    && (field != "string" || is_string_module_expr(object))
                {
                    fmt_rust_ident(&map_std_type_alias(field))
                } else {
                    fmt_rust_ident(field)
                };
                let targs = if type_args.is_empty() {
                    String::new()
                } else {
                    let ta: Vec<_> = type_args.iter().map(|a| self.fmt_generic_arg(a)).collect();
                    format!("::<{}>", ta.join(", "))
                };
                format!("{}{}{}{}", obj, sep, mapped_field, targs)
            }
            Expr::TupleIndex { object, index } => {
                format!("{}.{}", self.fmt_postfix_object(object), index)
            }
            Expr::Index { object, indices } => {
                let mut out = self.fmt_postfix_object(object);
                for idx in indices {
                    out.push_str(&format!("[{}]", self.fmt_expr(idx)));
                }
                out
            }
            Expr::ArrayLit(elems) => {
                let items: Vec<_> = elems.iter().map(|e| self.fmt_list_elem_expr(e)).collect();
                format!("[{}]", items.join(", "))
            }
            Expr::ArrayRepeat { value, count } => {
                format!("[{}; {}]", self.fmt_expr(value), self.fmt_expr(count))
            }
            Expr::TupleLit(elems) => {
                let items: Vec<_> = elems.iter().map(|e| self.fmt_list_elem_expr(e)).collect();
                if items.len() == 1 {
                    format!("({},)", items[0])
                } else {
                    format!("({})", items.join(", "))
                }
            }
            Expr::StructLit { path, fields, rest } => {
                let prefix = if path.global { "::" } else { "" };
                let path_s = format!("{}{}", prefix, self.fmt_turbofish_segments(&path.segments));
                let mut fs = Vec::new();
                for f in fields {
                    let mut p = String::new();
                    for a in &f.attrs {
                        p.push_str(&format_attr(a));
                        p.push(' ');
                    }
                    let fname = fmt_rust_ident(&f.name);
                    match &f.value {
                        Some(v) => p.push_str(&format!("{}: {}", fname, self.fmt_expr(v))),
                        None => p.push_str(&fname),
                    }
                    fs.push(p);
                }
                if let Some(r) = rest {
                    let rest_s = self.fmt_expr(r);
                    if fs.is_empty() {
                        format!("{} {{ ..{} }}", path_s, rest_s)
                    } else {
                        format!("{} {{ {}, ..{} }}", path_s, fs.join(", "), rest_s)
                    }
                } else {
                    format!("{} {{ {} }}", path_s, fs.join(", "))
                }
            }
            Expr::StructUpdate { base, fields } => {
                let struct_name = self.resolve_struct_name(base);
                let base_s = self.fmt_expr(base);
                let mut fs = Vec::new();
                for f in fields {
                    let mut p = String::new();
                    for a in &f.attrs {
                        p.push_str(&format_attr(a));
                        p.push(' ');
                    }
                    let fname = fmt_rust_ident(&f.name);
                    match &f.value {
                        Some(v) => p.push_str(&format!("{}: {}", fname, self.fmt_expr(v))),
                        None => p.push_str(&fname),
                    }
                    fs.push(p);
                }
                if fs.is_empty() {
                    format!("{} {{ ..{} }}", struct_name, base_s)
                } else {
                    format!("{} {{ {}, ..{} }}", struct_name, fs.join(", "), base_s)
                }
            }
            Expr::Return(opt) => match opt {
                Some(e) => format!("return {}", self.fmt_expr(e)),
                None => "return".to_string(),
            },
            Expr::Break { label, value } => {
                let mut out = String::from("break");
                if let Some(l) = label {
                    out.push_str(&format!(" {}", fmt_rust_lifetime(l)));
                }
                if let Some(v) = value {
                    out.push(' ');
                    out.push_str(&self.fmt_break_value_expr(label.is_some(), v));
                }
                out
            }
            Expr::Continue { label } => match label {
                Some(l) => format!("continue {}", fmt_rust_lifetime(l)),
                None => "continue".to_string(),
            },
            Expr::IfExpr {
                cond,
                then_expr,
                else_expr,
            } => {
                let mut sub = self.child_formatter();
                let mut bound_pats = Vec::new();
                let cond_s = self.fmt_condition_expr(cond, &mut sub, &mut bound_pats);
                let mut checks = Vec::new();
                for bp in &bound_pats {
                    checks.extend(self.collect_pattern_type_checks(bp));
                }
                let then_s = sub.fmt_expr(then_expr);
                let else_s = self.fmt_expr(else_expr);
                if checks.is_empty() {
                    format!("if {} {{ {} }} else {{ {} }}", cond_s, then_s, else_s)
                } else {
                    format!(
                        "if {} {{ {} {} }} else {{ {} }}",
                        cond_s,
                        checks.join(" "),
                        then_s,
                        else_s
                    )
                }
            }
            Expr::While { label, cond, body } => {
                let lbl = label
                    .as_ref()
                    .map(|l| format!("{}: ", fmt_rust_lifetime(l)))
                    .unwrap_or_default();
                let mut sub = self.child_formatter();
                sub.emit_cond_block(&format!("{}while ", lbl), cond, body);
                sub.line("}");
                sub.out.trim_end().to_string()
            }
            Expr::Foreach {
                label,
                pat,
                iter,
                body,
            } => {
                let lbl = label
                    .as_ref()
                    .map(|l| format!("{}: ", fmt_rust_lifetime(l)))
                    .unwrap_or_default();
                let mut sub = self.child_formatter();
                let p = sub.fmt_pattern(pat);
                let it = sub.fmt_expr(iter);
                sub.line(&format!("{}for {} in {} {{", lbl, p, it));
                sub.indent += 1;
                sub.push_scope();
                sub.emit_inner_docs_and_attrs(&body.inner_docs, &body.inner_attrs);
                sub.emit_pattern_type_checks(pat);
                for st in &body.stmts {
                    sub.emit_stmt(st);
                }
                sub.pop_scope();
                sub.indent -= 1;
                sub.line("}");
                sub.out.trim_end().to_string()
            }
            Expr::Lambda {
                is_async,
                is_move,
                params,
                ret_ty,
                body,
            } => {
                let mut prefix = String::new();
                if *is_async {
                    prefix.push_str("async ");
                }
                if *is_move {
                    prefix.push_str("move ");
                }
                let mut sub = self.child_formatter();
                let mut ps = Vec::new();
                let mut param_checks = Vec::new();
                for p in params {
                    match p {
                        Param::Pattern {
                            attrs,
                            pat,
                            explicit_ty,
                        } => {
                            sub.register_pattern_vars(pat);
                            let mut apfx = String::new();
                            for a in attrs {
                                apfx.push_str(&format_attr(a));
                                apfx.push(' ');
                            }
                            let pat_s = sub.fmt_pattern(pat);
                            if let Some(ty) = explicit_ty {
                                ps.push(format!("{}{}: {}", apfx, pat_s, sub.fmt_type(ty)));
                            } else {
                                // Use body type checks rather than parameter-type reconstruction so
                                // Rust match ergonomics (`&(A, B)` destructured as `|(a, b)|`) works.
                                ps.push(format!("{}{}", apfx, pat_s));
                                param_checks.extend(sub.collect_pattern_type_checks(pat));
                            }
                        }
                        Param::Normal {
                            attrs,
                            is_mut,
                            ty: Type::ImplTrait(bounds),
                            name,
                        } => {
                            sub.declare_var(name, &Type::ImplTrait(bounds.clone()));
                            let mut apfx = String::new();
                            for a in attrs {
                                apfx.push_str(&format_attr(a));
                                apfx.push(' ');
                            }
                            let m = if *is_mut { "mut " } else { "" };
                            let rname = fmt_rust_ident(name);
                            ps.push(format!("{}{}{}", apfx, m, rname));
                            if let Some(chk) = sub.fmt_impl_trait_check(&rname, bounds) {
                                param_checks.push(chk);
                            }
                        }
                        _ => ps.push(sub.fmt_param(p)),
                    }
                }
                let ret = match ret_ty {
                    Some(t) => {
                        let eff = unwrap_async_task_type(*is_async, t);
                        if matches!(eff, Type::Void) {
                            String::new()
                        } else {
                            format!(" -> {}", self.fmt_type(eff))
                        }
                    }
                    None => String::new(),
                };
                match body {
                    LambdaBody::Expr(e) => {
                        let es = sub.fmt_expr(e);
                        if param_checks.is_empty() && ret.is_empty() && !*is_async {
                            format!("{}|{}| {}", prefix, ps.join(", "), es)
                        } else if param_checks.is_empty() {
                            format!("{}|{}|{} {{ {} }}", prefix, ps.join(", "), ret, es)
                        } else {
                            format!(
                                "{}|{}|{} {{ {} {} }}",
                                prefix,
                                ps.join(", "),
                                ret,
                                param_checks.join(" "),
                                es
                            )
                        }
                    }
                    LambdaBody::Block(b) => {
                        sub.emit_value_block_stmts_with_prefix(b, &param_checks);
                        format!("{}|{}|{} {{\n{}}}", prefix, ps.join(", "), ret, sub.out)
                    }
                }
            }
            Expr::Match(m) => {
                let mut sub = self.child_formatter();
                sub.emit_match(m, false);
                sub.out.trim_end().to_string()
            }
            Expr::Loop { label, block } => {
                let lbl = label
                    .as_ref()
                    .map(|l| format!("{}: ", fmt_rust_lifetime(l)))
                    .unwrap_or_default();
                let mut sub = self.child_formatter();
                sub.emit_block_stmts(block);
                format!("{}loop {{\n{}}}", lbl, sub.out)
            }
            Expr::LabeledBlock { label, block } => {
                let mut sub = self.child_formatter();
                sub.emit_block_stmts(block);
                format!("{}: {{\n{}}}", fmt_rust_lifetime(label), sub.out)
            }
            Expr::ConstBlock(cb) => match cb {
                ConstBody::Expr(e) => format!("const {{ {} }}", self.fmt_expr(e)),
                ConstBody::Block(b) => {
                    let mut sub = self.child_formatter();
                    sub.emit_value_block_stmts(b);
                    format!("const {{\n{}}}", sub.out)
                }
            },
            Expr::AsyncBlock { is_move, block } => {
                let mv = if *is_move { "move " } else { "" };
                let mut sub = self.child_formatter();
                sub.emit_value_block_stmts(block);
                format!("async {} {{\n{}}}", mv, sub.out)
            }
            Expr::UnsafeBlock(block) => {
                let mut sub = self.child_formatter();
                sub.emit_value_block_stmts(block);
                format!("unsafe {{\n{}}}", sub.out)
            }
            Expr::InlineRust(raw) => format!("{{ {} }}", raw.trim()),
            Expr::MacroCall(mc) => format_macro_call(mc),
            Expr::IsPattern { expr, negated, pat } => {
                let e = self.fmt_expr(expr);
                let p = self.fmt_pattern(pat);
                if *negated {
                    format!("!matches!({}, {})", e, p)
                } else {
                    format!("matches!({}, {})", e, p)
                }
            }
        }
    }

    fn child_formatter(&self) -> Self {
        Self {
            options: self.options,
            out: String::new(),
            indent: self.indent + 1,
            known_paths: self.known_paths.clone(),
            known_types: self.known_types.clone(),
            known_values: self.known_values.clone(),
            enum_variants: self.enum_variants.clone(),
            shadowed_expr_aliases: self.shadowed_expr_aliases.clone(),
            scopes: self.scopes.clone(),
            current_self_type: self.current_self_type.clone(),
            in_const_fn: self.in_const_fn,
            first_err: Rc::clone(&self.first_err),
        }
    }

    fn fmt_list_elem_expr(&self, expr: &Expr) -> String {
        if let Expr::Attributed { attrs, expr: inner } = expr {
            let mut prefix = String::new();
            for a in attrs {
                prefix.push_str(&format_attr(a));
                prefix.push(' ');
            }
            format!("{}{}", prefix, self.fmt_expr(inner))
        } else {
            self.fmt_expr(expr)
        }
    }

    fn fmt_break_value_expr(&self, has_label: bool, v: &Expr) -> String {
        let vs = self.fmt_expr(v);
        if has_label
            && matches!(
                v,
                Expr::Loop { .. }
                    | Expr::While { .. }
                    | Expr::Foreach { .. }
                    | Expr::LabeledBlock { .. }
                    | Expr::InlineRust(_)
                    | Expr::Attributed { .. }
            )
        {
            format!("({})", vs)
        } else {
            vs
        }
    }

    fn fmt_postfix_object(&self, object: &Expr) -> String {
        let s = self.fmt_expr(object);
        if matches!(
            object,
            Expr::Range { .. }
                | Expr::Unary { .. }
                | Expr::Deref(_)
                | Expr::Read(_)
                | Expr::Write(_)
        ) {
            format!("({})", s)
        } else {
            s
        }
    }

    fn is_local_var_root(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Ident(name) => self.is_local_var(name),
            Expr::Member { object, .. } => self.is_local_var_root(object),
            _ => false,
        }
    }

    fn is_const_or_static_ident(&self, name: &str) -> bool {
        self.known_values.contains(name)
            || (is_all_caps_const(name) && !self.known_types.contains(name))
    }

    fn is_path_expr(&self, expr: &Expr) -> bool {
        match expr {
            Expr::SelfType => true,
            Expr::Path { segments, .. } => segments.last().is_none_or(|s| {
                if segments.len() == 1 {
                    !self.is_const_or_static_ident(&s.name)
                } else {
                    !(is_all_caps_const(&s.name) && !self.known_types.contains(&s.name))
                }
            }),
            Expr::This => !self.is_local_var("this"),
            Expr::Ident(name) => {
                if self.is_local_var(name) || self.is_const_or_static_ident(name) {
                    return false;
                }
                self.known_paths.contains(name)
                    || name.chars().next().is_some_and(|c| c.is_uppercase())
            }
            Expr::Member {
                object,
                field,
                type_args,
            } => {
                if is_all_caps_const(field) && !self.known_types.contains(field) {
                    return false;
                }
                self.is_path_expr(object)
                    || (!type_args.is_empty()
                        && field.chars().next().is_some_and(|c| c.is_uppercase())
                        && !self.is_local_var_root(object))
            }
            _ => false,
        }
    }

    fn resolve_struct_name(&self, base: &Expr) -> String {
        let res = match base {
            Expr::This => self.current_self_type.clone().ok_or_else(|| {
                RSharpError::codegen("Cannot resolve struct type for 'this with { ... }'")
            }),
            Expr::Ident(name) => self
                .lookup_var(name)
                .and_then(|v| v.struct_path.clone())
                .ok_or_else(|| {
                    RSharpError::codegen(format!(
                        "Cannot determine struct type for '{} with {{ ... }}'",
                        name
                    ))
                }),
            _ => Err(RSharpError::codegen(
                "Struct 'with' update requires a typed variable or 'this' as base",
            )),
        };
        match res {
            Ok(name) => name,
            Err(err) => {
                self.record_err(err);
                String::new()
            }
        }
    }
}

fn expr_needs_require_parens(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::IfExpr { .. }
            | Expr::Ternary { .. }
            | Expr::Match(_)
            | Expr::Loop { .. }
            | Expr::While { .. }
            | Expr::Foreach { .. }
            | Expr::LabeledBlock { .. }
            | Expr::ConstBlock(_)
            | Expr::AsyncBlock { .. }
            | Expr::UnsafeBlock(_)
            | Expr::StructLit { .. }
            | Expr::StructUpdate { .. }
    )
}

fn register_using_tree(tree: &UsingTree, known: &mut HashSet<String>) {
    match tree {
        UsingTree::Path { prefix, alias, .. } => {
            if prefix.len() > 1
                && let Some(first) = prefix.first()
                && first != "_"
            {
                known.insert(first.clone());
            }
            if let Some(a) = alias {
                if a != "_" {
                    known.insert(a.clone());
                }
            } else if let Some(last) = prefix.last()
                && last != "self"
                && last != "_"
            {
                known.insert(last.clone());
            }
        }
        UsingTree::Group { prefix, items, .. } => {
            if let Some(first) = prefix.first()
                && first != "_"
            {
                known.insert(first.clone());
            }
            for item in items {
                if let UsingTree::Path {
                    prefix: p,
                    alias: None,
                    ..
                } = item
                    && p.len() == 1
                    && p[0] == "self"
                    && let Some(parent) = prefix.last()
                {
                    known.insert(parent.clone());
                }
                register_using_tree(item, known);
            }
        }
    }
}

fn collect_syn_pat_idents(pat: &syn::Pat, out: &mut Vec<String>) {
    match pat {
        syn::Pat::Ident(pi) => {
            out.push(pi.ident.to_string());
            if let Some((_, sub)) = &pi.subpat {
                collect_syn_pat_idents(sub, out);
            }
        }
        syn::Pat::Type(pt) => collect_syn_pat_idents(&pt.pat, out),
        syn::Pat::Paren(pp) => collect_syn_pat_idents(&pp.pat, out),
        syn::Pat::Reference(pr) => collect_syn_pat_idents(&pr.pat, out),
        syn::Pat::Tuple(pt) => {
            for e in &pt.elems {
                collect_syn_pat_idents(e, out);
            }
        }
        syn::Pat::Slice(ps) => {
            for e in &ps.elems {
                collect_syn_pat_idents(e, out);
            }
        }
        syn::Pat::TupleStruct(pts) => {
            for e in &pts.elems {
                collect_syn_pat_idents(e, out);
            }
        }
        syn::Pat::Struct(ps) => {
            for f in &ps.fields {
                collect_syn_pat_idents(&f.pat, out);
            }
        }
        syn::Pat::Or(po) => {
            for first in po.cases.iter().take(1) {
                collect_syn_pat_idents(first, out);
            }
        }
        _ => {}
    }
}

fn format_using_tree(tree: &UsingTree) -> String {
    format_using_tree_inner(tree, false, None)
}

fn format_using_tree_inner(tree: &UsingTree, in_std: bool, parent_mod: Option<&str>) -> String {
    match tree {
        UsingTree::Path {
            global,
            prefix,
            wildcard,
            alias,
        } => {
            let std_here = in_std
                || prefix.first().is_some_and(|r| {
                    matches!(r.as_str(), "std" | "core" | "alloc" | "rsharp_prelude")
                });
            let last_idx = prefix.len().saturating_sub(1);
            let mapped: Vec<String> = prefix
                .iter()
                .enumerate()
                .map(|(idx, s)| {
                    let is_leaf = !*wildcard && idx == last_idx;
                    if std_here && is_leaf {
                        let prev = if idx > 0 {
                            Some(prefix[idx - 1].as_str())
                        } else {
                            parent_mod
                        };
                        if s == "string" && !matches!(prev, Some("string" | "rsharp_prelude")) {
                            fmt_rust_ident(s)
                        } else {
                            fmt_rust_ident(&map_std_type_alias(s))
                        }
                    } else {
                        fmt_rust_ident(s)
                    }
                })
                .collect();
            let mut s = if *global {
                format!("::{}", mapped.join("::"))
            } else {
                mapped.join("::")
            };
            if *wildcard {
                if !prefix.is_empty() {
                    s.push_str("::");
                }
                s.push('*');
            }
            if let Some(a) = alias {
                s.push_str(&format!(" as {}", fmt_rust_ident(a)));
            }
            s
        }
        UsingTree::Group {
            global,
            prefix,
            items,
        } => {
            let std_here = in_std
                || prefix.first().is_some_and(|r| {
                    matches!(r.as_str(), "std" | "core" | "alloc" | "rsharp_prelude")
                });
            let mapped: Vec<String> = prefix.iter().map(|s| fmt_rust_ident(s)).collect();
            let next_parent = prefix.last().map(String::as_str).or(parent_mod);
            let subs: Vec<String> = items
                .iter()
                .map(|it| format_using_tree_inner(it, std_here, next_parent))
                .collect();
            let g = if *global { "::" } else { "" };
            if mapped.is_empty() {
                format!("{}{{{}}}", g, subs.join(", "))
            } else {
                format!("{}{}::{{{}}}", g, mapped.join("::"), subs.join(", "))
            }
        }
    }
}

fn infer_param_pattern_type(pat: &Pattern) -> Option<Type> {
    match pat {
        Pattern::Binding { ty: Some(t), .. } => Some(t.clone()),
        Pattern::BindAt { ty, .. } => Some(ty.clone()),
        Pattern::Paren(inner) => infer_param_pattern_type(inner),
        Pattern::Or(alts) => alts.first().and_then(infer_param_pattern_type),
        Pattern::DerefPat { is_mut, inner } => {
            let inner_ty = infer_param_pattern_type(inner)?;
            if *is_mut {
                Some(Type::Write {
                    life: None,
                    inner: Box::new(inner_ty),
                })
            } else {
                Some(Type::Read {
                    life: None,
                    inner: Box::new(inner_ty),
                })
            }
        }
        Pattern::BorrowRef { ty, .. } => Some(ty.clone()),
        Pattern::Tuple(ps) => {
            let elems: Option<Vec<TupleTypeElem>> = ps
                .iter()
                .map(|p| infer_param_pattern_type(p).map(|ty| TupleTypeElem { ty, name: None }))
                .collect();
            Some(Type::Tuple(elems?))
        }
        Pattern::Struct { path, .. } if path.qself.is_none() => Some(Type::Path(TypePath {
            global: path.global,
            segments: path.segments.clone(),
        })),
        Pattern::TupleStruct { path, .. } if path.qself.is_none() && path.segments.len() == 1 => {
            Some(Type::Path(TypePath {
                global: path.global,
                segments: path.segments.clone(),
            }))
        }
        Pattern::Slice(elems) => {
            let first = elems.first().and_then(|e| match e {
                SlicePatElem::Pat(p) => infer_param_pattern_type(p),
                _ => None,
            })?;
            Some(Type::Array {
                elem: Box::new(first),
                dims: vec![Expr::Lit(Literal::Int(elems.len().to_string()))],
            })
        }
        _ => None,
    }
}

fn var_info_from_type(ty: &Type) -> VarInfo {
    match ty {
        Type::Path(tp) => {
            if tp.segments.len() == 1 && map_primitive_alias(&tp.segments[0].name).is_some() {
                return VarInfo::default();
            }
            let name = tp
                .segments
                .iter()
                .map(|s| map_std_type_alias(&s.name))
                .collect::<Vec<_>>()
                .join("::");
            VarInfo {
                struct_path: Some(name),
                tuple_fields: None,
            }
        }
        Type::Tuple(elems) => {
            let mut map = HashMap::new();
            for (idx, elem) in elems.iter().enumerate() {
                if let Some(name) = &elem.name {
                    map.insert(name.clone(), idx);
                }
            }
            VarInfo {
                struct_path: None,
                tuple_fields: if map.is_empty() { None } else { Some(map) },
            }
        }
        Type::Read { inner, .. } | Type::Write { inner, .. } => var_info_from_type(inner),
        _ => VarInfo::default(),
    }
}

fn type_to_struct_name(ty: &Type) -> Option<String> {
    if let Type::Path(tp) = ty {
        Some(
            tp.segments
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join("::"),
        )
    } else {
        None
    }
}

fn is_list_type(ty: &Type) -> bool {
    if let Type::Path(tp) = ty
        && let Some(last) = tp.segments.last()
    {
        return matches!(last.name.as_str(), "List" | "Vec");
    }
    false
}

fn unwrap_async_task_type(is_async: bool, ty: &Type) -> &Type {
    if !is_async {
        return ty;
    }
    if let Type::Path(tp) = ty
        && tp.segments.len() == 1
        && tp.segments[0].name == "Task"
    {
        match &tp.segments[0].args {
            TypePathArgs::None => return &Type::Void,
            TypePathArgs::Angle(args) => {
                if let Some(GenericArg::Type(inner)) = args.first() {
                    return inner;
                }
            }
            TypePathArgs::Parenthesized { .. } => {}
        }
    }
    ty
}

fn collect_pattern_bindings(pat: &Pattern, out: &mut Vec<(String, Option<Type>)>) {
    match pat {
        Pattern::Binding { ty, name, .. } => {
            if name != "_" {
                out.push((name.clone(), ty.clone()));
            }
        }
        Pattern::BorrowRef { mode, ty, name } => {
            let ref_ty = match mode {
                RefMode::Read => Type::Read {
                    life: None,
                    inner: Box::new(ty.clone()),
                },
                RefMode::Write => Type::Write {
                    life: None,
                    inner: Box::new(ty.clone()),
                },
            };
            out.push((name.clone(), Some(ref_ty)));
        }
        Pattern::DerefPat { inner, .. } | Pattern::Paren(inner) => {
            collect_pattern_bindings(inner, out)
        }
        Pattern::BindAt {
            ref_mode,
            ty,
            name,
            subpat,
            ..
        } => {
            out.push((name.clone(), Some(wrap_ref_mode(*ref_mode, ty.clone()))));
            collect_pattern_bindings(subpat, out);
        }
        Pattern::Tuple(ps) | Pattern::TupleStruct { fields: ps, .. } => {
            for p in ps {
                collect_pattern_bindings(p, out);
            }
        }
        Pattern::Slice(elems) => {
            for el in elems {
                match el {
                    SlicePatElem::Pat(p) => collect_pattern_bindings(p, out),
                    SlicePatElem::RestBind {
                        ref_mode, ty, name, ..
                    } => out.push((
                        name.clone(),
                        ty.as_ref().map(|t| wrap_ref_mode(*ref_mode, t.clone())),
                    )),
                    _ => {}
                }
            }
        }
        Pattern::Struct { fields, .. } => {
            for f in fields {
                if let Some(sub) = &f.pat {
                    collect_pattern_bindings(sub, out);
                } else {
                    out.push((
                        f.name.clone(),
                        f.ty.as_ref().map(|t| wrap_ref_mode(f.ref_mode, t.clone())),
                    ));
                }
            }
        }
        Pattern::Path(pp)
            if pp.qself.is_none()
                && !pp.global
                && pp.segments.len() == 1
                && pp.segments[0]
                    .name
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_lowercase() || c == '_') =>
        {
            out.push((pp.segments[0].name.clone(), None));
        }
        Pattern::Or(alts) => {
            collect_pattern_bindings(&alts[0], out);
        }
        _ => {}
    }
}

fn wrap_ref_mode(mode: Option<RefMode>, ty: Type) -> Type {
    match mode {
        Some(RefMode::Read) => Type::Read {
            life: None,
            inner: Box::new(ty),
        },
        Some(RefMode::Write) => Type::Write {
            life: None,
            inner: Box::new(ty),
        },
        None => ty,
    }
}

fn format_binding_prefix(ref_mode: Option<RefMode>, is_mut: bool) -> &'static str {
    match (ref_mode, is_mut) {
        (Some(RefMode::Read), _) => "ref ",
        (Some(RefMode::Write), _) => "ref mut ",
        (None, true) => "mut ",
        (None, false) => "",
    }
}

fn format_vis(vis: &Visibility) -> String {
    match vis {
        Visibility::Private => String::new(),
        Visibility::Public => "pub ".to_string(),
        Visibility::Internal => "pub(crate) ".to_string(),
        Visibility::Super => "pub(super) ".to_string(),
        Visibility::This => "pub(self) ".to_string(),
        Visibility::InPath(p) => {
            let mapped: Vec<String> = p.iter().map(|s| fmt_rust_ident(s)).collect();
            format!("pub(in {}) ", mapped.join("::"))
        }
    }
}

fn format_generic_args_from_params(params: &[GenericParam]) -> String {
    if params.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = params
        .iter()
        .map(|p| match &p.kind {
            GenericParamKind::Lifetime { name, .. } => fmt_rust_lifetime(name),
            GenericParamKind::Type { name, .. } => fmt_rust_ident(name),
            GenericParamKind::Const { name, .. } => fmt_rust_ident(name),
        })
        .collect();
    format!("<{}>", parts.join(", "))
}

fn format_attr(attr: &Attribute) -> String {
    let prefix = if attr.inner { "#![" } else { "#[" };
    let mapped_name = map_attr_name(&attr.name);
    let inner_body = match &attr.args {
        Some(AttrArgs::Eq(val)) => format!("{} = {}", mapped_name, val),
        Some(AttrArgs::Paren(args)) => {
            let mapped_args = if mapped_name == "inline" {
                match args.as_str() {
                    "Always" => "always".to_string(),
                    "Never" => "never".to_string(),
                    other => other.to_string(),
                }
            } else {
                args.clone()
            };
            format!("{}({})", mapped_name, mapped_args)
        }
        Some(AttrArgs::Bracket(args)) => format!("{}[{}]", mapped_name, args),
        Some(AttrArgs::Brace(args)) => format!("{}{{{}}}", mapped_name, args),
        None => mapped_name.to_string(),
    };
    if attr.is_unsafe {
        format!("{}unsafe({})]", prefix, inner_body)
    } else {
        format!("{}{}]", prefix, inner_body)
    }
}

fn map_attr_name(name: &str) -> &str {
    match name {
        "Derive" => "derive",
        "Test" => "test",
        "Cfg" => "cfg",
        "CfgAttr" => "cfg_attr",
        "Inline" => "inline",
        "Allow" => "allow",
        "Warn" => "warn",
        "Deny" => "deny",
        "Forbid" => "forbid",
        "Expect" => "expect",
        "Repr" => "repr",
        "NoMangle" => "no_mangle",
        "ExportName" => "export_name",
        "LinkSection" => "link_section",
        "Naked" => "naked",
        "MustUse" => "must_use",
        "Deprecated" => "deprecated",
        "ShouldPanic" => "should_panic",
        "Ignore" => "ignore",
        "NonExhaustive" => "non_exhaustive",
        "Doc" => "doc",
        "Cold" => "cold",
        "TrackCaller" => "track_caller",
        "ProcMacro" => "proc_macro",
        "ProcMacroDerive" => "proc_macro_derive",
        "ProcMacroAttribute" => "proc_macro_attribute",
        other => other,
    }
}

pub(crate) fn map_primitive_alias(name: &str) -> Option<&'static str> {
    match name {
        "sbyte" => Some("i8"),
        "byte" => Some("u8"),
        "short" => Some("i16"),
        "ushort" => Some("u16"),
        "int" => Some("i32"),
        "uint" => Some("u32"),
        "long" => Some("i64"),
        "ulong" => Some("u64"),
        "int128" => Some("i128"),
        "uint128" => Some("u128"),
        "nint" => Some("isize"),
        "nuint" => Some("usize"),
        "float" => Some("f32"),
        "double" => Some("f64"),
        "string" => Some("String"),
        _ => None,
    }
}

pub(crate) fn map_std_type_alias(name: &str) -> String {
    match name {
        "List" => "Vec".to_string(),
        "Queue" => "VecDeque".to_string(),
        "Dictionary" => "HashMap".to_string(),
        "SortedDictionary" => "BTreeMap".to_string(),
        "SortedSet" => "BTreeSet".to_string(),
        "PriorityQueue" => "BinaryHeap".to_string(),
        "string" => "String".to_string(),
        other => other.to_string(),
    }
}

fn format_rust_raw_str(prefix: &str, value: &str) -> String {
    let mut hashes = 1usize;
    while value.contains(&format!("\"{}", "#".repeat(hashes))) {
        hashes += 1;
    }
    let h = "#".repeat(hashes);
    format!("{}{}\"{}\"{}", prefix, h, value, h)
}

fn fmt_literal(lit: &Literal) -> String {
    match lit {
        Literal::Int(s) | Literal::Float(s) => s.clone(),
        Literal::Str(s) => format!("\"{}\"", s),
        Literal::RawStr(s) => format_rust_raw_str("r", s),
        Literal::Byte(b) => format!("b'{}'", b),
        Literal::ByteStr { value, raw: false } => format!("b\"{}\"", value),
        Literal::ByteStr { value, raw: true } => format_rust_raw_str("br", value),
        Literal::CStr { value, raw: false } => format!("c\"{}\"", value),
        Literal::CStr { value, raw: true } => format_rust_raw_str("cr", value),
        Literal::Char(c) => format!("'{}'", c),
        Literal::Bool(b) => b.to_string(),
    }
}

fn is_rust_keyword(name: &str) -> bool {
    matches!(
        name,
        "as" | "async"
            | "await"
            | "break"
            | "const"
            | "continue"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "gen"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "static"
            | "struct"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "abstract"
            | "become"
            | "box"
            | "do"
            | "final"
            | "macro"
            | "override"
            | "priv"
            | "try"
            | "typeof"
            | "unsized"
            | "virtual"
            | "yield"
    )
}

fn is_std_collection_alias(name: &str) -> bool {
    matches!(
        name,
        "List" | "Queue" | "Dictionary" | "SortedDictionary" | "SortedSet" | "PriorityQueue"
    )
}

fn register_shadowed_using_aliases(
    tree: &UsingTree,
    enum_variants: &HashMap<String, Vec<String>>,
    out: &mut HashSet<String>,
) {
    match tree {
        UsingTree::Path {
            prefix,
            wildcard,
            alias,
            ..
        } => {
            if let Some(a) = alias {
                if is_std_collection_alias(a) {
                    out.insert(a.clone());
                }
            } else if *wildcard {
                if let Some(last) = prefix.last()
                    && let Some(vs) = enum_variants.get(last)
                {
                    for v in vs {
                        out.insert(v.clone());
                    }
                }
            } else if let Some(last) = prefix.last()
                && !prefix.first().is_some_and(|r| {
                    matches!(r.as_str(), "std" | "core" | "alloc" | "rsharp_prelude")
                })
                && is_std_collection_alias(last)
            {
                out.insert(last.clone());
            }
        }
        UsingTree::Group { prefix, items, .. } => {
            let is_std = prefix
                .first()
                .is_some_and(|r| matches!(r.as_str(), "std" | "core" | "alloc" | "rsharp_prelude"));
            if !is_std {
                for item in items {
                    register_shadowed_using_aliases(item, enum_variants, out);
                }
            }
        }
    }
}

fn is_std_collections_root(expr: &Expr) -> bool {
    match expr {
        Expr::Ident(name) => matches!(name.as_str(), "std" | "core" | "alloc"),
        Expr::Path { segments, .. } => segments
            .first()
            .is_some_and(|s| matches!(s.name.as_str(), "std" | "core" | "alloc")),
        Expr::Member { object, .. } => is_std_collections_root(object),
        _ => false,
    }
}

fn bound_contains_never(b: &TypeBound) -> bool {
    let TypeBound::Trait(path) = b else {
        return false;
    };
    path.segments.iter().any(|seg| match &seg.args {
        TypePathArgs::Parenthesized { params, ret } => {
            params.iter().any(|t| matches!(t, Type::Never))
                || ret.as_deref().is_some_and(|t| matches!(t, Type::Never))
        }
        TypePathArgs::Angle(args) => args.iter().any(|a| match a {
            GenericArg::Type(t) | GenericArg::AssocBinding { ty: t, .. } => {
                matches!(t, Type::Never)
            }
            _ => false,
        }),
        TypePathArgs::None => false,
    })
}

fn is_string_module_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Member { field, .. } => field == "string",
        Expr::Path { segments, .. } => segments.last().is_some_and(|s| s.name == "string"),
        _ => false,
    }
}

fn is_all_caps_const(name: &str) -> bool {
    name.len() >= 2
        && name.bytes().any(|b| b.is_ascii_uppercase())
        && name
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

fn fmt_rust_ident(name: &str) -> String {
    if name.starts_with("r#") {
        name.to_string()
    } else if is_rust_keyword(name) {
        format!("r#{}", name)
    } else {
        name.to_string()
    }
}

fn fmt_rust_lifetime(name: &str) -> String {
    if name == "static" || name == "_" {
        format!("'{}", name)
    } else if is_rust_keyword(name) {
        format!("'r#{}", name)
    } else {
        format!("'{}", name)
    }
}

fn format_macro_call(mc: &MacroCall) -> String {
    let mapped: Vec<String> = mc.path.iter().map(|s| fmt_rust_ident(s)).collect();
    let prefix = if mc.global { "::" } else { "" };
    let path = format!("{}{}", prefix, mapped.join("::"));
    match mc.delim {
        MacroDelim::Paren => format!("{}!({})", path, mc.body),
        MacroDelim::Bracket => format!("{}![{}]", path, mc.body),
        MacroDelim::Brace => format!("{}!{{ {} }}", path, mc.body),
    }
}
