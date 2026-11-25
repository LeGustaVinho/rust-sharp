use std::collections::{HashMap, HashSet};

use quote::ToTokens;
use syn::{
    Attribute, BinOp, Block, Expr, FnArg, GenericArgument, GenericParam, Generics, Item, Local,
    Member, Pat, Path, PathArguments, ReturnType, Stmt, TraitItem, Type, TypeParamBound, UnOp,
    Visibility, WherePredicate,
};

use crate::error::{RSharpError, Result};
use crate::rustc_runner;
use crate::token::Keyword;

/// Options controlling Rust -> R# conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FromRustOptions {
    /// When local type inference cannot resolve an untyped `let` binding,
    /// invoke `rustc` to query the compiler's inferred type.
    pub use_rustc_inference: bool,
}

impl Default for FromRustOptions {
    fn default() -> Self {
        Self {
            use_rustc_inference: true,
        }
    }
}

/// Converts Rust source code into idiomatic R# source code.
pub fn convert_from_rust(rust_source: &str, options: FromRustOptions) -> Result<String> {
    let parsed: syn::File = syn::parse_file(rust_source)
        .map_err(|e| RSharpError::from_rust(format!("Invalid Rust syntax: {}", e)))?;
    let mut conv = RustToRSharp::new(rust_source.to_string(), options);
    conv.collect_signatures(&parsed.items);
    conv.emit_file(&parsed)?;
    Ok(conv.out)
}

struct RustToRSharp {
    raw_source: String,
    options: FromRustOptions,
    out: String,
    indent: usize,
    fn_returns: HashMap<String, String>,
    scopes: Vec<HashMap<String, String>>,
    declared_in_fn: HashSet<String>,
    probe_counter: usize,
}

