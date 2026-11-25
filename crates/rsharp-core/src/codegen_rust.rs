use std::collections::{HashMap, HashSet};

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
    codegen.emit_file(file)?;
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
    scopes: Vec<HashMap<String, VarInfo>>,
    current_self_type: Option<String>,
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
            scopes: vec![HashMap::new()],
            current_self_type: None,
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn declare_var(&mut self, name: &str, ty: &Type) {
        let info = var_info_from_type(ty);
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), info);
        }
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
                }
                ItemKind::Enum(e) => {
                    self.known_paths.insert(e.name.clone());
                }
                ItemKind::Union(u) => {
                    self.known_paths.insert(u.name.clone());
                }
                ItemKind::Trait(t) => {
                    self.known_paths.insert(t.name.clone());
                }
                ItemKind::Alias(a) => {
                    self.known_paths.insert(a.name.clone());
                }
                ItemKind::ExternCrate(ec) => {
                    let key = ec.alias.clone().unwrap_or_else(|| ec.name.clone());
                    if key != "_" {
                        self.known_paths.insert(key);
                    }
                }
                ItemKind::Namespace(ns) => {
                    if let Some(first) = ns.path.first() {
                        self.known_paths.insert(first.clone());
                    }
                }
                ItemKind::Using(u) => {
                    register_using_tree(&u.tree, &mut self.known_paths);
                }
                _ => {}
            }
        }
    }

    fn emit_file(&mut self, file: &FileAst) -> Result<()> {
        self.register_items(&file.items);

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
            PreludeMode::None => {}
        }

        let mut idx = 0;
        let mut file_ns: Option<Vec<String>> = None;
        if let Some(first) = file.items.first()
            && let ItemKind::Namespace(ns) = &first.kind
            && ns.items.is_none()
        {
            file_ns = Some(ns.path.clone());
            idx = 1;
        }

        if let Some(ref ns_path) = file_ns {
            for seg in ns_path {
                self.line(&format!("pub mod {} {{", seg));
                self.indent += 1;
                if self.options.prelude_mode != PreludeMode::None {
                    self.line("use super::*;");
                }
            }
        }

        for item in &file.items[idx..] {
            self.emit_item(item)?;
        }

        if let Some(ref ns_path) = file_ns {
            for _ in ns_path {
                self.indent -= 1;
                self.line("}");
            }
        }
        Ok(())
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

    fn emit_item(&mut self, item: &Item) -> Result<()> {
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
                        self.emit_item(sub)?;
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
            ItemKind::Struct(s) => self.emit_struct(&vis, s)?,
            ItemKind::Enum(e) => self.emit_enum(&vis, e)?,
            ItemKind::Union(u) => self.emit_union(&vis, u)?,
            ItemKind::Trait(t) => self.emit_trait(&vis, t)?,
            ItemKind::Impl(im) => self.emit_impl(im)?,
            ItemKind::Alias(a) => {
                let gens = self.fmt_generics(&a.generics)?;
                let wh = self.fmt_where(&a.where_clause)?;
                let target = self.fmt_type(&a.target)?;
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
                let ty = self.fmt_type(&c.ty)?;
                let val = self.fmt_expr(&c.value)?;
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
                let ty = self.fmt_type(&s.ty)?;
                let name = fmt_rust_ident(&s.name);
                match &s.value {
                    Some(v) => {
                        let val = self.fmt_expr(v)?;
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
            ItemKind::Fn(f) => self.emit_fn(&vis, f)?,
            ItemKind::ExternBlock(eb) => {
                let abi = eb
                    .abi
                    .as_ref()
                    .map(|a| format!(" \"{}\"", a))
                    .unwrap_or_default();
                // Rust 2024 requires `unsafe extern` for all extern blocks.
                let _ = eb.is_unsafe;
                self.line(&format!("unsafe extern{} {{", abi));
                self.indent += 1;
                self.emit_inner_docs_and_attrs(&eb.inner_docs, &eb.inner_attrs);
                for sub in &eb.items {
                    self.emit_item(sub)?;
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
        Ok(())
    }

    fn emit_struct(&mut self, vis: &str, s: &StructDecl) -> Result<()> {
        let gens = self.fmt_generics(&s.generics)?;
        let wh = self.fmt_where(&s.where_clause)?;
        match &s.kind {
            StructKind::Unit => {
                self.line(&format!("{}struct {}{}{};", vis, s.name, gens, wh));
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
                    p.push_str(&self.fmt_type(&f.ty)?);
                    fstrs.push(p);
                }
                self.line(&format!(
                    "{}struct {}{}({}){};",
                    vis,
                    s.name,
                    gens,
                    fstrs.join(", "),
                    wh
                ));
            }
            StructKind::Named(fields) => {
                self.line(&format!(
                    "{}struct {}{}{} {{",
                    vis,
                    fmt_rust_ident(&s.name),
                    gens,
                    wh
                ));
                self.indent += 1;
                for f in fields {
                    self.emit_docs_and_attrs(&f.docs, &f.attrs);
                    let fvis = format_vis(&f.vis);
                    let fty = self.fmt_type(&f.ty)?;
                    self.line(&format!("{}{}: {},", fvis, fmt_rust_ident(&f.name), fty));
                }
                self.indent -= 1;
                self.line("}");
            }
        }

        if !s.methods.is_empty() {
            let gen_args = format_generic_args_from_params(&s.generics);
            self.line(&format!("impl{} {}{}{} {{", gens, s.name, gen_args, wh));
            let prev_self = self.current_self_type.replace(s.name.clone());
            self.indent += 1;
            for m in &s.methods {
                self.emit_item(m)?;
            }
            self.indent -= 1;
            self.current_self_type = prev_self;
            self.line("}");
        }
        Ok(())
    }

    fn emit_enum(&mut self, vis: &str, e: &EnumDecl) -> Result<()> {
        let gens = self.fmt_generics(&e.generics)?;
        let wh = self.fmt_where(&e.where_clause)?;
        self.line(&format!("{}enum {}{}{} {{", vis, e.name, gens, wh));
        self.indent += 1;
        for v in &e.variants {
            self.emit_docs_and_attrs(&v.docs, &v.attrs);
            let disc = match &v.discriminant {
                Some(ex) => format!(" = {}", self.fmt_expr(ex)?),
                None => String::new(),
            };
            match &v.kind {
                VariantKind::Unit => {
                    self.line(&format!("{}{},", v.name, disc));
                }
                VariantKind::Tuple(fields) => {
                    let mut tys = Vec::new();
                    for f in fields {
                        let mut p = String::new();
                        for a in &f.attrs {
                            p.push_str(&format_attr(a));
                            p.push(' ');
                        }
                        p.push_str(&self.fmt_type(&f.ty)?);
                        tys.push(p);
                    }
                    self.line(&format!(
                        "{}({}){},",
                        fmt_rust_ident(&v.name),
                        tys.join(", "),
                        disc
                    ));
                }
                VariantKind::Named(fields) => {
                    self.line(&format!("{} {{", v.name));
                    self.indent += 1;
                    for f in fields {
                        self.emit_docs_and_attrs(&f.docs, &f.attrs);
                        let fty = self.fmt_type(&f.ty)?;
                        self.line(&format!("{}: {},", f.name, fty));
                    }
                    self.indent -= 1;
                    self.line(&format!("}}{},", disc));
                }
            }
        }
        self.indent -= 1;
        self.line("}");

        if !e.methods.is_empty() {
            let gen_args = format_generic_args_from_params(&e.generics);
            self.line(&format!("impl{} {}{}{} {{", gens, e.name, gen_args, wh));
            let prev_self = self.current_self_type.replace(e.name.clone());
            self.indent += 1;
            for m in &e.methods {
                self.emit_item(m)?;
            }
            self.indent -= 1;
            self.current_self_type = prev_self;
            self.line("}");
        }
        Ok(())
    }

    fn emit_union(&mut self, vis: &str, u: &UnionDecl) -> Result<()> {
        let gens = self.fmt_generics(&u.generics)?;
        let wh = self.fmt_where(&u.where_clause)?;
        let uname = fmt_rust_ident(&u.name);
        self.line(&format!("{}union {}{}{} {{", vis, uname, gens, wh));
        self.indent += 1;
        for f in &u.fields {
            self.emit_docs_and_attrs(&f.docs, &f.attrs);
            let fvis = format_vis(&f.vis);
            let fty = self.fmt_type(&f.ty)?;
            self.line(&format!("{}{}: {},", fvis, fmt_rust_ident(&f.name), fty));
        }
        self.indent -= 1;
        self.line("}");

        if !u.methods.is_empty() {
            let gen_args = format_generic_args_from_params(&u.generics);
            self.line(&format!("impl{} {}{}{} {{", gens, uname, gen_args, wh));
            let prev_self = self.current_self_type.replace(u.name.clone());
            self.indent += 1;
            for m in &u.methods {
                self.emit_item(m)?;
            }
            self.indent -= 1;
            self.current_self_type = prev_self;
            self.line("}");
        }
        Ok(())
    }

    fn emit_trait(&mut self, vis: &str, t: &TraitDecl) -> Result<()> {
        let unsf = if t.is_unsafe { "unsafe " } else { "" };
        let gens = self.fmt_generics(&t.generics)?;
        let supers = if t.supertraits.is_empty() {
            String::new()
        } else {
            let bs: Result<Vec<_>> = t.supertraits.iter().map(|b| self.fmt_bound(b)).collect();
            format!(": {}", bs?.join(" + "))
        };
        let wh = self.fmt_where(&t.where_clause)?;
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
                TraitItem::Fn(item) => {
                    self.emit_docs_and_attrs(&item.docs, &item.attrs);
                    if let ItemKind::Fn(f) = &item.kind {
                        self.emit_fn("", f)?;
                    }
                }
                TraitItem::AssocType {
                    attrs,
                    name,
                    generics,
                    bounds,
                    where_clause,
                    default,
                } => {
                    for a in attrs {
                        self.line(&format_attr(a));
                    }
                    let ag = self.fmt_generics(generics)?;
                    let b = if bounds.is_empty() {
                        String::new()
                    } else {
                        let bs: Result<Vec<_>> = bounds.iter().map(|b| self.fmt_bound(b)).collect();
                        format!(": {}", bs?.join(" + "))
                    };
                    let d = match default {
                        Some(ty) => format!(" = {}", self.fmt_type(ty)?),
                        None => String::new(),
                    };
                    let aw = self.fmt_where(where_clause)?;
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
                    attrs,
                    ty,
                    name,
                    default,
                } => {
                    for a in attrs {
                        self.line(&format_attr(a));
                    }
                    let ty_s = self.fmt_type(ty)?;
                    let d = match default {
                        Some(ex) => format!(" = {}", self.fmt_expr(ex)?),
                        None => String::new(),
                    };
                    self.line(&format!("const {}: {}{};", fmt_rust_ident(name), ty_s, d));
                }
                TraitItem::MacroCall(mc) => {
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
        Ok(())
    }

    fn emit_impl(&mut self, im: &ImplDecl) -> Result<()> {
        let unsf = if im.is_unsafe { "unsafe " } else { "" };
        let gens = self.fmt_generics(&im.generics)?;
        let target = self.fmt_type(&im.target_ty)?;
        let wh = self.fmt_where(&im.where_clause)?;
        let header = if let Some(tp) = &im.trait_path {
            format!(
                "{}impl{} {} for {}{} {{",
                unsf,
                gens,
                self.fmt_type_path(tp)?,
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
                ImplItem::Fn(it) => {
                    if im.trait_path.is_some() {
                        self.emit_docs_and_attrs(&it.docs, &it.attrs);
                        if let ItemKind::Fn(f) = &it.kind {
                            self.emit_fn("", f)?;
                        }
                    } else {
                        self.emit_item(it)?;
                    }
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
                    let ag = self.fmt_generics(generics)?;
                    let t = self.fmt_type(ty)?;
                    let aw = self.fmt_where(where_clause)?;
                    self.line(&format!(
                        "type {}{} = {}{};",
                        fmt_rust_ident(name),
                        ag,
                        t,
                        aw
                    ));
                }
                ImplItem::AssocConst(it) => self.emit_item(it)?,
                ImplItem::MacroCall(mc) => {
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
        Ok(())
    }

    fn emit_fn(&mut self, vis: &str, f: &FnDecl) -> Result<()> {
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
        sig.push_str(&self.fmt_generics(&f.generics)?);
        sig.push('(');

        self.push_scope();
        let mut pstrs = Vec::new();
        for p in &f.params {
            pstrs.push(self.fmt_param(p)?);
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
            sig.push_str(&self.fmt_type(effective_ret)?);
        }
        sig.push_str(&self.fmt_where(&f.where_clause)?);

        if let Some(body) = &f.body {
            self.line(&format!("{} {{", sig));
            self.indent += 1;
            self.emit_block_stmts(body)?;
            self.indent -= 1;
            self.line("}");
        } else {
            self.line(&format!("{};", sig));
        }
        self.pop_scope();
        Ok(())
    }

    fn fmt_param(&mut self, p: &Param) -> Result<String> {
        Ok(match p {
            Param::Receiver(rk) => {
                self.declare_var("this", &Type::Void);
                match rk {
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
                        format!("{}self: {}", mk, self.fmt_type(ty)?)
                    }
                }
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
                    self.fmt_type(ty)?
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
                let pat_s = self.fmt_pattern(pat)?;
                let ty = explicit_ty
                    .clone()
                    .or_else(|| infer_param_pattern_type(pat))
                    .ok_or_else(|| {
                        RSharpError::codegen("Could not determine parameter type from pattern")
                    })?;
                format!("{}{}: {}", prefix, pat_s, self.fmt_type(&ty)?)
            }
        })
    }

    fn emit_value_block_stmts(&mut self, block: &Block) -> Result<()> {
        self.push_scope();
        self.emit_inner_docs_and_attrs(&block.inner_docs, &block.inner_attrs);
        if let Some((last, init)) = block.stmts.split_last()
            && let StmtKind::Return(Some(tail)) = &last.kind
        {
            for st in init {
                self.emit_stmt(st)?;
            }
            let t = self.fmt_expr(tail)?;
            self.line(&t);
        } else {
            for st in &block.stmts {
                self.emit_stmt(st)?;
            }
        }
        self.pop_scope();
        Ok(())
    }

    fn emit_block_stmts(&mut self, block: &Block) -> Result<()> {
        self.push_scope();
        self.emit_inner_docs_and_attrs(&block.inner_docs, &block.inner_attrs);
        for stmt in &block.stmts {
            self.emit_stmt(stmt)?;
        }
        self.pop_scope();
        Ok(())
    }

    fn emit_stmt(&mut self, stmt: &Stmt) -> Result<()> {
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
                let ty_s = self.fmt_type(ty)?;
                let init_s = match init {
                    Some(expr) => {
                        let rhs = if is_list_type(ty) {
                            match expr {
                                Expr::ArrayLit(elems) => {
                                    let items: Result<Vec<_>> =
                                        elems.iter().map(|e| self.fmt_expr(e)).collect();
                                    format!("vec![{}]", items?.join(", "))
                                }
                                Expr::ArrayRepeat { value, count } => {
                                    format!(
                                        "vec![{}; {}]",
                                        self.fmt_expr(value)?,
                                        self.fmt_expr(count)?
                                    )
                                }
                                _ => self.fmt_expr(expr)?,
                            }
                        } else {
                            self.fmt_expr(expr)?
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
                    let check_bounds: Result<Vec<String>> = bounds
                        .iter()
                        .filter(|b| {
                            !matches!(b, TypeBound::PreciseCapture(_) | TypeBound::Lifetime(_))
                        })
                        .map(|b| self.fmt_bound(b))
                        .collect();
                    let check_bounds = check_bounds?;
                    if !check_bounds.is_empty() {
                        self.line(&format!(
                            "{{ fn __rsharp_impl_check<__T: {}>(_: &__T) {{}} __rsharp_impl_check(&{}); }}",
                            check_bounds.join(" + "),
                            rname
                        ));
                    }
                } else {
                    self.line(&format!("let {}{}: {}{};", m, rname, ty_s, init_s));
                }
            }
            StmtKind::Destructure { pat, init } => {
                let pat_s = self.fmt_pattern(pat)?;
                let rhs = self.fmt_expr(init)?;
                self.line(&format!("let {} = {};", pat_s, rhs));
                self.emit_pattern_type_checks(pat)?;
            }
            StmtKind::DestructureAssign { pat, value } => {
                let pat_s = self.fmt_pattern(pat)?;
                let rhs = self.fmt_expr(value)?;
                self.line(&format!("{} = {};", pat_s, rhs));
            }
            StmtKind::Discard(expr) => {
                let e = self.fmt_expr(expr)?;
                self.line(&format!("let _ = {};", e));
            }
            StmtKind::Item(item) => {
                self.register_items(std::slice::from_ref(item.as_ref()));
                self.emit_item(item)?;
            }
            StmtKind::Return(expr) => match expr {
                Some(e) => {
                    let s = self.fmt_expr(e)?;
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
                    line.push_str(&self.fmt_expr(v)?);
                }
                line.push(';');
                self.line(&line);
            }
            StmtKind::Continue { label } => match label {
                Some(l) => self.line(&format!("continue {};", fmt_rust_lifetime(l))),
                None => self.line("continue;"),
            },
            StmtKind::If(if_stmt) => self.emit_if(if_stmt, false)?,
            StmtKind::Require {
                expr,
                pat,
                else_block,
            } => {
                let pat_s = self.fmt_pattern(pat)?;
                let expr_s = self.fmt_expr(expr)?;
                self.line(&format!("let {} = {} else {{", pat_s, expr_s));
                self.indent += 1;
                self.emit_block_stmts(else_block)?;
                self.indent -= 1;
                self.line("};");
                self.emit_pattern_type_checks(pat)?;
            }
            StmtKind::While { label, cond, body } => {
                let lbl = label
                    .as_ref()
                    .map(|l| format!("{}: ", fmt_rust_lifetime(l)))
                    .unwrap_or_default();
                self.emit_cond_block(&format!("{}while ", lbl), cond, body)?;
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
                    self.emit_stmt(init_stmt)?;
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
                    let step_s = self.fmt_expr(step_expr)?;
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
                    let c = self.fmt_expr(cond_expr)?;
                    self.line(&format!("if !({}) {{ break; }}", c));
                }
                for st in &body.stmts {
                    self.emit_stmt(st)?;
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
                let p = self.fmt_pattern(pat)?;
                let it = self.fmt_expr(iter)?;
                self.line(&format!("{}for {} in {} {{", lbl, p, it));
                self.indent += 1;
                self.push_scope();
                self.emit_pattern_type_checks(pat)?;
                for st in &body.stmts {
                    self.emit_stmt(st)?;
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
                self.emit_block_stmts(body)?;
                self.indent -= 1;
                self.line("}");
            }
            StmtKind::Match(m) => {
                self.emit_match(m)?;
            }
            StmtKind::Unsafe(b) => {
                self.line("unsafe {");
                self.indent += 1;
                self.emit_block_stmts(b)?;
                self.indent -= 1;
                self.line("}");
            }
            StmtKind::InlineRust(raw) => {
                for l in raw.trim().lines() {
                    self.line(l);
                }
            }
            StmtKind::Expr(expr) => {
                let s = self.fmt_expr(expr)?;
                self.line(&format!("{};", s));
            }
        }
        Ok(())
    }

    fn emit_cond_block(&mut self, prefix: &str, cond: &Condition, body: &Block) -> Result<()> {
        match cond {
            Condition::Bool(e) => {
                let s = self.fmt_expr(e)?;
                self.line(&format!("{}{} {{", prefix, s));
                self.indent += 1;
                self.emit_block_stmts(body)?;
                self.indent -= 1;
            }
            Condition::IsPattern {
                expr,
                negated: false,
                pat,
            } => {
                let p = self.fmt_pattern(pat)?;
                let e = self.fmt_expr(expr)?;
                self.line(&format!("{}let {} = {} {{", prefix, p, e));
                self.indent += 1;
                self.push_scope();
                self.emit_pattern_type_checks(pat)?;
                for st in &body.stmts {
                    self.emit_stmt(st)?;
                }
                self.pop_scope();
                self.indent -= 1;
            }
            Condition::IsPattern {
                expr,
                negated: true,
                pat,
            } => {
                let p = self.fmt_pattern(pat)?;
                let e = self.fmt_expr(expr)?;
                self.line(&format!("{}!matches!({}, {}) {{", prefix, e, p));
                self.indent += 1;
                self.emit_block_stmts(body)?;
                self.indent -= 1;
            }
            Condition::LetChain(items) => {
                self.push_scope();
                let mut parts = Vec::new();
                let mut pats = Vec::new();
                for item in items {
                    match item {
                        ChainItem::IsPattern { expr, pat } => {
                            let p = self.fmt_pattern(pat)?;
                            let e = self.fmt_expr(expr)?;
                            self.register_pattern_vars(pat);
                            parts.push(format!("let {} = {}", p, e));
                            pats.push(pat);
                        }
                        ChainItem::Bool(e) => {
                            parts.push(self.fmt_expr(e)?);
                        }
                    }
                }
                self.line(&format!("{}{} {{", prefix, parts.join(" && ")));
                self.indent += 1;
                for pat in pats {
                    self.emit_pattern_type_checks(pat)?;
                }
                for st in &body.stmts {
                    self.emit_stmt(st)?;
                }
                self.indent -= 1;
                self.pop_scope();
            }
        }
        Ok(())
    }

    fn emit_if(&mut self, if_stmt: &IfStmt, is_else_if: bool) -> Result<()> {
        let prefix = if is_else_if { "else if " } else { "if " };
        self.emit_cond_block(prefix, &if_stmt.cond, &if_stmt.then_block)?;
        match &if_stmt.else_branch {
            Some(ElseBranch::ElseIf(next)) => {
                self.line("}");
                self.emit_if(next, true)?;
            }
            Some(ElseBranch::Else(eb)) => {
                self.line("} else {");
                self.indent += 1;
                self.emit_block_stmts(eb)?;
                self.indent -= 1;
                self.line("}");
            }
            None => self.line("}"),
        }
        Ok(())
    }

    fn fmt_condition_expr<'a>(
        &self,
        cond: &'a Condition,
        sub: &mut RustCodegen,
        bound_pats: &mut Vec<&'a Pattern>,
    ) -> Result<String> {
        Ok(match cond {
            Condition::Bool(e) => sub.fmt_expr(e)?,
            Condition::IsPattern {
                expr,
                negated: false,
                pat,
            } => {
                let p = sub.fmt_pattern(pat)?;
                let e = sub.fmt_expr(expr)?;
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
                sub.fmt_expr(expr)?,
                sub.fmt_pattern(pat)?
            ),
            Condition::LetChain(items) => {
                let mut parts = Vec::new();
                for item in items {
                    match item {
                        ChainItem::IsPattern { expr, pat } => {
                            let p = sub.fmt_pattern(pat)?;
                            let e = sub.fmt_expr(expr)?;
                            sub.register_pattern_vars(pat);
                            bound_pats.push(pat);
                            parts.push(format!("let {} = {}", p, e));
                        }
                        ChainItem::Bool(e) => {
                            parts.push(sub.fmt_expr(e)?);
                        }
                    }
                }
                parts.join(" && ")
            }
        })
    }

    fn emit_match(&mut self, m: &MatchExpr) -> Result<()> {
        let scr = self.fmt_expr(&m.scrutinee)?;
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
                let const_cond = format!("__rsharp_c == const {{ {} }}", self.fmt_expr(ce)?);
                let combined_guard = match &arm.guard {
                    Some(gc) => {
                        let gs = self.fmt_condition_expr(gc, &mut guard_sub, &mut guard_pats)?;
                        format!(" if ({}) && {}", const_cond, gs)
                    }
                    None => format!(" if {}", const_cond),
                };
                ("__rsharp_c".to_string(), combined_guard)
            } else {
                let p = self.fmt_pattern(&arm.pat)?;
                let g = match &arm.guard {
                    Some(gc) => {
                        let gs = self.fmt_condition_expr(gc, &mut guard_sub, &mut guard_pats)?;
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
                    self.emit_pattern_type_checks(&arm.pat)?;
                    for gp in &guard_pats {
                        self.emit_pattern_type_checks(gp)?;
                    }
                    for st in &b.stmts {
                        self.emit_stmt(st)?;
                    }
                    self.pop_scope();
                    self.indent -= 1;
                    self.line("}");
                }
                MatchArmBody::Expr(e) => {
                    let mut checks = self.collect_pattern_type_checks(&arm.pat)?;
                    for gp in &guard_pats {
                        checks.extend(self.collect_pattern_type_checks(gp)?);
                    }
                    self.push_scope();
                    self.register_pattern_vars(&arm.pat);
                    for gp in &guard_pats {
                        self.register_pattern_vars(gp);
                    }
                    let es = self.fmt_expr(e)?;
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
        self.line("}");
        Ok(())
    }

    fn register_pattern_vars(&mut self, pat: &Pattern) {
        let mut bindings = Vec::new();
        collect_pattern_bindings(pat, &mut bindings);
        for (name, ty) in bindings {
            self.declare_var(&name, &ty);
        }
    }

    fn collect_pattern_type_checks(&self, pat: &Pattern) -> Result<Vec<String>> {
        let mut bindings = Vec::new();
        collect_pattern_bindings(pat, &mut bindings);
        let mut checks = Vec::new();
        for (name, ty) in bindings {
            let ty_s = self.fmt_type(&ty)?;
            checks.push(format!("let _: &{} = &{};", ty_s, fmt_rust_ident(&name)));
        }
        Ok(checks)
    }

    fn emit_pattern_type_checks(&mut self, pat: &Pattern) -> Result<()> {
        let checks = self.collect_pattern_type_checks(pat)?;
        self.register_pattern_vars(pat);
        for c in checks {
            self.line(&c);
        }
        Ok(())
    }

    // ==================== TYPES & PATTERNS ====================

    fn fmt_type(&self, ty: &Type) -> Result<String> {
        Ok(match ty {
            Type::Void => "()".to_string(),
            Type::Never => "!".to_string(),
            Type::Dynamic => "Box<dyn std::any::Any>".to_string(),
            Type::Infer => "_".to_string(),
            Type::Path(p) => self.fmt_type_path(p)?,
            Type::QualifiedPath {
                self_ty,
                trait_path,
                item,
                args,
            } => {
                let mut out = format!(
                    "<{} as {}>::{}",
                    self.fmt_type(self_ty)?,
                    self.fmt_type_path(trait_path)?,
                    item
                );
                if !args.is_empty() {
                    let ga: Result<Vec<_>> = args.iter().map(|a| self.fmt_generic_arg(a)).collect();
                    out.push_str(&format!("<{}>", ga?.join(", ")));
                }
                out
            }
            Type::Grouped(inner) => format!("({})", self.fmt_type(inner)?),
            Type::Read { life, inner } => {
                let l = life
                    .as_ref()
                    .map(|n| format!("{} ", fmt_rust_lifetime(n)))
                    .unwrap_or_default();
                format!("&{}{}", l, self.fmt_type(inner)?)
            }
            Type::Write { life, inner } => {
                let l = life
                    .as_ref()
                    .map(|n| format!("{} ", fmt_rust_lifetime(n)))
                    .unwrap_or_default();
                format!("&{}mut {}", l, self.fmt_type(inner)?)
            }
            Type::Slice(inner) => format!("[{}]", self.fmt_type(inner)?),
            Type::ReadPtr(inner) => format!("*const {}", self.fmt_type(inner)?),
            Type::WritePtr(inner) => format!("*mut {}", self.fmt_type(inner)?),
            Type::Optional(inner) => format!("Option<{}>", self.fmt_type(inner)?),
            Type::Array { elem, dims } => {
                let mut cur = self.fmt_type(elem)?;
                for d in dims.iter().rev() {
                    cur = format!("[{}; {}]", cur, self.fmt_expr(d)?);
                }
                cur
            }
            Type::Tuple(elems) => {
                let parts: Result<Vec<_>> = elems.iter().map(|e| self.fmt_type(&e.ty)).collect();
                let parts = parts?;
                if parts.len() == 1 {
                    format!("({},)", parts[0])
                } else {
                    format!("({})", parts.join(", "))
                }
            }
            Type::DynTrait(bounds) => {
                let bs: Result<Vec<_>> = bounds.iter().map(|b| self.fmt_bound(b)).collect();
                format!("dyn {}", bs?.join(" + "))
            }
            Type::ImplTrait(bounds) => {
                let bs: Result<Vec<_>> = bounds.iter().map(|b| self.fmt_bound(b)).collect();
                format!("impl {}", bs?.join(" + "))
            }
            Type::FnPtr {
                lifetimes,
                is_unsafe,
                extern_abi,
                params,
                is_variadic,
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
                    let t = self.fmt_type(&p.ty)?;
                    if let Some(name) = &p.name {
                        ps.push(format!("{}: {}", fmt_rust_ident(name), t));
                    } else {
                        ps.push(t);
                    }
                }
                if *is_variadic {
                    ps.push("...".to_string());
                }
                let r = self.fmt_type(ret)?;
                if matches!(ret.as_ref(), Type::Void) {
                    format!("{}fn({})", prefix, ps.join(", "))
                } else {
                    format!("{}fn({}) -> {}", prefix, ps.join(", "), r)
                }
            }
            Type::Macro(mc) => format_macro_call(mc),
        })
    }

    fn fmt_type_path(&self, path: &TypePath) -> Result<String> {
        if !path.global
            && path.segments.len() == 1
            && matches!(path.segments[0].args, TypePathArgs::None)
            && let Some(prim) = map_primitive_alias(&path.segments[0].name)
        {
            return Ok(prim.to_string());
        }
        let mut out = Vec::new();
        for seg in &path.segments {
            let mapped = fmt_rust_ident(&map_std_type_alias(&seg.name));
            match &seg.args {
                TypePathArgs::None => out.push(mapped),
                TypePathArgs::Angle(args) => {
                    let ga: Result<Vec<_>> = args.iter().map(|a| self.fmt_generic_arg(a)).collect();
                    out.push(format!("{}<{}>", mapped, ga?.join(", ")));
                }
                TypePathArgs::Parenthesized { params, ret } => {
                    let ps: Result<Vec<_>> = params.iter().map(|p| self.fmt_type(p)).collect();
                    let ret_s = match ret {
                        Some(r) if !matches!(r.as_ref(), Type::Void) => {
                            format!(" -> {}", self.fmt_type(r)?)
                        }
                        _ => String::new(),
                    };
                    out.push(format!("{}({}){}", mapped, ps?.join(", "), ret_s));
                }
            }
        }
        let joined = out.join("::");
        if path.global {
            Ok(format!("::{}", joined))
        } else {
            Ok(joined)
        }
    }

    fn fmt_generic_arg(&self, arg: &GenericArg) -> Result<String> {
        Ok(match arg {
            GenericArg::Lifetime(l) => fmt_rust_lifetime(l),
            GenericArg::Type(t) => self.fmt_type(t)?,
            GenericArg::AssocBinding { name, generics, ty } => {
                let gs = if generics.is_empty() {
                    String::new()
                } else {
                    let ga: Result<Vec<_>> =
                        generics.iter().map(|a| self.fmt_generic_arg(a)).collect();
                    format!("<{}>", ga?.join(", "))
                };
                format!("{}{} = {}", fmt_rust_ident(name), gs, self.fmt_type(ty)?)
            }
            GenericArg::AssocBound {
                name,
                generics,
                bounds,
            } => {
                let gs = if generics.is_empty() {
                    String::new()
                } else {
                    let ga: Result<Vec<_>> =
                        generics.iter().map(|a| self.fmt_generic_arg(a)).collect();
                    format!("<{}>", ga?.join(", "))
                };
                let bs: Result<Vec<_>> = bounds.iter().map(|b| self.fmt_bound(b)).collect();
                format!("{}{}: {}", fmt_rust_ident(name), gs, bs?.join(" + "))
            }
            GenericArg::ConstExpr(e) => self.fmt_expr(e)?,
            GenericArg::ConstBlock(e) => format!("{{ {} }}", self.fmt_expr(e)?),
        })
    }

    fn fmt_bound(&self, b: &TypeBound) -> Result<String> {
        Ok(match b {
            TypeBound::Trait(tp) => self.fmt_type_path(tp)?,
            TypeBound::Relaxed(tp) => format!("?{}", self.fmt_type_path(tp)?),
            TypeBound::Forall {
                lifetimes,
                trait_path,
            } => {
                let ls: Vec<String> = lifetimes.iter().map(|l| fmt_rust_lifetime(l)).collect();
                format!("for<{}> {}", ls.join(", "), self.fmt_type_path(trait_path)?)
            }
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
        })
    }

    fn fmt_generics(&self, params: &[GenericParam]) -> Result<String> {
        if params.is_empty() {
            return Ok(String::new());
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
                        let bs: Result<Vec<_>> = bounds.iter().map(|b| self.fmt_bound(b)).collect();
                        s.push_str(&format!(": {}", bs?.join(" + ")));
                    }
                    if let Some(def) = default {
                        s.push_str(&format!(" = {}", self.fmt_type(def)?));
                    }
                    parts.push(s);
                }
                GenericParamKind::Const { ty, name, default } => {
                    let mut s = format!(
                        "{}const {}: {}",
                        prefix,
                        fmt_rust_ident(name),
                        self.fmt_type(ty)?
                    );
                    if let Some(def) = default {
                        s.push_str(&format!(" = {}", self.fmt_expr(def)?));
                    }
                    parts.push(s);
                }
            }
        }
        Ok(format!("<{}>", parts.join(", ")))
    }

    fn fmt_where(&self, wc: &Option<WhereClause>) -> Result<String> {
        let Some(wc) = wc else {
            return Ok(String::new());
        };
        if wc.predicates.is_empty() {
            return Ok(String::new());
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
                    let bs: Result<Vec<_>> = bounds.iter().map(|b| self.fmt_bound(b)).collect();
                    preds.push(format!(
                        "{}{}: {}",
                        forall,
                        self.fmt_type(ty)?,
                        bs?.join(" + ")
                    ));
                }
                WherePredicate::Lifetime { life, outlives } => {
                    let os: Vec<String> = outlives.iter().map(|o| fmt_rust_lifetime(o)).collect();
                    preds.push(format!("{}: {}", fmt_rust_lifetime(life), os.join(" + ")));
                }
            }
        }
        Ok(format!(" where {}", preds.join(", ")))
    }

    fn fmt_pattern(&self, pat: &Pattern) -> Result<String> {
        Ok(match pat {
            Pattern::Wildcard => "_".to_string(),
            Pattern::Rest => "..".to_string(),
            Pattern::Lit(l) => fmt_literal(l),
            Pattern::Const(e) => format!("const {{ {} }}", self.fmt_expr(e)?),
            Pattern::Range {
                start,
                end,
                inclusive,
            } => {
                let s = match start {
                    Some(e) => self.fmt_expr(e)?,
                    None => String::new(),
                };
                let e = match end {
                    Some(ex) => self.fmt_expr(ex)?,
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
                    format!("&mut {}", self.fmt_pattern(inner)?)
                } else {
                    format!("&{}", self.fmt_pattern(inner)?)
                }
            }
            Pattern::Paren(inner) => format!("({})", self.fmt_pattern(inner)?),
            Pattern::BindAt {
                is_mut,
                name,
                subpat,
                ..
            } => {
                let mk = if *is_mut { "mut " } else { "" };
                format!(
                    "{}{} @ {}",
                    mk,
                    fmt_rust_ident(name),
                    self.fmt_pattern(subpat)?
                )
            }
            Pattern::Tuple(ps) => {
                let items: Result<Vec<_>> = ps.iter().map(|p| self.fmt_pattern(p)).collect();
                let items = items?;
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
                        SlicePatElem::Pat(p) => parts.push(self.fmt_pattern(p)?),
                        SlicePatElem::Rest => parts.push("..".to_string()),
                        SlicePatElem::RestBind { is_mut, name, .. } => {
                            let mk = if *is_mut { "mut " } else { "" };
                            parts.push(format!("{}{} @ ..", mk, fmt_rust_ident(name)));
                        }
                    }
                }
                format!("[{}]", parts.join(", "))
            }
            Pattern::TupleStruct { path, fields } => {
                let fs: Result<Vec<_>> = fields.iter().map(|f| self.fmt_pattern(f)).collect();
                let mapped: Vec<String> = path.iter().map(|s| fmt_rust_ident(s)).collect();
                format!("{}({})", mapped.join("::"), fs?.join(", "))
            }
            Pattern::Struct { path, fields, rest } => {
                let mut parts = Vec::new();
                for f in fields {
                    let fname = fmt_rust_ident(&f.name);
                    match &f.pat {
                        Some(sub) => parts.push(format!("{}: {}", fname, self.fmt_pattern(sub)?)),
                        None if f.is_mut => parts.push(format!("mut {}", fname)),
                        None => parts.push(fname),
                    }
                }
                if *rest {
                    parts.push("..".to_string());
                }
                let mapped: Vec<String> = path.iter().map(|s| fmt_rust_ident(s)).collect();
                format!("{} {{ {} }}", mapped.join("::"), parts.join(", "))
            }
            Pattern::Path(p) => {
                let mapped: Vec<String> = p.iter().map(|s| fmt_rust_ident(s)).collect();
                mapped.join("::")
            }
            Pattern::Or(alts) => {
                let items: Result<Vec<_>> = alts.iter().map(|p| self.fmt_pattern(p)).collect();
                items?.join(" | ")
            }
            Pattern::Macro(mc) => format_macro_call(mc),
        })
    }

    // ==================== EXPRESSIONS ====================

    fn fmt_expr(&self, expr: &Expr) -> Result<String> {
        Ok(match expr {
            Expr::Lit(l) => fmt_literal(l),
            Expr::Infer => "_".to_string(),
            Expr::Attributed { attrs, expr } => {
                let mut prefix = String::new();
                for a in attrs {
                    prefix.push_str(&format_attr(a));
                    prefix.push(' ');
                }
                // Emit as a statement-attributed block so attributes work on stable Rust without `#![feature(stmt_expr_attributes)]`.
                format!("{{ {}{} }}", prefix, self.fmt_expr(expr)?)
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
                            args.push(self.fmt_expr(expr)?);
                        }
                    }
                }
                if args.is_empty() {
                    format!("format!(\"{}\")", fmt_str)
                } else {
                    format!("format!(\"{}\", {})", fmt_str, args.join(", "))
                }
            }
            Expr::Ident(name) => fmt_rust_ident(&map_std_type_alias(name)),
            Expr::This => "self".to_string(),
            Expr::SelfType => "Self".to_string(),
            Expr::Path { global, segments } => {
                let mut parts = Vec::new();
                for seg in segments {
                    let mapped = fmt_rust_ident(&map_std_type_alias(&seg.name));
                    if seg.type_args.is_empty() {
                        parts.push(mapped);
                    } else {
                        let args: Result<Vec<_>> = seg
                            .type_args
                            .iter()
                            .map(|a| self.fmt_generic_arg(a))
                            .collect();
                        parts.push(format!("{}::<{}>", mapped, args?.join(", ")));
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
                member,
                type_args,
            } => {
                let targs = if type_args.is_empty() {
                    String::new()
                } else {
                    let ta: Result<Vec<_>> =
                        type_args.iter().map(|a| self.fmt_generic_arg(a)).collect();
                    format!("::<{}>", ta?.join(", "))
                };
                // If `self_ty` is a simple identifier that is a local variable, emit a cast + member call
                if let Type::Path(tp) = self_ty.as_ref()
                    && tp.segments.len() == 1
                    && matches!(tp.segments[0].args, TypePathArgs::None)
                    && self.is_local_var(&tp.segments[0].name)
                {
                    return Ok(format!(
                        "({} as {}).{}{}",
                        tp.segments[0].name,
                        self.fmt_type_path(trait_path)?,
                        member,
                        targs
                    ));
                }
                format!(
                    "<{} as {}>::{}{}",
                    self.fmt_type(self_ty)?,
                    self.fmt_type_path(trait_path)?,
                    member,
                    targs
                )
            }
            Expr::Unary { op, expr } => {
                let inner = self.fmt_expr(expr)?;
                match op {
                    UnaryOp::Neg => format!("-{}", inner),
                    UnaryOp::Not | UnaryOp::BitNot => format!("!{}", inner),
                }
            }
            Expr::Binary { op, lhs, rhs } => {
                let l = self.fmt_expr(lhs)?;
                let r = self.fmt_expr(rhs)?;
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
                let l = self.fmt_expr(lhs)?;
                let r = self.fmt_expr(rhs)?;
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
                let t = self.fmt_expr(target)?;
                if *inc {
                    format!("{} += 1", t)
                } else {
                    format!("{} -= 1", t)
                }
            }
            Expr::Cast { expr, ty } => {
                format!("({} as {})", self.fmt_expr(expr)?, self.fmt_type(ty)?)
            }
            Expr::Ternary {
                cond,
                then_expr,
                else_expr,
            } => {
                format!(
                    "if {} {{ {} }} else {{ {} }}",
                    self.fmt_expr(cond)?,
                    self.fmt_expr(then_expr)?,
                    self.fmt_expr(else_expr)?
                )
            }
            Expr::Coalesce { lhs, rhs } => {
                format!(
                    "({}).unwrap_or_else(|| {})",
                    self.fmt_expr(lhs)?,
                    self.fmt_expr(rhs)?
                )
            }
            Expr::Try(inner) => format!("({})?", self.fmt_expr(inner)?),
            Expr::Await(inner) => format!("({}).await", self.fmt_expr(inner)?),
            Expr::Read(inner) | Expr::ReadSlice(inner) => format!("&{}", self.fmt_expr(inner)?),
            Expr::Write(inner) | Expr::WriteSlice(inner) => {
                format!("&mut {}", self.fmt_expr(inner)?)
            }
            Expr::RawReadPtr(inner) => format!("&raw const {}", self.fmt_expr(inner)?),
            Expr::RawWritePtr(inner) => format!("&raw mut {}", self.fmt_expr(inner)?),
            Expr::Take(inner) => self.fmt_expr(inner)?,
            Expr::Deref(inner) => format!("*{}", self.fmt_expr(inner)?),
            Expr::Range {
                start,
                end,
                inclusive,
            } => {
                let s = match start {
                    Some(e) => self.fmt_expr(e)?,
                    None => String::new(),
                };
                let e = match end {
                    Some(ex) => self.fmt_expr(ex)?,
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
                            Expr::Lit(Literal::Str(s)) => Ok(format!("panic!(\"{}\")", s)),
                            other => Ok(format!("panic!(\"{{}}\", {})", self.fmt_expr(other)?)),
                        };
                    }
                    let a: Result<Vec<_>> = args.iter().map(|e| self.fmt_expr(e)).collect();
                    return Ok(format!("panic!({})", a?.join(", ")));
                }
                let c = self.fmt_expr(callee)?;
                let targs = if type_args.is_empty() {
                    String::new()
                } else {
                    let ta: Result<Vec<_>> =
                        type_args.iter().map(|a| self.fmt_generic_arg(a)).collect();
                    format!("::<{}>", ta?.join(", "))
                };
                let a: Result<Vec<_>> = args.iter().map(|e| self.fmt_expr(e)).collect();
                format!("{}{}({})", c, targs, a?.join(", "))
            }
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
                    return Ok(format!("{}.{}", var_name, idx));
                }
                let sep = if self.is_path_expr(object) { "::" } else { "." };
                let obj = self.fmt_expr(object)?;
                let mapped_field = if sep == "::" {
                    fmt_rust_ident(&map_std_type_alias(field))
                } else {
                    fmt_rust_ident(field)
                };
                let targs = if type_args.is_empty() {
                    String::new()
                } else {
                    let ta: Result<Vec<_>> =
                        type_args.iter().map(|a| self.fmt_generic_arg(a)).collect();
                    format!("::<{}>", ta?.join(", "))
                };
                format!("{}{}{}{}", obj, sep, mapped_field, targs)
            }
            Expr::TupleIndex { object, index } => {
                format!("{}.{}", self.fmt_expr(object)?, index)
            }
            Expr::Index { object, indices } => {
                let mut out = self.fmt_expr(object)?;
                for idx in indices {
                    out.push_str(&format!("[{}]", self.fmt_expr(idx)?));
                }
                out
            }
            Expr::ArrayLit(elems) => {
                let items: Result<Vec<_>> = elems.iter().map(|e| self.fmt_expr(e)).collect();
                format!("[{}]", items?.join(", "))
            }
            Expr::ArrayRepeat { value, count } => {
                format!("[{}; {}]", self.fmt_expr(value)?, self.fmt_expr(count)?)
            }
            Expr::TupleLit(elems) => {
                let items: Result<Vec<_>> = elems.iter().map(|e| self.fmt_expr(e)).collect();
                let items = items?;
                if items.len() == 1 {
                    format!("({},)", items[0])
                } else {
                    format!("({})", items.join(", "))
                }
            }
            Expr::StructLit { path, fields } => {
                let mapped: Vec<String> = path
                    .iter()
                    .map(|s| fmt_rust_ident(&map_std_type_alias(s)))
                    .collect();
                let mut fs = Vec::new();
                for f in fields {
                    let mut p = String::new();
                    for a in &f.attrs {
                        p.push_str(&format_attr(a));
                        p.push(' ');
                    }
                    let fname = fmt_rust_ident(&f.name);
                    match &f.value {
                        Some(v) => p.push_str(&format!("{}: {}", fname, self.fmt_expr(v)?)),
                        None => p.push_str(&fname),
                    }
                    fs.push(p);
                }
                format!("{} {{ {} }}", mapped.join("::"), fs.join(", "))
            }
            Expr::StructUpdate { base, fields } => {
                let struct_name = self.resolve_struct_name(base)?;
                let base_s = self.fmt_expr(base)?;
                let mut fs = Vec::new();
                for f in fields {
                    let mut p = String::new();
                    for a in &f.attrs {
                        p.push_str(&format_attr(a));
                        p.push(' ');
                    }
                    let fname = fmt_rust_ident(&f.name);
                    match &f.value {
                        Some(v) => p.push_str(&format!("{}: {}", fname, self.fmt_expr(v)?)),
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
                Some(e) => format!("return {}", self.fmt_expr(e)?),
                None => "return".to_string(),
            },
            Expr::Break { label, value } => {
                let mut out = String::from("break");
                if let Some(l) = label {
                    out.push_str(&format!(" {}", fmt_rust_lifetime(l)));
                }
                if let Some(v) = value {
                    out.push(' ');
                    out.push_str(&self.fmt_expr(v)?);
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
                let cond_s = self.fmt_condition_expr(cond, &mut sub, &mut bound_pats)?;
                let mut checks = Vec::new();
                for bp in &bound_pats {
                    checks.extend(self.collect_pattern_type_checks(bp)?);
                }
                let then_s = sub.fmt_expr(then_expr)?;
                let else_s = self.fmt_expr(else_expr)?;
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
                sub.emit_cond_block(&format!("{}while ", lbl), cond, body)?;
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
                let p = sub.fmt_pattern(pat)?;
                let it = sub.fmt_expr(iter)?;
                sub.line(&format!("{}for {} in {} {{", lbl, p, it));
                sub.indent += 1;
                sub.push_scope();
                sub.emit_inner_docs_and_attrs(&body.inner_docs, &body.inner_attrs);
                sub.emit_pattern_type_checks(pat)?;
                for st in &body.stmts {
                    sub.emit_stmt(st)?;
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
                for p in params {
                    ps.push(sub.fmt_param(p)?);
                }
                let ret = match ret_ty {
                    Some(t) => {
                        let eff = unwrap_async_task_type(*is_async, t);
                        if matches!(eff, Type::Void) {
                            String::new()
                        } else {
                            format!(" -> {}", self.fmt_type(eff)?)
                        }
                    }
                    None => String::new(),
                };
                match body {
                    LambdaBody::Expr(e) => {
                        let es = sub.fmt_expr(e)?;
                        if ret.is_empty() && !*is_async {
                            format!("{}|{}| {}", prefix, ps.join(", "), es)
                        } else {
                            format!("{}|{}|{} {{ {} }}", prefix, ps.join(", "), ret, es)
                        }
                    }
                    LambdaBody::Block(b) => {
                        sub.emit_block_stmts(b)?;
                        format!("{}|{}|{} {{\n{}}}", prefix, ps.join(", "), ret, sub.out)
                    }
                }
            }
            Expr::Match(m) => {
                let mut sub = self.child_formatter();
                sub.emit_match(m)?;
                sub.out.trim_end().to_string()
            }
            Expr::Loop { label, block } => {
                let lbl = label
                    .as_ref()
                    .map(|l| format!("{}: ", fmt_rust_lifetime(l)))
                    .unwrap_or_default();
                let mut sub = self.child_formatter();
                sub.emit_block_stmts(block)?;
                format!("{}loop {{\n{}}}", lbl, sub.out)
            }
            Expr::LabeledBlock { label, block } => {
                let mut sub = self.child_formatter();
                sub.emit_block_stmts(block)?;
                format!("{}: {{\n{}}}", fmt_rust_lifetime(label), sub.out)
            }
            Expr::ConstBlock(cb) => match cb {
                ConstBody::Expr(e) => format!("const {{ {} }}", self.fmt_expr(e)?),
                ConstBody::Block(b) => {
                    let mut sub = self.child_formatter();
                    sub.emit_value_block_stmts(b)?;
                    format!("const {{\n{}}}", sub.out)
                }
            },
            Expr::AsyncBlock { is_move, block } => {
                let mv = if *is_move { "move " } else { "" };
                let mut sub = self.child_formatter();
                sub.emit_value_block_stmts(block)?;
                format!("async {} {{\n{}}}", mv, sub.out)
            }
            Expr::UnsafeBlock(block) => {
                let mut sub = self.child_formatter();
                sub.emit_value_block_stmts(block)?;
                format!("unsafe {{\n{}}}", sub.out)
            }
            Expr::InlineRust(raw) => format!("{{ {} }}", raw.trim()),
            Expr::MacroCall(mc) => format_macro_call(mc),
            Expr::IsPattern { expr, negated, pat } => {
                let e = self.fmt_expr(expr)?;
                let p = self.fmt_pattern(pat)?;
                if *negated {
                    format!("!matches!({}, {})", e, p)
                } else {
                    format!("matches!({}, {})", e, p)
                }
            }
        })
    }

    fn child_formatter(&self) -> Self {
        Self {
            options: self.options,
            out: String::new(),
            indent: self.indent + 1,
            known_paths: self.known_paths.clone(),
            scopes: self.scopes.clone(),
            current_self_type: self.current_self_type.clone(),
        }
    }

    fn is_path_expr(&self, expr: &Expr) -> bool {
        match expr {
            Expr::SelfType | Expr::Path { .. } => true,
            Expr::This => !self.is_local_var("this"),
            Expr::Ident(name) => {
                if self.is_local_var(name) {
                    return false;
                }
                self.known_paths.contains(name)
                    || name.chars().next().is_some_and(|c| c.is_uppercase())
            }
            Expr::Member { object, .. } => self.is_path_expr(object),
            _ => false,
        }
    }

    fn resolve_struct_name(&self, base: &Expr) -> Result<String> {
        match base {
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
        }
    }
}

