use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use quote::ToTokens;
use syn::{
    Attribute, BinOp, Block, Expr, FnArg, GenericArgument, GenericParam, Generics, Item, Local,
    Member, Pat, Path, PathArguments, ReturnType, Stmt, TraitBoundModifier, TraitItem, Type,
    TypeParamBound, UnOp, Visibility, WherePredicate,
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
    conv.emit_file(&parsed);
    if let Some(err) = conv.first_err.borrow_mut().take() {
        return Err(err);
    }
    Ok(conv.out)
}

#[derive(Clone, Copy)]
enum BlockTail<'a> {
    None,
    Return,
    Break(&'a str),
}

struct RustToRSharp {
    raw_source: String,
    options: FromRustOptions,
    out: String,
    indent: usize,
    fn_returns: HashMap<String, String>,
    struct_fields: HashMap<String, HashMap<String, String>>,
    scopes: Vec<HashMap<String, String>>,
    declared_in_fn: HashSet<String>,
    probe_counter: Rc<Cell<usize>>,
    first_err: Rc<RefCell<Option<RSharpError>>>,
}

impl RustToRSharp {
    fn new(raw_source: String, options: FromRustOptions) -> Self {
        Self {
            raw_source,
            options,
            out: String::new(),
            indent: 0,
            fn_returns: HashMap::new(),
            struct_fields: HashMap::new(),
            scopes: vec![HashMap::new()],
            declared_in_fn: HashSet::new(),
            probe_counter: Rc::new(Cell::new(0)),
            first_err: Rc::new(RefCell::new(None)),
        }
    }

    fn child(&self) -> Self {
        Self {
            raw_source: self.raw_source.clone(),
            options: self.options,
            out: String::new(),
            indent: self.indent + 1,
            fn_returns: self.fn_returns.clone(),
            struct_fields: self.struct_fields.clone(),
            scopes: self.scopes.clone(),
            declared_in_fn: HashSet::new(),
            probe_counter: Rc::clone(&self.probe_counter),
            first_err: Rc::clone(&self.first_err),
        }
    }