impl RustToRSharp {
    fn new(raw_source: String, options: FromRustOptions) -> Self {
        Self {
            raw_source,
            options,
            out: String::new(),
            indent: 0,
            fn_returns: HashMap::new(),
            scopes: vec![HashMap::new()],
            declared_in_fn: HashSet::new(),
            probe_counter: 0,
        }
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

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn bind_var(&mut self, name: &str, ty: String) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), ty);
        }
        self.declared_in_fn.insert(name.to_string());
    }

    fn lookup_var(&self, name: &str) -> Option<&str> {
        for scope in self.scopes.iter().rev() {
            if let Some(ty) = scope.get(name) {
                return Some(ty.as_str());
            }
        }
        None
    }

    fn collect_signatures(&mut self, items: &[Item]) {
        for item in items {
            match item {
                Item::Fn(f) => {
                    let ret = self.fmt_return_type(&f.sig.output, false);
                    self.fn_returns.insert(f.sig.ident.to_string(), ret);
                }
                Item::Impl(im) => {
                    let self_ty = self.fmt_type(&im.self_ty);
                    for sub in &im.items {
                        if let syn::ImplItem::Fn(m) = sub {
                            let mut ret = self.fmt_return_type(&m.sig.output, false);
                            if ret == "Self" {
                                ret = self_ty.clone();
                            }
                            self.fn_returns
                                .insert(format!("{}::{}", self_ty, m.sig.ident), ret.clone());
                            self.fn_returns.insert(m.sig.ident.to_string(), ret);
                        }
                    }
                }
                Item::Mod(m) => {
                    if let Some((_, sub_items)) = &m.content {
                        self.collect_signatures(sub_items);
                    }
                }
                _ => {}
            }
        }
    }

    fn emit_file(&mut self, file: &syn::File) -> Result<()> {
        for attr in &file.attrs {
            self.emit_attr(attr, true);
        }
        for item in &file.items {
            self.emit_item(item)?;
        }
        Ok(())
    }

    fn emit_attr(&mut self, attr: &Attribute, inner: bool) {
        if attr.path().is_ident("doc")
            && let syn::Meta::NameValue(nv) = &attr.meta
            && let Expr::Lit(el) = &nv.value
            && let syn::Lit::Str(ls) = &el.lit
        {
            let s = ls.value();
            let trimmed = s.strip_prefix(' ').unwrap_or(&s);
            self.line(&format!("/// {}", trimmed));
            return;
        }
        let prefix = if inner { "module: " } else { "" };
        match &attr.meta {
            syn::Meta::Path(p) => {
                let path_str = path_to_rsharp(p);
                let name = map_rust_attr_name(&path_str);
                self.line(&format!("[{}{}]", prefix, name));
            }
            syn::Meta::List(ml) => {
                let path_str = path_to_rsharp(&ml.path);
                let name = map_rust_attr_name(&path_str);
                let raw_args = ml.tokens.to_string();
                let args = if name == "Inline" {
                    match raw_args.trim() {
                        "always" => "Always".to_string(),
                        "never" => "Never".to_string(),
                        other => other.to_string(),
                    }
                } else {
                    raw_args
                };
                self.line(&format!("[{}{}({})]", prefix, name, args));
            }
            syn::Meta::NameValue(nv) => {
                let path_str = path_to_rsharp(&nv.path);
                let name = map_rust_attr_name(&path_str);
                let val = nv.value.to_token_stream().to_string();
                self.line(&format!("[{}{} = {}]", prefix, name, val));
            }
        }
    }

    fn emit_item(&mut self, item: &Item) -> Result<()> {
        match item {
            Item::Use(u) => {
                for a in &u.attrs {
                    self.emit_attr(a, false);
                }
                let vis = fmt_vis(&u.vis);
                let mut paths = Vec::new();
                flatten_use_tree(&u.tree, Vec::new(), &mut paths);
                for line in paths {
                    self.line(&format!("{}using {};", vis, line));
                }
            }
            Item::Mod(m) => {
                for a in &m.attrs {
                    self.emit_attr(a, false);
                }
                let vis = fmt_vis(&m.vis);
                let name = escape_ident(&m.ident.to_string());
                if let Some((_, items)) = &m.content {
                    self.line(&format!("{}namespace {} {{", vis, name));
                    self.indent += 1;
                    for sub in items {
                        self.emit_item(sub)?;
                    }
                    self.indent -= 1;
                    self.line("}");
                } else {
                    self.line(&format!("{}namespace {};", vis, name));
                }
            }
            Item::Struct(s) => {
                for a in &s.attrs {
                    self.emit_attr(a, false);
                }
                let vis = fmt_vis(&s.vis);
                let name = escape_ident(&s.ident.to_string());
                let gens = self.fmt_generics(&s.generics);
                let wh = self.fmt_where(&s.generics);
                match &s.fields {
                    syn::Fields::Unit => {
                        self.line(&format!("{}struct {}{}{};", vis, name, gens, wh));
                    }
                    syn::Fields::Unnamed(uf) => {
                        let fs: Vec<String> = uf
                            .unnamed
                            .iter()
                            .map(|f| format!("{}{}", fmt_vis(&f.vis), self.fmt_type(&f.ty)))
                            .collect();
                        self.line(&format!(
                            "{}struct {}{}({}){};",
                            vis,
                            name,
                            gens,
                            fs.join(", "),
                            wh
                        ));
                    }
                    syn::Fields::Named(nf) => {
                        self.line(&format!("{}struct {}{}{} {{", vis, name, gens, wh));
                        self.indent += 1;
                        for f in &nf.named {
                            for a in &f.attrs {
                                self.emit_attr(a, false);
                            }
                            let fvis = fmt_vis(&f.vis);
                            let fname = escape_ident(&f.ident.as_ref().unwrap().to_string());
                            let fty = self.fmt_type(&f.ty);
                            self.line(&format!("{}{} {};", fvis, fty, fname));
                        }
                        self.indent -= 1;
                        self.line("}");
                    }
                }
            }
            Item::Enum(e) => {
                for a in &e.attrs {
                    self.emit_attr(a, false);
                }
                let vis = fmt_vis(&e.vis);
                let name = escape_ident(&e.ident.to_string());
                let gens = self.fmt_generics(&e.generics);
                let wh = self.fmt_where(&e.generics);
                self.line(&format!("{}enum {}{}{} {{", vis, name, gens, wh));
                self.indent += 1;
                for v in &e.variants {
                    for a in &v.attrs {
                        self.emit_attr(a, false);
                    }
                    let vname = escape_ident(&v.ident.to_string());
                    let disc = match &v.discriminant {
                        Some((_, expr)) => format!(" = {}", self.fmt_expr(expr)?),
                        None => String::new(),
                    };
                    match &v.fields {
                        syn::Fields::Unit => self.line(&format!("{}{},", vname, disc)),
                        syn::Fields::Unnamed(uf) => {
                            let ts: Vec<String> =
                                uf.unnamed.iter().map(|f| self.fmt_type(&f.ty)).collect();
                            self.line(&format!("{}({}){},", vname, ts.join(", "), disc));
                        }
                        syn::Fields::Named(nf) => {
                            self.line(&format!("{} {{", vname));
                            self.indent += 1;
                            for f in &nf.named {
                                let fname = escape_ident(&f.ident.as_ref().unwrap().to_string());
                                let fty = self.fmt_type(&f.ty);
                                self.line(&format!("{} {};", fty, fname));
                            }
                            self.indent -= 1;
                            self.line(&format!("}}{},", disc));
                        }
                    }
                }
                self.indent -= 1;
                self.line("}");
            }
            Item::Union(u) => {
                for a in &u.attrs {
                    self.emit_attr(a, false);
                }
                let vis = fmt_vis(&u.vis);
                let name = escape_ident(&u.ident.to_string());
                let gens = self.fmt_generics(&u.generics);
                let wh = self.fmt_where(&u.generics);
                self.line(&format!("{}union {}{}{} {{", vis, name, gens, wh));
                self.indent += 1;
                for f in &u.fields.named {
                    let fvis = fmt_vis(&f.vis);
                    let fname = escape_ident(&f.ident.as_ref().unwrap().to_string());
                    let fty = self.fmt_type(&f.ty);
                    self.line(&format!("{}{} {};", fvis, fty, fname));
                }
                self.indent -= 1;
                self.line("}");
            }
            Item::Trait(t) => {
                for a in &t.attrs {
                    self.emit_attr(a, false);
                }
                let vis = fmt_vis(&t.vis);
                let unsf = if t.unsafety.is_some() { "unsafe " } else { "" };
                let name = escape_ident(&t.ident.to_string());
                let gens = self.fmt_generics(&t.generics);
                let supers = if t.supertraits.is_empty() {
                    String::new()
                } else {
                    let bs: Vec<String> = t.supertraits.iter().map(|b| self.fmt_bound(b)).collect();
                    format!(" : {}", bs.join(", "))
                };
                let wh = self.fmt_where(&t.generics);
                self.line(&format!(
                    "{}{}trait {}{}{}{} {{",
                    vis, unsf, name, gens, supers, wh
                ));
                self.indent += 1;
                for ti in &t.items {
                    match ti {
                        TraitItem::Fn(tf) => {
                            for a in &tf.attrs {
                                self.emit_attr(a, false);
                            }
                            self.emit_fn_sig_and_body("", &tf.sig, tf.default.as_ref(), false)?;
                        }
                        TraitItem::Type(tt) => {
                            let aname = escape_ident(&tt.ident.to_string());
                            let bs = if tt.bounds.is_empty() {
                                String::new()
                            } else {
                                let b: Vec<String> =
                                    tt.bounds.iter().map(|b| self.fmt_bound(b)).collect();
                                format!(" : {}", b.join(", "))
                            };
                            let def = tt
                                .default
                                .as_ref()
                                .map(|(_, ty)| format!(" = {}", self.fmt_type(ty)))
                                .unwrap_or_default();
                            self.line(&format!("type {}{}{};", aname, bs, def));
                        }
                        TraitItem::Const(tc) => {
                            let cty = self.fmt_type(&tc.ty);
                            let cname = escape_ident(&tc.ident.to_string());
                            let def = match &tc.default {
                                Some((_, e)) => format!(" = {}", self.fmt_expr(e)?),
                                None => String::new(),
                            };
                            self.line(&format!("const {} {}{};", cty, cname, def));
                        }
                        other => {
                            self.line(&format!("rust {{ {} }}", other.to_token_stream()));
                        }
                    }
                }
                self.indent -= 1;
                self.line("}");
            }
            Item::Impl(im) => {
                for a in &im.attrs {
                    self.emit_attr(a, false);
                }
                let unsf = if im.unsafety.is_some() { "unsafe " } else { "" };
                let gens = self.fmt_generics(&im.generics);
                let target = self.fmt_type(&im.self_ty);
                let wh = self.fmt_where(&im.generics);
                let header = match &im.trait_ {
                    Some((_, tp, _)) => {
                        format!(
                            "{}impl{} {} for {}{} {{",
                            unsf,
                            gens,
                            self.fmt_path(tp),
                            target,
                            wh
                        )
                    }
                    None => format!("{}impl{} {}{} {{", unsf, gens, target, wh),
                };
                self.line(&header);
                self.indent += 1;
                for sub in &im.items {
                    match sub {
                        syn::ImplItem::Fn(m) => {
                            for a in &m.attrs {
                                self.emit_attr(a, false);
                            }
                            let mvis = fmt_vis(&m.vis);
                            let is_static =
                                !m.sig.inputs.iter().any(|a| matches!(a, FnArg::Receiver(_)));
                            self.emit_fn_sig_and_body(mvis, &m.sig, Some(&m.block), is_static)?;
                        }
                        syn::ImplItem::Type(t) => {
                            let aname = escape_ident(&t.ident.to_string());
                            let ty = self.fmt_type(&t.ty);
                            self.line(&format!("type {} = {};", aname, ty));
                        }
                        syn::ImplItem::Const(c) => {
                            let cvis = fmt_vis(&c.vis);
                            let cty = self.fmt_type(&c.ty);
                            let cname = escape_ident(&c.ident.to_string());
                            let val = self.fmt_expr(&c.expr)?;
                            self.line(&format!("{}const {} {} = {};", cvis, cty, cname, val));
                        }
                        other => {
                            self.line(&format!("rust {{ {} }}", other.to_token_stream()));
                        }
                    }
                }
                self.indent -= 1;
                self.line("}");
            }
            Item::Type(t) => {
                for a in &t.attrs {
                    self.emit_attr(a, false);
                }
                let vis = fmt_vis(&t.vis);
                let name = escape_ident(&t.ident.to_string());
                let gens = self.fmt_generics(&t.generics);
                let ty = self.fmt_type(&t.ty);
                self.line(&format!("{}alias {}{} = {};", vis, name, gens, ty));
            }
            Item::Const(c) => {
                for a in &c.attrs {
                    self.emit_attr(a, false);
                }
                let vis = fmt_vis(&c.vis);
                let ty = self.fmt_type(&c.ty);
                let name = escape_ident(&c.ident.to_string());
                let val = self.fmt_expr(&c.expr)?;
                self.line(&format!("{}const {} {} = {};", vis, ty, name, val));
            }
            Item::Static(s) => {
                for a in &s.attrs {
                    self.emit_attr(a, false);
                }
                let vis = fmt_vis(&s.vis);
                let mut_kw = if matches!(s.mutability, syn::StaticMutability::Mut(_)) {
                    "mut "
                } else {
                    ""
                };
                let ty = self.fmt_type(&s.ty);
                let name = escape_ident(&s.ident.to_string());
                let val = self.fmt_expr(&s.expr)?;
                self.line(&format!(
                    "{}static {}{} {} = {};",
                    vis, mut_kw, ty, name, val
                ));
            }
            Item::Fn(f) => {
                for a in &f.attrs {
                    self.emit_attr(a, false);
                }
                let vis = fmt_vis(&f.vis);
                self.emit_fn_sig_and_body(vis, &f.sig, Some(&f.block), false)?;
            }
            Item::ForeignMod(fm) => {
                let abi = fm
                    .abi
                    .name
                    .as_ref()
                    .map(|l| format!(" \"{}\"", l.value()))
                    .unwrap_or_default();
                self.line(&format!("extern{} {{", abi));
                self.indent += 1;
                for fi in &fm.items {
                    if let syn::ForeignItem::Fn(ff) = fi {
                        let fvis = fmt_vis(&ff.vis);
                        self.emit_fn_sig_and_body(fvis, &ff.sig, None, false)?;
                    } else {
                        self.line(&format!("rust {{ {} }}", fi.to_token_stream()));
                    }
                }
                self.indent -= 1;
                self.line("}");
            }
            Item::Macro(im) => {
                if im.mac.path.is_ident("macro_rules")
                    && let Some(id) = &im.ident
                {
                    self.line(&format!("macro {} {{ {} }}", id, im.mac.tokens));
                    return Ok(());
                }
                self.line(&format!("{};", fmt_syn_macro(&im.mac)));
            }
            other => {
                let pretty = pretty_item(other);
                self.line(&format!("rust {{ {} }}", pretty.trim()));
            }
        }
        Ok(())
    }

    fn emit_fn_sig_and_body(
        &mut self,
        vis: &str,
        sig: &syn::Signature,
        body: Option<&Block>,
        is_static: bool,
    ) -> Result<()> {
        let mut header = String::new();
        header.push_str(vis);
        if sig.constness.is_some() {
            header.push_str("const ");
        }
        let is_async = sig.asyncness.is_some();
        if is_async {
            header.push_str("async ");
        }
        if sig.unsafety.is_some() {
            header.push_str("unsafe ");
        }
        if let Some(abi) = &sig.abi {
            let name = abi
                .name
                .as_ref()
                .map(|s| s.value())
                .unwrap_or_else(|| "C".to_string());
            header.push_str(&format!("extern \"{}\" ", name));
        }
        if is_static {
            header.push_str("static ");
        }

        let ret_ty = self.fmt_return_type(&sig.output, is_async);
        let fn_name = escape_ident(&sig.ident.to_string());
        let gens = self.fmt_generics(&sig.generics);

        self.push_scope();
        let prev_declared = std::mem::take(&mut self.declared_in_fn);

        let mut params = Vec::new();
        for arg in &sig.inputs {
            match arg {
                FnArg::Receiver(r) => {
                    if let Some((_, lt)) = &r.reference {
                        let life = lt
                            .as_ref()
                            .map(|l| format!("<{}>", l.ident))
                            .unwrap_or_default();
                        if r.mutability.is_some() {
                            params.push(format!("write{} this", life));
                        } else {
                            params.push(format!("read{} this", life));
                        }
                    } else if r.colon_token.is_some() {
                        params.push(format!("take {} this", self.fmt_type(&r.ty)));
                    } else {
                        params.push("take this".to_string());
                    }
                }
                FnArg::Typed(pt) => {
                    let ty_s = self.fmt_type(&pt.ty);
                    let (is_mut, pname) = extract_param_pat(&pt.pat);
                    self.bind_var(&pname, ty_s.clone());
                    let mk = if is_mut { "mut " } else { "" };
                    params.push(format!("{}{} {}", mk, ty_s, escape_ident(&pname)));
                }
            }
        }

        let wh = self.fmt_where(&sig.generics);
        header.push_str(&format!(
            "{} {}{}({}){}",
            ret_ty,
            fn_name,
            gens,
            params.join(", "),
            wh
        ));

        let expects_return = !matches!(sig.output, ReturnType::Default);

        if let Some(blk) = body {
            self.line(&format!("{} {{", header));
            self.indent += 1;
            self.emit_block_stmts(blk, expects_return)?;
            self.indent -= 1;
            self.line("}");
        } else {
            self.line(&format!("{};", header));
        }

        self.declared_in_fn = prev_declared;
        self.pop_scope();
        Ok(())
    }

    fn emit_block_stmts(&mut self, block: &Block, tail_returns: bool) -> Result<()> {
        self.push_scope();
        let len = block.stmts.len();
        for (idx, stmt) in block.stmts.iter().enumerate() {
            let is_last = idx + 1 == len;
            match stmt {
                Stmt::Local(local) => self.emit_local(local)?,
                Stmt::Item(item) => self.emit_item(item)?,
                Stmt::Expr(expr, semi) => {
                    if is_last && semi.is_none() && tail_returns {
                        self.emit_tail_return(expr)?;
                    } else {
                        self.emit_expr_stmt(expr)?;
                    }
                }
                Stmt::Macro(sm) => {
                    self.line(&format!("{};", fmt_syn_macro(&sm.mac)));
                }
            }
        }
        self.pop_scope();
        Ok(())
    }

    fn emit_tail_return(&mut self, expr: &Expr) -> Result<()> {
        match expr {
            Expr::Return(_) => self.emit_expr_stmt(expr),
            Expr::If(eif) => self.emit_if(eif, false, true),
            Expr::Match(em) => self.emit_match_stmt(em, true),
            other => {
                let s = self.fmt_expr(other)?;
                self.line(&format!("return {};", s));
                Ok(())
            }
        }
    }

    fn emit_local(&mut self, local: &Local) -> Result<()> {
        // Handle `let Pat = expr else { ... };` -> `require (expr is Pat) else { ... }`
        if let Some(init) = &local.init
            && let Some((_, diverge)) = &init.diverge
        {
            let expr_s = self.fmt_expr(&init.expr)?;
            let pat_s = self.fmt_pat(&local.pat, None)?;
            self.line(&format!("require ({} is {}) else {{", expr_s, pat_s));
            self.indent += 1;
            if let Expr::Block(eb) = diverge.as_ref() {
                self.emit_block_stmts(&eb.block, false)?;
            } else {
                let ds = self.fmt_expr(diverge)?;
                self.line(&format!("{};", ds));
            }
            self.indent -= 1;
            self.line("}");
            return Ok(());
        }

        // Discard `let _ = expr;`
        if matches!(local.pat, Pat::Wild(_))
            && let Some(init) = &local.init
        {
            let e = self.fmt_expr(&init.expr)?;
            self.line(&format!("_ = {};", e));
            return Ok(());
        }

        // Unwrap explicit type annotation if present: `let x: T = ...;`
        let (inner_pat, explicit_ty) = match &local.pat {
            Pat::Type(pt) => (pt.pat.as_ref(), Some(pt.ty.as_ref())),
            other => (other, None),
        };

        // Tuple destructuring `let (a, b): (i32, i32) = pair;`
        if let Pat::Tuple(ptup) = inner_pat {
            let elem_tys: Vec<String> = match explicit_ty {
                Some(Type::Tuple(tt)) => tt.elems.iter().map(|t| self.fmt_type(t)).collect(),
                _ => {
                    if let Some(init) = &local.init {
                        if let Expr::Tuple(et) = init.expr.as_ref() {
                            et.elems
                                .iter()
                                .map(|e| self.infer_expr_type(e))
                                .collect::<Option<Vec<_>>>()
                                .ok_or_else(|| {
                                    RSharpError::unresolved_type(
                                        "tuple",
                                        "Could not infer tuple element types",
                                    )
                                })?
                        } else {
                            return Err(RSharpError::unresolved_type(
                                "tuple",
                                "Add explicit type annotation for tuple destructuring",
                            ));
                        }
                    } else {
                        return Err(RSharpError::unresolved_type(
                            "tuple",
                            "Tuple binding requires initializer or type annotation",
                        ));
                    }
                }
            };
            let mut parts = Vec::new();
            for (pat, ty_s) in ptup.elems.iter().zip(elem_tys) {
                let (is_mut, name) = extract_param_pat(pat);
                self.bind_var(&name, ty_s.clone());
                let mk = if is_mut { "mut " } else { "" };
                parts.push(format!("{}{} {}", mk, ty_s, escape_ident(&name)));
            }
            let rhs = match &local.init {
                Some(init) => self.fmt_expr(&init.expr)?,
                None => {
                    return Err(RSharpError::from_rust(
                        "Tuple destructuring without initializer",
                    ));
                }
            };
            self.line(&format!("({}) = {};", parts.join(", "), rhs));
            return Ok(());
        }

        // Standard variable binding `let (mut) x(: T)? (= init)?;`
        let (is_mut, name) = extract_param_pat(inner_pat);
        let ty_s = if let Some(ty) = explicit_ty {
            self.fmt_type(ty)
        } else if let Some(init) = &local.init {
            if let Some(inferred) = self.infer_expr_type(&init.expr) {
                inferred
            } else if self.options.use_rustc_inference {
                self.probe_type_via_rustc(&name)?
            } else {
                return Err(RSharpError::unresolved_type(
                    &name,
                    "Local inference could not determine type and rustc probing is disabled",
                ));
            }
        } else {
            return Err(RSharpError::unresolved_type(
                &name,
                "Unannotated variable declaration has no initializer",
            ));
        };

        let is_shadow = self.declared_in_fn.contains(&name);
        let sh_kw = if is_shadow { "shadow " } else { "" };
        let mut_kw = if is_mut { "mut " } else { "" };
        let init_s = match &local.init {
            Some(init) => format!(" = {}", self.fmt_expr(&init.expr)?),
            None => String::new(),
        };
        self.bind_var(&name, ty_s.clone());
        self.line(&format!(
            "{}{}{} {}{};",
            sh_kw,
            mut_kw,
            ty_s,
            escape_ident(&name),
            init_s
        ));
        Ok(())
    }

    fn probe_type_via_rustc(&mut self, var_name: &str) -> Result<String> {
        self.probe_counter += 1;
        let rust_ty =
            rustc_runner::probe_local_type(&self.raw_source, var_name, self.probe_counter)?;
        let parsed_ty: Type = syn::parse_str(&rust_ty).map_err(|_| {
            RSharpError::unresolved_type(
                var_name,
                format!("rustc reported unparseable type '{}'", rust_ty),
            )
        })?;
        Ok(strip_non_static_lifetimes(&self.fmt_type(&parsed_ty)))
    }

    fn probe_closure_param_via_rustc(&mut self, param_name: &str) -> Result<String> {
        self.probe_counter += 1;
        let rust_ty = rustc_runner::probe_closure_param_type(
            &self.raw_source,
            param_name,
            self.probe_counter,
        )?;
        let parsed_ty: Type = syn::parse_str(&rust_ty).map_err(|_| {
            RSharpError::unresolved_type(
                param_name,
                format!(
                    "rustc reported unparseable closure param type '{}'",
                    rust_ty
                ),
            )
        })?;
        Ok(strip_non_static_lifetimes(&self.fmt_type(&parsed_ty)))
    }

    fn emit_expr_stmt(&mut self, expr: &Expr) -> Result<()> {
        match expr {
            Expr::If(eif) => self.emit_if(eif, false, false),
            Expr::While(ew) => {
                let lbl = ew
                    .label
                    .as_ref()
                    .map(|l| format!("{}: ", l.name.ident))
                    .unwrap_or_default();
                let cond = self.fmt_condition(&ew.cond)?;
                self.line(&format!("{}while ({}) {{", lbl, cond));
                self.indent += 1;
                self.emit_block_stmts(&ew.body, false)?;
                self.indent -= 1;
                self.line("}");
                Ok(())
            }
            Expr::ForLoop(ef) => {
                let lbl = ef
                    .label
                    .as_ref()
                    .map(|l| format!("{}: ", l.name.ident))
                    .unwrap_or_default();
                let elem_hint = self.infer_iter_elem_type(&ef.expr);
                let pat = self.fmt_pat(&ef.pat, elem_hint.as_deref())?;
                let iter = self.fmt_expr(&ef.expr)?;
                self.line(&format!("{}foreach ({} in {}) {{", lbl, pat, iter));
                self.indent += 1;
                self.emit_block_stmts(&ef.body, false)?;
                self.indent -= 1;
                self.line("}");
                Ok(())
            }
            Expr::Loop(el) => {
                let lbl = el
                    .label
                    .as_ref()
                    .map(|l| format!("{}: ", l.name.ident))
                    .unwrap_or_default();
                self.line(&format!("{}loop {{", lbl));
                self.indent += 1;
                self.emit_block_stmts(&el.body, false)?;
                self.indent -= 1;
                self.line("}");
                Ok(())
            }
            Expr::Match(em) => self.emit_match_stmt(em, false),
            Expr::Unsafe(eu) => {
                self.line("unsafe {");
                self.indent += 1;
                self.emit_block_stmts(&eu.block, false)?;
                self.indent -= 1;
                self.line("}");
                Ok(())
            }
            Expr::Return(er) => {
                if let Some(e) = &er.expr {
                    let s = self.fmt_expr(e)?;
                    self.line(&format!("return {};", s));
                } else {
                    self.line("return;");
                }
                Ok(())
            }
            Expr::Break(eb) => {
                let mut line = String::from("break");
                if let Some(l) = &eb.label {
                    line.push_str(&format!(" {}", l.ident));
                }
                if let Some(e) = &eb.expr {
                    line.push(' ');
                    line.push_str(&self.fmt_expr(e)?);
                }
                line.push(';');
                self.line(&line);
                Ok(())
            }
            Expr::Continue(ec) => {
                if let Some(l) = &ec.label {
                    self.line(&format!("continue {};", l.ident));
                } else {
                    self.line("continue;");
                }
                Ok(())
            }
            other => {
                let s = self.fmt_expr(other)?;
                self.line(&format!("{};", s));
                Ok(())
            }
        }
    }

    fn emit_if(&mut self, eif: &syn::ExprIf, is_else_if: bool, tail_returns: bool) -> Result<()> {
        let kw = if is_else_if { "else if" } else { "if" };
        let cond = self.fmt_condition(&eif.cond)?;
        self.line(&format!("{} ({}) {{", kw, cond));
        self.indent += 1;
        self.emit_block_stmts(&eif.then_branch, tail_returns)?;
        self.indent -= 1;
        if let Some((_, else_expr)) = &eif.else_branch {
            match else_expr.as_ref() {
                Expr::If(next) => {
                    self.line("}");
                    self.emit_if(next, true, tail_returns)?;
                }
                Expr::Block(eb) => {
                    self.line("} else {");
                    self.indent += 1;
                    self.emit_block_stmts(&eb.block, tail_returns)?;
                    self.indent -= 1;
                    self.line("}");
                }
                other => {
                    self.line("} else {");
                    self.indent += 1;
                    if tail_returns {
                        self.emit_tail_return(other)?;
                    } else {
                        self.emit_expr_stmt(other)?;
                    }
                    self.indent -= 1;
                    self.line("}");
                }
            }
        } else {
            self.line("}");
        }
        Ok(())
    }

    fn emit_match_stmt(&mut self, em: &syn::ExprMatch, tail_returns: bool) -> Result<()> {
        let scr = self.fmt_expr(&em.expr)?;
        self.line(&format!("match ({}) {{", scr));
        self.indent += 1;
        for arm in &em.arms {
            let pat = self.fmt_pat(&arm.pat, None)?;
            let guard = match &arm.guard {
                Some((_, ge)) => format!(" when {}", self.fmt_expr(ge)?),
                None => String::new(),
            };
            match arm.body.as_ref() {
                Expr::Block(eb) => {
                    self.line(&format!("{}{} => {{", pat, guard));
                    self.indent += 1;
                    self.emit_block_stmts(&eb.block, tail_returns)?;
                    self.indent -= 1;
                    self.line("}");
                }
                other if tail_returns => {
                    let s = self.fmt_expr(other)?;
                    self.line(&format!("{}{} => {{ return {}; }}", pat, guard, s));
                }
                other => {
                    let s = self.fmt_expr(other)?;
                    self.line(&format!("{}{} => {},", pat, guard, s));
                }
            }
        }
        self.indent -= 1;
        self.line("}");
        Ok(())
    }

    fn fmt_condition(&mut self, expr: &Expr) -> Result<String> {
        if let Expr::Let(el) = expr {
            let rhs = self.fmt_expr(&el.expr)?;
            let pat = self.fmt_pat(&el.pat, None)?;
            Ok(format!("{} is {}", rhs, pat))
        } else {
            self.fmt_expr(expr)
        }
    }

    // ==================== TYPES & GENERICS ====================

    fn fmt_return_type(&self, ret: &ReturnType, is_async: bool) -> String {
        match ret {
            ReturnType::Default => {
                if is_async {
                    "Task".to_string()
                } else {
                    "void".to_string()
                }
            }
            ReturnType::Type(_, ty) => {
                let inner = self.fmt_type(ty);
                if is_async {
                    format!("Task<{}>", inner)
                } else {
                    inner
                }
            }
        }
    }

    fn fmt_type(&self, ty: &Type) -> String {
        match ty {
            Type::Tuple(t) if t.elems.is_empty() => "void".to_string(),
            Type::Tuple(t) => {
                let es: Vec<String> = t.elems.iter().map(|e| self.fmt_type(e)).collect();
                format!("({})", es.join(", "))
            }
            Type::Never(_) => "never".to_string(),
            Type::Reference(r) => {
                let kw = if r.mutability.is_some() {
                    "write"
                } else {
                    "read"
                };
                let life = r
                    .lifetime
                    .as_ref()
                    .map(|l| format!("<{}>", l.ident))
                    .unwrap_or_default();
                format!("{}{} {}", kw, life, self.fmt_type(&r.elem))
            }
            Type::Slice(s) => format!("slice<{}>", self.fmt_type(&s.elem)),
            Type::Ptr(p) => {
                let kw = if p.mutability.is_some() {
                    "write"
                } else {
                    "read"
                };
                format!("{} ptr<{}>", kw, self.fmt_type(&p.elem))
            }
            Type::Array(arr) => {
                let mut dims = vec![arr.len.to_token_stream().to_string()];
                let mut cur = arr.elem.as_ref();
                while let Type::Array(inner) = cur {
                    dims.push(inner.len.to_token_stream().to_string());
                    cur = inner.elem.as_ref();
                }
                format!("{}[{}]", self.fmt_type(cur), dims.join(", "))
            }
            Type::TraitObject(to) => {
                let bs: Vec<String> = to.bounds.iter().map(|b| self.fmt_bound(b)).collect();
                format!("dyn {}", bs.join(" + "))
            }
            Type::ImplTrait(it) => {
                let bs: Vec<String> = it.bounds.iter().map(|b| self.fmt_bound(b)).collect();
                format!("impl {}", bs.join(" + "))
            }
            Type::BareFn(bf) => {
                let ps: Vec<String> = bf.inputs.iter().map(|i| self.fmt_type(&i.ty)).collect();
                let ret = self.fmt_return_type(&bf.output, false);
                format!("({}) -> {}", ps.join(", "), ret)
            }
            Type::Path(tp) => {
                // Detect `Box<dyn Any>` -> `dynamic`
                if is_boxed_any(&tp.path) {
                    return "dynamic".to_string();
                }
                // Detect `Option<T>` -> `T?` for simple inner types
                if let Some(inner) = extract_option_inner(&tp.path) {
                    let inner_s = self.fmt_type(inner);
                    if !inner_s.starts_with("read ")
                        && !inner_s.starts_with("write ")
                        && !inner_s.starts_with("dyn ")
                        && !inner_s.starts_with("impl ")
                    {
                        return format!("{}?", inner_s);
                    }
                }
                self.fmt_path(&tp.path)
            }
            other => other.to_token_stream().to_string(),
        }
    }

    fn fmt_path(&self, path: &Path) -> String {
        if path.segments.len() == 1 && path.segments[0].arguments.is_empty() {
            let name = path.segments[0].ident.to_string();
            if let Some(mapped) = map_rust_primitive(&name) {
                return mapped.to_string();
            }
        }
        let mut out = Vec::new();
        for seg in &path.segments {
            let name = map_rust_std_alias(&seg.ident.to_string());
            match &seg.arguments {
                PathArguments::None => out.push(name),
                PathArguments::AngleBracketed(ab) => {
                    let args: Vec<String> = ab
                        .args
                        .iter()
                        .map(|a| match a {
                            GenericArgument::Lifetime(l) => format!("life {}", l.ident),
                            GenericArgument::Type(t) => self.fmt_type(t),
                            GenericArgument::AssocType(at) => {
                                format!("{} = {}", at.ident, self.fmt_type(&at.ty))
                            }
                            GenericArgument::Const(e) => e.to_token_stream().to_string(),
                            other => other.to_token_stream().to_string(),
                        })
                        .collect();
                    out.push(format!("{}<{}>", name, args.join(", ")));
                }
                PathArguments::Parenthesized(p) => {
                    let ins: Vec<String> = p.inputs.iter().map(|t| self.fmt_type(t)).collect();
                    let ret = self.fmt_return_type(&p.output, false);
                    out.push(format!("{}({}) -> {}", name, ins.join(", "), ret));
                }
            }
        }
        out.join(".")
    }

    fn fmt_generics(&self, generics: &Generics) -> String {
        if generics.params.is_empty() {
            return String::new();
        }
        let mut parts = Vec::new();
        for p in &generics.params {
            match p {
                GenericParam::Lifetime(lt) => {
                    parts.push(format!("life {}", lt.lifetime.ident));
                }
                GenericParam::Type(tp) => {
                    if tp.bounds.is_empty() {
                        parts.push(tp.ident.to_string());
                    } else {
                        let bs: Vec<String> = tp.bounds.iter().map(|b| self.fmt_bound(b)).collect();
                        parts.push(format!("{} : {}", tp.ident, bs.join(" + ")));
                    }
                }
                GenericParam::Const(cp) => {
                    parts.push(format!("const {} {}", self.fmt_type(&cp.ty), cp.ident));
                }
            }
        }
        format!("<{}>", parts.join(", "))
    }

    fn fmt_where(&self, generics: &Generics) -> String {
        let Some(wc) = &generics.where_clause else {
            return String::new();
        };
        if wc.predicates.is_empty() {
            return String::new();
        }
        let mut preds = Vec::new();
        for p in &wc.predicates {
            match p {
                WherePredicate::Type(pt) => {
                    let bs: Vec<String> = pt.bounds.iter().map(|b| self.fmt_bound(b)).collect();
                    preds.push(format!(
                        "{} : {}",
                        self.fmt_type(&pt.bounded_ty),
                        bs.join(", ")
                    ));
                }
                WherePredicate::Lifetime(pl) => {
                    let os: Vec<String> = pl.bounds.iter().map(|l| l.ident.to_string()).collect();
                    preds.push(format!("life {} : {}", pl.lifetime.ident, os.join(", ")));
                }
                _ => {}
            }
        }
        format!(" where {}", preds.join(", "))
    }

    fn fmt_bound(&self, b: &TypeParamBound) -> String {
        match b {
            TypeParamBound::Trait(tb) => self.fmt_path(&tb.path),
            TypeParamBound::Lifetime(lt) => {
                if lt.ident == "static" {
                    "static".to_string()
                } else {
                    format!("life {}", lt.ident)
                }
            }
            other => other.to_token_stream().to_string(),
        }
    }

    // ==================== PATTERNS & EXPRESSIONS ====================

    fn fmt_pat(&mut self, pat: &Pat, ty_hint: Option<&str>) -> Result<String> {
        Ok(match pat {
            Pat::Wild(_) => "_".to_string(),
            Pat::Lit(l) => l.lit.to_token_stream().to_string(),
            Pat::Range(r) => {
                let s = r
                    .start
                    .as_ref()
                    .map(|e| self.fmt_expr(e))
                    .transpose()?
                    .unwrap_or_default();
                let e = r
                    .end
                    .as_ref()
                    .map(|e| self.fmt_expr(e))
                    .transpose()?
                    .unwrap_or_default();
                let op = match r.limits {
                    syn::RangeLimits::Closed(_) => "..=",
                    syn::RangeLimits::HalfOpen(_) => "..",
                };
                format!("{}{}{}", s, op, e)
            }
            Pat::Ident(pi) => {
                let name = escape_ident(&pi.ident.to_string());
                let mk = if pi.mutability.is_some() { "mut " } else { "" };
                if let Some(ty) = ty_hint {
                    self.bind_var(&pi.ident.to_string(), ty.to_string());
                    format!("{}{} {}", mk, ty, name)
                } else {
                    format!("{}{}", mk, name)
                }
            }
            Pat::Tuple(pt) => {
                let ps: Result<Vec<_>> = pt.elems.iter().map(|p| self.fmt_pat(p, None)).collect();
                format!("({})", ps?.join(", "))
            }
            Pat::TupleStruct(pts) => {
                let fs: Result<Vec<_>> = pts.elems.iter().map(|p| self.fmt_pat(p, None)).collect();
                format!("{}({})", self.fmt_path(&pts.path), fs?.join(", "))
            }
            Pat::Struct(ps) => {
                let mut fs = Vec::new();
                for f in &ps.fields {
                    let fname = match &f.member {
                        Member::Named(id) => escape_ident(&id.to_string()),
                        Member::Unnamed(idx) => idx.index.to_string(),
                    };
                    if f.colon_token.is_some() {
                        fs.push(format!("{}: {}", fname, self.fmt_pat(&f.pat, None)?));
                    } else {
                        fs.push(fname);
                    }
                }
                if ps.rest.is_some() {
                    fs.push("..".to_string());
                }
                format!("{} {{ {} }}", self.fmt_path(&ps.path), fs.join(", "))
            }
            Pat::Path(pp) => self.fmt_path(&pp.path),
            Pat::Or(po) => {
                let alts: Result<Vec<_>> = po.cases.iter().map(|p| self.fmt_pat(p, None)).collect();
                alts?.join(" | ")
            }
            other => other.to_token_stream().to_string(),
        })
    }

    fn fmt_expr(&mut self, expr: &Expr) -> Result<String> {
        Ok(match expr {
            Expr::Lit(l) => l.lit.to_token_stream().to_string(),
            Expr::Path(ep) => {
                if ep.path.is_ident("self") {
                    "this".to_string()
                } else {
                    self.fmt_path(&ep.path)
                }
            }
            Expr::Paren(ep) => format!("({})", self.fmt_expr(&ep.expr)?),
            Expr::Unary(eu) => {
                let inner = self.fmt_expr(&eu.expr)?;
                match eu.op {
                    UnOp::Deref(_) => format!("deref {}", inner),
                    UnOp::Not(_) => format!("!{}", inner),
                    UnOp::Neg(_) => format!("-{}", inner),
                    _ => format!("{}{}", eu.op.to_token_stream(), inner),
                }
            }
            Expr::Reference(er) => {
                let kw = if er.mutability.is_some() {
                    "write"
                } else {
                    "read"
                };
                format!("{} {}", kw, self.fmt_expr(&er.expr)?)
            }
            Expr::Binary(eb) => {
                let op = fmt_bin_op(&eb.op);
                format!(
                    "({} {} {})",
                    self.fmt_expr(&eb.left)?,
                    op,
                    self.fmt_expr(&eb.right)?
                )
            }
            Expr::Assign(ea) => {
                format!(
                    "{} = {}",
                    self.fmt_expr(&ea.left)?,
                    self.fmt_expr(&ea.right)?
                )
            }
            Expr::Cast(ec) => {
                format!(
                    "({} as {})",
                    self.fmt_expr(&ec.expr)?,
                    self.fmt_type(&ec.ty)
                )
            }
            Expr::Try(et) => format!("try {}", self.fmt_expr(&et.expr)?),
            Expr::Await(ea) => format!("await {}", self.fmt_expr(&ea.base)?),
            Expr::Call(ec) => {
                let callee = self.fmt_expr(&ec.func)?;
                let args: Result<Vec<_>> = ec.args.iter().map(|a| self.fmt_expr(a)).collect();
                format!("{}({})", callee, args?.join(", "))
            }
            Expr::MethodCall(mc) => {
                let recv = self.fmt_expr(&mc.receiver)?;
                let mname = mc.method.to_string();
                let targs = match &mc.turbofish {
                    Some(tf) => {
                        let ts: Vec<String> = tf
                            .args
                            .iter()
                            .map(|a| match a {
                                GenericArgument::Type(t) => self.fmt_type(t),
                                other => other.to_token_stream().to_string(),
                            })
                            .collect();
                        format!("<{}>", ts.join(", "))
                    }
                    None => String::new(),
                };
                let args: Result<Vec<_>> = mc.args.iter().map(|a| self.fmt_expr(a)).collect();
                format!("{}.{}{}({})", recv, mname, targs, args?.join(", "))
            }
            Expr::Field(ef) => {
                let base = self.fmt_expr(&ef.base)?;
                match &ef.member {
                    Member::Named(id) => format!("{}.{}", base, id),
                    Member::Unnamed(idx) => format!("{}.{}", base, idx.index),
                }
            }
            Expr::Index(ei) => {
                format!(
                    "{}[{}]",
                    self.fmt_expr(&ei.expr)?,
                    self.fmt_expr(&ei.index)?
                )
            }
            Expr::Array(ea) => {
                let es: Result<Vec<_>> = ea.elems.iter().map(|e| self.fmt_expr(e)).collect();
                format!("[{}]", es?.join(", "))
            }
            Expr::Repeat(er) => {
                format!(
                    "[{}; {}]",
                    self.fmt_expr(&er.expr)?,
                    self.fmt_expr(&er.len)?
                )
            }
            Expr::Tuple(et) => {
                let es: Result<Vec<_>> = et.elems.iter().map(|e| self.fmt_expr(e)).collect();
                let es = es?;
                if es.len() == 1 {
                    format!("({},)", es[0])
                } else {
                    format!("({})", es.join(", "))
                }
            }
            Expr::Range(er) => {
                let s = er
                    .start
                    .as_ref()
                    .map(|e| self.fmt_expr(e))
                    .transpose()?
                    .unwrap_or_default();
                let e = er
                    .end
                    .as_ref()
                    .map(|e| self.fmt_expr(e))
                    .transpose()?
                    .unwrap_or_default();
                let op = match er.limits {
                    syn::RangeLimits::Closed(_) => "..=",
                    syn::RangeLimits::HalfOpen(_) => "..",
                };
                format!("{}{}{}", s, op, e)
            }
            Expr::Struct(es) => {
                let mut fs = Vec::new();
                for f in &es.fields {
                    let fname = match &f.member {
                        Member::Named(id) => escape_ident(&id.to_string()),
                        Member::Unnamed(idx) => idx.index.to_string(),
                    };
                    fs.push(format!("{} = {}", fname, self.fmt_expr(&f.expr)?));
                }
                if let Some(rest) = &es.rest {
                    format!("{} with {{ {} }}", self.fmt_expr(rest)?, fs.join(", "))
                } else {
                    format!("new {} {{ {} }}", self.fmt_path(&es.path), fs.join(", "))
                }
            }
            Expr::If(eif) => {
                // Single-expression if/else converts cleanly to C# ternary `cond ? a : b`
                if let (Some(then_e), Some((_, else_box))) =
                    (single_tail_expr(&eif.then_branch), &eif.else_branch)
                    && let Expr::Block(eb) = else_box.as_ref()
                    && let Some(else_e) = single_tail_expr(&eb.block)
                {
                    let c = self.fmt_expr(&eif.cond)?;
                    let t = self.fmt_expr(then_e)?;
                    let e = self.fmt_expr(else_e)?;
                    return Ok(format!("({} ? {} : {})", c, t, e));
                }
                format!("rust {{ {} }}", eif.to_token_stream())
            }
            Expr::Match(em) => {
                let scr = self.fmt_expr(&em.expr)?;
                let mut arms = Vec::new();
                for arm in &em.arms {
                    let pat = self.fmt_pat(&arm.pat, None)?;
                    let g = match &arm.guard {
                        Some((_, ge)) => format!(" when {}", self.fmt_expr(ge)?),
                        None => String::new(),
                    };
                    let body = self.fmt_expr(&arm.body)?;
                    arms.push(format!("{}{} => {}", pat, g, body));
                }
                format!("match ({}) {{ {} }}", scr, arms.join(", "))
            }
            Expr::Closure(ec) => {
                let mv = if ec.capture.is_some() { "take " } else { "" };
                let mut ps = Vec::new();
                for inp in &ec.inputs {
                    if let Pat::Type(pt) = inp {
                        let (is_mut, pname) = extract_param_pat(&pt.pat);
                        let mk = if is_mut { "mut " } else { "" };
                        ps.push(format!(
                            "{}{} {}",
                            mk,
                            self.fmt_type(&pt.ty),
                            escape_ident(&pname)
                        ));
                    } else {
                        let (is_mut, pname) = extract_param_pat(inp);
                        if self.options.use_rustc_inference {
                            let inferred = self.probe_closure_param_via_rustc(&pname)?;
                            let mk = if is_mut { "mut " } else { "" };
                            ps.push(format!("{}{} {}", mk, inferred, escape_ident(&pname)));
                        } else {
                            return Err(RSharpError::unresolved_type(
                                pname,
                                "Closure parameter requires explicit type in R#",
                            ));
                        }
                    }
                }
                let ret = match &ec.output {
                    ReturnType::Default => String::new(),
                    ReturnType::Type(_, ty) => format!(" : {}", self.fmt_type(ty)),
                };
                match ec.body.as_ref() {
                    Expr::Block(eb) => {
                        let mut sub = RustToRSharp::new(self.raw_source.clone(), self.options);
                        sub.indent = self.indent + 1;
                        sub.scopes = self.scopes.clone();
                        sub.emit_block_stmts(&eb.block, !matches!(ec.output, ReturnType::Default))?;
                        format!("{}({}){} => {{\n{}}}", mv, ps.join(", "), ret, sub.out)
                    }
                    other => format!(
                        "{}({}){} => {}",
                        mv,
                        ps.join(", "),
                        ret,
                        self.fmt_expr(other)?
                    ),
                }
            }
            Expr::Block(eb) => {
                if let Some(lbl) = &eb.label {
                    let mut sub = RustToRSharp::new(self.raw_source.clone(), self.options);
                    sub.indent = self.indent + 1;
                    sub.scopes = self.scopes.clone();
                    sub.emit_block_stmts(&eb.block, false)?;
                    format!("{}: {{\n{}}}", lbl.name.ident, sub.out)
                } else if let Some(single) = single_tail_expr(&eb.block) {
                    self.fmt_expr(single)?
                } else {
                    format!("rust {{ {} }}", eb.to_token_stream())
                }
            }
            Expr::Async(ea) => {
                let mv = if ea.capture.is_some() { "take " } else { "" };
                let mut sub = RustToRSharp::new(self.raw_source.clone(), self.options);
                sub.indent = self.indent + 1;
                sub.scopes = self.scopes.clone();
                sub.emit_block_stmts(&ea.block, true)?;
                format!("async {}{{\n{}}}", mv, sub.out)
            }
            Expr::Unsafe(eu) => {
                let mut sub = RustToRSharp::new(self.raw_source.clone(), self.options);
                sub.indent = self.indent + 1;
                sub.scopes = self.scopes.clone();
                sub.emit_block_stmts(&eu.block, false)?;
                format!("unsafe {{\n{}}}", sub.out)
            }
            Expr::Macro(em) => fmt_syn_macro(&em.mac),
            other => format!("rust {{ {} }}", other.to_token_stream()),
        })
    }

    // ==================== LOCAL TYPE INFERENCE ====================

    fn infer_expr_type(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Lit(el) => match &el.lit {
                syn::Lit::Int(li) => {
                    Some(map_rust_primitive(li.suffix()).unwrap_or("int").to_string())
                }
                syn::Lit::Float(lf) => Some(
                    map_rust_primitive(lf.suffix())
                        .unwrap_or("double")
                        .to_string(),
                ),
                syn::Lit::Str(_) => Some("read str".to_string()),
                syn::Lit::Char(_) => Some("char".to_string()),
                syn::Lit::Bool(_) => Some("bool".to_string()),
                _ => None,
            },
            Expr::Paren(ep) => self.infer_expr_type(&ep.expr),
            Expr::Cast(ec) => Some(self.fmt_type(&ec.ty)),
            Expr::Reference(er) => {
                let inner = self.infer_expr_type(&er.expr)?;
                let kw = if er.mutability.is_some() {
                    "write"
                } else {
                    "read"
                };
                Some(format!("{} {}", kw, inner))
            }
            Expr::Unary(eu) => match eu.op {
                UnOp::Not(_) => self.infer_expr_type(&eu.expr),
                UnOp::Neg(_) => self.infer_expr_type(&eu.expr),
                UnOp::Deref(_) => {
                    let inner = self.infer_expr_type(&eu.expr)?;
                    inner
                        .strip_prefix("read ")
                        .or_else(|| inner.strip_prefix("write "))
                        .map(|s| s.to_string())
                }
                _ => None,
            },
            Expr::Binary(eb) => match eb.op {
                BinOp::Eq(_)
                | BinOp::Ne(_)
                | BinOp::Lt(_)
                | BinOp::Le(_)
                | BinOp::Gt(_)
                | BinOp::Ge(_)
                | BinOp::And(_)
                | BinOp::Or(_) => Some("bool".to_string()),
                _ => self
                    .infer_expr_type(&eb.left)
                    .or_else(|| self.infer_expr_type(&eb.right)),
            },
            Expr::Struct(es) => Some(self.fmt_path(&es.path)),
            Expr::Tuple(et) => {
                let elems: Option<Vec<String>> =
                    et.elems.iter().map(|e| self.infer_expr_type(e)).collect();
                Some(format!("({})", elems?.join(", ")))
            }
            Expr::Array(ea) => {
                let first = ea.elems.first()?;
                let elem_ty = self.infer_expr_type(first)?;
                Some(format!("{}[{}]", elem_ty, ea.elems.len()))
            }
            Expr::Repeat(er) => {
                let elem_ty = self.infer_expr_type(&er.expr)?;
                Some(format!("{}[{}]", elem_ty, er.len.to_token_stream()))
            }
            Expr::Path(ep) => {
                if let Some(id) = ep.path.get_ident() {
                    self.lookup_var(&id.to_string()).map(|s| s.to_string())
                } else {
                    None
                }
            }
            Expr::Macro(em) => {
                let name = em.mac.path.segments.last()?.ident.to_string();
                match name.as_str() {
                    "format" => Some("string".to_string()),
                    "vec" => {
                        let ts = em.mac.tokens.to_string();
                        if let Ok(arr) = syn::parse_str::<syn::ExprArray>(&format!("[{}]", ts)) {
                            let elem = arr.elems.first().and_then(|e| self.infer_expr_type(e))?;
                            return Some(format!("List<{}>", elem));
                        }
                        None
                    }
                    _ => None,
                }
            }
            Expr::Call(ec) => {
                if let Expr::Path(ep) = ec.func.as_ref() {
                    let full = path_to_rsharp(&ep.path);
                    if full == "String.from" || full == "String.new" {
                        return Some("string".to_string());
                    }
                    if full == "Some" || full == "Option.Some" {
                        let inner = ec.args.first().and_then(|a| self.infer_expr_type(a))?;
                        return Some(format!("{}?", inner));
                    }
                    if full == "Box.new" {
                        let inner = ec.args.first().and_then(|a| self.infer_expr_type(a))?;
                        return Some(format!("Box<{}>", inner));
                    }
                    let rust_key = ep
                        .path
                        .segments
                        .iter()
                        .map(|s| s.ident.to_string())
                        .collect::<Vec<_>>()
                        .join("::");
                    if let Some(ret) = self.fn_returns.get(&rust_key) {
                        return Some(strip_non_static_lifetimes(ret));
                    }
                }
                None
            }
            Expr::MethodCall(mc) => {
                let m = mc.method.to_string();
                match m.as_str() {
                    "to_string" | "to_owned" => Some("string".to_string()),
                    "len" => Some("nuint".to_string()),
                    "is_empty" => Some("bool".to_string()),
                    "clone" => self.infer_expr_type(&mc.receiver),
                    _ => self
                        .fn_returns
                        .get(&m)
                        .map(|r| strip_non_static_lifetimes(r)),
                }
            }
            Expr::If(eif) => {
                single_tail_expr(&eif.then_branch).and_then(|e| self.infer_expr_type(e))
            }
            Expr::Match(em) => em.arms.iter().find_map(|a| self.infer_expr_type(&a.body)),
            Expr::Range(er) => {
                let elem = er
                    .start
                    .as_ref()
                    .and_then(|e| self.infer_expr_type(e))
                    .or_else(|| er.end.as_ref().and_then(|e| self.infer_expr_type(e)))?;
                match er.limits {
                    syn::RangeLimits::Closed(_) => {
                        Some(format!("std.ops.RangeInclusive<{}>", elem))
                    }
                    syn::RangeLimits::HalfOpen(_) => Some(format!("std.ops.Range<{}>", elem)),
                }
            }
            Expr::Async(ea) => {
                let out = ea
                    .block
                    .stmts
                    .last()
                    .and_then(|st| match st {
                        Stmt::Expr(e, None) => self.infer_expr_type(e),
                        _ => None,
                    })
                    .unwrap_or_else(|| "void".to_string());
                Some(format!("impl std.future.Future<Output = {}>", out))
            }
            Expr::Closure(ec) => {
                let mut param_tys = Vec::new();
                for inp in &ec.inputs {
                    if let Pat::Type(pt) = inp {
                        param_tys.push(self.fmt_type(&pt.ty));
                    } else {
                        return None;
                    }
                }
                let ret = match &ec.output {
                    ReturnType::Type(_, ty) => self.fmt_type(ty),
                    ReturnType::Default => self
                        .infer_expr_type(&ec.body)
                        .unwrap_or_else(|| "void".to_string()),
                };
                Some(format!("({}) -> {}", param_tys.join(", "), ret))
            }
            _ => None,
        }
    }

    fn infer_iter_elem_type(&self, iter: &Expr) -> Option<String> {
        match iter {
            Expr::Range(er) => er
                .start
                .as_ref()
                .and_then(|e| self.infer_expr_type(e))
                .or_else(|| er.end.as_ref().and_then(|e| self.infer_expr_type(e))),
            Expr::Reference(er) => {
                let base = self.infer_expr_type(&er.expr)?;
                let elem = extract_collection_elem(&base)?;
                let kw = if er.mutability.is_some() {
                    "write"
                } else {
                    "read"
                };
                Some(format!("{} {}", kw, elem))
            }
            _ => {
                let base = self.infer_expr_type(iter)?;
                if let Some(inner) = base
                    .strip_prefix("read slice<")
                    .and_then(|s| s.strip_suffix('>'))
                {
                    return Some(format!("read {}", inner));
                }
                if let Some(inner) = base
                    .strip_prefix("write slice<")
                    .and_then(|s| s.strip_suffix('>'))
                {
                    return Some(format!("write {}", inner));
                }
                extract_collection_elem(&base)
            }
        }
    }
}