fn register_using_tree(tree: &UsingTree, known: &mut HashSet<String>) {
    match tree {
        UsingTree::Path { prefix, alias, .. } => {
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

fn format_using_tree(tree: &UsingTree) -> String {
    match tree {
        UsingTree::Path {
            global,
            prefix,
            wildcard,
            alias,
        } => {
            let mapped: Vec<String> = prefix
                .iter()
                .map(|s| fmt_rust_ident(&map_std_type_alias(s)))
                .collect();
            let mut s = if *global {
                format!("::{}", mapped.join("::"))
            } else {
                mapped.join("::")
            };
            if *wildcard {
                if !s.is_empty() && !s.ends_with("::") {
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
            let mapped: Vec<String> = prefix
                .iter()
                .map(|s| fmt_rust_ident(&map_std_type_alias(s)))
                .collect();
            let subs: Vec<String> = items.iter().map(format_using_tree).collect();
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
        Pattern::Struct { path, .. } => Some(Type::Path(TypePath {
            global: false,
            segments: path
                .iter()
                .map(|n| TypePathSegment {
                    name: n.clone(),
                    args: TypePathArgs::None,
                })
                .collect(),
        })),
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

fn collect_pattern_bindings(pat: &Pattern, out: &mut Vec<(String, Type)>) {
    match pat {
        Pattern::Binding {
            ty: Some(t), name, ..
        } => {
            if name != "_" {
                out.push((name.clone(), t.clone()));
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
            out.push((name.clone(), ref_ty));
        }
        Pattern::DerefPat { inner, .. } | Pattern::Paren(inner) => {
            collect_pattern_bindings(inner, out)
        }
        Pattern::BindAt {
            ty, name, subpat, ..
        } => {
            out.push((name.clone(), ty.clone()));
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
                        ty: Some(t), name, ..
                    } => out.push((name.clone(), t.clone())),
                    _ => {}
                }
            }
        }
        Pattern::Struct { fields, .. } => {
            for f in fields {
                if let Some(ty) = &f.ty {
                    out.push((f.name.clone(), ty.clone()));
                }
                if let Some(sub) = &f.pat {
                    collect_pattern_bindings(sub, out);
                }
            }
        }
        Pattern::Or(alts) => {
            if let Some(first) = alts.first() {
                collect_pattern_bindings(first, out);
            }
        }
        _ => {}
    }
}

fn format_vis(vis: &Visibility) -> String {
    match vis {
        Visibility::Private => String::new(),
        Visibility::Public => "pub ".to_string(),
        Visibility::Internal => "pub(crate) ".to_string(),
        Visibility::Super => "pub(super) ".to_string(),
        Visibility::This => "pub(self) ".to_string(),
        Visibility::InPath(p) => format!("pub(in {}) ", p.join("::")),
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
        Some(args) if args.starts_with("= ") => format!("{} {}", mapped_name, args),
        Some(args) => {
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

fn fmt_rust_ident(name: &str) -> String {
    if is_rust_keyword(name) {
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
