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

/// Transpiles an R# `FileAst` to idiomatic Rust source code.
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
                ItemKind::Namespace(ns) => {
                    if let Some(first) = ns.path.first() {
                        self.known_paths.insert(first.clone());
                    }
                }
                ItemKind::Using(u) => {
                    if let Some(alias) = &u.alias {
                        self.known_paths.insert(alias.clone());
                    } else if let Some(last) = u.path.last() {
                        self.known_paths.insert(last.clone());
                    }
                }
                _ => {}
            }
        }
    }

    fn emit_file(&mut self, file: &FileAst) -> Result<()> {
        self.register_items(&file.items);

        // Inner attributes
        for attr in &file.inner_attrs {
            self.line(&format_attr(attr));
        }

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

        // File-scoped namespace wraps all remaining items
        let mut idx = 0;
        let mut file_ns: Option<Vec<String>> = None;
        if let Some(first) = file.items.first()
            && let ItemKind::Namespace(ns) = &first.kind
                && ns.items.is_none() {
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
        let vis = format_vis(item.vis);
        match &item.kind {
            ItemKind::Namespace(ns) => {
                if let Some(items) = &ns.items {
                    for seg in &ns.path {
                        self.line(&format!("{}mod {} {{", vis, seg));
                        self.indent += 1;
                    }
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
                    self.line(&format!("{}mod {};", vis, name));
                }
            }
            ItemKind::Using(u) => {
                let mapped: Vec<String> = u.path.iter().map(|s| map_std_type_alias(s)).collect();
                let mut line = format!("{}use {}", vis, mapped.join("::"));
                if u.wildcard {
                    line.push_str("::*");
                }
                if let Some(alias) = &u.alias {
                    line.push_str(&format!(" as {}", alias));
                }
                line.push(';');
                self.line(&line);
            }
            ItemKind::Struct(s) => self.emit_struct(vis, s)?,
            ItemKind::Enum(e) => self.emit_enum(vis, e)?,
            ItemKind::Union(u) => self.emit_union(vis, u)?,
            ItemKind::Trait(t) => self.emit_trait(vis, t)?,
            ItemKind::Impl(im) => self.emit_impl(im)?,
            ItemKind::Alias(a) => {
                let gens = self.fmt_generics(&a.generics)?;
                let target = self.fmt_type(&a.target)?;
                self.line(&format!("{}type {}{} = {};", vis, a.name, gens, target));
            }
            ItemKind::Const(c) => {
                let ty = self.fmt_type(&c.ty)?;
                let val = self.fmt_expr(&c.value)?;
                self.line(&format!("{}const {}: {} = {};", vis, c.name, ty, val));
            }
            ItemKind::Static(s) => {
                let mut_kw = if s.is_mut { "mut " } else { "" };
                let ty = self.fmt_type(&s.ty)?;
                let val = self.fmt_expr(&s.value)?;
                self.line(&format!(
                    "{}static {}{}: {} = {};",
                    vis, mut_kw, s.name, ty, val
                ));
            }
            ItemKind::Fn(f) => self.emit_fn(vis, f)?,
            ItemKind::ExternBlock(eb) => {
                let abi = eb
                    .abi
                    .as_ref()
                    .map(|a| format!(" \"{}\"", a))
                    .unwrap_or_default();
                self.line(&format!("unsafe extern{} {{", abi));
                self.indent += 1;
                for sub in &eb.items {
                    self.emit_item(sub)?;
                }
                self.indent -= 1;
                self.line("}");
            }
            ItemKind::MacroDecl(m) => {
                self.line(&format!("macro_rules! {} {{ {} }}", m.name, m.body));
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
                    p.push_str(format_vis(f.vis));
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
                self.line(&format!("{}struct {}{}{} {{", vis, s.name, gens, wh));
                self.indent += 1;
                for f in fields {
                    self.emit_docs_and_attrs(&f.docs, &f.attrs);
                    let fvis = format_vis(f.vis);
                    let fty = self.fmt_type(&f.ty)?;
                    self.line(&format!("{}{}: {},", fvis, f.name, fty));
                }
                self.indent -= 1;
                self.line("}");
            }
        }

        if !s.methods.is_empty() {
            let gen_args = format_generic_args_from_params(&s.generics);
            self.line(&format!(
                "impl{} {}{}{} {{",
                gens, s.name, gen_args, wh
            ));
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
                        tys.push(self.fmt_type(&f.ty)?);
                    }
                    self.line(&format!("{}({}){},", v.name, tys.join(", "), disc));
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
        self.line(&format!("{}union {}{}{} {{", vis, u.name, gens, wh));
        self.indent += 1;
        for f in &u.fields {
            self.emit_docs_and_attrs(&f.docs, &f.attrs);
            let fvis = format_vis(f.vis);
            let fty = self.fmt_type(&f.ty)?;
            self.line(&format!("{}{}: {},", fvis, f.name, fty));
        }
        self.indent -= 1;
        self.line("}");
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
            vis, unsf, t.name, gens, supers, wh
        ));
        self.indent += 1;
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
                    bounds,
                    default,
                } => {
                    for a in attrs {
                        self.line(&format_attr(a));
                    }
                    let b = if bounds.is_empty() {
                        String::new()
                    } else {
                        let bs: Result<Vec<_>> =
                            bounds.iter().map(|b| self.fmt_bound(b)).collect();
                        format!(": {}", bs?.join(" + "))
                    };
                    let d = match default {
                        Some(ty) => format!(" = {}", self.fmt_type(ty)?),
                        None => String::new(),
                    };
                    self.line(&format!("type {}{}{};", name, b, d));
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
                    self.line(&format!("const {}: {}{};", name, ty_s, d));
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
                ImplItem::AssocType { attrs, name, ty } => {
                    for a in attrs {
                        self.line(&format_attr(a));
                    }
                    let t = self.fmt_type(ty)?;
                    self.line(&format!("type {} = {};", name, t));
                }
                ImplItem::AssocConst(it) => self.emit_item(it)?,
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
        if f.is_unsafe {
            sig.push_str("unsafe ");
        }
        if let Some(abi) = &f.extern_abi {
            sig.push_str(&format!("extern \"{}\" ", abi));
        }
        sig.push_str("fn ");
        sig.push_str(&f.name);
        sig.push_str(&self.fmt_generics(&f.generics)?);
        sig.push('(');

        self.push_scope();
        let mut pstrs = Vec::new();
        for p in &f.params {
            match p {
                Param::Receiver(rk) => match rk {
                    ReceiverKind::Read(life) => {
                        let l = life
                            .as_ref()
                            .map(|n| format!("'{} ", n))
                            .unwrap_or_default();
                        pstrs.push(format!("&{}self", l));
                    }
                    ReceiverKind::Write(life) => {
                        let l = life
                            .as_ref()
                            .map(|n| format!("'{} ", n))
                            .unwrap_or_default();
                        pstrs.push(format!("&{}mut self", l));
                    }
                    ReceiverKind::Take => pstrs.push("self".to_string()),
                    ReceiverKind::Typed { ty } => {
                        pstrs.push(format!("self: {}", self.fmt_type(ty)?));
                    }
                },
                Param::Normal { is_mut, ty, name } => {
                    self.declare_var(name, ty);
                    let m = if *is_mut { "mut " } else { "" };
                    pstrs.push(format!("{}{}: {}", m, name, self.fmt_type(ty)?));
                }
            }
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

    fn emit_block_stmts(&mut self, block: &Block) -> Result<()> {
        self.push_scope();
        for stmt in &block.stmts {
            self.emit_stmt(stmt)?;
        }
        self.pop_scope();
        Ok(())
    }

    fn emit_stmt(&mut self, stmt: &Stmt) -> Result<()> {
        match stmt {
            Stmt::Local {
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
                self.line(&format!("let {}{}: {}{};", m, name, ty_s, init_s));
            }
            Stmt::DestructureTuple { bindings, init } => {
                let rhs = self.fmt_expr(init)?;
                let pats: Vec<String> = bindings
                    .iter()
                    .map(|(m, _, n)| if *m { format!("mut {}", n) } else { n.clone() })
                    .collect();
                let tys: Result<Vec<String>> =
                    bindings.iter().map(|(_, t, _)| self.fmt_type(t)).collect();
                for (_, ty, name) in bindings {
                    self.declare_var(name, ty);
                }
                self.line(&format!(
                    "let ({}): ({}) = {};",
                    pats.join(", "),
                    tys?.join(", "),
                    rhs
                ));
            }
            Stmt::Discard(expr) => {
                let e = self.fmt_expr(expr)?;
                self.line(&format!("let _ = {};", e));
            }
            Stmt::Item(item) => {
                self.register_items(std::slice::from_ref(item.as_ref()));
                self.emit_item(item)?;
            }
            Stmt::Return(expr) => match expr {
                Some(e) => {
                    let s = self.fmt_expr(e)?;
                    self.line(&format!("return {};", s));
                }
                None => self.line("return;"),
            },
            Stmt::Break { label, value } => {
                let mut line = String::from("break");
                if let Some(l) = label {
                    line.push_str(&format!(" '{}", l));
                }
                if let Some(v) = value {
                    line.push(' ');
                    line.push_str(&self.fmt_expr(v)?);
                }
                line.push(';');
                self.line(&line);
            }
            Stmt::Continue { label } => match label {
                Some(l) => self.line(&format!("continue '{};", l)),
                None => self.line("continue;"),
            },
            Stmt::If(if_stmt) => self.emit_if(if_stmt, false)?,
            Stmt::Require {
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
            Stmt::While { label, cond, body } => {
                let lbl = label
                    .as_ref()
                    .map(|l| format!("'{}: ", l))
                    .unwrap_or_default();
                match cond {
                    Condition::Bool(e) => {
                        let s = self.fmt_expr(e)?;
                        self.line(&format!("{}while {} {{", lbl, s));
                        self.indent += 1;
                        self.emit_block_stmts(body)?;
                        self.indent -= 1;
                        self.line("}");
                    }
                    Condition::IsPattern {
                        expr,
                        negated: false,
                        pat,
                    } => {
                        let p = self.fmt_pattern(pat)?;
                        let e = self.fmt_expr(expr)?;
                        self.line(&format!("{}while let {} = {} {{", lbl, p, e));
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
                    Condition::IsPattern {
                        expr,
                        negated: true,
                        pat,
                    } => {
                        let p = self.fmt_pattern(pat)?;
                        let e = self.fmt_expr(expr)?;
                        self.line(&format!("{}while !matches!({}, {}) {{", lbl, e, p));
                        self.indent += 1;
                        self.emit_block_stmts(body)?;
                        self.indent -= 1;
                        self.line("}");
                    }
                }
            }
            Stmt::For {
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
                let has_step = step.is_some();
                if has_step {
                    self.line("let mut __rsharp_first = true;");
                }
                let lbl = label
                    .as_ref()
                    .map(|l| format!("'{}: ", l))
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
            Stmt::Foreach {
                label,
                pat,
                iter,
                body,
            } => {
                let lbl = label
                    .as_ref()
                    .map(|l| format!("'{}: ", l))
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
            Stmt::Loop { label, body } => {
                let lbl = label
                    .as_ref()
                    .map(|l| format!("'{}: ", l))
                    .unwrap_or_default();
                self.line(&format!("{}loop {{", lbl));
                self.indent += 1;
                self.emit_block_stmts(body)?;
                self.indent -= 1;
                self.line("}");
            }
            Stmt::Match(m) => {
                self.emit_match(m)?;
            }
            Stmt::Unsafe(b) => {
                self.line("unsafe {");
                self.indent += 1;
                self.emit_block_stmts(b)?;
                self.indent -= 1;
                self.line("}");
            }
            Stmt::InlineRust(raw) => {
                for l in raw.trim().lines() {
                    self.line(l);
                }
            }
            Stmt::Expr(expr) => {
                let s = self.fmt_expr(expr)?;
                self.line(&format!("{};", s));
            }
        }
        Ok(())
    }

    fn emit_if(&mut self, if_stmt: &IfStmt, is_else_if: bool) -> Result<()> {
        let prefix = if is_else_if { "else if " } else { "if " };
        match &if_stmt.cond {
            Condition::Bool(e) => {
                let s = self.fmt_expr(e)?;
                self.line(&format!("{}{} {{", prefix, s));
                self.indent += 1;
                self.emit_block_stmts(&if_stmt.then_block)?;
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
                for st in &if_stmt.then_block.stmts {
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
                self.emit_block_stmts(&if_stmt.then_block)?;
                self.indent -= 1;
            }
        }
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

    fn emit_match(&mut self, m: &MatchExpr) -> Result<()> {
        let scr = self.fmt_expr(&m.scrutinee)?;
        self.line(&format!("match {} {{", scr));
        self.indent += 1;
        for arm in &m.arms {
            let p = self.fmt_pattern(&arm.pat)?;
            let g = match &arm.guard {
                Some(ge) => format!(" if {}", self.fmt_expr(ge)?),
                None => String::new(),
            };
            match &arm.body {
                MatchArmBody::Block(b) => {
                    self.line(&format!("{}{} => {{", p, g));
                    self.indent += 1;
                    self.push_scope();
                    self.emit_pattern_type_checks(&arm.pat)?;
                    for st in &b.stmts {
                        self.emit_stmt(st)?;
                    }
                    self.pop_scope();
                    self.indent -= 1;
                    self.line("}");
                }
                MatchArmBody::Expr(e) => {
                    let checks = self.collect_pattern_type_checks(&arm.pat)?;
                    self.push_scope();
                    self.register_pattern_vars(&arm.pat);
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
            checks.push(format!("let _: &{} = &{};", ty_s, name));
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
            Type::Path(p) => self.fmt_type_path(p)?,
            Type::Read { life, inner } => {
                let l = life
                    .as_ref()
                    .map(|n| format!("'{} ", n))
                    .unwrap_or_default();
                format!("&{}{}", l, self.fmt_type(inner)?)
            }
            Type::Write { life, inner } => {
                let l = life
                    .as_ref()
                    .map(|n| format!("'{} ", n))
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
            Type::FnPtr { params, ret } => {
                let ps: Result<Vec<_>> = params.iter().map(|p| self.fmt_type(p)).collect();
                let r = self.fmt_type(ret)?;
                if matches!(ret.as_ref(), Type::Void) {
                    format!("fn({})", ps?.join(", "))
                } else {
                    format!("fn({}) -> {}", ps?.join(", "), r)
                }
            }
        })
    }

    fn fmt_type_path(&self, path: &TypePath) -> Result<String> {
        if path.segments.len() == 1 && path.segments[0].args.is_empty()
            && let Some(prim) = map_primitive_alias(&path.segments[0].name) {
                return Ok(prim.to_string());
            }
        let mut out = Vec::new();
        for seg in &path.segments {
            let mapped = map_std_type_alias(&seg.name);
            if seg.args.is_empty() {
                out.push(mapped);
            } else {
                let args: Result<Vec<_>> = seg.args.iter().map(|a| self.fmt_generic_arg(a)).collect();
                out.push(format!("{}<{}>", mapped, args?.join(", ")));
            }
        }
        Ok(out.join("::"))
    }

    fn fmt_generic_arg(&self, arg: &GenericArg) -> Result<String> {
        Ok(match arg {
            GenericArg::Lifetime(l) => format!("'{}", l),
            GenericArg::Type(t) => self.fmt_type(t)?,
            GenericArg::AssocBinding { name, ty } => format!("{} = {}", name, self.fmt_type(ty)?),
            GenericArg::ConstExpr(e) => self.fmt_expr(e)?,
        })
    }

    fn fmt_bound(&self, b: &TypeBound) -> Result<String> {
        Ok(match b {
            TypeBound::Trait(tp) => self.fmt_type_path(tp)?,
            TypeBound::Lifetime(l) => format!("'{}", l),
        })
    }

    fn fmt_generics(&self, params: &[GenericParam]) -> Result<String> {
        if params.is_empty() {
            return Ok(String::new());
        }
        let mut parts = Vec::new();
        for p in params {
            match &p.kind {
                GenericParamKind::Lifetime(l) => parts.push(format!("'{}", l)),
                GenericParamKind::Type { name, bounds } => {
                    if bounds.is_empty() {
                        parts.push(name.clone());
                    } else {
                        let bs: Result<Vec<_>> =
                            bounds.iter().map(|b| self.fmt_bound(b)).collect();
                        parts.push(format!("{}: {}", name, bs?.join(" + ")));
                    }
                }
                GenericParamKind::Const { ty, name } => {
                    parts.push(format!("const {}: {}", name, self.fmt_type(ty)?));
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
                WherePredicate::Type { ty, bounds } => {
                    let bs: Result<Vec<_>> = bounds.iter().map(|b| self.fmt_bound(b)).collect();
                    preds.push(format!("{}: {}", self.fmt_type(ty)?, bs?.join(" + ")));
                }
                WherePredicate::Lifetime { life, outlives } => {
                    let os: Vec<String> = outlives.iter().map(|o| format!("'{}", o)).collect();
                    preds.push(format!("'{}: {}", life, os.join(" + ")));
                }
            }
        }
        Ok(format!(" where {}", preds.join(", ")))
    }

    fn fmt_pattern(&self, pat: &Pattern) -> Result<String> {
        Ok(match pat {
            Pattern::Wildcard => "_".to_string(),
            Pattern::Lit(l) => fmt_literal(l),
            Pattern::Range {
                start,
                end,
                inclusive,
            } => {
                let op = if *inclusive { "..=" } else { ".." };
                format!("{}{}{}", self.fmt_expr(start)?, op, self.fmt_expr(end)?)
            }
            Pattern::Binding { is_mut, name, .. } => {
                if *is_mut {
                    format!("mut {}", name)
                } else {
                    name.clone()
                }
            }
            Pattern::Tuple(ps) => {
                let items: Result<Vec<_>> = ps.iter().map(|p| self.fmt_pattern(p)).collect();
                format!("({})", items?.join(", "))
            }
            Pattern::TupleStruct { path, fields } => {
                let fs: Result<Vec<_>> = fields.iter().map(|f| self.fmt_pattern(f)).collect();
                format!("{}({})", path.join("::"), fs?.join(", "))
            }
            Pattern::Struct { path, fields, rest } => {
                let mut parts = Vec::new();
                for f in fields {
                    match &f.pat {
                        Some(sub) => parts.push(format!("{}: {}", f.name, self.fmt_pattern(sub)?)),
                        None if f.is_mut => parts.push(format!("mut {}", f.name)),
                        None => parts.push(f.name.clone()),
                    }
                }
                if *rest {
                    parts.push("..".to_string());
                }
                format!("{} {{ {} }}", path.join("::"), parts.join(", "))
            }
            Pattern::Path(p) => p.join("::"),
            Pattern::Or(alts) => {
                let items: Result<Vec<_>> = alts.iter().map(|p| self.fmt_pattern(p)).collect();
                items?.join(" | ")
            }
        })
    }

    // ==================== EXPRESSIONS ====================

    fn fmt_expr(&self, expr: &Expr) -> Result<String> {
        Ok(match expr {
            Expr::Lit(l) => fmt_literal(l),
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
            Expr::Ident(name) => map_std_type_alias(name),
            Expr::This => "self".to_string(),
            Expr::SelfType => "Self".to_string(),
            Expr::Path { segments } => {
                let mut parts = Vec::new();
                for seg in segments {
                    let mapped = map_std_type_alias(&seg.name);
                    if seg.type_args.is_empty() {
                        parts.push(mapped);
                    } else {
                        let args: Result<Vec<_>> =
                            seg.type_args.iter().map(|a| self.fmt_generic_arg(a)).collect();
                        parts.push(format!("{}::<{}>", mapped, args?.join(", ")));
                    }
                }
                parts.join("::")
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
                // Special-case `panic(...)` -> `panic!(...)`
                if let Expr::Ident(name) = callee.as_ref()
                    && name == "panic" && type_args.is_empty() {
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
                // Check named tuple field access `p.X` -> `p.0`
                if type_args.is_empty()
                    && let Expr::Ident(var_name) = object.as_ref()
                        && let Some(info) = self.lookup_var(var_name)
                            && let Some(map) = &info.tuple_fields
                                && let Some(idx) = map.get(field) {
                                    return Ok(format!("{}.{}", var_name, idx));
                                }
                let sep = if self.is_path_expr(object) { "::" } else { "." };
                let obj = self.fmt_expr(object)?;
                let mapped_field = if sep == "::" {
                    map_std_type_alias(field)
                } else {
                    field.clone()
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
                let mapped: Vec<String> = path.iter().map(|s| map_std_type_alias(s)).collect();
                let mut fs = Vec::new();
                for (name, val) in fields {
                    fs.push(format!("{}: {}", name, self.fmt_expr(val)?));
                }
                format!("{} {{ {} }}", mapped.join("::"), fs.join(", "))
            }
            Expr::StructUpdate { base, fields } => {
                let struct_name = self.resolve_struct_name(base)?;
                let base_s = self.fmt_expr(base)?;
                let mut fs = Vec::new();
                for (name, val) in fields {
                    fs.push(format!("{}: {}", name, self.fmt_expr(val)?));
                }
                format!("{} {{ {}, ..{} }}", struct_name, fs.join(", "), base_s)
            }
            Expr::Lambda {
                is_move,
                params,
                ret_ty,
                body,
            } => {
                let mv = if *is_move { "move " } else { "" };
                let mut ps = Vec::new();
                for (m, ty, name) in params {
                    let mk = if *m { "mut " } else { "" };
                    ps.push(format!("{}{}: {}", mk, name, self.fmt_type(ty)?));
                }
                let ret = match ret_ty {
                    Some(t) => format!(" -> {}", self.fmt_type(t)?),
                    None => String::new(),
                };
                match body {
                    LambdaBody::Expr(e) => {
                        let mut sub = self.child_formatter();
                        for (_, ty, name) in params {
                            sub.declare_var(name, ty);
                        }
                        let es = sub.fmt_expr(e)?;
                        if ret.is_empty() {
                            format!("{}|{}| {}", mv, ps.join(", "), es)
                        } else {
                            format!("{}|{}|{} {{ {} }}", mv, ps.join(", "), ret, es)
                        }
                    }
                    LambdaBody::Block(b) => {
                        let mut sub = self.child_formatter();
                        for (_, ty, name) in params {
                            sub.declare_var(name, ty);
                        }
                        sub.emit_block_stmts(b)?;
                        format!("{}|{}|{} {{\n{}}}", mv, ps.join(", "), ret, sub.out)
                    }
                }
            }
            Expr::Match(m) => {
                let mut sub = self.child_formatter();
                sub.emit_match(m)?;
                sub.out.trim_end().to_string()
            }
            Expr::LabeledBlock { label, block } => {
                let mut sub = self.child_formatter();
                sub.emit_block_stmts(block)?;
                format!("'{}: {{\n{}}}", label, sub.out)
            }
            Expr::AsyncBlock { is_move, block } => {
                let mv = if *is_move { "move " } else { "" };
                let mut sub = self.child_formatter();
                sub.emit_block_stmts(block)?;
                format!("async {} {{\n{}}}", mv, sub.out)
            }
            Expr::UnsafeBlock(block) => {
                let mut sub = self.child_formatter();
                sub.emit_block_stmts(block)?;
                format!("unsafe {{\n{}}}", sub.out)
            }
            Expr::InlineRust(raw) => format!("{{ {} }}", raw.trim()),
            Expr::MacroCall(mc) => format_macro_call(mc),
            Expr::IsPattern {
                expr,
                negated,
                pat,
            } => {
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
            Expr::This => self
                .current_self_type
                .clone()
                .ok_or_else(|| RSharpError::codegen("Cannot resolve struct type for 'this with { ... }'")),
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

fn var_info_from_type(ty: &Type) -> VarInfo {
    match ty {
        Type::Path(tp) => {
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
        && let Some(last) = tp.segments.last() {
            return matches!(last.name.as_str(), "List" | "Vec");
        }
    false
}

fn unwrap_async_task_type(is_async: bool, ty: &Type) -> &Type {
    if !is_async {
        return ty;
    }
    if let Type::Path(tp) = ty
        && tp.segments.len() == 1 && tp.segments[0].name == "Task" {
            if tp.segments[0].args.is_empty() {
                return &Type::Void;
            }
            if let Some(GenericArg::Type(inner)) = tp.segments[0].args.first() {
                return inner;
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
        Pattern::Tuple(ps) | Pattern::TupleStruct { fields: ps, .. } => {
            for p in ps {
                collect_pattern_bindings(p, out);
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

fn format_vis(vis: Visibility) -> &'static str {
    match vis {
        Visibility::Private => "",
        Visibility::Public => "pub ",
        Visibility::Internal => "pub(crate) ",
        Visibility::Super => "pub(super) ",
    }
}

fn format_generic_args_from_params(params: &[GenericParam]) -> String {
    if params.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = params
        .iter()
        .map(|p| match &p.kind {
            GenericParamKind::Lifetime(l) => format!("'{}", l),
            GenericParamKind::Type { name, .. } => name.clone(),
            GenericParamKind::Const { name, .. } => name.clone(),
        })
        .collect();
    format!("<{}>", parts.join(", "))
}

fn format_attr(attr: &Attribute) -> String {
    let prefix = if attr.inner { "#![" } else { "#[" };
    let mapped_name = map_attr_name(&attr.name);
    match &attr.args {
        Some(args) if args.starts_with("= ") => format!("{}{} {}]", prefix, mapped_name, args),
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
            format!("{}{}({})]", prefix, mapped_name, mapped_args)
        }
        None => format!("{}{}]", prefix, mapped_name),
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

fn fmt_literal(lit: &Literal) -> String {
    match lit {
        Literal::Int(s) | Literal::Float(s) => s.clone(),
        Literal::Str(s) => format!("\"{}\"", s),
        Literal::Char(c) => format!("'{}'", c),
        Literal::Bool(b) => b.to_string(),
    }
}

fn format_macro_call(mc: &MacroCall) -> String {
    let path = mc.path.join("::");
    match mc.delim {
        MacroDelim::Paren => format!("{}!({})", path, mc.body),
        MacroDelim::Bracket => format!("{}![{}]", path, mc.body),
        MacroDelim::Brace => format!("{}!{{ {} }}", path, mc.body),
    }
}