fn strip_non_static_lifetimes(ty: &str) -> String {
    let mut out = String::with_capacity(ty.len());
    let mut rest = ty;
    while !rest.is_empty() {
        let prefix = if rest.starts_with("read<") {
            Some(("read<", "read"))
        } else if rest.starts_with("write<") {
            Some(("write<", "write"))
        } else {
            None
        };
        if let Some((open, base)) = prefix
            && let Some(close_idx) = rest[open.len()..].find('>')
        {
            let life = &rest[open.len()..open.len() + close_idx];
            if life == "static" {
                out.push_str(&rest[..open.len() + close_idx + 1]);
            } else {
                out.push_str(base);
            }
            rest = &rest[open.len() + close_idx + 1..];
            continue;
        }
        let ch = rest.chars().next().unwrap();
        out.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    out
}

fn extract_collection_elem(ty: &str) -> Option<String> {
    if let Some(inner) = ty.strip_prefix("List<").and_then(|s| s.strip_suffix('>')) {
        return Some(inner.to_string());
    }
    if let Some((elem, _)) = ty.split_once('[') {
        return Some(elem.to_string());
    }
    None
}

fn single_tail_expr(block: &Block) -> Option<&Expr> {
    if block.stmts.len() == 1
        && let Stmt::Expr(e, None) = &block.stmts[0]
    {
        return Some(e);
    }
    None
}

fn extract_param_pat(pat: &Pat) -> (bool, String) {
    match pat {
        Pat::Ident(pi) => (pi.mutability.is_some(), pi.ident.to_string()),
        Pat::Wild(_) => (false, "_".to_string()),
        other => (false, other.to_token_stream().to_string()),
    }
}

fn flatten_use_tree(tree: &syn::UseTree, mut prefix: Vec<String>, out: &mut Vec<String>) {
    match tree {
        syn::UseTree::Path(p) => {
            prefix.push(p.ident.to_string());
            flatten_use_tree(&p.tree, prefix, out);
        }
        syn::UseTree::Name(n) => {
            prefix.push(map_rust_std_alias(&n.ident.to_string()));
            out.push(prefix.join("."));
        }
        syn::UseTree::Rename(r) => {
            prefix.push(map_rust_std_alias(&r.ident.to_string()));
            out.push(format!(
                "{} = {}",
                escape_ident(&r.rename.to_string()),
                prefix.join(".")
            ));
        }
        syn::UseTree::Glob(_) => {
            prefix.push("*".to_string());
            out.push(prefix.join("."));
        }
        syn::UseTree::Group(g) => {
            for item in &g.items {
                flatten_use_tree(item, prefix.clone(), out);
            }
        }
    }
}

fn fmt_vis(vis: &Visibility) -> &'static str {
    match vis {
        Visibility::Public(_) => "public ",
        Visibility::Restricted(r) => {
            if r.path.is_ident("crate") {
                "internal "
            } else {
                "public(super) "
            }
        }
        Visibility::Inherited => "",
    }
}