    fn record_err(&self, err: RSharpError) {
        self.first_err.borrow_mut().get_or_insert(err);
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
        let idx = self.scopes.len() - 1;
        self.scopes[idx].insert(name.to_string(), ty);
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
                    if f.sig.generics.type_params().next().is_none() {
                        let ret = self.fmt_return_type(&f.sig.output, false);
                        self.fn_returns.insert(f.sig.ident.to_string(), ret);
                    }
                }
                Item::Impl(im) => {
                    let self_ty = self.fmt_type(&im.self_ty);
                    let is_trait_impl = im.trait_.is_some();
                    for sub in &im.items {
                        if let syn::ImplItem::Fn(m) = sub
                            && m.sig.generics.type_params().next().is_none()
                        {
                            let mut ret = self.fmt_return_type(&m.sig.output, false);
                            if ret == "Self" {
                                ret = self_ty.clone();
                            }
                            self.fn_returns
                                .insert(format!("{}::{}", self_ty, m.sig.ident), ret.clone());
                            if !is_trait_impl {
                                self.fn_returns.insert(m.sig.ident.to_string(), ret);
                            }
                        }
                    }
                }
                Item::Struct(s) => {
                    let mut fields = HashMap::new();
                    match &s.fields {
                        syn::Fields::Named(nf) => {
                            for f in &nf.named {
                                fields.insert(
                                    f.ident.as_ref().unwrap().to_string(),
                                    self.fmt_type(&f.ty),
                                );
                            }
                        }
                        syn::Fields::Unnamed(uf) => {
                            for (idx, f) in uf.unnamed.iter().enumerate() {
                                fields.insert(idx.to_string(), self.fmt_type(&f.ty));
                            }
                        }
                        syn::Fields::Unit => {}
                    }
                    if !fields.is_empty() {
                        self.struct_fields.insert(s.ident.to_string(), fields);
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

    fn emit_file(&mut self, file: &syn::File) {
        if let Some(sb) = &file.shebang {
            self.line(sb);
        }
        for attr in &file.attrs {
            self.emit_attr(attr, true);
        }
        for item in &file.items {
            self.emit_item(item);
        }
    }

    fn fmt_attr_inline(&self, attr: &Attribute, inner_target: Option<&str>) -> String {
        let prefix = match inner_target {
            Some(t) => format!("{}: ", t),
            None => String::new(),
        };
        // Rust 2024 unsafe attributes: `#[unsafe(no_mangle)]`, `#[unsafe(export_name = "f")]`
        if let syn::Meta::List(ml) = &attr.meta
            && ml.path.is_ident("unsafe")
            && let Ok(inner_meta) = syn::parse2::<syn::Meta>(ml.tokens.clone())
        {
            return format_meta_attr(&inner_meta, &prefix, "unsafe ");
        }
        format_meta_attr(&attr.meta, &prefix, "")
    }

    fn emit_attr(&mut self, attr: &Attribute, inner: bool) {
        let target = if inner { Some("module") } else { None };
        self.emit_attr_with_target(attr, target);
    }

    fn emit_attr_with_target(&mut self, attr: &Attribute, inner_target: Option<&str>) {
        if attr.path().is_ident("doc")
            && let syn::Meta::NameValue(nv) = &attr.meta
            && let Expr::Lit(el) = &nv.value
            && let syn::Lit::Str(ls) = &el.lit
        {
            let s = ls.value();
            let trimmed = s.strip_prefix(' ').unwrap_or(&s);
            let marker = if inner_target.is_some() { "//!" } else { "///" };
            self.line(&format!("{} {}", marker, trimmed));
            return;
        }
        let formatted = self.fmt_attr_inline(attr, inner_target);
        self.line(&formatted);
    }

    fn emit_split_attrs(&mut self, attrs: &[Attribute], emit_inner: bool, inner_target: &str) {
        for a in attrs {
            let is_inner = matches!(a.style, syn::AttrStyle::Inner(_));
            if is_inner == emit_inner {
                let target = if is_inner { Some(inner_target) } else { None };
                self.emit_attr_with_target(a, target);
            }
        }
    }

    fn emit_item(&mut self, item: &Item) {
        match item {
            Item::Use(u) => {
                for a in &u.attrs {
                    self.emit_attr(a, false);
                }
                let vis = fmt_vis(&u.vis);
                let g = if u.leading_colon.is_some() {
                    "global::"
                } else {
                    ""
                };
                self.line(&format!("{}using {}{};", vis, g, fmt_use_tree(&u.tree)));
            }
            Item::ExternCrate(ec) => {
                for a in &ec.attrs {
                    self.emit_attr(a, false);
                }
                let vis = fmt_vis(&ec.vis);
                let alias = ec
                    .rename
                    .as_ref()
                    .map(|(_, id)| format!(" as {}", escape_ident(&id.to_string())))
                    .unwrap_or_default();
                let crate_name = if ec.ident == "self" {
                    "this".to_string()
                } else {
                    escape_ident(&ec.ident.to_string())
                };
                self.line(&format!("{}extern crate {}{};", vis, crate_name, alias));
            }
            Item::Mod(m) => {
                self.emit_split_attrs(&m.attrs, false, "module");
                let vis = fmt_vis(&m.vis);
                let name = escape_ident(&m.ident.to_string());
                if let Some((_, items)) = &m.content {
                    self.line(&format!("{}namespace {} {{", vis, name));
                    self.indent += 1;
                    self.emit_split_attrs(&m.attrs, true, "module");
                    for sub in items {
                        self.emit_item(sub);
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
                            .map(|f| {
                                let mut p = String::new();
                                for a in &f.attrs {
                                    p.push_str(&self.fmt_attr_inline(a, None));
                                    p.push(' ');
                                }
                                p.push_str(&fmt_vis(&f.vis));
                                p.push_str(&self.fmt_type(&f.ty));
                                p
                            })
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
                        Some((_, expr)) => format!(" = {}", self.fmt_expr(expr)),
                        None => String::new(),
                    };
                    match &v.fields {
                        syn::Fields::Unit => self.line(&format!("{}{},", vname, disc)),
                        syn::Fields::Unnamed(uf) => {
                            let ts: Vec<String> = uf
                                .unnamed
                                .iter()
                                .map(|f| {
                                    let mut p = String::new();
                                    for a in &f.attrs {
                                        p.push_str(&self.fmt_attr_inline(a, None));
                                        p.push(' ');
                                    }
                                    p.push_str(&self.fmt_type(&f.ty));
                                    p
                                })
                                .collect();
                            self.line(&format!("{}({}){},", vname, ts.join(", "), disc));
                        }
                        syn::Fields::Named(nf) => {
                            self.line(&format!("{} {{", vname));
                            self.indent += 1;
                            for f in &nf.named {
                                for a in &f.attrs {
                                    self.emit_attr(a, false);
                                }
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
            Item::Trait(t) => {
                self.emit_split_attrs(&t.attrs, false, "inner");
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
                self.emit_split_attrs(&t.attrs, true, "inner");
                for ti in &t.items {
                    match ti {
                        TraitItem::Fn(tf) => {
                            self.emit_split_attrs(&tf.attrs, false, "inner");
                            let inner_attrs: Vec<Attribute> = tf
                                .attrs
                                .iter()
                                .filter(|a| matches!(a.style, syn::AttrStyle::Inner(_)))
                                .cloned()
                                .collect();
                            self.emit_fn_sig_and_body(
                                "",
                                &tf.sig,
                                tf.default.as_ref(),
                                &inner_attrs,
                                false,
                                false,
                            );
                        }
                        TraitItem::Type(tt) => {
                            for a in &tt.attrs {
                                self.emit_attr(a, false);
                            }
                            let aname = escape_ident(&tt.ident.to_string());
                            let agens = self.fmt_generics(&tt.generics);
                            let bs = if tt.bounds.is_empty() {
                                String::new()
                            } else {
                                let b: Vec<String> =
                                    tt.bounds.iter().map(|b| self.fmt_bound(b)).collect();
                                format!(" : {}", b.join(", "))
                            };
                            let awh = self.fmt_where(&tt.generics);
                            let def = tt
                                .default
                                .as_ref()
                                .map(|(_, ty)| format!(" = {}", self.fmt_type(ty)))
                                .unwrap_or_default();
                            self.line(&format!("type {}{}{}{}{};", aname, agens, bs, awh, def));
                        }
                        TraitItem::Const(tc) => {
                            for a in &tc.attrs {
                                self.emit_attr(a, false);
                            }
                            let cty = self.fmt_type(&tc.ty);
                            let cname = escape_ident(&tc.ident.to_string());
                            let def = match &tc.default {
                                Some((_, e)) => format!(" = {}", self.fmt_expr(e)),
                                None => String::new(),
                            };
                            self.line(&format!("const {} {}{};", cty, cname, def));
                        }
                        TraitItem::Macro(tm) => {
                            for a in &tm.attrs {
                                self.emit_attr(a, false);
                            }
                            let ms = fmt_syn_macro(&tm.mac);
                            if matches!(tm.mac.delimiter, syn::MacroDelimiter::Brace(_)) {
                                self.line(&ms);
                            } else {
                                self.line(&format!("{};", ms));
                            }
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
                self.emit_split_attrs(&im.attrs, false, "inner");
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
                self.emit_split_attrs(&im.attrs, true, "inner");
                for sub in &im.items {
                    match sub {
                        syn::ImplItem::Fn(m) => {
                            self.emit_split_attrs(&m.attrs, false, "inner");
                            let inner_attrs: Vec<Attribute> = m
                                .attrs
                                .iter()
                                .filter(|a| matches!(a.style, syn::AttrStyle::Inner(_)))
                                .cloned()
                                .collect();
                            let mvis = fmt_vis(&m.vis);
                            let is_static =
                                !m.sig.inputs.iter().any(|a| matches!(a, FnArg::Receiver(_)));
                            self.emit_fn_sig_and_body(
                                &mvis,
                                &m.sig,
                                Some(&m.block),
                                &inner_attrs,
                                is_static,
                                false,
                            );
                        }
                        syn::ImplItem::Type(t) => {
                            for a in &t.attrs {
                                self.emit_attr(a, false);
                            }
                            let aname = escape_ident(&t.ident.to_string());
                            let agens = self.fmt_generics(&t.generics);
                            let ty = self.fmt_type(&t.ty);
                            let awh = self.fmt_where(&t.generics);
                            self.line(&format!("type {}{} = {}{};", aname, agens, ty, awh));
                        }
                        syn::ImplItem::Const(c) => {
                            for a in &c.attrs {
                                self.emit_attr(a, false);
                            }
                            let cvis = fmt_vis(&c.vis);
                            let cty = self.fmt_type(&c.ty);
                            let cname = escape_ident(&c.ident.to_string());
                            let val = self.fmt_expr(&c.expr);
                            self.line(&format!("{}const {} {} = {};", cvis, cty, cname, val));
                        }
                        syn::ImplItem::Macro(mc) => {
                            for a in &mc.attrs {
                                self.emit_attr(a, false);
                            }
                            let ms = fmt_syn_macro(&mc.mac);
                            if matches!(mc.mac.delimiter, syn::MacroDelimiter::Brace(_)) {
                                self.line(&ms);
                            } else {
                                self.line(&format!("{};", ms));
                            }
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
                let wh = self.fmt_where(&t.generics);
                let ty = self.fmt_type(&t.ty);
                self.line(&format!("{}alias {}{}{} = {};", vis, name, gens, wh, ty));
            }
            Item::Const(c) => {
                for a in &c.attrs {
                    self.emit_attr(a, false);
                }
                let vis = fmt_vis(&c.vis);
                let ty = self.fmt_type(&c.ty);
                let name = escape_ident(&c.ident.to_string());
                let val = self.fmt_expr(&c.expr);
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
                let val = self.fmt_expr(&s.expr);
                self.line(&format!(
                    "{}static {}{} {} = {};",
                    vis, mut_kw, ty, name, val
                ));
            }
            Item::Fn(f) => {
                self.emit_split_attrs(&f.attrs, false, "inner");
                let inner_attrs: Vec<Attribute> = f
                    .attrs
                    .iter()
                    .filter(|a| matches!(a.style, syn::AttrStyle::Inner(_)))
                    .cloned()
                    .collect();
                let vis = fmt_vis(&f.vis);
                self.emit_fn_sig_and_body(&vis, &f.sig, Some(&f.block), &inner_attrs, false, false);
            }
            Item::ForeignMod(fm) => {
                self.emit_split_attrs(&fm.attrs, false, "inner");
                let unsf = if fm.unsafety.is_some() { "unsafe " } else { "" };
                let abi = fm
                    .abi
                    .name
                    .as_ref()
                    .map(|l| format!(" \"{}\"", l.value()))
                    .unwrap_or_default();
                self.line(&format!("{}extern{} {{", unsf, abi));
                self.indent += 1;
                self.emit_split_attrs(&fm.attrs, true, "inner");
                for fi in &fm.items {
                    match fi {
                        syn::ForeignItem::Fn(ff) => {
                            self.emit_foreign_fn(ff, false);
                        }
                        syn::ForeignItem::Static(fs) => {
                            self.emit_foreign_static(fs, "");
                        }
                        syn::ForeignItem::Macro(fm) => {
                            for a in &fm.attrs {
                                self.emit_attr(a, false);
                            }
                            let ms = fmt_syn_macro(&fm.mac);
                            if matches!(fm.mac.delimiter, syn::MacroDelimiter::Brace(_)) {
                                self.line(&ms);
                            } else {
                                self.line(&format!("{};", ms));
                            }
                        }
                        syn::ForeignItem::Verbatim(ts) => {
                            if !self.try_emit_safe_foreign_verbatim(ts) {
                                self.line(&format!("rust {{ {} }}", ts));
                            }
                        }
                        other => {
                            self.line(&format!("rust {{ {} }}", other.to_token_stream()));
                        }
                    }
                }
                self.indent -= 1;
                self.line("}");
            }
            Item::Macro(im) => {
                for a in &im.attrs {
                    self.emit_attr(a, false);
                }
                if im.mac.path.is_ident("macro_rules")
                    && let Some(id) = &im.ident
                {
                    let mname = escape_ident(&id.to_string());
                    match im.mac.delimiter {
                        syn::MacroDelimiter::Brace(_) => {
                            self.line(&format!("macro {} {{ {} }}", mname, im.mac.tokens));
                        }
                        syn::MacroDelimiter::Paren(_) => {
                            self.line(&format!("macro {} ( {} );", mname, im.mac.tokens));
                        }
                        syn::MacroDelimiter::Bracket(_) => {
                            self.line(&format!("macro {} [ {} ];", mname, im.mac.tokens));
                        }
                    }
                    return;
                }
                self.line(&format!("{};", fmt_syn_macro(&im.mac)));
            }
            other => {
                let pretty = pretty_item(other);
                self.line(&format!("rust {{ {} }}", pretty.trim()));
            }
        }
    }

    fn emit_foreign_fn(&mut self, ff: &syn::ForeignItemFn, is_safe: bool) {
        for a in &ff.attrs {
            self.emit_attr(a, false);
        }
        let fvis = fmt_vis(&ff.vis);
        self.emit_fn_sig_and_body(&fvis, &ff.sig, None, &[], false, is_safe);
    }

    fn emit_foreign_static(&mut self, fs: &syn::ForeignItemStatic, safety: &str) {
        for a in &fs.attrs {
            self.emit_attr(a, false);
        }
        let fvis = fmt_vis(&fs.vis);
        let mut_kw = if matches!(fs.mutability, syn::StaticMutability::Mut(_)) {
            "mut "
        } else {
            ""
        };
        let ty = self.fmt_type(&fs.ty);
        let name = escape_ident(&fs.ident.to_string());
        self.line(&format!(
            "{}{}static {}{} {};",
            fvis, safety, mut_kw, ty, name
        ));
    }

    /// Parses Rust 2024 `safe fn` and `safe`/`unsafe static` items inside `unsafe extern` blocks,
    /// which `syn` 2.0 represents as `ForeignItem::Verbatim`.
    fn try_emit_safe_foreign_verbatim(&mut self, tokens: &proc_macro2::TokenStream) -> bool {
        let tts: Vec<proc_macro2::TokenTree> = tokens.clone().into_iter().collect();
        for i in 0..tts.len().saturating_sub(1) {
            let proc_macro2::TokenTree::Ident(safety_id) = &tts[i] else {
                continue;
            };
            let proc_macro2::TokenTree::Ident(next_id) = &tts[i + 1] else {
                continue;
            };
            let safety_str = safety_id.to_string();
            let next_str = next_id.to_string();
            let stripped: proc_macro2::TokenStream = tts
                .iter()
                .enumerate()
                .filter(|(idx, _)| *idx != i)
                .map(|(_, tt)| tt.clone())
                .collect();
            if safety_str == "safe"
                && next_str == "fn"
                && let Ok(ff) = syn::parse2::<syn::ForeignItemFn>(stripped.clone())
            {
                self.emit_foreign_fn(&ff, true);
                return true;
            }
            if (safety_str == "safe" || safety_str == "unsafe")
                && next_str == "static"
                && let Ok(fs) = syn::parse2::<syn::ForeignItemStatic>(stripped)
            {
                let safety = if safety_str == "safe" {
                    "safe "
                } else {
                    "unsafe "
                };
                self.emit_foreign_static(&fs, safety);
                return true;
            }
        }
        false
    }

    fn emit_fn_sig_and_body(
        &mut self,
        vis: &str,
        sig: &syn::Signature,
        body: Option<&Block>,
        inner_attrs: &[Attribute],
        is_static: bool,
        is_safe: bool,
    ) {
        let mut header = String::new();
        header.push_str(vis);
        if sig.constness.is_some() {
            header.push_str("const ");
        }
        let is_async = sig.asyncness.is_some();
        if is_async {
            header.push_str("async ");
        }
        if is_safe {
            header.push_str("safe ");
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
                    let mut pfx = String::new();
                    for a in &r.attrs {
                        pfx.push_str(&self.fmt_attr_inline(a, None));
                        pfx.push(' ');
                    }
                    if let Some((_, lt)) = &r.reference {
                        let life = lt
                            .as_ref()
                            .map(|l| format!("<{}>", escape_lifetime(l)))
                            .unwrap_or_default();
                        if r.mutability.is_some() {
                            params.push(format!("{}write{} this", pfx, life));
                        } else {
                            params.push(format!("{}read{} this", pfx, life));
                        }
                    } else if r.colon_token.is_some() {
                        let mk = if r.mutability.is_some() { "mut " } else { "" };
                        params.push(format!("{}{}take {} this", pfx, mk, self.fmt_type(&r.ty)));
                    } else if r.mutability.is_some() {
                        params.push(format!("{}mut take this", pfx));
                    } else {
                        params.push(format!("{}take this", pfx));
                    }
                }
                FnArg::Typed(pt) => {
                    params.push(self.fmt_typed_fn_param(&pt.attrs, &pt.pat, &pt.ty));
                }
            }
        }
        if let Some(vp) = &sig.variadic {
            let mut pfx = String::new();
            for a in &vp.attrs {
                pfx.push_str(&self.fmt_attr_inline(a, None));
                pfx.push(' ');
            }
            if let Some((pat, _)) = &vp.pat {
                let (is_mut, vname) = extract_param_pat(pat);
                self.bind_var(&vname, "core.ffi.VaList".to_string());
                let mk = if is_mut { "mut " } else { "" };
                params.push(format!("{}{}... {}", pfx, mk, escape_ident(&vname)));
            } else {
                params.push(format!("{}...", pfx));
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

        let tail = if matches!(sig.output, ReturnType::Default) {
            BlockTail::None
        } else {
            BlockTail::Return
        };

        if let Some(blk) = body {
            self.line(&format!("{} {{", header));
            self.indent += 1;
            for a in inner_attrs {
                self.emit_attr_with_target(a, Some("inner"));
            }
            self.emit_block_stmts(blk, tail);
            self.indent -= 1;
            self.line("}");
        } else {
            self.line(&format!("{};", header));
        }

        self.declared_in_fn = prev_declared;
        self.pop_scope();
    }

    fn fmt_typed_fn_param(&mut self, attrs: &[Attribute], pat: &Pat, ty: &Type) -> String {
        let mut pfx = String::new();
        for a in attrs {
            pfx.push_str(&self.fmt_attr_inline(a, None));
            pfx.push(' ');
        }
        let ty_s = self.fmt_type(ty);
        match pat {
            Pat::Ident(pi) if pi.by_ref.is_some() => {
                let pname = pi.ident.to_string();
                let mode = if pi.mutability.is_some() {
                    "write"
                } else {
                    "read"
                };
                self.bind_var(&pname, format!("{} {}", mode, ty_s));
                format!("{}borrow {} {} {}", pfx, mode, ty_s, escape_ident(&pname))
            }
            Pat::Ident(pi) => {
                let pname = pi.ident.to_string();
                self.bind_var(&pname, ty_s.clone());
                let mk = if pi.mutability.is_some() { "mut " } else { "" };
                format!("{}{}{} {}", pfx, mk, ty_s, escape_ident(&pname))
            }
            Pat::Wild(_) => format!("{}{} _", pfx, ty_s),
            other => {
                let (pat_s, self_typed) = self.fmt_typed_pat(other, ty);
                if self_typed {
                    format!("{}{}", pfx, pat_s)
                } else {
                    format!("{}{} : {}", pfx, pat_s, ty_s)
                }
            }
        }
    }

    fn fmt_typed_pat(&mut self, pat: &Pat, ty: &Type) -> (String, bool) {
        match (pat, ty) {
            (Pat::Tuple(pt), Type::Tuple(tt)) if pt.elems.len() == tt.elems.len() => {
                let mut parts = Vec::new();
                let mut all_typed = true;
                for (p, t) in pt.elems.iter().zip(tt.elems.iter()) {
                    let (ps, ok) = self.fmt_typed_pat(p, t);
                    all_typed &= ok;
                    parts.push(ps);
                }
                let s = if parts.len() == 1 {
                    format!("({},)", parts[0])
                } else {
                    format!("({})", parts.join(", "))
                };
                (s, all_typed)
            }
            (Pat::Reference(pr), Type::Reference(tr)) => {
                let (inner, ok) = self.fmt_typed_pat(&pr.pat, &tr.elem);
                let kw = if pr.mutability.is_some() {
                    "deref write "
                } else {
                    "deref "
                };
                (format!("{}{}", kw, inner), ok)
            }
            (Pat::TupleStruct(pts), _) => {
                let struct_key = pts
                    .path
                    .segments
                    .last()
                    .map(|s| s.ident.to_string())
                    .unwrap_or_default();
                let known_fields = self.struct_fields.get(&struct_key).cloned();
                let fs: Vec<String> = pts
                    .elems
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        let fty = known_fields
                            .as_ref()
                            .and_then(|m| m.get(&i.to_string()))
                            .map(|s| s.as_str());
                        self.fmt_pat(p, fty)
                    })
                    .collect();
                let path_s = self.fmt_pat_qself_or_path(&pts.qself, &pts.path);
                let self_typed =
                    pts.qself.is_none() && pts.path.segments.len() == 1 && known_fields.is_some();
                (format!("{}({})", path_s, fs.join(", ")), self_typed)
            }
            (Pat::Ident(pi), _) => {
                let ty_s = self.fmt_type(ty);
                let pname = pi.ident.to_string();
                if pi.by_ref.is_some() {
                    let mode = if pi.mutability.is_some() {
                        "write"
                    } else {
                        "read"
                    };
                    self.bind_var(&pname, format!("{} {}", mode, ty_s));
                    return (
                        format!("borrow {} {} {}", mode, ty_s, escape_ident(&pname)),
                        true,
                    );
                }
                let mk = if pi.mutability.is_some() { "mut " } else { "" };
                self.bind_var(&pname, ty_s.clone());
                (format!("{}{} {}", mk, ty_s, escape_ident(&pname)), true)
            }
            (Pat::Paren(pp), _) => {
                let (inner, ok) = self.fmt_typed_pat(&pp.pat, ty);
                (format!("({})", inner), ok)
            }
            _ => (self.fmt_pat(pat, None), false),
        }
    }

    fn emit_block_stmts(&mut self, block: &Block, tail: BlockTail<'_>) {
        self.push_scope();
        let len = block.stmts.len();
        for (idx, stmt) in block.stmts.iter().enumerate() {
            let is_last = idx + 1 == len;
            match stmt {
                Stmt::Local(local) => self.emit_local(local),
                Stmt::Item(item) => self.emit_item(item),
                Stmt::Expr(expr, semi) => {
                    if is_last && semi.is_none() {
                        self.emit_tail(expr, tail);
                    } else {
                        self.emit_expr_stmt(expr);
                    }
                }
                Stmt::Macro(sm) => {
                    for a in &sm.attrs {
                        self.emit_attr(a, false);
                    }
                    let ms = fmt_syn_macro(&sm.mac);
                    if matches!(sm.mac.delimiter, syn::MacroDelimiter::Brace(_))
                        && sm.semi_token.is_none()
                    {
                        self.line(&ms);
                    } else {
                        self.line(&format!("{};", ms));
                    }
                }
            }
        }
        self.pop_scope();
    }

    fn emit_expr_as_block_body(&mut self, expr: &Expr, tail: BlockTail<'_>) {
        if let Expr::Block(eb) = expr {
            self.emit_block_stmts(&eb.block, tail);
        } else {
            self.emit_tail(expr, tail);
        }
    }

    fn emit_tail(&mut self, expr: &Expr, tail: BlockTail<'_>) {
        match tail {
            BlockTail::None => self.emit_expr_stmt(expr),
            BlockTail::Return => match expr {
                Expr::Return(_) => self.emit_expr_stmt(expr),
                Expr::If(eif) => self.emit_if(eif, false, BlockTail::Return),
                Expr::Match(em) => self.emit_match_stmt(em, true),
                other => {
                    let s = self.fmt_expr(other);
                    self.line(&format!("return {};", s));
                }
            },
            BlockTail::Break(label) => {
                let s = self.fmt_expr(expr);
                self.line(&format!("break {} {};", label, s));
            }
        }
    }

    fn emit_local(&mut self, local: &Local) {
        for a in &local.attrs {
            self.emit_attr(a, false);
        }

        // `let Pat = expr else { ... };` -> `require (expr is Pat) else { ... }`
        if let Some(init) = &local.init
            && let Some((_, diverge)) = &init.diverge
        {
            let expr_s = self.fmt_expr(&init.expr);
            let pat_s = self.fmt_pat(&local.pat, None);
            self.line(&format!("require ({} is {}) else {{", expr_s, pat_s));
            self.indent += 1;
            self.emit_expr_as_block_body(diverge, BlockTail::None);
            self.indent -= 1;
            self.line("}");
            return;
        }

        if matches!(local.pat, Pat::Wild(_))
            && let Some(init) = &local.init
        {
            let e = self.fmt_expr(&init.expr);
            self.line(&format!("_ = {};", e));
            return;
        }

        let (inner_pat, explicit_ty) = match &local.pat {
            Pat::Type(pt) => (pt.pat.as_ref(), Some(pt.ty.as_ref())),
            other => (other, None),
        };

        // Struct / Slice destructuring `let Point { x, y } = pt;` / `let [a, b] = arr;`
        if let Pat::Slice(ps) = inner_pat {
            let elem_ty = match explicit_ty {
                Some(Type::Array(arr)) => Some(self.fmt_type(&arr.elem)),
                Some(Type::Slice(sl)) => Some(self.fmt_type(&sl.elem)),
                _ => local.init.as_ref().and_then(|init| {
                    self.infer_expr_type(&init.expr)
                        .and_then(|t| extract_collection_elem(&t))
                }),
            };
            let elems: Vec<_> = ps
                .elems
                .iter()
                .map(|p| self.fmt_pat(p, elem_ty.as_deref()))
                .collect();
            let Some(init) = &local.init else {
                self.record_err(RSharpError::from_rust("Destructuring without initializer"));
                return;
            };
            let rhs = self.fmt_expr(&init.expr);
            self.line(&format!("[{}] = {};", elems.join(", "), rhs));
            return;
        }
        if let Pat::Struct(ps) = inner_pat {
            let struct_key = ps
                .path
                .segments
                .last()
                .map(|s| s.ident.to_string())
                .unwrap_or_default();
            let known_fields = self.struct_fields.get(&struct_key).cloned();
            let mut fs = Vec::new();
            for f in &ps.fields {
                let mut apfx = String::new();
                for a in &f.attrs {
                    apfx.push_str(&self.fmt_attr_inline(a, None));
                    apfx.push(' ');
                }
                let raw_fname = match &f.member {
                    Member::Named(id) => id.to_string(),
                    Member::Unnamed(idx) => idx.index.to_string(),
                };
                let fname = match &f.member {
                    Member::Named(id) => escape_ident(&id.to_string()),
                    Member::Unnamed(idx) => idx.index.to_string(),
                };
                let fty = known_fields
                    .as_ref()
                    .and_then(|m| m.get(&raw_fname))
                    .map(|s| s.as_str());
                if f.colon_token.is_some() {
                    let sub_s = self.fmt_pat(&f.pat, fty);
                    fs.push(format!("{}{}: {}", apfx, fname, sub_s));
                } else if let Some(ty_s) = fty {
                    if let Pat::Ident(pi) = f.pat.as_ref()
                        && pi.by_ref.is_some()
                    {
                        let mode = if pi.mutability.is_some() {
                            "write"
                        } else {
                            "read"
                        };
                        self.bind_var(&raw_fname, format!("{} {}", mode, ty_s));
                        fs.push(format!("{}borrow {} {} {}", apfx, mode, ty_s, fname));
                    } else {
                        let (is_mut, _) = extract_param_pat(&f.pat);
                        let mk = if is_mut { "mut " } else { "" };
                        self.bind_var(&raw_fname, ty_s.to_string());
                        fs.push(format!("{}{}{} {}", apfx, mk, ty_s, fname));
                    }
                } else {
                    fs.push(format!("{}{}", apfx, fname));
                }
            }
            if ps.rest.is_some() {
                fs.push("..".to_string());
            }
            let Some(init) = &local.init else {
                self.record_err(RSharpError::from_rust("Destructuring without initializer"));
                return;
            };
            let rhs = self.fmt_expr(&init.expr);
            let path_s = self.fmt_pat_qself_or_path(&ps.qself, &ps.path);
            self.line(&format!("{} {{ {} }} = {};", path_s, fs.join(", "), rhs));
            return;
        }

        // Tuple-struct destructuring `let W(v) = w;`
        if let Pat::TupleStruct(pts) = inner_pat {
            let struct_key = pts
                .path
                .segments
                .last()
                .map(|s| s.ident.to_string())
                .unwrap_or_default();
            let known_fields = self.struct_fields.get(&struct_key).cloned();
            let fs: Vec<String> = pts
                .elems
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let fty = known_fields
                        .as_ref()
                        .and_then(|m| m.get(&i.to_string()))
                        .map(|s| s.as_str());
                    self.fmt_pat(p, fty)
                })
                .collect();
            let Some(init) = &local.init else {
                self.record_err(RSharpError::from_rust("Destructuring without initializer"));
                return;
            };
            let rhs = self.fmt_expr(&init.expr);
            let path_s = self.fmt_pat_qself_or_path(&pts.qself, &pts.path);
            self.line(&format!("{}({}) = {};", path_s, fs.join(", "), rhs));
            return;
        }

        // Reference-stripping local `let &v = r;` / `let &mut v = r;`
        if let Pat::Reference(pr) = inner_pat {
            let Some(init) = &local.init else {
                self.record_err(RSharpError::from_rust("Destructuring without initializer"));
                return;
            };
            let inner_ty = match explicit_ty {
                Some(Type::Reference(tr)) => Some(self.fmt_type(&tr.elem)),
                _ => self.infer_expr_type(&init.expr).and_then(|t| {
                    t.strip_prefix("read ")
                        .or_else(|| t.strip_prefix("write "))
                        .map(|s| s.to_string())
                }),
            };
            let inner_pat_s = self.fmt_pat(&pr.pat, inner_ty.as_deref());
            let kw = if pr.mutability.is_some() {
                "deref write "
            } else {
                "deref "
            };
            let rhs = self.fmt_expr(&init.expr);
            self.line(&format!("{}{} = {};", kw, inner_pat_s, rhs));
            return;
        }

        // Tuple destructuring `let (a, b): (i32, i32) = pair;`
        if let Pat::Tuple(ptup) = inner_pat {
            let elem_tys: Vec<String> = match explicit_ty {
                Some(Type::Tuple(tt)) => tt.elems.iter().map(|t| self.fmt_type(t)).collect(),
                _ => {
                    let Some(init) = &local.init else {
                        self.record_err(RSharpError::unresolved_type(
                            "tuple",
                            "Tuple binding requires initializer or type annotation",
                        ));
                        return;
                    };
                    if let Expr::Tuple(et) = init.expr.as_ref() {
                        let Some(elems) = et
                            .elems
                            .iter()
                            .map(|e| self.infer_expr_type(e))
                            .collect::<Option<Vec<_>>>()
                        else {
                            self.record_err(RSharpError::unresolved_type(
                                "tuple",
                                "Could not infer tuple element types",
                            ));
                            return;
                        };
                        elems
                    } else if let Some(inferred) = self.infer_expr_type(&init.expr)
                        && let Some(elems) = split_tuple_type_str(&inferred)
                    {
                        elems
                    } else {
                        self.record_err(RSharpError::unresolved_type(
                            "tuple",
                            "Add explicit type annotation for tuple destructuring",
                        ));
                        return;
                    }
                }
            };
            let rest_pos = ptup.elems.iter().position(|p| matches!(p, Pat::Rest(_)));
            let mut parts = Vec::new();
            for (idx, pat) in ptup.elems.iter().enumerate() {
                if matches!(pat, Pat::Rest(_)) {
                    parts.push("..".to_string());
                    continue;
                }
                let ty_idx = if let Some(rp) = rest_pos {
                    if idx < rp {
                        idx
                    } else {
                        let from_end = ptup.elems.len() - idx;
                        elem_tys.len().saturating_sub(from_end)
                    }
                } else {
                    idx
                };
                let ty_hint = elem_tys.get(ty_idx).map(|s| s.as_str());
                parts.push(self.fmt_pat(pat, ty_hint));
            }
            let Some(init) = &local.init else {
                self.record_err(RSharpError::from_rust(
                    "Tuple destructuring without initializer",
                ));
                return;
            };
            let rhs = self.fmt_expr(&init.expr);
            let tuple_s = if parts.len() == 1 {
                format!("({},)", parts[0])
            } else {
                format!("({})", parts.join(", "))
            };
            self.line(&format!("{} = {};", tuple_s, rhs));
            return;
        }

        // Standard variable binding `let (ref) (mut) x(: T)? (= init)?;` or `let (x) = init;`
        let (is_paren, unwrapped_pat) = match inner_pat {
            Pat::Paren(pp) => (true, pp.pat.as_ref()),
            other => (false, other),
        };
        let by_ref = matches!(unwrapped_pat, Pat::Ident(pi) if pi.by_ref.is_some());
        let (is_mut, name) = extract_param_pat(unwrapped_pat);
        let ty_s = if let Some(ty) = explicit_ty {
            self.fmt_type(ty)
        } else if let Some(init) = &local.init {
            if let Some(inferred) = self.infer_expr_type(&init.expr) {
                inferred
            } else if self.options.use_rustc_inference {
                match self.probe_type_via_rustc(&name) {
                    Ok(t) => t,
                    Err(e) => {
                        self.record_err(e);
                        return;
                    }
                }
            } else {
                self.record_err(RSharpError::unresolved_type(
                    &name,
                    "Local inference could not determine type and rustc probing is disabled",
                ));
                return;
            }
        } else {
            self.record_err(RSharpError::unresolved_type(
                &name,
                "Unannotated variable declaration has no initializer",
            ));
            return;
        };

        let init_s = match &local.init {
            Some(init) => format!(" = {}", self.fmt_expr(&init.expr)),
            None => String::new(),
        };
        if by_ref {
            let mode = if is_mut { "write" } else { "read" };
            self.bind_var(&name, format!("{} {}", mode, ty_s));
            self.line(&format!(
                "borrow {} {} {}{};",
                mode,
                ty_s,
                escape_ident(&name),
                init_s
            ));
            return;
        }
        let is_shadow = self.declared_in_fn.contains(&name);
        let sh_kw = if is_shadow { "shadow " } else { "" };
        let mut_kw = if is_mut { "mut " } else { "" };
        self.bind_var(&name, ty_s.clone());
        if is_paren {
            self.line(&format!(
                "({}{} {}){};",
                mut_kw,
                ty_s,
                escape_ident(&name),
                init_s
            ));
        } else {
            self.line(&format!(
                "{}{}{} {}{};",
                sh_kw,
                mut_kw,
                ty_s,
                escape_ident(&name),
                init_s
            ));
        }
    }

    fn next_probe_id(&self) -> usize {
        let next = self.probe_counter.get() + 1;
        self.probe_counter.set(next);
        next
    }

    fn probe_type_via_rustc(&self, var_name: &str) -> Result<String> {
        let rust_ty =
            rustc_runner::probe_local_type(&self.raw_source, var_name, self.next_probe_id())?;
        let parsed_ty: Type = syn::parse_str(&rust_ty).map_err(|_| {
            RSharpError::unresolved_type(
                var_name,
                format!("rustc reported unparseable type '{}'", rust_ty),
            )
        })?;
        Ok(strip_non_static_lifetimes(&self.fmt_type(&parsed_ty)))
    }

    fn probe_closure_param_via_rustc(&self, param_name: &str) -> Result<String> {
        let rust_ty = rustc_runner::probe_closure_param_type(
            &self.raw_source,
            param_name,
            self.next_probe_id(),
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

    fn emit_expr_stmt(&mut self, expr: &Expr) {
        match expr {
            Expr::If(eif) => self.emit_if(eif, false, BlockTail::None),
            Expr::While(ew) => {
                let lbl = ew
                    .label
                    .as_ref()
                    .map(|l| format!("{}: ", escape_lifetime(&l.name)))
                    .unwrap_or_default();
                let cond = self.fmt_condition(&ew.cond);
                self.line(&format!("{}while ({}) {{", lbl, cond));
                self.indent += 1;
                self.emit_block_stmts(&ew.body, BlockTail::None);
                self.indent -= 1;
                self.line("}");
            }
            Expr::ForLoop(ef) => {
                let lbl = ef
                    .label
                    .as_ref()
                    .map(|l| format!("{}: ", escape_lifetime(&l.name)))
                    .unwrap_or_default();
                let elem_hint = self.infer_iter_elem_type(&ef.expr);
                let pat = self.fmt_pat(&ef.pat, elem_hint.as_deref());
                let iter = self.fmt_expr(&ef.expr);
                self.line(&format!("{}foreach ({} in {}) {{", lbl, pat, iter));
                self.indent += 1;
                self.emit_block_stmts(&ef.body, BlockTail::None);
                self.indent -= 1;
                self.line("}");
            }
            Expr::Loop(el) => {
                let lbl = el
                    .label
                    .as_ref()
                    .map(|l| format!("{}: ", escape_lifetime(&l.name)))
                    .unwrap_or_default();
                self.line(&format!("{}loop {{", lbl));
                self.indent += 1;
                self.emit_block_stmts(&el.body, BlockTail::None);
                self.indent -= 1;
                self.line("}");
            }
            Expr::Match(em) => self.emit_match_stmt(em, false),
            Expr::Block(eb) if eb.label.is_none() => {
                for a in &eb.attrs {
                    self.emit_attr(a, false);
                }
                self.line("{");
                self.indent += 1;
                self.emit_block_stmts(&eb.block, BlockTail::None);
                self.indent -= 1;
                self.line("}");
            }
            Expr::Unsafe(eu) => {
                self.line("unsafe {");
                self.indent += 1;
                self.emit_block_stmts(&eu.block, BlockTail::None);
                self.indent -= 1;
                self.line("}");
            }
            Expr::Return(er) => {
                if let Some(e) = &er.expr {
                    let s = self.fmt_expr(e);
                    self.line(&format!("return {};", s));
                } else {
                    self.line("return;");
                }
            }
            Expr::Break(eb) => {
                let mut line = String::from("break");
                if let Some(l) = &eb.label {
                    line.push_str(&format!(" {}", escape_lifetime(l)));
                }
                if let Some(e) = &eb.expr {
                    line.push(' ');
                    line.push_str(&self.fmt_expr(e));
                }
                line.push(';');
                self.line(&line);
            }
            Expr::Continue(ec) => {
                if let Some(l) = &ec.label {
                    self.line(&format!("continue {};", escape_lifetime(l)));
                } else {
                    self.line("continue;");
                }
            }
            other => {
                let s = self.fmt_expr(other);
                self.line(&format!("{};", s));
            }
        }
    }

    fn emit_if(&mut self, eif: &syn::ExprIf, is_else_if: bool, tail: BlockTail<'_>) {
        let kw = if is_else_if { "else if" } else { "if" };
        let cond = self.fmt_condition(&eif.cond);
        self.line(&format!("{} ({}) {{", kw, cond));
        self.indent += 1;
        self.emit_block_stmts(&eif.then_branch, tail);
        self.indent -= 1;
        if let Some((_, else_expr)) = &eif.else_branch {
            if let Expr::If(next) = else_expr.as_ref() {
                self.line("}");
                self.emit_if(next, true, tail);
            } else {
                self.line("} else {");
                self.indent += 1;
                self.emit_expr_as_block_body(else_expr, tail);
                self.indent -= 1;
                self.line("}");
            }
        } else {
            self.line("}");
        }
    }

    fn emit_match_stmt(&mut self, em: &syn::ExprMatch, tail_returns: bool) {
        let scr = self.fmt_expr(&em.expr);
        self.line(&format!("match ({}) {{", scr));
        self.indent += 1;
        let block_tail = if tail_returns {
            BlockTail::Return
        } else {
            BlockTail::None
        };
        for arm in &em.arms {
            for a in &arm.attrs {
                self.emit_attr(a, false);
            }
            let pat = self.fmt_pat(&arm.pat, None);
            let guard = match &arm.guard {
                Some((_, ge)) => format!(" when {}", self.fmt_condition(ge)),
                None => String::new(),
            };
            if tail_returns || matches!(arm.body.as_ref(), Expr::Block(_)) {
                self.line(&format!("{}{} => {{", pat, guard));
                self.indent += 1;
                self.emit_expr_as_block_body(&arm.body, block_tail);
                self.indent -= 1;
                self.line("}");
            } else {
                let s = self.fmt_expr(&arm.body);
                self.line(&format!("{}{} => {},", pat, guard, s));
            }
        }
        self.indent -= 1;
        self.line("}");
    }

    fn fmt_condition(&mut self, expr: &Expr) -> String {
        match expr {
            Expr::Let(el) => {
                let rhs = self.fmt_expr(&el.expr);
                let pat = self.fmt_pat(&el.pat, None);
                format!("{} is {}", rhs, pat)
            }
            Expr::Binary(eb) if matches!(eb.op, BinOp::And(_)) => {
                let l = self.fmt_condition(&eb.left);
                let r = self.fmt_condition(&eb.right);
                format!("{} && {}", l, r)
            }
            other => self.fmt_expr(other),
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
                if es.len() == 1 {
                    format!("({},)", es[0])
                } else {
                    format!("({})", es.join(", "))
                }
            }
            Type::Never(_) => "never".to_string(),
            Type::Infer(_) => "_".to_string(),
            Type::Macro(tm) => fmt_syn_macro(&tm.mac),
            Type::Paren(p) => format!("({})", self.fmt_type(&p.elem)),
            Type::Reference(r) => {
                let kw = if r.mutability.is_some() {
                    "write"
                } else {
                    "read"
                };
                let life = r
                    .lifetime
                    .as_ref()
                    .map(|l| format!("<{}>", escape_lifetime(l)))
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
                let mut dims = vec![self.fmt_const_expr(&arr.len)];
                let mut cur = arr.elem.as_ref();
                while let Type::Array(inner) = cur {
                    dims.push(self.fmt_const_expr(&inner.len));
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
                let mut prefix = String::new();
                if let Some(lts) = &bf.lifetimes {
                    prefix.push_str(&self.fmt_bound_lifetimes(lts));
                }
                if bf.unsafety.is_some() {
                    prefix.push_str("unsafe ");
                }
                if let Some(abi) = &bf.abi {
                    let name = abi
                        .name
                        .as_ref()
                        .map(|s| s.value())
                        .unwrap_or_else(|| "C".to_string());
                    prefix.push_str(&format!("extern \"{}\" ", name));
                }
                let mut ps: Vec<String> = bf
                    .inputs
                    .iter()
                    .map(|i| {
                        let mut apfx = String::new();
                        for a in &i.attrs {
                            apfx.push_str(&self.fmt_attr_inline(a, None));
                            apfx.push(' ');
                        }
                        let ty_s = self.fmt_type(&i.ty);
                        if let Some((id, _)) = &i.name {
                            format!("{}{} {}", apfx, ty_s, escape_ident(&id.to_string()))
                        } else {
                            format!("{}{}", apfx, ty_s)
                        }
                    })
                    .collect();
                if let Some(vp) = &bf.variadic {
                    let mut vpfx = String::new();
                    for a in &vp.attrs {
                        vpfx.push_str(&self.fmt_attr_inline(a, None));
                        vpfx.push(' ');
                    }
                    ps.push(format!("{}...", vpfx));
                }
                let ret = self.fmt_return_type(&bf.output, false);
                format!("{}({}) -> {}", prefix, ps.join(", "), ret)
            }
            Type::Path(tp) => {
                if let Some(qself) = &tp.qself {
                    return self.fmt_qself_path(qself, &tp.path);
                }
                if is_boxed_any(&tp.path) {
                    return "dynamic".to_string();
                }
                if let Some(inner) = extract_option_inner(&tp.path) {
                    let inner_s = self.fmt_type(inner);
                    if !inner_s.starts_with("read ")
                        && !inner_s.starts_with("write ")
                        && !inner_s.starts_with("dyn ")
                        && !inner_s.starts_with("impl ")
                        && !inner_s.starts_with("unsafe ")
                        && !inner_s.starts_with("extern ")
                        && !inner_s.starts_with("forall<")
                        && !inner_s.contains(" -> ")
                    {
                        return format!("{}?", inner_s);
                    }
                }
                self.fmt_path(&tp.path)
            }
            other => other.to_token_stream().to_string(),
        }
    }

    fn fmt_const_expr(&self, expr: &Expr) -> String {
        let mut sub = self.child();
        if let Expr::Block(eb) = expr {
            if let Some(single) = single_tail_expr(&eb.block) {
                return format!("{{ {} }}", sub.fmt_expr(single));
            }
            sub.emit_block_stmts(&eb.block, BlockTail::Return);
            return format!("{{\n{}}}", sub.out);
        }
        sub.fmt_expr(expr)
    }

    fn fmt_pat_qself_or_path(&self, qself: &Option<syn::QSelf>, path: &Path) -> String {
        match qself {
            Some(qs) => self.fmt_qself_path(qs, path),
            None => self.fmt_path(path),
        }
    }

    fn fmt_qself_path(&self, qself: &syn::QSelf, path: &Path) -> String {
        let self_ty = self.fmt_type(&qself.ty);
        let rest_segs: Vec<_> = path.segments.iter().skip(qself.position).cloned().collect();
        let rest_path = Path {
            leading_colon: None,
            segments: syn::punctuated::Punctuated::from_iter(rest_segs),
        };
        let rest_s = self.fmt_path(&rest_path);
        if qself.position == 0 {
            format!("({})::{}", self_ty, rest_s.replace('.', "::"))
        } else {
            let trait_segs: Vec<_> = path.segments.iter().take(qself.position).cloned().collect();
            let trait_path = Path {
                leading_colon: None,
                segments: syn::punctuated::Punctuated::from_iter(trait_segs),
            };
            let rest_joined = if rest_path.segments.len() > 1 {
                rest_s.replace('.', "::")
            } else {
                rest_s
            };
            format!(
                "({} as {}).{}",
                self_ty,
                self.fmt_path(&trait_path),
                rest_joined
            )
        }
    }

    fn fmt_path(&self, path: &Path) -> String {
        if path.leading_colon.is_none()
            && path.segments.len() == 1
            && path.segments[0].arguments.is_empty()
        {
            let name = path.segments[0].ident.to_string();
            if let Some(mapped) = map_rust_primitive(&name) {
                return mapped.to_string();
            }
        }
        let mut out = Vec::new();
        for (idx, seg) in path.segments.iter().enumerate() {
            let raw_name = seg.ident.to_string();
            let name = if idx == 0 && raw_name == "self" {
                "this".to_string()
            } else {
                escape_ident(&map_rust_std_alias(&raw_name))
            };
            match &seg.arguments {
                PathArguments::None => out.push(name),
                PathArguments::AngleBracketed(ab) => {
                    let args: Vec<String> =
                        ab.args.iter().map(|a| self.fmt_generic_arg(a)).collect();
                    out.push(format!("{}<{}>", name, args.join(", ")));
                }
                PathArguments::Parenthesized(p) => {
                    let ins: Vec<String> = p.inputs.iter().map(|t| self.fmt_type(t)).collect();
                    let ret = self.fmt_return_type(&p.output, false);
                    out.push(format!("{}({}) -> {}", name, ins.join(", "), ret));
                }
            }
        }
        let joined = out.join(".");
        if path.leading_colon.is_some() {
            format!("global::{}", joined)
        } else {
            joined
        }
    }

    fn fmt_generic_arg(&self, a: &GenericArgument) -> String {
        match a {
            GenericArgument::Lifetime(l) => format!("life {}", escape_lifetime(l)),
            GenericArgument::Type(t) => self.fmt_type(t),
            GenericArgument::AssocType(at) => {
                let gs = match &at.generics {
                    Some(ab) => {
                        let inner: Vec<String> =
                            ab.args.iter().map(|ga| self.fmt_generic_arg(ga)).collect();
                        format!("<{}>", inner.join(", "))
                    }
                    None => String::new(),
                };
                format!(
                    "{}{} = {}",
                    escape_ident(&at.ident.to_string()),
                    gs,
                    self.fmt_type(&at.ty)
                )
            }
            GenericArgument::Constraint(c) => {
                let gs = match &c.generics {
                    Some(ab) => {
                        let inner: Vec<String> =
                            ab.args.iter().map(|ga| self.fmt_generic_arg(ga)).collect();
                        format!("<{}>", inner.join(", "))
                    }
                    None => String::new(),
                };
                let bs: Vec<String> = c.bounds.iter().map(|b| self.fmt_bound(b)).collect();
                format!(
                    "{}{} : {}",
                    escape_ident(&c.ident.to_string()),
                    gs,
                    bs.join(" + ")
                )
            }
            GenericArgument::Const(e) => self.fmt_const_expr(e),
            other => other.to_token_stream().to_string(),
        }
    }

    fn fmt_generic_param(&self, p: &GenericParam) -> String {
        match p {
            GenericParam::Lifetime(lt) => {
                let mut s = String::new();
                for a in &lt.attrs {
                    s.push_str(&self.fmt_attr_inline(a, None));
                    s.push(' ');
                }
                s.push_str(&format!("life {}", escape_lifetime(&lt.lifetime)));
                if !lt.bounds.is_empty() {
                    let bs: Vec<String> = lt.bounds.iter().map(escape_lifetime).collect();
                    s.push_str(&format!(" : {}", bs.join(" + ")));
                }
                s
            }
            GenericParam::Type(tp) => {
                let mut s = String::new();
                for a in &tp.attrs {
                    s.push_str(&self.fmt_attr_inline(a, None));
                    s.push(' ');
                }
                s.push_str(&escape_ident(&tp.ident.to_string()));
                if !tp.bounds.is_empty() {
                    let bs: Vec<String> = tp.bounds.iter().map(|b| self.fmt_bound(b)).collect();
                    s.push_str(&format!(" : {}", bs.join(" + ")));
                }
                if let Some(def) = &tp.default {
                    s.push_str(&format!(" = {}", self.fmt_type(def)));
                }
                s
            }
            GenericParam::Const(cp) => {
                let mut s = String::new();
                for a in &cp.attrs {
                    s.push_str(&self.fmt_attr_inline(a, None));
                    s.push(' ');
                }
                s.push_str(&format!(
                    "const {} {}",
                    self.fmt_type(&cp.ty),
                    escape_ident(&cp.ident.to_string())
                ));
                if let Some(def) = &cp.default {
                    s.push_str(&format!(" = {}", self.fmt_const_expr(def)));
                }
                s
            }
        }
    }

    fn fmt_bound_lifetimes(&self, bl: &syn::BoundLifetimes) -> String {
        if bl.lifetimes.is_empty() {
            return String::new();
        }
        let ls: Vec<String> = bl
            .lifetimes
            .iter()
            .map(|gp| self.fmt_generic_param(gp))
            .collect();
        format!("forall<{}> ", ls.join(", "))
    }

    fn fmt_generics(&self, generics: &Generics) -> String {
        if generics.params.is_empty() {
            return String::new();
        }
        let parts: Vec<String> = generics
            .params
            .iter()
            .map(|p| self.fmt_generic_param(p))
            .collect();
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
            if let WherePredicate::Type(pt) = p {
                let forall = pt
                    .lifetimes
                    .as_ref()
                    .map(|bl| self.fmt_bound_lifetimes(bl))
                    .unwrap_or_default();
                let bs: Vec<String> = pt.bounds.iter().map(|b| self.fmt_bound(b)).collect();
                preds.push(format!(
                    "{}{} : {}",
                    forall,
                    self.fmt_type(&pt.bounded_ty),
                    bs.join(", ")
                ));
            }
            if let WherePredicate::Lifetime(pl) = p {
                let os: Vec<String> = pl.bounds.iter().map(escape_lifetime).collect();
                preds.push(format!(
                    "life {} : {}",
                    escape_lifetime(&pl.lifetime),
                    os.join(", ")
                ));
            }
        }
        format!(" where {}", preds.join(", "))
    }

    fn fmt_bound(&self, b: &TypeParamBound) -> String {
        match b {
            TypeParamBound::Trait(tb) => {
                let mut out = String::new();
                if matches!(tb.modifier, TraitBoundModifier::Maybe(_)) {
                    out.push_str("maybe ");
                }
                if let Some(bl) = &tb.lifetimes {
                    out.push_str(&self.fmt_bound_lifetimes(bl));
                }
                out.push_str(&self.fmt_path(&tb.path));
                if tb.paren_token.is_some() {
                    format!("({})", out)
                } else {
                    out
                }
            }
            TypeParamBound::Lifetime(lt) => {
                if lt.ident == "static" {
                    "static".to_string()
                } else {
                    format!("life {}", escape_lifetime(lt))
                }
            }
            TypeParamBound::PreciseCapture(pc) => {
                let args: Vec<String> = pc
                    .params
                    .iter()
                    .map(|p| match p {
                        syn::CapturedParam::Lifetime(l) => format!("life {}", escape_lifetime(l)),
                        other => other.to_token_stream().to_string(),
                    })
                    .collect();
                format!("capture<{}>", args.join(", "))
            }
            other => other.to_token_stream().to_string(),
        }
    }

    // ==================== PATTERNS & EXPRESSIONS ====================

    fn fmt_pat(&mut self, pat: &Pat, ty_hint: Option<&str>) -> String {
        match pat {
            Pat::Wild(_) => "_".to_string(),
            Pat::Rest(_) => "..".to_string(),
            Pat::Lit(l) => fmt_syn_lit(&l.lit),
            Pat::Const(pc) => {
                let inner = single_tail_expr(&pc.block)
                    .map(|e| self.fmt_expr(e))
                    .unwrap_or_else(|| pc.block.to_token_stream().to_string());
                format!("const({})", inner)
            }
            Pat::Reference(pr) => {
                if pr.mutability.is_some() {
                    format!("deref write {}", self.fmt_pat(&pr.pat, None))
                } else {
                    format!("deref {}", self.fmt_pat(&pr.pat, None))
                }
            }
            Pat::Paren(pp) => format!("({})", self.fmt_pat(&pp.pat, ty_hint)),
            Pat::Macro(pm) => fmt_syn_macro(&pm.mac),
            Pat::Range(r) => {
                let s = r
                    .start
                    .as_ref()
                    .map(|e| self.fmt_expr(e))
                    .unwrap_or_default();
                let e = r.end.as_ref().map(|e| self.fmt_expr(e)).unwrap_or_default();
                let op = match r.limits {
                    syn::RangeLimits::Closed(_) => "..=",
                    syn::RangeLimits::HalfOpen(_) => "..",
                };
                format!("{}{}{}", s, op, e)
            }
            Pat::Ident(pi) => {
                let name = escape_ident(&pi.ident.to_string());
                if let Some((_, sub)) = &pi.subpat {
                    if matches!(sub.as_ref(), Pat::Rest(_)) {
                        if pi.by_ref.is_some() {
                            let mode = if pi.mutability.is_some() {
                                "write"
                            } else {
                                "read"
                            };
                            return format!(".. borrow {} {}", mode, name);
                        }
                        let mk = if pi.mutability.is_some() { "mut " } else { "" };
                        return format!(".. {}{}", mk, name);
                    }
                    let sub_s = self.fmt_pat(sub, None);
                    let ty = ty_hint.unwrap_or("int");
                    if pi.by_ref.is_some() {
                        let mode = if pi.mutability.is_some() {
                            "write"
                        } else {
                            "read"
                        };
                        return format!("{} as borrow {} {} {}", sub_s, mode, ty, name);
                    }
                    let mk = if pi.mutability.is_some() { "mut " } else { "" };
                    return format!("{} as {}{} {}", sub_s, mk, ty, name);
                }
                if pi.by_ref.is_some() {
                    let mode = if pi.mutability.is_some() {
                        "write"
                    } else {
                        "read"
                    };
                    let ty = ty_hint.unwrap_or("_");
                    return format!("borrow {} {} {}", mode, ty, name);
                }
                let mk = if pi.mutability.is_some() { "mut " } else { "" };
                if let Some(ty) = ty_hint {
                    self.bind_var(&pi.ident.to_string(), ty.to_string());
                    format!("{}{} {}", mk, ty, name)
                } else {
                    format!("{}{}", mk, name)
                }
            }
            Pat::Tuple(pt) => {
                let ps: Vec<_> = pt.elems.iter().map(|p| self.fmt_pat(p, None)).collect();
                if ps.len() == 1 {
                    format!("({},)", ps[0])
                } else {
                    format!("({})", ps.join(", "))
                }
            }
            Pat::Slice(ps) => {
                let elems: Vec<_> = ps.elems.iter().map(|p| self.fmt_pat(p, None)).collect();
                format!("[{}]", elems.join(", "))
            }
            Pat::TupleStruct(pts) => {
                let fs: Vec<_> = pts.elems.iter().map(|p| self.fmt_pat(p, None)).collect();
                format!(
                    "{}({})",
                    self.fmt_pat_qself_or_path(&pts.qself, &pts.path),
                    fs.join(", ")
                )
            }
            Pat::Struct(ps) => {
                let mut fs = Vec::new();
                for f in &ps.fields {
                    let mut apfx = String::new();
                    for a in &f.attrs {
                        apfx.push_str(&self.fmt_attr_inline(a, None));
                        apfx.push(' ');
                    }
                    let fname = match &f.member {
                        Member::Named(id) => escape_ident(&id.to_string()),
                        Member::Unnamed(idx) => idx.index.to_string(),
                    };
                    if f.colon_token.is_some() {
                        fs.push(format!("{}{}: {}", apfx, fname, self.fmt_pat(&f.pat, None)));
                    } else if let Pat::Ident(pi) = f.pat.as_ref()
                        && pi.by_ref.is_some()
                    {
                        let mode = if pi.mutability.is_some() {
                            "write"
                        } else {
                            "read"
                        };
                        fs.push(format!("{}borrow {} {}", apfx, mode, fname));
                    } else {
                        fs.push(format!("{}{}", apfx, fname));
                    }
                }
                if ps.rest.is_some() {
                    fs.push("..".to_string());
                }
                format!(
                    "{} {{ {} }}",
                    self.fmt_pat_qself_or_path(&ps.qself, &ps.path),
                    fs.join(", ")
                )
            }
            Pat::Path(pp) => self.fmt_pat_qself_or_path(&pp.qself, &pp.path),
            Pat::Or(po) => {
                let alts: Vec<_> = po.cases.iter().map(|p| self.fmt_pat(p, None)).collect();
                alts.join(" | ")
            }
            other => other.to_token_stream().to_string(),
        }
    }

    fn fmt_expr(&mut self, expr: &Expr) -> String {
        let attrs = expr_outer_attrs(expr);
        let mut attr_pfx = String::new();
        for a in attrs {
            attr_pfx.push_str(&self.fmt_attr_inline(a, None));
            attr_pfx.push(' ');
        }
        let core = match expr {
            Expr::Lit(l) => fmt_syn_lit(&l.lit),
            Expr::Infer(_) => "_".to_string(),
            Expr::Path(ep) => {
                if let Some(qself) = &ep.qself {
                    return format!("{}{}", attr_pfx, self.fmt_qself_path(qself, &ep.path));
                }
                if ep.path.is_ident("self") {
                    "this".to_string()
                } else {
                    self.fmt_path(&ep.path)
                }
            }
            Expr::Paren(ep) => format!("({})", self.fmt_expr(&ep.expr)),
            Expr::Unary(eu) => {
                let inner = self.fmt_expr(&eu.expr);
                match eu.op {
                    UnOp::Deref(_) => format!("deref {}", inner),
                    UnOp::Not(_) => format!("!{}", inner),
                    _ => format!("{}{}", eu.op.to_token_stream(), inner),
                }
            }
            Expr::Reference(er) => {
                let kw = if er.mutability.is_some() {
                    "write"
                } else {
                    "read"
                };
                format!("{} {}", kw, self.fmt_expr(&er.expr))
            }
            Expr::RawAddr(er) => {
                let kw = match er.mutability {
                    syn::PointerMutability::Mut(_) => "write ptr",
                    syn::PointerMutability::Const(_) => "read ptr",
                };
                format!("{} {}", kw, self.fmt_expr(&er.expr))
            }
            Expr::Const(ec) => {
                if let Some(single) = single_tail_expr(&ec.block) {
                    format!("const({})", self.fmt_expr(single))
                } else {
                    let mut sub = self.child();
                    sub.emit_block_stmts(&ec.block, BlockTail::Return);
                    format!("const {{\n{}}}", sub.out)
                }
            }
            Expr::Binary(eb) => {
                let op = eb.op.to_token_stream().to_string();
                format!(
                    "({} {} {})",
                    self.fmt_expr(&eb.left),
                    op,
                    self.fmt_expr(&eb.right)
                )
            }
            Expr::Assign(ea) => {
                format!("{} = {}", self.fmt_expr(&ea.left), self.fmt_expr(&ea.right))
            }
            Expr::Cast(ec) => {
                format!("({} as {})", self.fmt_expr(&ec.expr), self.fmt_type(&ec.ty))
            }
            Expr::Try(et) => format!("try {}", self.fmt_expr(&et.expr)),
            Expr::Await(ea) => format!("await {}", self.fmt_expr(&ea.base)),
            Expr::Call(ec) => {
                let callee = self.fmt_expr(&ec.func);
                let args: Vec<_> = ec.args.iter().map(|a| self.fmt_expr(a)).collect();
                format!("{}({})", callee, args.join(", "))
            }
            Expr::MethodCall(mc) => {
                let recv = self.fmt_expr(&mc.receiver);
                let mname = escape_ident(&mc.method.to_string());
                let targs = match &mc.turbofish {
                    Some(tf) => {
                        let ts: Vec<String> =
                            tf.args.iter().map(|a| self.fmt_generic_arg(a)).collect();
                        format!("<{}>", ts.join(", "))
                    }
                    None => String::new(),
                };
                let args: Vec<_> = mc.args.iter().map(|a| self.fmt_expr(a)).collect();
                format!("{}.{}{}({})", recv, mname, targs, args.join(", "))
            }
            Expr::Field(ef) => {
                let base = self.fmt_expr(&ef.base);
                match &ef.member {
                    Member::Named(id) => format!("{}.{}", base, escape_ident(&id.to_string())),
                    Member::Unnamed(idx) => format!("{}.{}", base, idx.index),
                }
            }
            Expr::Index(ei) => {
                format!("{}[{}]", self.fmt_expr(&ei.expr), self.fmt_expr(&ei.index))
            }
            Expr::Array(ea) => {
                let es: Vec<_> = ea.elems.iter().map(|e| self.fmt_expr(e)).collect();
                format!("[{}]", es.join(", "))
            }
            Expr::Repeat(er) => {
                format!("[{}; {}]", self.fmt_expr(&er.expr), self.fmt_expr(&er.len))
            }
            Expr::Tuple(et) => {
                let es: Vec<_> = et.elems.iter().map(|e| self.fmt_expr(e)).collect();
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
                    .unwrap_or_default();
                let e = er
                    .end
                    .as_ref()
                    .map(|e| self.fmt_expr(e))
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
                    let mut pfx = String::new();
                    for a in &f.attrs {
                        pfx.push_str(&self.fmt_attr_inline(a, None));
                        pfx.push(' ');
                    }
                    let fname = match &f.member {
                        Member::Named(id) => escape_ident(&id.to_string()),
                        Member::Unnamed(idx) => idx.index.to_string(),
                    };
                    if f.colon_token.is_none() {
                        fs.push(format!("{}{}", pfx, fname));
                    } else {
                        fs.push(format!("{}{} = {}", pfx, fname, self.fmt_expr(&f.expr)));
                    }
                }
                if let Some(rest) = &es.rest {
                    let use_with = es.qself.is_none()
                        && es.path.segments.iter().all(|s| s.arguments.is_empty())
                        && matches!(
                            rest.as_ref(),
                            Expr::Path(ep) if ep
                                .path
                                .get_ident()
                                .is_some_and(|id| self.lookup_var(&id.to_string()).is_some())
                        );
                    let rest_s = self.fmt_expr(rest);
                    if use_with {
                        if fs.is_empty() {
                            format!("{} with {{}}", rest_s)
                        } else {
                            format!("{} with {{ {} }}", rest_s, fs.join(", "))
                        }
                    } else if fs.is_empty() {
                        format!("new {} {{ ..{} }}", self.fmt_path(&es.path), rest_s)
                    } else {
                        format!(
                            "new {} {{ {}, ..{} }}",
                            self.fmt_path(&es.path),
                            fs.join(", "),
                            rest_s
                        )
                    }
                } else {
                    format!("new {} {{ {} }}", self.fmt_path(&es.path), fs.join(", "))
                }
            }
            Expr::If(eif) => {
                if let Some(inline_if) = self.try_fmt_inline_if(eif, true) {
                    inline_if
                } else {
                    // Convert multi-statement `if` expression into a labeled value block without `rust { ... }`!
                    let mut sub = self.child();
                    sub.emit_if(eif, false, BlockTail::Break("__if_val"));
                    format!("__if_val: {{\n{}}}", sub.out)
                }
            }
            Expr::Loop(el) => {
                let lbl = el
                    .label
                    .as_ref()
                    .map(|l| format!("{}: ", escape_lifetime(&l.name)))
                    .unwrap_or_default();
                let mut sub = self.child();
                sub.emit_block_stmts(&el.body, BlockTail::None);
                format!("{}loop {{\n{}}}", lbl, sub.out)
            }
            Expr::While(ew) => {
                let lbl = ew
                    .label
                    .as_ref()
                    .map(|l| format!("{}: ", escape_lifetime(&l.name)))
                    .unwrap_or_default();
                let cond = self.fmt_condition(&ew.cond);
                let mut sub = self.child();
                sub.emit_block_stmts(&ew.body, BlockTail::None);
                format!("{}while ({}) {{\n{}}}", lbl, cond, sub.out)
            }
            Expr::ForLoop(ef) => {
                let lbl = ef
                    .label
                    .as_ref()
                    .map(|l| format!("{}: ", escape_lifetime(&l.name)))
                    .unwrap_or_default();
                let elem_hint = self.infer_iter_elem_type(&ef.expr);
                let pat = self.fmt_pat(&ef.pat, elem_hint.as_deref());
                let iter = self.fmt_expr(&ef.expr);
                let mut sub = self.child();
                sub.emit_block_stmts(&ef.body, BlockTail::None);
                format!("{}foreach ({} in {}) {{\n{}}}", lbl, pat, iter, sub.out)
            }
            Expr::Return(er) => match &er.expr {
                Some(e) => format!("return {}", self.fmt_expr(e)),
                None => "return".to_string(),
            },
            Expr::Break(eb) => {
                let mut out = String::from("break");
                if let Some(l) = &eb.label {
                    out.push_str(&format!(" {}", escape_lifetime(l)));
                }
                if let Some(e) = &eb.expr {
                    out.push(' ');
                    out.push_str(&self.fmt_expr(e));
                }
                out
            }
            Expr::Continue(ec) => match &ec.label {
                Some(l) => format!("continue {}", escape_lifetime(l)),
                None => "continue".to_string(),
            },
            Expr::Match(em) => {
                let scr = self.fmt_expr(&em.expr);
                let mut arms = Vec::new();
                for arm in &em.arms {
                    let mut pfx = String::new();
                    for a in &arm.attrs {
                        pfx.push_str(&self.fmt_attr_inline(a, None));
                        pfx.push(' ');
                    }
                    let pat = self.fmt_pat(&arm.pat, None);
                    let g = match &arm.guard {
                        Some((_, ge)) => format!(" when {}", self.fmt_condition(ge)),
                        None => String::new(),
                    };
                    let body = self.fmt_expr(&arm.body);
                    arms.push(format!("{}{}{} => {}", pfx, pat, g, body));
                }
                format!("match ({}) {{ {} }}", scr, arms.join(", "))
            }
            Expr::Closure(ec) => {
                let mut prefix = String::new();
                let is_async = ec.asyncness.is_some();
                if is_async {
                    prefix.push_str("async ");
                }
                if ec.capture.is_some() {
                    prefix.push_str("take ");
                }
                let mut ps = Vec::new();
                for inp in &ec.inputs {
                    if let Pat::Type(pt) = inp {
                        ps.push(self.fmt_typed_fn_param(&pt.attrs, &pt.pat, &pt.ty));
                    } else {
                        let (is_mut, pname) = extract_param_pat(inp);
                        if self.options.use_rustc_inference {
                            match self.probe_closure_param_via_rustc(&pname) {
                                Ok(inferred) => {
                                    let mk = if is_mut { "mut " } else { "" };
                                    ps.push(format!("{}{} {}", mk, inferred, escape_ident(&pname)));
                                }
                                Err(e) => self.record_err(e),
                            }
                        } else {
                            self.record_err(RSharpError::unresolved_type(
                                pname,
                                "Closure parameter requires explicit type in R#",
                            ));
                        }
                    }
                }
                let ret = match &ec.output {
                    ReturnType::Default => String::new(),
                    ReturnType::Type(_, ty) => {
                        let inner = self.fmt_type(ty);
                        if is_async {
                            format!(" : Task<{}>", inner)
                        } else {
                            format!(" : {}", inner)
                        }
                    }
                };
                match ec.body.as_ref() {
                    Expr::Block(eb) => {
                        let mut sub = self.child();
                        let has_value_tail =
                            matches!(eb.block.stmts.last(), Some(Stmt::Expr(_, None)));
                        let tail = if !matches!(ec.output, ReturnType::Default) || has_value_tail {
                            BlockTail::Return
                        } else {
                            BlockTail::None
                        };
                        sub.emit_block_stmts(&eb.block, tail);
                        format!("{}({}){} => {{\n{}}}", prefix, ps.join(", "), ret, sub.out)
                    }
                    other => format!(
                        "{}({}){} => {}",
                        prefix,
                        ps.join(", "),
                        ret,
                        self.fmt_expr(other)
                    ),
                }
            }
            Expr::Block(eb) => {
                if let Some(lbl) = &eb.label {
                    let mut sub = self.child();
                    sub.emit_block_stmts(&eb.block, BlockTail::None);
                    format!("{}: {{\n{}}}", escape_lifetime(&lbl.name), sub.out)
                } else if let Some(single) = single_tail_expr(&eb.block) {
                    self.fmt_expr(single)
                } else {
                    let mut sub = self.child();
                    sub.emit_block_stmts(&eb.block, BlockTail::Break("__blk_val"));
                    format!("__blk_val: {{\n{}}}", sub.out)
                }
            }
            Expr::Async(ea) => {
                let mv = if ea.capture.is_some() { "take " } else { "" };
                let mut sub = self.child();
                sub.emit_block_stmts(&ea.block, BlockTail::Return);
                format!("async {}{{\n{}}}", mv, sub.out)
            }
            Expr::Unsafe(eu) => {
                let mut sub = self.child();
                sub.emit_block_stmts(&eu.block, BlockTail::None);
                format!("unsafe {{\n{}}}", sub.out)
            }
            Expr::Macro(em) => fmt_syn_macro(&em.mac),
            other => format!("rust {{ {} }}", other.to_token_stream()),
        };
        format!("{}{}", attr_pfx, core)
    }

    fn try_fmt_inline_if(&mut self, eif: &syn::ExprIf, allow_ternary: bool) -> Option<String> {
        let then_e = single_tail_expr(&eif.then_branch)?;
        let (_, else_box) = &eif.else_branch.as_ref()?;
        if let Expr::If(next_if) = else_box.as_ref() {
            let else_s = self.try_fmt_inline_if(next_if, false)?;
            let c = self.fmt_condition(&eif.cond);
            let t = self.fmt_expr(then_e);
            return Some(format!("if ({}) => {} else {}", c, t, else_s));
        }
        let else_e = single_tail_expr_of(else_box)?;
        if allow_ternary && !expr_has_let(&eif.cond) {
            let c = self.fmt_expr(&eif.cond);
            let t = self.fmt_expr(then_e);
            let e = self.fmt_expr(else_e);
            return Some(format!("({} ? {} : {})", c, t, e));
        }
        let c = self.fmt_condition(&eif.cond);
        let t = self.fmt_expr(then_e);
        let e = self.fmt_expr(else_e);
        Some(format!("if ({}) => {} else => {}", c, t, e))
    }

    // ==================== LOCAL TYPE INFERENCE ====================

    fn infer_expr_type(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Int(li),
                ..
            }) => Some(map_rust_primitive(li.suffix()).unwrap_or("int").to_string()),
            Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Float(lf),
                ..
            }) => Some(
                map_rust_primitive(lf.suffix())
                    .unwrap_or("double")
                    .to_string(),
            ),
            Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(_),
                ..
            }) => Some("read str".to_string()),
            Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Byte(_),
                ..
            }) => Some("byte".to_string()),
            Expr::Lit(syn::ExprLit {
                lit: syn::Lit::ByteStr(bs),
                ..
            }) => Some(format!("read byte[{}]", bs.value().len())),
            Expr::Lit(syn::ExprLit {
                lit: syn::Lit::CStr(_),
                ..
            }) => Some("read<static> core.ffi.CStr".to_string()),
            Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Char(_),
                ..
            }) => Some("char".to_string()),
            Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Bool(_),
                ..
            }) => Some("bool".to_string()),
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
            Expr::RawAddr(er) => {
                let inner = self.infer_expr_type(&er.expr)?;
                let kw = match er.mutability {
                    syn::PointerMutability::Mut(_) => "write ptr",
                    syn::PointerMutability::Const(_) => "read ptr",
                };
                Some(format!("{}<{}>", kw, inner))
            }
            Expr::Const(ec) => single_tail_expr(&ec.block).and_then(|e| self.infer_expr_type(e)),
            Expr::Unary(eu) => match eu.op {
                UnOp::Deref(_) => {
                    let inner = self.infer_expr_type(&eu.expr)?;
                    inner
                        .strip_prefix("read ")
                        .or_else(|| inner.strip_prefix("write "))
                        .map(|s| s.to_string())
                }
                _ => self.infer_expr_type(&eu.expr),
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
                let name = em
                    .mac
                    .path
                    .segments
                    .last()
                    .expect("non-empty path")
                    .ident
                    .to_string();
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
            Expr::Match(em) => em.arms.iter().find_map(|a| {
                let body = single_tail_expr_of(&a.body).unwrap_or(&a.body);
                self.infer_expr_type(body)
            }),
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
                let mut typed_inputs = Vec::new();
                for inp in &ec.inputs {
                    if let Pat::Type(pt) = inp {
                        param_tys.push(self.fmt_type(&pt.ty));
                        typed_inputs.push((pt.pat.as_ref(), pt.ty.as_ref()));
                    } else {
                        return None;
                    }
                }
                let ret = match &ec.output {
                    ReturnType::Type(_, ty) => self.fmt_type(ty),
                    ReturnType::Default => self
                        .infer_closure_body_type(&typed_inputs, &ec.body)
                        .unwrap_or_else(|| "void".to_string()),
                };
                if ec.asyncness.is_some() {
                    Some(format!(
                        "impl std.ops.AsyncFn({}) -> {}",
                        param_tys.join(", "),
                        ret
                    ))
                } else {
                    Some(format!("({}) -> {}", param_tys.join(", "), ret))
                }
            }
            _ => None,
        }
    }

    fn bind_typed_pat(&mut self, pat: &Pat, ty: &syn::Type) {
        match (pat, ty) {
            (Pat::Ident(pi), _) => self.bind_var(&pi.ident.to_string(), self.fmt_type(ty)),
            (Pat::Tuple(pt), syn::Type::Tuple(tt)) => {
                for (p, t) in pt.elems.iter().zip(tt.elems.iter()) {
                    self.bind_typed_pat(p, t);
                }
            }
            _ => {}
        }
    }

    fn infer_closure_body_type(
        &self,
        inputs: &[(&Pat, &syn::Type)],
        body: &Expr,
    ) -> Option<String> {
        let mut sub = self.child();
        for &(pat, ty) in inputs {
            sub.bind_typed_pat(pat, ty);
        }
        match body {
            Expr::Block(eb) => {
                for st in &eb.block.stmts {
                    if let Stmt::Local(loc) = st
                        && let Pat::Type(pt) = &loc.pat
                    {
                        sub.bind_typed_pat(&pt.pat, &pt.ty);
                    }
                }
                match eb.block.stmts.last() {
                    Some(Stmt::Expr(tail, None)) => sub.infer_expr_type(tail),
                    _ => None,
                }
            }
            other => sub.infer_expr_type(other),
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

fn format_meta_attr(meta: &syn::Meta, prefix: &str, safety: &str) -> String {
    match meta {
        syn::Meta::Path(p) => {
            let path_str = path_to_rsharp(p);
            let name = map_rust_attr_name(&path_str);
            format!("[{}{}{}]", prefix, safety, name)
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
            match ml.delimiter {
                syn::MacroDelimiter::Paren(_) => {
                    format!("[{}{}{}({})]", prefix, safety, name, args)
                }
                syn::MacroDelimiter::Bracket(_) => {
                    format!("[{}{}{}[{}]]", prefix, safety, name, args)
                }
                syn::MacroDelimiter::Brace(_) => {
                    format!("[{}{}{}{{ {} }}]", prefix, safety, name, args)
                }
            }
        }
        syn::Meta::NameValue(nv) => {
            let path_str = path_to_rsharp(&nv.path);
            let name = map_rust_attr_name(&path_str);
            let val = nv.value.to_token_stream().to_string();
            format!("[{}{}{} = {}]", prefix, safety, name, val)
        }
    }
}

fn fmt_syn_lit(lit: &syn::Lit) -> String {
    match lit {
        syn::Lit::Str(ls) => {
            let raw = ls.to_token_stream().to_string();
            if raw.starts_with('r') {
                format_rsharp_raw_str(&ls.value(), "")
            } else {
                raw
            }
        }
        syn::Lit::Byte(lb) => lb.to_token_stream().to_string(),
        syn::Lit::ByteStr(lbs) => {
            let raw = lbs.to_token_stream().to_string();
            if let Ok(s) = std::str::from_utf8(&lbs.value()) {
                if raw.starts_with("br") {
                    format_rsharp_raw_str(s, "u8")
                } else {
                    let escaped = s.escape_default().to_string();
                    format!("\"{}\"u8", escaped)
                }
            } else {
                let mut escaped = String::new();
                for &b in &lbs.value() {
                    match b {
                        b'\\' => escaped.push_str("\\\\"),
                        b'"' => escaped.push_str("\\\""),
                        b'\n' => escaped.push_str("\\n"),
                        b'\r' => escaped.push_str("\\r"),
                        b'\t' => escaped.push_str("\\t"),
                        0x20..=0x7e => escaped.push(b as char),
                        _ => escaped.push_str(&format!("\\x{:02x}", b)),
                    }
                }
                format!("\"{}\"u8", escaped)
            }
        }
        syn::Lit::CStr(lcs) => {
            let raw = lcs.to_token_stream().to_string();
            if let Ok(s) = lcs.value().to_str() {
                if raw.starts_with("cr") {
                    format!("c{}", format_rsharp_raw_str(s, ""))
                } else {
                    let escaped = s.escape_default().to_string();
                    format!("c\"{}\"", escaped)
                }
            } else {
                raw
            }
        }
        other => other.to_token_stream().to_string(),
    }
}

fn format_rsharp_raw_str(value: &str, suffix: &str) -> String {
    if !value.contains('"') {
        format!("@\"{}\"{}", value, suffix)
    } else if value.starts_with('"') || value.ends_with('"') {
        format!("@\"{}\"{}", value.replace('"', "\"\""), suffix)
    } else {
        let mut max_run = 0usize;
        let mut cur = 0usize;
        for ch in value.chars() {
            if ch == '"' {
                cur += 1;
                max_run = max_run.max(cur);
            } else {
                cur = 0;
            }
        }
        let delim = "\"".repeat((max_run + 1).max(3));
        format!("{}{}{}{}", delim, value, delim, suffix)
    }
}

fn fmt_use_tree(tree: &syn::UseTree) -> String {
    fmt_use_tree_with_prefix(tree, Vec::new())
}

fn fmt_use_tree_with_prefix(tree: &syn::UseTree, mut prefix: Vec<String>) -> String {
    match tree {
        syn::UseTree::Path(p) => {
            let seg = if p.ident == "self" {
                "this".to_string()
            } else {
                escape_ident(&p.ident.to_string())
            };
            prefix.push(seg);
            fmt_use_tree_with_prefix(&p.tree, prefix)
        }
        syn::UseTree::Name(n) => {
            let seg = if n.ident == "self" {
                "this".to_string()
            } else {
                escape_ident(&map_rust_std_alias(&n.ident.to_string()))
            };
            prefix.push(seg);
            prefix.join(".")
        }
        syn::UseTree::Rename(r) => {
            let seg = if r.ident == "self" {
                "this".to_string()
            } else {
                escape_ident(&map_rust_std_alias(&r.ident.to_string()))
            };
            prefix.push(seg);
            format!(
                "{} = {}",
                escape_ident(&r.rename.to_string()),
                prefix.join(".")
            )
        }
        syn::UseTree::Glob(_) => {
            prefix.push("*".to_string());
            prefix.join(".")
        }
        syn::UseTree::Group(g) => {
            let subs: Vec<String> = g
                .items
                .iter()
                .map(|sub| fmt_use_tree_with_prefix(sub, Vec::new()))
                .collect();
            if prefix.is_empty() {
                format!("{{{}}}", subs.join(", "))
            } else {
                prefix.push(format!("{{{}}}", subs.join(", ")));
                prefix.join(".")
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

fn split_tuple_type_str(ty: &str) -> Option<Vec<String>> {
    let inner = ty.trim().strip_prefix('(')?.strip_suffix(')')?;
    let mut elems = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, ch) in inner.char_indices() {
        match ch {
            '(' | '<' | '[' => depth += 1,
            ')' | '>' | ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                elems.push(inner[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    let tail = inner[start..].trim();
    if !tail.is_empty() {
        elems.push(tail.to_string());
    }
    if elems.is_empty() { None } else { Some(elems) }
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

fn single_tail_expr_of(expr: &Expr) -> Option<&Expr> {
    match expr {
        Expr::Block(eb) => single_tail_expr(&eb.block),
        _ => None,
    }
}

fn extract_param_pat(pat: &Pat) -> (bool, String) {
    match pat {
        Pat::Ident(pi) => (pi.mutability.is_some(), pi.ident.to_string()),
        Pat::Wild(_) => (false, "_".to_string()),
        other => (false, other.to_token_stream().to_string()),
    }
}

fn fmt_vis(vis: &Visibility) -> String {
    match vis {
        Visibility::Public(_) => "public ".to_string(),
        Visibility::Restricted(r) => {
            if r.path.is_ident("crate") {
                "internal ".to_string()
            } else if r.path.is_ident("super") {
                "public(super) ".to_string()
            } else if r.path.is_ident("self") {
                "public(this) ".to_string()
            } else {
                format!("public(in {}) ", path_to_rsharp(&r.path))
            }
        }
        Visibility::Inherited => String::new(),
    }
}

fn path_to_rsharp(path: &Path) -> String {
    path.segments
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let raw = s.ident.to_string();
            if i == 0 && raw == "self" {
                "this".to_string()
            } else if i == 0 || raw.starts_with("r#") {
                escape_ident(&raw)
            } else {
                raw
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

fn is_boxed_any(path: &Path) -> bool {
    let last = path.segments.last().expect("non-empty path");
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
    let last = path.segments.last().expect("non-empty path");
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
        "forbid" => "Forbid",
        "expect" => "Expect",
        "repr" => "Repr",
        "no_mangle" => "NoMangle",
        "export_name" => "ExportName",
        "link_section" => "LinkSection",
        "naked" => "Naked",
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

fn expr_has_let(expr: &Expr) -> bool {
    match expr {
        Expr::Let(_) => true,
        Expr::Binary(eb) if matches!(eb.op, BinOp::And(_)) => {
            expr_has_let(&eb.left) || expr_has_let(&eb.right)
        }
        _ => false,
    }
}

fn expr_outer_attrs(expr: &Expr) -> &[Attribute] {
    match expr {
        Expr::Array(e) => &e.attrs,
        Expr::Assign(e) => &e.attrs,
        Expr::Async(e) => &e.attrs,
        Expr::Await(e) => &e.attrs,
        Expr::Binary(e) => &e.attrs,
        Expr::Block(e) => &e.attrs,
        Expr::Break(e) => &e.attrs,
        Expr::Call(e) => &e.attrs,
        Expr::Cast(e) => &e.attrs,
        Expr::Closure(e) => &e.attrs,
        Expr::Const(e) => &e.attrs,
        Expr::Continue(e) => &e.attrs,
        Expr::Field(e) => &e.attrs,
        Expr::ForLoop(e) => &e.attrs,
        Expr::If(e) => &e.attrs,
        Expr::Index(e) => &e.attrs,
        Expr::Infer(e) => &e.attrs,
        Expr::Let(e) => &e.attrs,
        Expr::Lit(e) => &e.attrs,
        Expr::Loop(e) => &e.attrs,
        Expr::Macro(e) => &e.attrs,
        Expr::Match(e) => &e.attrs,
        Expr::MethodCall(e) => &e.attrs,
        Expr::Paren(e) => &e.attrs,
        Expr::Path(e) => &e.attrs,
        Expr::Range(e) => &e.attrs,
        Expr::RawAddr(e) => &e.attrs,
        Expr::Reference(e) => &e.attrs,
        Expr::Repeat(e) => &e.attrs,
        Expr::Return(e) => &e.attrs,
        Expr::Struct(e) => &e.attrs,
        Expr::Try(e) => &e.attrs,
        Expr::Tuple(e) => &e.attrs,
        Expr::Unary(e) => &e.attrs,
        Expr::Unsafe(e) => &e.attrs,
        Expr::While(e) => &e.attrs,
        _ => &[],
    }
}

fn escape_ident(name: &str) -> String {
    let was_raw = name.starts_with("r#");
    let stripped = name.strip_prefix("r#").unwrap_or(name);
    if stripped == "_" || stripped == "Self" || stripped == "this" {
        return stripped.to_string();
    }
    if was_raw || Keyword::parse(stripped).is_some() {
        format!("@{}", stripped)
    } else {
        stripped.to_string()
    }
}

fn escape_lifetime(lt: &syn::Lifetime) -> String {
    let raw = lt.ident.to_string();
    let was_raw = raw.starts_with("r#");
    let stripped = raw.strip_prefix("r#").unwrap_or(&raw);
    if stripped == "static" || stripped == "_" {
        return stripped.to_string();
    }
    if was_raw || Keyword::parse(stripped).is_some() {
        format!("@{}", stripped)
    } else {
        stripped.to_string()
    }
}

fn fmt_syn_macro(mac: &syn::Macro) -> String {
    let prefix = if mac.path.leading_colon.is_some() {
        "global::"
    } else {
        ""
    };
    let path = format!("{}{}", prefix, path_to_rsharp(&mac.path));
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