fn fmt_bin_op(op: &BinOp) -> &'static str {
    match op {
        BinOp::Add(_) => "+",
        BinOp::Sub(_) => "-",
        BinOp::Mul(_) => "*",
        BinOp::Div(_) => "/",
        BinOp::Rem(_) => "%",
        BinOp::And(_) => "&&",
        BinOp::Or(_) => "||",
        BinOp::BitXor(_) => "^",
        BinOp::BitAnd(_) => "&",
        BinOp::BitOr(_) => "|",
        BinOp::Shl(_) => "<<",
        BinOp::Shr(_) => ">>",
        BinOp::Eq(_) => "==",
        BinOp::Lt(_) => "<",
        BinOp::Le(_) => "<=",
        BinOp::Ne(_) => "!=",
        BinOp::Ge(_) => ">=",
        BinOp::Gt(_) => ">",
        BinOp::AddAssign(_) => "+=",
        BinOp::SubAssign(_) => "-=",
        BinOp::MulAssign(_) => "*=",
        BinOp::DivAssign(_) => "/=",
        BinOp::RemAssign(_) => "%=",
        BinOp::BitXorAssign(_) => "^=",
        BinOp::BitAndAssign(_) => "&=",
        BinOp::BitOrAssign(_) => "|=",
        BinOp::ShlAssign(_) => "<<=",
        BinOp::ShrAssign(_) => ">>=",
        _ => "+",
    }
}

fn path_to_rsharp(path: &Path) -> String {
    path.segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect::<Vec<_>>()
        .join(".")
}

fn is_boxed_any(path: &Path) -> bool {
    let Some(last) = path.segments.last() else {
        return false;
    };
    if last.ident != "Box" {
        return false;
    }
    if let PathArguments::AngleBracketed(ab) = &last.arguments
        && let Some(GenericArgument::Type(Type::TraitObject(to))) = ab.args.first()
    {
        return to.bounds.iter().any(|b| {
            if let TypeParamBound::Trait(tb) = b {
                tb.path.segments.last().is_some_and(|s| s.ident == "Any")
            } else {
                false
            }
        });
    }
    false
}

fn extract_option_inner(path: &Path) -> Option<&Type> {
    let last = path.segments.last()?;
    if last.ident != "Option" {
        return None;
    }
    if let PathArguments::AngleBracketed(ab) = &last.arguments
        && let Some(GenericArgument::Type(inner)) = ab.args.first()
    {
        return Some(inner);
    }
    None
}

fn map_rust_primitive(name: &str) -> Option<&'static str> {
    match name {
        "i8" => Some("sbyte"),
        "u8" => Some("byte"),
        "i16" => Some("short"),
        "u16" => Some("ushort"),
        "i32" => Some("int"),
        "u32" => Some("uint"),
        "i64" => Some("long"),
        "u64" => Some("ulong"),
        "i128" => Some("int128"),
        "u128" => Some("uint128"),
        "isize" => Some("nint"),
        "usize" => Some("nuint"),
        "f32" => Some("float"),
        "f64" => Some("double"),
        "String" => Some("string"),
        _ => None,
    }
}

fn map_rust_std_alias(name: &str) -> String {
    match name {
        "Vec" => "List".to_string(),
        "VecDeque" => "Queue".to_string(),
        "HashMap" => "Dictionary".to_string(),
        "BTreeMap" => "SortedDictionary".to_string(),
        "BTreeSet" => "SortedSet".to_string(),
        "BinaryHeap" => "PriorityQueue".to_string(),
        "String" => "string".to_string(),
        other => other.to_string(),
    }
}

fn map_rust_attr_name(name: &str) -> &str {
    match name {
        "derive" => "Derive",
        "test" => "Test",
        "cfg" => "Cfg",
        "cfg_attr" => "CfgAttr",
        "inline" => "Inline",
        "allow" => "Allow",
        "warn" => "Warn",
        "deny" => "Deny",
        "repr" => "Repr",
        "no_mangle" => "NoMangle",
        "must_use" => "MustUse",
        "deprecated" => "Deprecated",
        "should_panic" => "ShouldPanic",
        "ignore" => "Ignore",
        "non_exhaustive" => "NonExhaustive",
        "doc" => "Doc",
        "cold" => "Cold",
        "track_caller" => "TrackCaller",
        "proc_macro" => "ProcMacro",
        "proc_macro_derive" => "ProcMacroDerive",
        "proc_macro_attribute" => "ProcMacroAttribute",
        other => other,
    }
}

fn escape_ident(name: &str) -> String {
    if name == "_" || name == "Self" || name == "this" {
        return name.to_string();
    }
    if Keyword::parse(name).is_some() {
        format!("@{}", name)
    } else {
        name.to_string()
    }
}

fn fmt_syn_macro(mac: &syn::Macro) -> String {
    let path = path_to_rsharp(&mac.path);
    match mac.delimiter {
        syn::MacroDelimiter::Paren(_) => format!("{}!({})", path, mac.tokens),
        syn::MacroDelimiter::Bracket(_) => format!("{}![{}]", path, mac.tokens),
        syn::MacroDelimiter::Brace(_) => format!("{}!{{ {} }}", path, mac.tokens),
    }
}

fn pretty_item(item: &Item) -> String {
    let file = syn::File {
        shebang: None,
        attrs: Vec::new(),
        items: vec![item.clone()],
    };
    prettyplease::unparse(&file)
}
