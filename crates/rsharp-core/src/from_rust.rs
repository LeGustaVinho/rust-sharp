use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use quote::ToTokens;
use syn::spanned::Spanned;
use syn::{
    Attribute, BinOp, Block, Expr, FnArg, GenericArgument, GenericParam, Generics, Item, Local,
    Member, Pat, Path, PathArguments, ReturnType, Stmt, TraitBoundModifier, TraitItem, Type,
    TypeParamBound, UnOp, Visibility, WherePredicate,
};

use crate::error::{RSharpError, Result};
use crate::ra_oracle::TypeHintMap;
use crate::rustc_runner::probes::TypeProbes;
use crate::token::Keyword;

const INDENT: &str = "    ";

#[derive(Clone, Copy, PartialEq, Eq)]
enum BlankLines {
    Preserve,
    Around,
    After,
    Imports,
}

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
    convert_from_rust_with_hints(rust_source, options, &TypeHintMap::default(), true)
}

/// Converts Rust source code into idiomatic R# using external `TypeHintMap` annotations
/// (e.g. from `RustAnalyzerOracle` for multi-crate Cargo workspaces).
///
/// When `strict_unnameable` is `false`, bindings whose Rust type cannot be written syntactically
/// (such as nested closure/iterator adapters or private crate-internal types) fall back to
/// a statement/expression-level `rust { ... }` block so conversion still succeeds cleanly.
pub fn convert_from_rust_with_hints(
    rust_source: &str,
    options: FromRustOptions,
    hints: &TypeHintMap,
    strict_unnameable: bool,
) -> Result<String> {
    let parsed: syn::File = syn::parse_file(rust_source)
        .map_err(|e| RSharpError::from_rust(format!("Invalid Rust syntax: {}", e)))?;
    let probes = TypeProbes::new(rust_source, &parsed);
    let mut conv = RustToRSharp::new(rust_source, options, hints, strict_unnameable, &probes);
    // Syn assigns spans after stripping the BOM and shebang.
    let source = rust_source.strip_prefix('\u{feff}').unwrap_or(rust_source);
    conv.spacing_source = &source[parsed.shebang.as_ref().map_or(0, String::len)..];
    conv.collect_signatures(&parsed.items);
    conv.emit_file(&parsed);
    if let Some(err) = conv.first_err.borrow_mut().take() {
        return Err(err);
    }
    Ok(conv.out)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockTail {
    None,
    UnitReturn,
    Return,
    Value,
}

enum HintLookup {
    None,
    Resolved(String),
    Unnameable,
}

struct RustToRSharp<'a> {
    probes: &'a TypeProbes<'a>,
    source_lines: Rc<Vec<&'a str>>,
    spacing_source: &'a str,
    options: FromRustOptions,
    type_hints: &'a TypeHintMap,
    strict_unnameable: bool,
    out: String,
    indent: usize,
    fn_returns: HashMap<String, String>,
    struct_fields: HashMap<String, HashMap<String, String>>,
    known_path_roots: HashSet<String>,
    in_scope_traits: HashSet<String>,
    renamed_use_origs: HashSet<String>,
    shadowed_std_aliases: HashSet<String>,
    scopes: Vec<HashMap<String, String>>,
    declared_in_fn: HashSet<String>,
    rustc_probe_aborted: Rc<Cell<bool>>,
    first_err: Rc<RefCell<Option<RSharpError>>>,
}

impl<'a> RustToRSharp<'a> {
    fn new(
        raw_source: &'a str,
        options: FromRustOptions,
        type_hints: &'a TypeHintMap,
        strict_unnameable: bool,
        probes: &'a TypeProbes<'a>,
    ) -> Self {
        let mut known_path_roots = HashSet::new();
        for root in ["std", "core", "alloc", "crate", "super", "self", "Self"] {
            known_path_roots.insert(root.to_string());
        }
        let source_lines = if type_hints.is_empty() {
            Vec::new()
        } else {
            raw_source.lines().collect()
        };
        Self {
            probes,
            source_lines: Rc::new(source_lines),
            spacing_source: raw_source,
            options,
            type_hints,
            strict_unnameable,
            out: String::new(),
            indent: 0,
            fn_returns: HashMap::new(),
            struct_fields: HashMap::new(),
            known_path_roots,
            in_scope_traits: HashSet::new(),
            renamed_use_origs: HashSet::new(),
            shadowed_std_aliases: HashSet::new(),
            scopes: vec![HashMap::new()],
            declared_in_fn: HashSet::new(),
            rustc_probe_aborted: Rc::new(Cell::new(false)),
            first_err: Rc::new(RefCell::new(None)),
        }
    }

    fn child(&self) -> Self {
        Self {
            probes: self.probes,
            source_lines: Rc::clone(&self.source_lines),
            spacing_source: self.spacing_source,
            options: self.options,
            type_hints: self.type_hints,
            strict_unnameable: self.strict_unnameable,
            out: String::new(),
            indent: self.indent + 1,
            fn_returns: self.fn_returns.clone(),
            struct_fields: self.struct_fields.clone(),
            known_path_roots: self.known_path_roots.clone(),
            in_scope_traits: self.in_scope_traits.clone(),
            renamed_use_origs: self.renamed_use_origs.clone(),
            shadowed_std_aliases: self.shadowed_std_aliases.clone(),
            scopes: self.scopes.clone(),
            declared_in_fn: HashSet::new(),
            rustc_probe_aborted: Rc::clone(&self.rustc_probe_aborted),
            first_err: Rc::clone(&self.first_err),
        }
    }

    fn span_end_utf16(&self, span: proc_macro2::Span) -> (u32, u32) {
        let end = span.end();
        let line_idx = end.line.saturating_sub(1);
        let utf16_col = self
            .source_lines
            .get(line_idx)
            .map(|l| {
                l.chars()
                    .take(end.column)
                    .map(char::len_utf16)
                    .sum::<usize>()
            })
            .unwrap_or(end.column) as u32;
        (end.line as u32, utf16_col)
    }

    fn lookup_hint_at_span(&self, span: proc_macro2::Span) -> HintLookup {
        self.lookup_hint_at_span_opts(span, false)
    }

    fn lookup_hint_at_span_opts(
        &self,
        span: proc_macro2::Span,
        require_verified_edit: bool,
    ) -> HintLookup {
        if self.type_hints.is_empty() {
            return HintLookup::None;
        }
        let (line, col) = self.span_end_utf16(span);
        let Some((raw, verified)) = self.type_hints.get_entry(line, col) else {
            return HintLookup::None;
        };
        if require_verified_edit && !verified && !raw.trim_start().starts_with("impl ") {
            return HintLookup::Unnameable;
        }
        self.format_hinted_rust_type(raw, verified)
    }

    fn format_hinted_rust_type(&self, raw: &str, verified: bool) -> HintLookup {
        let trimmed = raw.trim();
        if trimmed.is_empty()
            || trimmed == "_"
            || trimmed.contains("{unknown}")
            || trimmed.contains("{closure")
            || trimmed.contains("{async")
            || trimmed.contains("{coroutine")
            || trimmed.contains('…')
        {
            return HintLookup::Unnameable;
        }
        let Ok(mut ty) = syn::parse_str::<Type>(trimmed) else {
            return HintLookup::Unnameable;
        };
        normalize_unstable_stdlib_type(&mut ty);
        if contains_nested_impl_trait(&ty)
            || contains_nested_bare_fn(&ty)
            || matches!(
                &ty,
                Type::BareFn(bf)
                    if (bf.unsafety.is_some() && bf.abi.is_none())
                        || bare_fn_has_elided_ref(bf)
            )
            || has_unqualified_qself_trait(&ty)
            || impl_trait_returns_elided_ref(&ty)
            || contains_unimported_dyn_trait(&ty, &self.in_scope_traits, &self.renamed_use_origs)
            || contains_unimported_impl_future(&ty, &self.in_scope_traits)
            || contains_ref_to_static_dyn(&ty)
            || (!verified && (!all_unverified_paths_in_scope(&ty) || impl_trait_contains_dyn(&ty)))
        {
            return HintLookup::Unnameable;
        }
        HintLookup::Resolved(strip_non_static_lifetimes(&self.fmt_type(&ty)))
    }

    fn try_fmt_pat_with_hints(&mut self, pat: &Pat) -> HintLookup {
        match pat {
            Pat::Ident(pi) => match self.lookup_hint_at_span(pi.ident.span()) {
                HintLookup::Resolved(ty) => {
                    let eff_ty = if pi.by_ref.is_some() {
                        strip_leading_borrow_prefix(&ty, pi.mutability.is_some())
                    } else {
                        ty.as_str()
                    };
                    HintLookup::Resolved(self.fmt_pat(pat, Some(eff_ty)))
                }
                other => other,
            },
            Pat::Wild(pw) => match self.lookup_hint_at_span(pw.underscore_token.span) {
                HintLookup::Resolved(ty) => HintLookup::Resolved(format!("{} _", ty)),
                HintLookup::None => HintLookup::Resolved("_".to_string()),
                HintLookup::Unnameable => HintLookup::Unnameable,
            },
            Pat::Paren(pp) => self.try_fmt_pat_with_hints(&pp.pat),
            Pat::Reference(pr) => {
                let kw = if pr.mutability.is_some() {
                    "deref write "
                } else {
                    "deref "
                };
                match self.try_fmt_pat_with_hints(&pr.pat) {
                    HintLookup::Resolved(inner) => HintLookup::Resolved(format!("{}{}", kw, inner)),
                    other => other,
                }
            }
            Pat::Tuple(pt) => {
                let mut parts = Vec::new();
                let mut any_typed = false;
                for elem in &pt.elems {
                    if matches!(elem, Pat::Wild(_) | Pat::Rest(_)) {
                        parts.push(self.fmt_pat(elem, None));
                        continue;
                    }
                    match self.try_fmt_pat_with_hints(elem) {
                        HintLookup::Resolved(s) => {
                            any_typed = true;
                            parts.push(s);
                        }
                        other => return other,
                    }
                }
                if !any_typed {
                    return HintLookup::None;
                }
                let out = if parts.len() == 1 {
                    format!("({},)", parts[0])
                } else {
                    format!("({})", parts.join(", "))
                };
                HintLookup::Resolved(out)
            }
            Pat::Slice(ps) => {
                let mut parts = Vec::new();
                let mut any_typed = false;
                for elem in &ps.elems {
                    if matches!(elem, Pat::Wild(_) | Pat::Rest(_)) {
                        parts.push(self.fmt_pat(elem, None));
                        continue;
                    }
                    match self.try_fmt_pat_with_hints(elem) {
                        HintLookup::Resolved(s) => {
                            any_typed = true;
                            parts.push(s);
                        }
                        other => return other,
                    }
                }
                if !any_typed {
                    return HintLookup::None;
                }
                HintLookup::Resolved(format!("[{}]", parts.join(", ")))
            }
            Pat::TupleStruct(pts) => {
                let mut parts = Vec::new();
                let mut any_typed = false;
                for elem in &pts.elems {
                    if matches!(elem, Pat::Wild(_) | Pat::Rest(_)) {
                        parts.push(self.fmt_pat(elem, None));
                        continue;
                    }
                    match self.try_fmt_pat_with_hints(elem) {
                        HintLookup::Resolved(s) => {
                            any_typed = true;
                            parts.push(s);
                        }
                        other => return other,
                    }
                }
                if !any_typed {
                    return HintLookup::None;
                }
                let path_s = self.fmt_pat_qself_or_path(&pts.qself, &pts.path);
                HintLookup::Resolved(format!("{}({})", path_s, parts.join(", ")))
            }
            Pat::Struct(ps) => {
                let mut parts = Vec::new();
                let mut any_typed = false;
                for f in &ps.fields {
                    let fname = match &f.member {
                        Member::Named(id) => escape_ident(&id.to_string()),
                        Member::Unnamed(idx) => idx.index.to_string(),
                    };
                    if f.colon_token.is_some() {
                        match self.try_fmt_pat_with_hints(&f.pat) {
                            HintLookup::Resolved(sub_s) => {
                                any_typed = true;
                                parts.push(format!("{}: {}", fname, sub_s));
                            }
                            other => return other,
                        }
                    } else {
                        match self.try_fmt_pat_with_hints(&f.pat) {
                            HintLookup::Resolved(sub_s) => {
                                any_typed = true;
                                parts.push(sub_s);
                            }
                            other => return other,
                        }
                    }
                }
                if !any_typed {
                    return HintLookup::None;
                }
                if ps.rest.is_some() {
                    parts.push("..".to_string());
                }
                let path_s = self.fmt_pat_qself_or_path(&ps.qself, &ps.path);
                HintLookup::Resolved(format!("{} {{ {} }}", path_s, parts.join(", ")))
            }
            _ => HintLookup::None,
        }
    }

    fn record_err(&self, err: RSharpError) {
        self.first_err.borrow_mut().get_or_insert(err);
    }

    fn write_indent(&mut self) {
        for _ in 0..self.indent {
            self.out.push_str(INDENT);
        }
    }

    fn line(&mut self, s: &str) {
        self.write_indent();
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn separate(
        &mut self,
        previous: &mut Option<(BlankLines, usize)>,
        spacing: BlankLines,
        span: proc_macro2::Span,
        start: usize,
    ) {
        let range = span.byte_range();
        let Some((preceding, end)) = previous.replace((spacing, range.end)) else {
            return;
        };
        let needs_line = spacing == BlankLines::Around
            || matches!(preceding, BlankLines::Around | BlankLines::After)
            || (preceding == BlankLines::Imports && spacing != BlankLines::Imports);
        let preserved = self
            .spacing_source
            .get(end..range.start)
            .and_then(|gap| gap.rsplit_once('\n'))
            .map_or("", |(lines, _)| lines)
            .split('\n')
            .skip(1)
            .filter(|line| line.trim().is_empty())
            .take(2)
            .count();
        self.out
            .insert_str(start, &"\n\n"[..preserved.max(usize::from(needs_line))]);
    }

    fn field_line(&mut self, declaration: &str) -> BlankLines {
        self.line(declaration);
        if declaration.contains('\n') {
            BlankLines::Around
        } else {
            BlankLines::Preserve
        }
    }

    fn open_block(&mut self, header: &str) {
        self.line(header);
        self.line("{");
    }

    fn fmt_block(&self, body: &str) -> String {
        format!("{{\n{body}{}}}", INDENT.repeat(self.indent))
    }

    fn fmt_named_block(&self, header: &str, body: &str) -> String {
        format!(
            "{header}\n{}{}",
            INDENT.repeat(self.indent),
            self.fmt_block(body)
        )
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

    fn bind_scope_pat_idents(&mut self, pat: &Pat) {
        match pat {
            Pat::Ident(pi) => {
                let idx = self.scopes.len() - 1;
                self.scopes[idx]
                    .entry(pi.ident.to_string())
                    .or_insert_with(|| "_".to_string());
                if let Some((_, sub)) = &pi.subpat {
                    self.bind_scope_pat_idents(sub);
                }
            }
            Pat::Tuple(pt) => {
                for e in &pt.elems {
                    self.bind_scope_pat_idents(e);
                }
            }
            Pat::TupleStruct(pts) => {
                for e in &pts.elems {
                    self.bind_scope_pat_idents(e);
                }
            }
            Pat::Struct(ps) => {
                for f in &ps.fields {
                    self.bind_scope_pat_idents(&f.pat);
                }
            }
            Pat::Slice(ps) => {
                for e in &ps.elems {
                    self.bind_scope_pat_idents(e);
                }
            }
            Pat::Reference(pr) => self.bind_scope_pat_idents(&pr.pat),
            Pat::Paren(pp) => self.bind_scope_pat_idents(&pp.pat),
            Pat::Type(pt) => self.bind_scope_pat_idents(&pt.pat),
            Pat::Or(po) => {
                for case in &po.cases {
                    self.bind_scope_pat_idents(case);
                }
            }
            _ => {}
        }
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
                    record_shadowed_std_alias(&s.ident.to_string(), &mut self.shadowed_std_aliases);
                    if s.generics.type_params().next().is_none() {
                        let mut fields = HashMap::new();
                        match &s.fields {
                            syn::Fields::Named(nf) => {
                                for f in &nf.named {
                                    fields.insert(
                                        f.ident.as_ref().unwrap().to_string(),
                                        strip_non_static_lifetimes(&self.fmt_type(&f.ty)),
                                    );
                                }
                            }
                            syn::Fields::Unnamed(uf) => {
                                for (idx, f) in uf.unnamed.iter().enumerate() {
                                    fields.insert(
                                        idx.to_string(),
                                        strip_non_static_lifetimes(&self.fmt_type(&f.ty)),
                                    );
                                }
                            }
                            syn::Fields::Unit => {}
                        }
                        if !fields.is_empty() {
                            self.struct_fields.insert(s.ident.to_string(), fields);
                        }
                    }
                }
                Item::Enum(e) => {
                    record_shadowed_std_alias(&e.ident.to_string(), &mut self.shadowed_std_aliases);
                }
                Item::Type(t) => {
                    record_shadowed_std_alias(&t.ident.to_string(), &mut self.shadowed_std_aliases);
                }
                Item::Union(u) => {
                    record_shadowed_std_alias(&u.ident.to_string(), &mut self.shadowed_std_aliases);
                }
                Item::Trait(t) => {
                    record_shadowed_std_alias(&t.ident.to_string(), &mut self.shadowed_std_aliases);
                    self.in_scope_traits.insert(t.ident.to_string());
                }
                Item::Mod(m) => {
                    self.known_path_roots.insert(m.ident.to_string());
                }
                Item::ExternCrate(ec) => {
                    let key = ec
                        .rename
                        .as_ref()
                        .map(|(_, id)| id.to_string())
                        .unwrap_or_else(|| ec.ident.to_string());
                    self.known_path_roots.insert(key);
                }
                Item::Use(u) => {
                    collect_use_root_ident(&u.tree, &mut self.known_path_roots);
                    collect_use_leaf_idents(&u.tree, false, false, &mut self.in_scope_traits);
                    collect_use_renamed_origs(&u.tree, &mut self.renamed_use_origs);
                    collect_use_shadowed_std_aliases(
                        &u.tree,
                        false,
                        &mut self.shadowed_std_aliases,
                    );
                }
                _ => {}
            }
        }
    }

    fn emit_file(&mut self, file: &syn::File) {
        if let Some(sb) = &file.shebang {
            self.line(sb);
        }
        let mut header = false;
        for attr in &file.attrs {
            let is_comment = attr.path().is_ident("doc");
            if header && !is_comment {
                self.out.push('\n');
            }
            self.emit_attr(attr, true);
            header = is_comment;
        }
        if header && !file.items.is_empty() {
            self.out.push('\n');
        }
        self.emit_items(&file.items);
    }

    fn emit_items(&mut self, items: &[Item]) {
        let mut previous = None;
        for item in items {
            let start = self.out.len();
            let spacing = self.emit_item(item);
            self.separate(&mut previous, spacing, item.span(), start);
        }
    }

    fn emit_fields(&mut self, fields: &syn::FieldsNamed, include_visibility: bool) {
        let mut previous = None;
        for field in &fields.named {
            let start = self.out.len();
            for attr in &field.attrs {
                self.emit_attr(attr, false);
            }
            let vis = if include_visibility {
                fmt_vis(&field.vis)
            } else {
                String::new()
            };
            let name = escape_ident(&field.ident.as_ref().unwrap().to_string());
            let ty = self.fmt_type(&field.ty);
            let spacing = self.field_line(&format!("{}{} {};", vis, ty, name));
            self.separate(&mut previous, spacing, field.span(), start);
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
            let marker = if inner_target.is_some() { "//!" } else { "///" };
            for raw_line in s.split('\n') {
                let raw_line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
                let trimmed = raw_line.strip_prefix(' ').unwrap_or(raw_line);
                if trimmed.is_empty() {
                    self.line(marker);
                } else {
                    self.line(&format!("{} {}", marker, trimmed));
                }
            }
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

    fn emit_item(&mut self, item: &Item) -> BlankLines {
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
                    self.open_block(&format!("{}namespace {}", vis, name));
                    let mut sub = self.child();
                    sub.known_path_roots.clear();
                    sub.in_scope_traits.clear();
                    for root in ["std", "core", "alloc", "crate", "super", "self", "Self"] {
                        sub.known_path_roots.insert(root.to_string());
                    }
                    sub.collect_signatures(items);
                    sub.emit_split_attrs(&m.attrs, true, "module");
                    sub.emit_items(items);
                    self.out.push_str(&sub.out);
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
                        self.open_block(&format!("{}struct {}{}{}", vis, name, gens, wh));
                        self.indent += 1;
                        self.emit_fields(nf, true);
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
                self.open_block(&format!("{}enum {}{}{}", vis, name, gens, wh));
                self.indent += 1;
                let mut previous = None;
                for v in &e.variants {
                    let start = self.out.len();
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
                            self.open_block(&vname);
                            self.indent += 1;
                            self.emit_fields(nf, false);
                            self.indent -= 1;
                            self.line(&format!("}}{},", disc));
                        }
                    }
                    self.separate(&mut previous, BlankLines::Preserve, v.span(), start);
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
                self.open_block(&format!("{}union {}{}{}", vis, name, gens, wh));
                self.indent += 1;
                self.emit_fields(&u.fields, true);
                self.indent -= 1;
                self.line("}");
            }
            Item::Trait(t) => {
                self.emit_split_attrs(&t.attrs, false, "inner");
                let vis = fmt_vis(&t.vis);
                let unsf = if t.unsafety.is_some() { "unsafe " } else { "" };
                let auto_kw = if t.auto_token.is_some() { "auto " } else { "" };
                let name = escape_ident(&t.ident.to_string());
                let gens = self.fmt_generics(&t.generics);
                let supers = if t.supertraits.is_empty() {
                    String::new()
                } else {
                    let bs: Vec<String> = t.supertraits.iter().map(|b| self.fmt_bound(b)).collect();
                    format!(" : {}", bs.join(", "))
                };
                let wh = self.fmt_where(&t.generics);
                self.open_block(&format!(
                    "{}{}{}trait {}{}{}{}",
                    vis, unsf, auto_kw, name, gens, supers, wh
                ));
                self.indent += 1;
                self.emit_split_attrs(&t.attrs, true, "inner");
                let mut previous = None;
                for ti in &t.items {
                    let start = self.out.len();
                    let mut spacing = BlankLines::Preserve;
                    match ti {
                        TraitItem::Fn(tf) => {
                            self.emit_split_attrs(&tf.attrs, false, "inner");
                            let inner_attrs: Vec<Attribute> = tf
                                .attrs
                                .iter()
                                .filter(|a| matches!(a.style, syn::AttrStyle::Inner(_)))
                                .cloned()
                                .collect();
                            spacing = self.emit_fn_sig_and_body(
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
                            spacing = self.field_line(&format!("const {} {}{};", cty, cname, def));
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
                    self.separate(&mut previous, spacing, ti.span(), start);
                }
                self.indent -= 1;
                self.line("}");
            }
            Item::TraitAlias(ta) => {
                for a in &ta.attrs {
                    self.emit_attr(a, false);
                }
                let vis = fmt_vis(&ta.vis);
                let name = escape_ident(&ta.ident.to_string());
                let gens = self.fmt_generics(&ta.generics);
                let bs: Vec<String> = ta.bounds.iter().map(|b| self.fmt_bound(b)).collect();
                let wh = self.fmt_where(&ta.generics);
                self.line(&format!(
                    "{}trait {}{} = {}{};",
                    vis,
                    name,
                    gens,
                    bs.join(" + "),
                    wh
                ));
            }
            Item::Impl(im) => {
                self.emit_split_attrs(&im.attrs, false, "inner");
                let def = if im.defaultness.is_some() {
                    "default "
                } else {
                    ""
                };
                let unsf = if im.unsafety.is_some() { "unsafe " } else { "" };
                let gens = self.fmt_generics(&im.generics);
                let target = self.fmt_type(&im.self_ty);
                let wh = self.fmt_where(&im.generics);
                let header = match &im.trait_ {
                    Some((neg, tp, _)) => {
                        let bang = if neg.is_some() { "!" } else { "" };
                        format!(
                            "{}{}impl{} {}{} for {}{}",
                            def,
                            unsf,
                            gens,
                            bang,
                            self.fmt_path(tp),
                            target,
                            wh
                        )
                    }
                    None => format!("{}{}impl{} {}{}", def, unsf, gens, target, wh),
                };
                self.open_block(&header);
                self.indent += 1;
                self.emit_split_attrs(&im.attrs, true, "inner");
                let mut previous = None;
                for sub in &im.items {
                    let start = self.out.len();
                    let mut spacing = BlankLines::Preserve;
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
                            spacing = self.emit_fn_sig_and_body(
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
                            spacing = self
                                .field_line(&format!("{}const {} {} = {};", cvis, cty, cname, val));
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
                            if let syn::ImplItem::Verbatim(ts) = other
                                && let Ok(tt) = syn::parse2::<syn::TraitItemType>(ts.clone())
                                && tt.bounds.is_empty()
                                && tt.default.is_none()
                            {
                                for a in &tt.attrs {
                                    self.emit_attr(a, false);
                                }
                                let aname = escape_ident(&tt.ident.to_string());
                                let agens = self.fmt_generics(&tt.generics);
                                let awh = self.fmt_where(&tt.generics);
                                self.line(&format!("type {}{}{};", aname, agens, awh));
                            } else {
                                self.line(&format!("rust {{ {} }}", other.to_token_stream()));
                            }
                        }
                    }
                    self.separate(&mut previous, spacing, sub.span(), start);
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
                return self.field_line(&format!("{}const {} {} = {};", vis, ty, name, val));
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
                return self.field_line(&format!(
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
                return self.emit_fn_sig_and_body(
                    &vis,
                    &f.sig,
                    Some(&f.block),
                    &inner_attrs,
                    false,
                    false,
                );
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
                self.open_block(&format!("{}extern{}", unsf, abi));
                self.indent += 1;
                self.emit_split_attrs(&fm.attrs, true, "inner");
                let mut previous = None;
                for fi in &fm.items {
                    let start = self.out.len();
                    let mut spacing = BlankLines::Preserve;
                    match fi {
                        syn::ForeignItem::Fn(ff) => {
                            spacing = self.emit_foreign_fn(ff, false);
                        }
                        syn::ForeignItem::Static(fs) => {
                            self.emit_foreign_static(fs, "");
                        }
                        syn::ForeignItem::Type(ft) => {
                            for a in &ft.attrs {
                                self.emit_attr(a, false);
                            }
                            let tvis = fmt_vis(&ft.vis);
                            let tname = escape_ident(&ft.ident.to_string());
                            self.line(&format!("{}type {};", tvis, tname));
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
                        other => {
                            if !matches!(
                                other,
                                syn::ForeignItem::Verbatim(ts) if self.try_emit_safe_foreign_verbatim(ts)
                            ) {
                                self.line(&format!("rust {{ {} }}", other.to_token_stream()));
                            }
                        }
                    }
                    self.separate(&mut previous, spacing, fi.span(), start);
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
                    return BlankLines::Preserve;
                }
                self.line(&format!("{};", fmt_syn_macro(&im.mac)));
            }
            other => {
                if let Item::Verbatim(ts) = other
                    && let Ok(ft) = syn::parse2::<syn::ForeignItemType>(ts.clone())
                    && ft.generics.params.is_empty()
                    && ft.generics.where_clause.is_none()
                {
                    for a in &ft.attrs {
                        self.emit_attr(a, false);
                    }
                    let fvis = fmt_vis(&ft.vis);
                    let fname = escape_ident(&ft.ident.to_string());
                    self.line(&format!("{}type {};", fvis, fname));
                } else {
                    self.line(&format!("rust {{ {} }}", other.to_token_stream()));
                }
            }
        }
        match item {
            Item::Use(_) => BlankLines::Imports,
            Item::Mod(_)
            | Item::Struct(_)
            | Item::Enum(_)
            | Item::Union(_)
            | Item::Trait(_)
            | Item::TraitAlias(_)
            | Item::Impl(_)
            | Item::Type(_) => BlankLines::Around,
            _ => BlankLines::Preserve,
        }
    }

    fn emit_foreign_fn(&mut self, ff: &syn::ForeignItemFn, is_safe: bool) -> BlankLines {
        for a in &ff.attrs {
            self.emit_attr(a, false);
        }
        let fvis = fmt_vis(&ff.vis);
        self.emit_fn_sig_and_body(&fvis, &ff.sig, None, &[], false, is_safe)
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
    ) -> BlankLines {
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
            BlockTail::UnitReturn
        } else {
            BlockTail::Return
        };

        let spacing = if let Some(blk) = body {
            self.open_block(&header);
            self.indent += 1;
            for a in inner_attrs {
                self.emit_attr_with_target(a, Some("inner"));
            }
            self.emit_block_stmts(blk, tail);
            self.indent -= 1;
            self.line("}");
            BlankLines::Around
        } else {
            self.field_line(&format!("{};", header))
        };

        self.declared_in_fn = prev_declared;
        self.pop_scope();
        spacing
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

    fn fmt_inferred_closure_param(&mut self, pat: &Pat) -> String {
        let attrs: &[Attribute] = match pat {
            Pat::Ident(p) => &p.attrs,
            Pat::Wild(p) => &p.attrs,
            Pat::Tuple(p) => &p.attrs,
            Pat::Struct(p) => &p.attrs,
            Pat::TupleStruct(p) => &p.attrs,
            Pat::Slice(p) => &p.attrs,
            Pat::Reference(p) => &p.attrs,
            _ => &[],
        };
        let mut pfx = String::new();
        for a in attrs {
            pfx.push_str(&self.fmt_attr_inline(a, None));
            pfx.push(' ');
        }
        format!("{}{}", pfx, self.fmt_pat(pat, None))
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

    fn emit_block_stmts(&mut self, block: &Block, tail: BlockTail) {
        self.push_scope();
        let len = block.stmts.len();
        let last_is_cfg_tail = !matches!(tail, BlockTail::None | BlockTail::UnitReturn)
            && block.stmts.last().is_some_and(|s| match s {
                Stmt::Expr(e, None) => has_cfg_attr(expr_outer_attrs(e)),
                Stmt::Macro(sm) => sm.semi_token.is_none() && has_cfg_attr(&sm.attrs),
                _ => false,
            });
        let mut previous = None;
        for (idx, stmt) in block.stmts.iter().enumerate() {
            let is_last = idx + 1 == len;
            let start = self.out.len();
            let mut spacing = BlankLines::Preserve;
            match stmt {
                Stmt::Local(local) => self.emit_local(local, &block.stmts[idx + 1..]),
                Stmt::Item(item) => spacing = self.emit_item(item),
                Stmt::Expr(expr, semi) => {
                    if is_last
                        && tail == BlockTail::Value
                        && let Expr::Return(er) = expr
                        && let Some(ret_val) = &er.expr
                    {
                        for a in &er.attrs {
                            self.emit_attr(a, false);
                        }
                        let s = self.fmt_expr(ret_val);
                        self.line(&format!("return (return {});", s));
                    } else if semi.is_none()
                        && (is_last || (last_is_cfg_tail && has_cfg_attr(expr_outer_attrs(expr))))
                    {
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
                    if sm.semi_token.is_none()
                        && (is_last || (last_is_cfg_tail && has_cfg_attr(&sm.attrs)))
                        && matches!(tail, BlockTail::Return | BlockTail::Value)
                    {
                        self.line(&format!("return {};", ms));
                    } else if matches!(sm.mac.delimiter, syn::MacroDelimiter::Brace(_))
                        && sm.semi_token.is_none()
                    {
                        self.line(&ms);
                    } else {
                        self.line(&format!("{};", ms));
                    }
                }
            }
            if matches!(stmt, Stmt::Local(_) | Stmt::Expr(_, _))
                && self.out[start..].ends_with("}\n")
            {
                spacing = BlankLines::After;
            }
            self.separate(&mut previous, spacing, stmt.span(), start);
        }
        self.pop_scope();
    }

    fn emit_expr_as_block_body(&mut self, expr: &Expr, tail: BlockTail) {
        if let Expr::Block(eb) = expr {
            self.emit_block_stmts(&eb.block, tail);
        } else {
            self.emit_tail(expr, tail);
        }
    }

    fn emit_tail(&mut self, expr: &Expr, tail: BlockTail) {
        match tail {
            BlockTail::None => self.emit_expr_stmt(expr),
            BlockTail::UnitReturn => {
                if matches!(expr, Expr::Call(_) | Expr::MethodCall(_)) {
                    self.emit_tail(expr, BlockTail::Return);
                } else {
                    self.emit_expr_stmt(expr);
                }
            }
            BlockTail::Return => match expr {
                Expr::Return(_) => self.emit_expr_stmt(expr),
                Expr::If(eif) => {
                    for a in &eif.attrs {
                        self.emit_attr(a, false);
                    }
                    self.emit_if(eif, false, BlockTail::Return);
                }
                Expr::Match(em) => {
                    for a in &em.attrs {
                        self.emit_attr(a, false);
                    }
                    self.emit_match_stmt(em, true);
                }
                Expr::Block(eb) if eb.label.is_none() => {
                    for a in &eb.attrs {
                        self.emit_attr(a, false);
                    }
                    self.line("{");
                    self.indent += 1;
                    self.emit_block_stmts(&eb.block, BlockTail::Return);
                    self.indent -= 1;
                    self.line("}");
                }
                other => {
                    for a in expr_outer_attrs(other) {
                        self.emit_attr(a, false);
                    }
                    let s = self.fmt_expr_with_attrs(other, false);
                    self.line(&format!("return {};", s));
                }
            },
            BlockTail::Value => match expr {
                Expr::Return(er) if er.expr.is_none() => self.emit_expr_stmt(expr),
                other => {
                    for a in expr_outer_attrs(other) {
                        self.emit_attr(a, false);
                    }
                    let s = self.fmt_expr_with_attrs(other, false);
                    self.line(&format!("return {};", s));
                }
            },
        }
    }

    fn emit_local(&mut self, local: &Local, following_stmts: &[Stmt]) {
        self.emit_local_inner(local, following_stmts);
        self.bind_scope_pat_idents(&local.pat);
    }

    fn emit_local_inner(&mut self, local: &Local, following_stmts: &[Stmt]) {
        for a in &local.attrs {
            self.emit_attr(a, false);
        }

        // `let Pat = expr else { ... };` -> `require (expr is Pat) else { ... }`
        if let Some(init) = &local.init
            && let Some((_, diverge)) = &init.diverge
        {
            let expr_s = self.fmt_expr(&init.expr);
            let pat_s = if let Pat::Type(pt) = &local.pat {
                format!(
                    "{} : {}",
                    self.fmt_pat(&pt.pat, None),
                    self.fmt_type(&pt.ty)
                )
            } else {
                self.fmt_pat(&local.pat, None)
            };
            self.open_block(&format!("require ({} is {}) else", expr_s, pat_s));
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
            Pat::Type(pt) => (
                pt.pat.as_ref(),
                (!matches!(pt.ty.as_ref(), syn::Type::Infer(_))).then_some(pt.ty.as_ref()),
            ),
            other => (other, None),
        };

        if matches!(inner_pat, Pat::Ident(pi) if pi.subpat.is_some())
            || matches!(inner_pat, Pat::Macro(_))
        {
            let pat_s = self.fmt_pat(inner_pat, None);
            let ty_s = explicit_ty
                .map(|t| format!(" : {}", self.fmt_type(t)))
                .unwrap_or_default();
            let init_s = local
                .init
                .as_ref()
                .map(|i| format!(" = {}", self.fmt_expr(&i.expr)))
                .unwrap_or_default();
            self.line(&format!("var {}{}{};", pat_s, ty_s, init_s));
            return;
        }

        // Explicit outer type annotation on any destructuring `let`:
        // preserve `pat : Type = expr;` so Rust unsize/slice coercions (e.g. `(&str, &[&str])`)
        // apply identically to the RHS expression.
        if let Some(ty) = explicit_ty
            && matches!(
                inner_pat,
                Pat::Tuple(_)
                    | Pat::Slice(_)
                    | Pat::Struct(_)
                    | Pat::TupleStruct(_)
                    | Pat::Reference(_)
            )
            && let Some(init) = &local.init
        {
            let pat_s = self.fmt_pat(inner_pat, None);
            let ty_s = self.fmt_type(ty);
            let rhs = self.fmt_expr(&init.expr);
            self.line(&format!("{} : {} = {};", pat_s, ty_s, rhs));
            return;
        }

        // Struct / Slice destructuring `let Point { x, y } = pt;` / `let [a, b] = arr;`
        if let Pat::Slice(ps) = inner_pat {
            let Some(init) = &local.init else {
                self.record_err(RSharpError::from_rust("Destructuring without initializer"));
                return;
            };
            if let HintLookup::Resolved(slice_s) = self.try_fmt_pat_with_hints(inner_pat) {
                let rhs = self.fmt_expr(&init.expr);
                self.line(&format!("{} = {};", slice_s, rhs));
                return;
            }
            let elem_ty = self
                .infer_expr_type(&init.expr)
                .and_then(|t| extract_collection_elem(&t));
            if elem_ty.is_none() && !self.strict_unnameable {
                let pat_s = self.fmt_pat(inner_pat, None);
                let rhs = self.fmt_expr(&init.expr);
                self.line(&format!("var {} = {};", pat_s, rhs));
                return;
            }
            let elems: Vec<_> = ps
                .elems
                .iter()
                .map(|p| self.fmt_pat(p, elem_ty.as_deref()))
                .collect();
            let rhs = self.fmt_expr(&init.expr);
            self.line(&format!("[{}] = {};", elems.join(", "), rhs));
            return;
        }
        if let Pat::Struct(ps) = inner_pat {
            let Some(init) = &local.init else {
                self.record_err(RSharpError::from_rust("Destructuring without initializer"));
                return;
            };
            if let HintLookup::Resolved(struct_s) = self.try_fmt_pat_with_hints(inner_pat) {
                let rhs = self.fmt_expr(&init.expr);
                self.line(&format!("{} = {};", struct_s, rhs));
                return;
            }
            let struct_key = ps
                .path
                .segments
                .last()
                .map(|s| s.ident.to_string())
                .unwrap_or_default();
            let known_fields = self.struct_fields.get(&struct_key).cloned();
            let mut fs = Vec::new();
            let mut any_typed = false;
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
                    if fty.is_some() {
                        any_typed = true;
                    }
                    let sub_s = self.fmt_pat(&f.pat, fty);
                    fs.push(format!("{}{}: {}", apfx, fname, sub_s));
                } else if let Some(ty_s) = fty {
                    any_typed = true;
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
                } else if let Pat::Ident(pi) = f.pat.as_ref()
                    && pi.mutability.is_some()
                {
                    fs.push(format!("{}mut {}", apfx, fname));
                } else {
                    fs.push(format!("{}{}", apfx, fname));
                }
            }
            if !any_typed && !self.strict_unnameable {
                let pat_s = self.fmt_pat(inner_pat, None);
                let rhs = self.fmt_expr(&init.expr);
                self.line(&format!("var {} = {};", pat_s, rhs));
                return;
            }
            if ps.rest.is_some() {
                fs.push("..".to_string());
            }
            let rhs = self.fmt_expr(&init.expr);
            let path_s = self.fmt_pat_qself_or_path(&ps.qself, &ps.path);
            self.line(&format!("{} {{ {} }} = {};", path_s, fs.join(", "), rhs));
            return;
        }

        // Tuple-struct destructuring `let W(v) = w;`
        if let Pat::TupleStruct(pts) = inner_pat {
            let Some(init) = &local.init else {
                self.record_err(RSharpError::from_rust("Destructuring without initializer"));
                return;
            };
            if let HintLookup::Resolved(ts_s) = self.try_fmt_pat_with_hints(inner_pat) {
                let rhs = self.fmt_expr(&init.expr);
                self.line(&format!("{} = {};", ts_s, rhs));
                return;
            }
            let struct_key = pts
                .path
                .segments
                .last()
                .map(|s| s.ident.to_string())
                .unwrap_or_default();
            let known_fields = self.struct_fields.get(&struct_key).cloned();
            if known_fields.is_none() && !self.strict_unnameable {
                let pat_s = self.fmt_pat(inner_pat, None);
                let rhs = self.fmt_expr(&init.expr);
                self.line(&format!("var {} = {};", pat_s, rhs));
                return;
            }
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
            if let HintLookup::Resolved(ref_s) = self.try_fmt_pat_with_hints(inner_pat) {
                let rhs = self.fmt_expr(&init.expr);
                self.line(&format!("{} = {};", ref_s, rhs));
                return;
            }
            let inner_ty = self.infer_expr_type(&init.expr).and_then(|t| {
                t.strip_prefix("read ")
                    .or_else(|| t.strip_prefix("write "))
                    .map(|s| s.to_string())
            });
            if inner_ty.is_none() && !self.strict_unnameable {
                let pat_s = self.fmt_pat(inner_pat, None);
                let rhs = self.fmt_expr(&init.expr);
                self.line(&format!("var {} = {};", pat_s, rhs));
                return;
            }
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
            if explicit_ty.is_none() {
                match &local.init {
                    None => {
                        let mut resolved = Vec::new();
                        let mut all_ok = !ptup.elems.is_empty();
                        for elem in &ptup.elems {
                            if let Pat::Ident(pi) = elem
                                && let HintLookup::Resolved(ty) =
                                    self.lookup_hint_at_span(pi.ident.span())
                            {
                                resolved.push((pi.mutability.is_some(), pi.ident.to_string(), ty));
                            } else {
                                all_ok = false;
                                break;
                            }
                        }
                        if all_ok {
                            for (is_mut, name, ty_s) in resolved {
                                let is_shadow = self.declared_in_fn.contains(&name);
                                let sh_kw = if is_shadow { "shadow " } else { "" };
                                let mut_kw = if is_mut { "mut " } else { "" };
                                self.bind_var(&name, ty_s.clone());
                                self.line(&format!(
                                    "{}{}{} {};",
                                    sh_kw,
                                    mut_kw,
                                    ty_s,
                                    escape_ident(&name)
                                ));
                            }
                            return;
                        }
                        if !self.strict_unnameable {
                            let pat_s = self.fmt_pat(inner_pat, None);
                            self.line(&format!("var {};", pat_s));
                            return;
                        }
                    }
                    Some(init) => match self.try_fmt_pat_with_hints(inner_pat) {
                        HintLookup::Resolved(tuple_s) => {
                            let rhs = self.fmt_expr(&init.expr);
                            self.line(&format!("{} = {};", tuple_s, rhs));
                            return;
                        }
                        HintLookup::Unnameable if !self.strict_unnameable => {
                            let pat_s = self.fmt_pat(inner_pat, None);
                            let rhs = self.fmt_expr(&init.expr);
                            self.line(&format!("var {} = {};", pat_s, rhs));
                            return;
                        }
                        _ => {}
                    },
                }
            }
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
                            if !self.strict_unnameable {
                                let pat_s = self.fmt_pat(inner_pat, None);
                                let rhs = self.fmt_expr(&init.expr);
                                self.line(&format!("var {} = {};", pat_s, rhs));
                                return;
                            }
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
                        if !self.strict_unnameable {
                            let pat_s = self.fmt_pat(inner_pat, None);
                            let rhs = self.fmt_expr(&init.expr);
                            self.line(&format!("var {} = {};", pat_s, rhs));
                            return;
                        }
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
        } else {
            let hint_res = match unwrapped_pat {
                Pat::Ident(pi) => self.lookup_hint_at_span_opts(pi.ident.span(), true),
                _ => HintLookup::None,
            };
            match hint_res {
                HintLookup::Resolved(t) => t,
                HintLookup::Unnameable => {
                    if !self.strict_unnameable {
                        "var".to_string()
                    } else {
                        self.record_err(RSharpError::unresolved_type(
                            &name,
                            "Binding has an unnameable Rust type",
                        ));
                        return;
                    }
                }
                HintLookup::None => {
                    if let Some(init) = &local.init {
                        if !self.type_hints.is_empty()
                            && !self.strict_unnameable
                            && is_unsuffixed_num_lit(&init.expr)
                        {
                            "var".to_string()
                        } else if let Some(inferred) = self.infer_expr_type(&init.expr) {
                            inferred
                        } else if self.options.use_rustc_inference
                            && !self.rustc_probe_aborted.get()
                        {
                            match self.probe_type_via_rustc(unwrapped_pat, &name, false) {
                                Ok(t) => t,
                                Err(e) => {
                                    self.rustc_probe_aborted.set(true);
                                    self.record_err(e);
                                    return;
                                }
                            }
                        } else if !self.strict_unnameable {
                            "var".to_string()
                        } else {
                            self.record_err(RSharpError::unresolved_type(
                                &name,
                                "Local inference could not determine type and rustc probing is disabled",
                            ));
                            return;
                        }
                    } else if let Some(inferred) =
                        self.infer_deferred_local_type(&name, following_stmts)
                    {
                        inferred
                    } else if !self.strict_unnameable {
                        "var".to_string()
                    } else {
                        self.record_err(RSharpError::unresolved_type(
                            &name,
                            "Unannotated variable declaration has no initializer",
                        ));
                        return;
                    }
                }
            }
        };

        let init_s = match &local.init {
            Some(init) => format!(" = {}", self.fmt_expr(&init.expr)),
            None => String::new(),
        };
        let is_var = ty_s == "var";
        let bound_ty = if is_var {
            "_".to_string()
        } else {
            ty_s.clone()
        };
        if by_ref {
            let mode = if is_mut { "write" } else { "read" };
            if is_var {
                self.bind_var(&name, "_".to_string());
                self.line(&format!(
                    "var borrow {} {}{};",
                    mode,
                    escape_ident(&name),
                    init_s
                ));
            } else {
                self.bind_var(&name, format!("{} {}", mode, ty_s));
                self.line(&format!(
                    "borrow {} {} {}{};",
                    mode,
                    ty_s,
                    escape_ident(&name),
                    init_s
                ));
            }
            return;
        }
        let is_shadow = self.declared_in_fn.contains(&name);
        let sh_kw = if is_shadow { "shadow " } else { "" };
        let mut_kw = if is_mut { "mut " } else { "" };
        self.bind_var(&name, bound_ty);
        if is_paren {
            if is_var {
                self.line(&format!(
                    "var ({}{}){};",
                    mut_kw,
                    escape_ident(&name),
                    init_s
                ));
            } else {
                self.line(&format!(
                    "({}{} {}){};",
                    mut_kw,
                    ty_s,
                    escape_ident(&name),
                    init_s
                ));
            }
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

    fn probe_type_via_rustc(&self, pat: &Pat, name: &str, closure: bool) -> Result<String> {
        let rust_ty = self.probes.infer(pat, name, closure)?;
        let parsed_ty: Type = syn::parse_str(&rust_ty).map_err(|_| {
            let kind = if closure {
                "closure param type"
            } else {
                "type"
            };
            RSharpError::unresolved_type(
                name,
                format!("rustc reported unparseable {kind} '{rust_ty}'"),
            )
        })?;
        Ok(strip_non_static_lifetimes(&self.fmt_type(&parsed_ty)))
    }

    fn emit_expr_stmt(&mut self, expr: &Expr) {
        if matches!(
            expr,
            Expr::If(_)
                | Expr::While(_)
                | Expr::ForLoop(_)
                | Expr::Loop(_)
                | Expr::Match(_)
                | Expr::Unsafe(_)
                | Expr::Return(_)
                | Expr::Break(_)
                | Expr::Continue(_)
        ) {
            for a in expr_outer_attrs(expr) {
                self.emit_attr(a, false);
            }
        }
        match expr {
            Expr::If(eif) => self.emit_if(eif, false, BlockTail::None),
            Expr::While(ew) => {
                let lbl = ew
                    .label
                    .as_ref()
                    .map(|l| format!("{}: ", escape_lifetime(&l.name)))
                    .unwrap_or_default();
                let cond = self.fmt_condition(&ew.cond);
                self.open_block(&format!("{}while ({})", lbl, cond));
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
                let pat = self.fmt_for_loop_pat(&ef.pat, &ef.expr);
                let iter = self.fmt_expr(&ef.expr);
                self.open_block(&format!("{}foreach ({} in {})", lbl, pat, iter));
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
                self.open_block(&format!("{}loop", lbl));
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
                self.open_block("unsafe");
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
                    line.push_str(&self.fmt_break_value(eb.label.is_none(), e));
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

    fn emit_if(&mut self, eif: &syn::ExprIf, is_else_if: bool, tail: BlockTail) {
        let kw = if is_else_if { "else if" } else { "if" };
        let cond = self.fmt_condition(&eif.cond);
        self.open_block(&format!("{} ({})", kw, cond));
        self.indent += 1;
        self.emit_block_stmts(&eif.then_branch, tail);
        self.indent -= 1;
        self.line("}");
        if let Some((_, else_expr)) = &eif.else_branch {
            if let Expr::If(next) = else_expr.as_ref() {
                self.emit_if(next, true, tail);
            } else {
                self.open_block("else");
                self.indent += 1;
                self.emit_expr_as_block_body(else_expr, tail);
                self.indent -= 1;
                self.line("}");
            }
        }
    }

    fn emit_match_stmt(&mut self, em: &syn::ExprMatch, tail_returns: bool) {
        let scr = self.fmt_expr(&em.expr);
        self.open_block(&format!("match ({})", scr));
        self.indent += 1;
        let block_tail = if tail_returns {
            BlockTail::Return
        } else {
            BlockTail::None
        };
        let mut previous = None;
        for arm in &em.arms {
            let start = self.out.len();
            for a in &arm.attrs {
                self.emit_attr(a, false);
            }
            let pat = self.fmt_pat(&arm.pat, None);
            self.push_scope();
            self.bind_scope_pat_idents(&arm.pat);
            let guard = match &arm.guard {
                Some((_, ge)) => format!(" when {}", self.fmt_condition(ge)),
                None => String::new(),
            };
            if tail_returns || matches!(arm.body.as_ref(), Expr::Block(_)) {
                self.open_block(&format!("{}{} =>", pat, guard));
                self.indent += 1;
                self.emit_expr_as_block_body(&arm.body, block_tail);
                self.indent -= 1;
                self.line("}");
            } else {
                let s = self.fmt_expr(&arm.body);
                self.line(&format!("{}{} => {},", pat, guard, s));
            }
            self.pop_scope();
            self.separate(&mut previous, BlankLines::Preserve, arm.span(), start);
        }
        self.indent -= 1;
        self.line("}");
    }

    fn fmt_condition(&mut self, expr: &Expr) -> String {
        match expr {
            Expr::Let(el) => {
                let raw_rhs = self.fmt_expr(&el.expr);
                let rhs = if matches!(
                    el.expr.as_ref(),
                    Expr::If(_) | Expr::Binary(_) | Expr::Closure(_) | Expr::Range(_)
                ) {
                    format!("({})", raw_rhs)
                } else {
                    raw_rhs
                };
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
                let elem_s = self.fmt_type(cur);
                if type_needs_postfix_parens(cur) {
                    format!("({})[{}]", elem_s, dims.join(", "))
                } else {
                    format!("{}[{}]", elem_s, dims.join(", "))
                }
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
                if let Some(inner) = extract_option_inner(&tp.path)
                    && !type_needs_postfix_parens(inner)
                {
                    return format!("{}?", self.fmt_type(inner));
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
            return self.fmt_block(&sub.out);
        }
        sub.fmt_expr(expr)
    }

    fn fmt_pat_qself_or_path(&self, qself: &Option<syn::QSelf>, path: &Path) -> String {
        match qself {
            Some(qs) => self.fmt_qself_path(qs, path),
            None => self.fmt_pat_path(path),
        }
    }

    fn fmt_pat_path(&self, path: &Path) -> String {
        // Pattern paths (variants/structs like `String(s)`) must not map `String` -> `string`.
        if path.leading_colon.is_none()
            && path.segments.len() == 1
            && path.segments[0].arguments.is_empty()
        {
            return escape_ident(&path.segments[0].ident.to_string());
        }
        self.fmt_path(path)
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

    fn fmt_expr_path(&self, path: &Path) -> String {
        let formatted = self.fmt_path(path);
        if path.leading_colon.is_none()
            && path.segments.len() > 1
            && let Some(first) = path.segments.first()
        {
            let first_raw = first.ident.to_string();
            let first_clean = first_raw.strip_prefix("r#").unwrap_or(&first_raw);
            let has_all_caps_prefix = path
                .segments
                .iter()
                .take(path.segments.len() - 1)
                .any(|s| is_all_caps_ident(&s.ident.to_string()));
            if has_all_caps_prefix
                || first_clean == "self"
                || (first_clean.chars().next().is_some_and(|c| c.is_lowercase())
                    && (!self.known_path_roots.contains(first_clean)
                        || (first_clean != "crate"
                            && first_clean != "super"
                            && self.lookup_var(first_clean).is_some())))
            {
                return self.fmt_path_with_sep(path, "::");
            }
        }
        formatted
    }

    fn fmt_path(&self, path: &Path) -> String {
        self.fmt_path_with_sep(path, ".")
    }

    fn fmt_path_with_sep(&self, path: &Path, sep: &str) -> String {
        if path.leading_colon.is_none()
            && path.segments.len() == 1
            && path.segments[0].arguments.is_empty()
        {
            let name = path.segments[0].ident.to_string();
            if let Some(mapped) = map_rust_primitive(&name) {
                return mapped.to_string();
            }
        }
        let is_std_root = path.segments.first().is_some_and(|s| {
            matches!(
                s.ident.to_string().as_str(),
                "std" | "core" | "alloc" | "rsharp_prelude"
            )
        });
        let seg_count = path.segments.len();
        let mut out = Vec::new();
        for (idx, seg) in path.segments.iter().enumerate() {
            let raw_name = seg.ident.to_string();
            let name = if idx == 0 && raw_name == "self" {
                "this".to_string()
            } else if path.leading_colon.is_none()
                && (seg_count == 1 || (idx + 1 == seg_count && is_std_root))
            {
                let mapped = map_rust_std_alias(&raw_name);
                if seg_count == 1 && self.shadowed_std_aliases.contains(&mapped) {
                    escape_ident(&raw_name)
                } else {
                    escape_ident(&mapped)
                }
            } else {
                escape_ident(&raw_name)
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
        let joined = out.join(sep);
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
                    let hinted_ty = match self.lookup_hint_at_span(pi.ident.span()) {
                        HintLookup::Resolved(t) => Some(if pi.by_ref.is_some() {
                            strip_leading_borrow_prefix(&t, pi.mutability.is_some()).to_string()
                        } else {
                            t
                        }),
                        _ => None,
                    };
                    let inferred_ty = hinted_ty
                        .or_else(|| ty_hint.filter(|t| *t != "_").map(str::to_string))
                        .or_else(|| infer_pat_literal_type(sub).map(str::to_string))
                        .unwrap_or_else(|| "_".to_string());
                    if pi.by_ref.is_some() {
                        let mode = if pi.mutability.is_some() {
                            "write"
                        } else {
                            "read"
                        };
                        return format!("{} as borrow {} {} {}", sub_s, mode, inferred_ty, name);
                    }
                    let mk = if pi.mutability.is_some() { "mut " } else { "" };
                    return format!("{} as {}{} {}", sub_s, mk, inferred_ty, name);
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
                if let Some(ty) = ty_hint.filter(|t| *t != "_") {
                    self.bind_var(&pi.ident.to_string(), ty.to_string());
                    format!("{}{} {}", mk, ty, name)
                } else {
                    format!("{}{}", mk, name)
                }
            }
            Pat::Tuple(pt) => {
                let elem_tys = ty_hint.and_then(split_tuple_type_str);
                let ps: Vec<_> = pt
                    .elems
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        let hint = elem_tys.as_ref().and_then(|v| v.get(i)).map(String::as_str);
                        self.fmt_pat(p, hint)
                    })
                    .collect();
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
                    } else if let Pat::Ident(pi) = f.pat.as_ref()
                        && pi.mutability.is_some()
                    {
                        fs.push(format!("{}mut {}", apfx, fname));
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
        self.fmt_expr_with_attrs(expr, true)
    }

    fn fmt_expr_with_attrs(&mut self, expr: &Expr, include_attrs: bool) -> String {
        let mut attr_pfx = String::new();
        if include_attrs {
            for a in expr_outer_attrs(expr) {
                attr_pfx.push_str(&self.fmt_attr_inline(a, None));
                attr_pfx.push(' ');
            }
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
                    self.fmt_expr_path(&ep.path)
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
                    self.fmt_named_block("const", &sub.out)
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
                let callee = self.fmt_receiver_expr(&ec.func);
                let args: Vec<_> = ec.args.iter().map(|a| self.fmt_expr(a)).collect();
                format!("{}({})", callee, args.join(", "))
            }
            Expr::MethodCall(mc) => {
                let recv = self.fmt_member_receiver(&mc.receiver);
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
                let base = self.fmt_member_receiver(&ef.base);
                match &ef.member {
                    Member::Named(id) => format!("{}.{}", base, escape_ident(&id.to_string())),
                    Member::Unnamed(idx) => format!("{}.{}", base, idx.index),
                }
            }
            Expr::Index(ei) => {
                format!(
                    "{}[{}]",
                    self.fmt_receiver_expr(&ei.expr),
                    self.fmt_expr(&ei.index)
                )
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
                    let struct_path = self.fmt_path(&es.path);
                    let use_with = es.qself.is_none()
                        && es.path.segments.iter().all(|s| s.arguments.is_empty())
                        && matches!(
                            rest.as_ref(),
                            Expr::Path(ep) if ep
                                .path
                                .get_ident()
                                .is_some_and(|id| self.lookup_var(&id.to_string()) == Some(struct_path.as_str()))
                        );
                    let rest_s = self.fmt_expr(rest);
                    if use_with {
                        if fs.is_empty() {
                            format!("{} with {{}}", rest_s)
                        } else {
                            format!("{} with {{ {} }}", rest_s, fs.join(", "))
                        }
                    } else if fs.is_empty() {
                        format!("new {} {{ ..{} }}", struct_path, rest_s)
                    } else {
                        format!("new {} {{ {}, ..{} }}", struct_path, fs.join(", "), rest_s)
                    }
                } else {
                    format!("new {} {{ {} }}", self.fmt_path(&es.path), fs.join(", "))
                }
            }
            Expr::If(eif) => {
                if let Some(inline_if) = self.try_fmt_inline_if(eif, true) {
                    inline_if
                } else {
                    let mut sub = self.child();
                    sub.indent = self.indent;
                    sub.emit_if(eif, false, BlockTail::Value);
                    sub.out.trim_start().trim_end().to_string()
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
                self.fmt_named_block(&format!("{}loop", lbl), &sub.out)
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
                self.fmt_named_block(&format!("{}while ({})", lbl, cond), &sub.out)
            }
            Expr::ForLoop(ef) => {
                let lbl = ef
                    .label
                    .as_ref()
                    .map(|l| format!("{}: ", escape_lifetime(&l.name)))
                    .unwrap_or_default();
                let pat = self.fmt_for_loop_pat(&ef.pat, &ef.expr);
                let iter = self.fmt_expr(&ef.expr);
                let mut sub = self.child();
                sub.emit_block_stmts(&ef.body, BlockTail::None);
                self.fmt_named_block(&format!("{}foreach ({} in {})", lbl, pat, iter), &sub.out)
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
                    out.push_str(&self.fmt_break_value(eb.label.is_none(), e));
                }
                out
            }
            Expr::Continue(ec) => match &ec.label {
                Some(l) => format!("continue {}", escape_lifetime(l)),
                None => "continue".to_string(),
            },
            Expr::Match(em) => {
                let scr = self.fmt_expr(&em.expr);
                let mut arms = String::new();
                self.indent += 1;
                let indent = INDENT.repeat(self.indent);
                for arm in &em.arms {
                    let mut pfx = String::new();
                    for a in &arm.attrs {
                        pfx.push_str(&self.fmt_attr_inline(a, None));
                        pfx.push(' ');
                    }
                    let pat = self.fmt_pat(&arm.pat, None);
                    self.push_scope();
                    self.bind_scope_pat_idents(&arm.pat);
                    let g = match &arm.guard {
                        Some((_, ge)) => format!(" when {}", self.fmt_condition(ge)),
                        None => String::new(),
                    };
                    let header = format!("{}{}{} =>", pfx, pat, g);
                    let arm = if let Expr::Block(eb) = arm.body.as_ref()
                        && eb.label.is_none()
                        && eb.attrs.is_empty()
                        && single_tail_expr(&eb.block).is_none()
                    {
                        let has_val_tail = matches!(
                            eb.block.stmts.last(),
                            Some(Stmt::Expr(e, None)) if !matches!(e, Expr::Return(_))
                        ) || matches!(
                            eb.block.stmts.last(),
                            Some(Stmt::Macro(sm)) if sm.semi_token.is_none()
                        );
                        if has_val_tail {
                            format!("{} ({})", header, self.fmt_expr(&arm.body))
                        } else {
                            let mut sub = self.child();
                            sub.emit_block_stmts(&eb.block, BlockTail::None);
                            self.fmt_named_block(&header, &sub.out)
                        }
                    } else {
                        format!("{} {}", header, self.fmt_expr(&arm.body))
                    };
                    self.pop_scope();
                    arms.push_str(&format!("{indent}{arm},\n"));
                }
                self.indent -= 1;
                self.fmt_named_block(&format!("match ({})", scr), &arms)
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
                self.push_scope();
                let mut ps = Vec::new();
                for inp in &ec.inputs {
                    if let Pat::Type(pt) = inp {
                        ps.push(self.fmt_typed_fn_param(&pt.attrs, &pt.pat, &pt.ty));
                        self.bind_scope_pat_idents(&pt.pat);
                    } else {
                        match self.try_fmt_pat_with_hints(inp) {
                            HintLookup::Resolved(hinted)
                                if self.strict_unnameable || !hint_str_has_elided_ref(&hinted) =>
                            {
                                ps.push(hinted);
                                self.bind_scope_pat_idents(inp);
                                continue;
                            }
                            HintLookup::Resolved(_) => {
                                ps.push(self.fmt_inferred_closure_param(inp));
                                self.bind_scope_pat_idents(inp);
                                continue;
                            }
                            HintLookup::Unnameable if !self.strict_unnameable => {
                                ps.push(self.fmt_inferred_closure_param(inp));
                                self.bind_scope_pat_idents(inp);
                                continue;
                            }
                            HintLookup::Unnameable => {
                                let (_, pname) = extract_param_pat(inp);
                                self.record_err(RSharpError::unresolved_type(
                                    pname,
                                    "Closure parameter has an unnameable Rust type",
                                ));
                                continue;
                            }
                            HintLookup::None => {}
                        }
                        let (is_mut, pname) = extract_param_pat(inp);
                        if self.options.use_rustc_inference && !self.rustc_probe_aborted.get() {
                            match self.probe_type_via_rustc(inp, &pname, true) {
                                Ok(inferred) => {
                                    let mk = if is_mut { "mut " } else { "" };
                                    ps.push(format!("{}{} {}", mk, inferred, escape_ident(&pname)));
                                    self.bind_var(&pname, inferred);
                                }
                                Err(e) => {
                                    self.rustc_probe_aborted.set(true);
                                    self.record_err(e);
                                }
                            }
                        } else if !self.strict_unnameable {
                            ps.push(self.fmt_inferred_closure_param(inp));
                            self.bind_scope_pat_idents(inp);
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
                let res = match ec.body.as_ref() {
                    Expr::Block(eb) => {
                        let mut sub = self.child();
                        let has_value_tail = matches!(
                            eb.block.stmts.last(),
                            Some(Stmt::Expr(_, None))
                                | Some(Stmt::Macro(syn::StmtMacro {
                                    semi_token: None,
                                    ..
                                }))
                        );
                        let tail = if !matches!(ec.output, ReturnType::Default) || has_value_tail {
                            BlockTail::Return
                        } else {
                            BlockTail::None
                        };
                        sub.emit_block_stmts(&eb.block, tail);
                        self.fmt_named_block(
                            &format!("{}({}){} =>", prefix, ps.join(", "), ret),
                            &sub.out,
                        )
                    }
                    other => format!(
                        "{}({}){} => {}",
                        prefix,
                        ps.join(", "),
                        ret,
                        self.fmt_expr(other)
                    ),
                };
                self.pop_scope();
                res
            }
            Expr::Block(eb) => {
                if let Some(lbl) = &eb.label {
                    let mut sub = self.child();
                    sub.emit_block_stmts(&eb.block, BlockTail::None);
                    self.fmt_named_block(&format!("{}:", escape_lifetime(&lbl.name)), &sub.out)
                } else if let Some(single) = single_tail_expr(&eb.block) {
                    if matches!(single, Expr::Field(_)) {
                        format!("{{ return {}; }}", self.fmt_expr(single))
                    } else {
                        self.fmt_expr(single)
                    }
                } else {
                    let mut sub = self.child();
                    sub.emit_block_stmts(&eb.block, BlockTail::Value);
                    self.fmt_block(&sub.out)
                }
            }
            Expr::Async(ea) => {
                let header = if ea.capture.is_some() {
                    "async take"
                } else {
                    "async"
                };
                let mut sub = self.child();
                sub.emit_block_stmts(&ea.block, BlockTail::Return);
                self.fmt_named_block(header, &sub.out)
            }
            Expr::Unsafe(eu) => {
                let mut sub = self.child();
                sub.emit_block_stmts(&eu.block, BlockTail::Value);
                self.fmt_named_block("unsafe", &sub.out)
            }
            Expr::TryBlock(etb) => {
                let mut sub = self.child();
                sub.emit_block_stmts(&etb.block, BlockTail::Value);
                self.fmt_named_block("try", &sub.out)
            }
            Expr::Yield(ey) => match &ey.expr {
                Some(e) => format!("yield {}", self.fmt_expr(e)),
                None => "yield ()".to_string(),
            },
            Expr::Let(el) => {
                format!(
                    "({} is {})",
                    self.fmt_expr(&el.expr),
                    self.fmt_pat(&el.pat, None)
                )
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
                    let rest = inner
                        .strip_prefix("read ")
                        .or_else(|| inner.strip_prefix("write "))?;
                    Some(
                        rest.strip_prefix("ptr<")
                            .and_then(|s| s.strip_suffix('>'))
                            .unwrap_or(rest)
                            .to_string(),
                    )
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
                _ => {
                    let l = self.infer_expr_type(&eb.left);
                    let r = self.infer_expr_type(&eb.right);
                    match (l, r) {
                        (Some(lt), Some(rt)) => {
                            if is_unsuffixed_num_lit(&eb.left) && !is_unsuffixed_num_lit(&eb.right)
                            {
                                Some(rt)
                            } else {
                                Some(lt)
                            }
                        }
                        (Some(lt), None) if !is_unsuffixed_num_lit(&eb.left) => Some(lt),
                        (None, Some(rt)) if !is_unsuffixed_num_lit(&eb.right) => Some(rt),
                        _ => None,
                    }
                }
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
                Some(format!("{}[{}]", elem_ty, self.fmt_const_expr(&er.len)))
            }
            Expr::Path(ep) => {
                if let Some(id) = ep.path.get_ident() {
                    self.lookup_var(&id.to_string())
                        .filter(|s| *s != "_")
                        .map(|s| s.to_string())
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
                        if inner.starts_with("read")
                            || inner.starts_with("write")
                            || inner.starts_with("dyn ")
                            || inner.starts_with("impl ")
                            || inner.starts_with("unsafe ")
                            || inner.starts_with("extern ")
                            || inner.starts_with("forall<")
                            || inner.contains(" -> ")
                        {
                            return Some(format!("Option<{}>", inner));
                        }
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

    fn infer_deferred_local_type(&self, var_name: &str, following: &[Stmt]) -> Option<String> {
        for stmt in following {
            if let Stmt::Expr(expr, _) = stmt
                && let Some(ty) = self.find_assigned_type_in_expr(var_name, expr)
            {
                return Some(ty);
            }
        }
        None
    }

    fn find_assigned_type_in_expr(&self, var_name: &str, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Assign(ea) => {
                if let Expr::Path(ep) = ea.left.as_ref()
                    && ep.path.is_ident(var_name)
                {
                    return self.infer_expr_type(&ea.right);
                }
                None
            }
            Expr::If(eif) => self
                .infer_deferred_local_type(var_name, &eif.then_branch.stmts)
                .or_else(|| {
                    eif.else_branch
                        .as_ref()
                        .and_then(|(_, e)| self.find_assigned_type_in_expr(var_name, e))
                }),
            Expr::Block(eb) => self.infer_deferred_local_type(var_name, &eb.block.stmts),
            Expr::Match(em) => em
                .arms
                .iter()
                .find_map(|a| self.find_assigned_type_in_expr(var_name, &a.body)),
            _ => None,
        }
    }

    fn fmt_break_value(&mut self, unlabeled: bool, expr: &Expr) -> String {
        self.fmt_break_value_with_attrs(unlabeled, expr, true)
    }

    fn fmt_break_value_with_attrs(
        &mut self,
        unlabeled: bool,
        expr: &Expr,
        include_attrs: bool,
    ) -> String {
        let s = self.fmt_expr_with_attrs(expr, include_attrs);
        // `break ident;` is parsed as a labeled break in R#; parenthesize bare-ident values.
        // `break lbl -10;` or `break lbl [a];` would parse `-`/`[` as continuing `lbl`.
        if (unlabeled
            && matches!(expr, Expr::Path(ep) if ep.qself.is_none() && ep.path.get_ident().is_some()))
            || (!unlabeled
                && s.starts_with([
                    '-', '[', '.', '*', '+', '/', '%', '&', '|', '^', '<', '>', ':', '!', '?',
                ]))
        {
            format!("({})", s)
        } else {
            s
        }
    }

    fn fmt_member_receiver(&mut self, expr: &Expr) -> String {
        if let Expr::Path(ep) = expr
            && ep.qself.is_none()
            && !ep.path.is_ident("self")
            && (ep.path.segments.len() >= 2
                || ep.path.segments.first().is_some_and(|seg| {
                    seg.ident
                        .to_string()
                        .chars()
                        .next()
                        .is_some_and(char::is_uppercase)
                }))
        {
            return format!("({})", self.fmt_expr(expr));
        }
        self.fmt_receiver_expr(expr)
    }

    fn fmt_receiver_expr(&mut self, expr: &Expr) -> String {
        let s = self.fmt_expr(expr);
        if matches!(
            expr,
            Expr::Try(_)
                | Expr::Await(_)
                | Expr::Unary(_)
                | Expr::Reference(_)
                | Expr::RawAddr(_)
                | Expr::Range(_)
                | Expr::Assign(_)
                | Expr::Closure(_)
                | Expr::If(_)
                | Expr::Return(_)
                | Expr::Break(_)
        ) {
            format!("({})", s)
        } else {
            s
        }
    }

    fn fmt_for_loop_pat(&mut self, pat: &Pat, iter: &Expr) -> String {
        if !self.type_hints.is_empty() {
            // Only emit explicit types on `for` bindings when rust-analyzer produced verified
            // qualified paths (e.g., primitives); unqualified display hints can reference types
            // not imported in the current module.
            if let Pat::Ident(pi) = pat
                && pi.subpat.is_none()
                && pi.by_ref.is_none()
                && let HintLookup::Resolved(ty) =
                    self.lookup_hint_at_span_opts(pi.ident.span(), true)
            {
                return self.fmt_pat(pat, Some(&ty));
            }
            return self.fmt_pat(pat, None);
        }
        let elem_hint = self.infer_iter_elem_type(iter);
        self.fmt_pat(pat, elem_hint.as_deref())
    }

    fn infer_iter_elem_type(&self, iter: &Expr) -> Option<String> {
        match iter {
            Expr::Range(er) => {
                let start_unsuffixed = er.start.as_deref().is_some_and(|e| {
                    matches!(
                        e,
                        Expr::Lit(syn::ExprLit {
                            lit: syn::Lit::Int(li),
                            ..
                        }) if li.suffix().is_empty()
                    )
                });
                if start_unsuffixed && let Some(end) = er.end.as_deref() {
                    return self.infer_expr_type(end);
                }
                er.start
                    .as_ref()
                    .and_then(|e| self.infer_expr_type(e))
                    .or_else(|| er.end.as_ref().and_then(|e| self.infer_expr_type(e)))
            }
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
                    .filter(|s| *s != "_")
                {
                    return Some(format!("read {}", inner));
                }
                if let Some(inner) = base
                    .strip_prefix("write slice<")
                    .and_then(|s| s.strip_suffix('>'))
                    .filter(|s| *s != "_")
                {
                    return Some(format!("write {}", inner));
                }
                extract_collection_elem(&base)
            }
        }
    }
}

fn type_needs_postfix_parens(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Reference(_)
            | Type::Ptr(_)
            | Type::TraitObject(_)
            | Type::ImplTrait(_)
            | Type::BareFn(_)
    )
}

fn is_all_caps_ident(raw: &str) -> bool {
    let name = raw.strip_prefix("r#").unwrap_or(raw);
    name.len() >= 2
        && name.bytes().any(|b| b.is_ascii_uppercase())
        && name
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

fn infer_pat_literal_type(pat: &Pat) -> Option<&'static str> {
    match pat {
        Pat::Lit(pl) => match &pl.lit {
            syn::Lit::Byte(_) => Some("byte"),
            syn::Lit::Char(_) => Some("char"),
            syn::Lit::Str(_) => Some("read str"),
            syn::Lit::Bool(_) => Some("bool"),
            syn::Lit::Int(li) => Some(map_rust_primitive(li.suffix()).unwrap_or("int")),
            syn::Lit::Float(lf) => Some(map_rust_primitive(lf.suffix()).unwrap_or("double")),
            _ => None,
        },
        Pat::Range(pr) => pr
            .start
            .as_deref()
            .or(pr.end.as_deref())
            .and_then(|e| match e {
                Expr::Lit(el) => match &el.lit {
                    syn::Lit::Byte(_) => Some("byte"),
                    syn::Lit::Char(_) => Some("char"),
                    syn::Lit::Int(li) => Some(map_rust_primitive(li.suffix()).unwrap_or("int")),
                    _ => None,
                },
                _ => None,
            }),
        Pat::Paren(pp) => infer_pat_literal_type(&pp.pat),
        Pat::Or(po) => po.cases.iter().find_map(infer_pat_literal_type),
        _ => None,
    }
}

fn any_type_node(ty: &Type, pred: &mut dyn FnMut(&Type) -> bool) -> bool {
    if pred(ty) {
        return true;
    }
    any_child_type(ty, pred)
}

fn any_child_type(ty: &Type, pred: &mut dyn FnMut(&Type) -> bool) -> bool {
    match ty {
        Type::Reference(r) => any_type_node(&r.elem, pred),
        Type::Ptr(p) => any_type_node(&p.elem, pred),
        Type::Slice(s) => any_type_node(&s.elem, pred),
        Type::Array(a) => any_type_node(&a.elem, pred),
        Type::Paren(p) => any_type_node(&p.elem, pred),
        Type::Tuple(t) => t.elems.iter().any(|e| any_type_node(e, pred)),
        Type::Path(tp) => {
            tp.qself
                .as_ref()
                .is_some_and(|q| any_type_node(&q.ty, pred))
                || path_args_any_type(&tp.path, pred)
        }
        Type::TraitObject(to) => to.bounds.iter().any(|b| match b {
            TypeParamBound::Trait(tb) => path_args_any_type(&tb.path, pred),
            _ => false,
        }),
        Type::ImplTrait(it) => it.bounds.iter().any(|b| match b {
            TypeParamBound::Trait(tb) => path_args_any_type(&tb.path, pred),
            _ => false,
        }),
        Type::BareFn(bf) => {
            bf.inputs.iter().any(|i| any_type_node(&i.ty, pred))
                || matches!(&bf.output, ReturnType::Type(_, r) if any_type_node(r, pred))
        }
        _ => false,
    }
}

fn path_args_any_type(path: &Path, pred: &mut dyn FnMut(&Type) -> bool) -> bool {
    path.segments.iter().any(|s| match &s.arguments {
        PathArguments::AngleBracketed(ab) => ab.args.iter().any(|a| match a {
            GenericArgument::Type(t) => any_type_node(t, pred),
            GenericArgument::AssocType(at) => any_type_node(&at.ty, pred),
            _ => false,
        }),
        PathArguments::Parenthesized(p) => {
            p.inputs.iter().any(|t| any_type_node(t, pred))
                || matches!(&p.output, ReturnType::Type(_, r) if any_type_node(r, pred))
        }
        PathArguments::None => false,
    })
}

fn path_has_non_static_lifetime(path: &Path) -> bool {
    path.segments.iter().any(|s| match &s.arguments {
        PathArguments::AngleBracketed(ab) => ab
            .args
            .iter()
            .any(|a| matches!(a, GenericArgument::Lifetime(l) if l.ident != "static")),
        _ => false,
    })
}

fn impl_trait_returns_elided_ref(ty: &Type) -> bool {
    matches!(ty, Type::ImplTrait(_))
        && any_type_node(ty, &mut |t| {
            match t {
            Type::Reference(r) => !matches!(&r.lifetime, Some(l) if l.ident == "static"),
            Type::Path(tp) => path_has_non_static_lifetime(&tp.path),
            Type::ImplTrait(it) => it.bounds.iter().any(|b| {
                matches!(b, TypeParamBound::Trait(tb) if path_has_non_static_lifetime(&tb.path))
            }),
            _ => false,
        }
        })
}

fn has_unqualified_qself_trait(ty: &Type) -> bool {
    any_type_node(ty, &mut |t| {
        matches!(t, Type::Path(tp) if tp.qself.as_ref().is_some_and(|q| {
            q.position == 1
                && tp.path.leading_colon.is_none()
                && tp.path.segments.first().is_some_and(|s| {
                    !is_rust_prelude_or_primitive_type(&s.ident.to_string())
                })
        }))
    })
}

fn contains_nested_bare_fn(ty: &Type) -> bool {
    any_child_type(ty, &mut |t| matches!(t, Type::BareFn(_)))
}

fn bare_fn_has_elided_ref(bf: &syn::TypeBareFn) -> bool {
    if bf.lifetimes.is_some() {
        return false;
    }
    let has_elided = |ty: &Type| {
        any_type_node(
            ty,
            &mut |t| matches!(t, Type::Reference(r) if r.lifetime.is_none()),
        )
    };
    if bf.inputs.iter().any(|arg| has_elided(&arg.ty)) {
        return true;
    }
    let syn::ReturnType::Type(_, ret) = &bf.output else {
        return false;
    };
    has_elided(ret)
}

/// Returns true when a formatted R# closure parameter hint contains an elided-lifetime reference
/// (`read T` / `write T` without `<static>`), which would force a rigid higher-ranked lifetime
/// in Rust and break return-lifetime unification (e.g. `let f = |x: &str| x;`).
fn hint_str_has_elided_ref(hinted: &str) -> bool {
    for kw in ["read ", "write "] {
        let mut rest = hinted;
        while let Some(idx) = rest.find(kw) {
            let boundary_ok = idx == 0
                || !rest[..idx]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
            if boundary_ok {
                return true;
            }
            rest = &rest[idx + kw.len()..];
        }
    }
    false
}

fn contains_unimported_impl_future(ty: &Type, in_scope_traits: &HashSet<String>) -> bool {
    let Type::ImplTrait(it) = ty else {
        return false;
    };
    it.bounds.iter().any(|b| {
        let TypeParamBound::Trait(tb) = b else {
            return false;
        };
        tb.path.leading_colon.is_none()
            && tb.path.segments.len() == 1
            && tb.path.segments.first().is_some_and(|seg| {
                let name = seg.ident.to_string();
                matches!(
                    name.as_str(),
                    "Future" | "IntoFuture" | "AsyncFn" | "AsyncFnMut" | "AsyncFnOnce"
                ) && !in_scope_traits.contains(&name)
            })
    })
}

fn is_rust_prelude_or_primitive_type(name: &str) -> bool {
    matches!(
        name,
        "bool"
            | "char"
            | "str"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "f32"
            | "f64"
            | "Self"
            | "Option"
            | "String"
            | "Vec"
            | "Box"
            | "Copy"
            | "Clone"
            | "Send"
            | "Sync"
            | "Sized"
            | "Unpin"
            | "Drop"
            | "Fn"
            | "FnMut"
            | "FnOnce"
            | "Iterator"
            | "IntoIterator"
            | "ExactSizeIterator"
            | "DoubleEndedIterator"
    )
}

fn is_unqualified_non_prelude_path(path: &Path) -> bool {
    path.leading_colon.is_none()
        && path.segments.len() == 1
        && path
            .segments
            .first()
            .is_some_and(|seg| !is_rust_prelude_or_primitive_type(&seg.ident.to_string()))
}

fn is_unsuffixed_num_lit(expr: &Expr) -> bool {
    match expr {
        Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Int(li),
            ..
        }) => li.suffix().is_empty(),
        Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Float(lf),
            ..
        }) => lf.suffix().is_empty(),
        Expr::Paren(ep) => is_unsuffixed_num_lit(&ep.expr),
        _ => false,
    }
}

fn contains_ref_to_static_dyn(ty: &Type) -> bool {
    any_type_node(ty, &mut |t| {
        let Type::Reference(tr) = t else {
            return false;
        };
        let mut inner = tr.elem.as_ref();
        while let Type::Paren(tp) = inner {
            inner = tp.elem.as_ref();
        }
        matches!(
            inner,
            Type::TraitObject(to)
                if to.bounds.iter().any(|b| {
                    matches!(b, TypeParamBound::Lifetime(lt) if lt.ident == "static")
                })
        )
    })
}

fn contains_unimported_dyn_trait(
    ty: &Type,
    in_scope: &HashSet<String>,
    renamed_origs: &HashSet<String>,
) -> bool {
    any_type_node(ty, &mut |t| {
        matches!(t, Type::TraitObject(to) if to.bounds.iter().any(|b| {
            matches!(b, TypeParamBound::Trait(tb)
                if tb.path.leading_colon.is_none()
                    && tb.path.segments.len() == 1
                    && tb.path.segments.first().is_some_and(|seg| {
                        let name = seg.ident.to_string();
                        renamed_origs.contains(&name)
                            || (!is_rust_prelude_or_primitive_type(&name)
                                && !in_scope.contains(&name))
                    }))
        }))
    })
}

fn all_unverified_paths_in_scope(ty: &Type) -> bool {
    !any_type_node(ty, &mut |t| match t {
        Type::Path(tp) => is_unqualified_non_prelude_path(&tp.path),
        Type::TraitObject(to) => to.bounds.iter().any(
            |b| matches!(b, TypeParamBound::Trait(tb) if is_unqualified_non_prelude_path(&tb.path)),
        ),
        Type::ImplTrait(it) => it.bounds.iter().any(
            |b| matches!(b, TypeParamBound::Trait(tb) if is_unqualified_non_prelude_path(&tb.path)),
        ),
        Type::Infer(_) | Type::Macro(_) | Type::Verbatim(_) => true,
        _ => false,
    })
}

fn impl_trait_contains_dyn(ty: &Type) -> bool {
    matches!(ty, Type::ImplTrait(_))
        && any_child_type(ty, &mut |t| matches!(t, Type::TraitObject(_)))
}

/// Rewrites Rust 1.99+ `sync::atomic::Atomic<T>` (unstable `generic_atomic`) returned by
/// `rust-analyzer` back to the stable `AtomicBool`/`AtomicUsize`/`AtomicPtr<T>` aliases.
fn normalize_unstable_stdlib_type(ty: &mut Type) {
    match ty {
        Type::Reference(r) => normalize_unstable_stdlib_type(&mut r.elem),
        Type::Ptr(p) => normalize_unstable_stdlib_type(&mut p.elem),
        Type::Slice(s) => normalize_unstable_stdlib_type(&mut s.elem),
        Type::Array(a) => normalize_unstable_stdlib_type(&mut a.elem),
        Type::Paren(p) => normalize_unstable_stdlib_type(&mut p.elem),
        Type::Tuple(t) => {
            for e in &mut t.elems {
                normalize_unstable_stdlib_type(e);
            }
        }
        Type::Path(tp) => {
            if let Some(q) = &mut tp.qself {
                normalize_unstable_stdlib_type(&mut q.ty);
            }
            for seg in &mut tp.path.segments {
                if let PathArguments::AngleBracketed(ab) = &mut seg.arguments {
                    let has_non_static_lt = ab.args.iter().any(
                        |a| matches!(a, GenericArgument::Lifetime(lt) if lt.ident != "static"),
                    );
                    for arg in &mut ab.args {
                        match arg {
                            GenericArgument::Lifetime(lt)
                                if has_non_static_lt && lt.ident == "static" =>
                            {
                                lt.ident = syn::Ident::new("_", lt.ident.span());
                            }
                            GenericArgument::Type(t) => normalize_unstable_stdlib_type(t),
                            GenericArgument::AssocType(at) => {
                                normalize_unstable_stdlib_type(&mut at.ty)
                            }
                            _ => {}
                        }
                    }
                }
            }
            if let Some(last) = tp.path.segments.last_mut()
                && last.ident == "Atomic"
                && let PathArguments::AngleBracketed(ab) = &last.arguments
                && ab.args.len() == 1
                && let GenericArgument::Type(inner) = &ab.args[0]
            {
                let stable_name = match inner {
                    Type::Path(itp) if itp.qself.is_none() && itp.path.segments.len() == 1 => {
                        match itp.path.segments[0].ident.to_string().as_str() {
                            "bool" => Some(("AtomicBool", None)),
                            "i8" => Some(("AtomicI8", None)),
                            "i16" => Some(("AtomicI16", None)),
                            "i32" => Some(("AtomicI32", None)),
                            "i64" => Some(("AtomicI64", None)),
                            "isize" => Some(("AtomicIsize", None)),
                            "u8" => Some(("AtomicU8", None)),
                            "u16" => Some(("AtomicU16", None)),
                            "u32" => Some(("AtomicU32", None)),
                            "u64" => Some(("AtomicU64", None)),
                            "usize" => Some(("AtomicUsize", None)),
                            _ => None,
                        }
                    }
                    Type::Ptr(ptr) if ptr.mutability.is_some() => {
                        Some(("AtomicPtr", Some(ptr.elem.as_ref().clone())))
                    }
                    _ => None,
                };
                if let Some((alias, ptr_inner)) = stable_name {
                    last.ident = syn::Ident::new(alias, last.ident.span());
                    if let Some(elem) = ptr_inner {
                        let mut args = syn::punctuated::Punctuated::new();
                        args.push(GenericArgument::Type(elem));
                        last.arguments =
                            PathArguments::AngleBracketed(syn::AngleBracketedGenericArguments {
                                colon2_token: None,
                                lt_token: Default::default(),
                                args,
                                gt_token: Default::default(),
                            });
                    } else {
                        last.arguments = PathArguments::None;
                    }
                }
            }
        }
        _ => {}
    }
}

fn contains_nested_impl_trait(ty: &Type) -> bool {
    any_child_type(ty, &mut |t| matches!(t, Type::ImplTrait(_)))
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
            if raw.starts_with("br")
                && let Ok(s) = std::str::from_utf8(&lbs.value())
            {
                format_rsharp_raw_str(s, "u8")
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

fn collect_use_root_ident(tree: &syn::UseTree, roots: &mut HashSet<String>) {
    match tree {
        syn::UseTree::Path(p) => {
            roots.insert(p.ident.to_string());
        }
        syn::UseTree::Name(n) => {
            roots.insert(n.ident.to_string());
        }
        syn::UseTree::Rename(r) => {
            roots.insert(r.rename.to_string());
        }
        syn::UseTree::Group(g) => {
            for item in &g.items {
                collect_use_root_ident(item, roots);
            }
        }
        syn::UseTree::Glob(_) => {}
    }
}

fn collect_use_leaf_idents(
    tree: &syn::UseTree,
    root_is_std_core: bool,
    in_std_error_mod: bool,
    leaves: &mut HashSet<String>,
) {
    match tree {
        syn::UseTree::Path(p) => {
            let seg = p.ident.to_string();
            let is_std_core = root_is_std_core || matches!(seg.as_str(), "std" | "core");
            let is_err_mod = root_is_std_core && seg == "error";
            collect_use_leaf_idents(&p.tree, is_std_core, is_err_mod, leaves);
        }
        syn::UseTree::Name(n) => {
            let name = n.ident.to_string();
            if name != "Error" || in_std_error_mod {
                leaves.insert(name);
            }
        }
        syn::UseTree::Rename(r) => {
            leaves.insert(r.rename.to_string());
        }
        syn::UseTree::Group(g) => {
            for item in &g.items {
                collect_use_leaf_idents(item, root_is_std_core, in_std_error_mod, leaves);
            }
        }
        syn::UseTree::Glob(_) => {}
    }
}

fn record_shadowed_std_alias(name: &str, out: &mut HashSet<String>) {
    if matches!(
        name,
        "List" | "Queue" | "Dictionary" | "SortedDictionary" | "SortedSet" | "PriorityQueue"
    ) {
        out.insert(name.to_string());
    }
}

fn collect_use_shadowed_std_aliases(tree: &syn::UseTree, in_std: bool, out: &mut HashSet<String>) {
    match tree {
        syn::UseTree::Path(p) => {
            let std_here = in_std
                || matches!(
                    p.ident.to_string().as_str(),
                    "std" | "core" | "alloc" | "rsharp_prelude"
                );
            collect_use_shadowed_std_aliases(&p.tree, std_here, out);
        }
        syn::UseTree::Name(n) if !in_std => {
            record_shadowed_std_alias(&n.ident.to_string(), out);
        }
        syn::UseTree::Rename(r) => {
            record_shadowed_std_alias(&r.rename.to_string(), out);
        }
        syn::UseTree::Group(g) => {
            for item in &g.items {
                collect_use_shadowed_std_aliases(item, in_std, out);
            }
        }
        _ => {}
    }
}

fn collect_use_renamed_origs(tree: &syn::UseTree, renamed: &mut HashSet<String>) {
    match tree {
        syn::UseTree::Path(p) => collect_use_renamed_origs(&p.tree, renamed),
        syn::UseTree::Rename(r) => {
            if r.ident != r.rename {
                renamed.insert(r.ident.to_string());
            }
        }
        syn::UseTree::Group(g) => {
            for item in &g.items {
                collect_use_renamed_origs(item, renamed);
            }
        }
        syn::UseTree::Name(_) | syn::UseTree::Glob(_) => {}
    }
}

fn fmt_use_tree(tree: &syn::UseTree) -> String {
    fmt_use_tree_with_prefix(tree, Vec::new(), false)
}

fn fmt_use_tree_with_prefix(tree: &syn::UseTree, mut prefix: Vec<String>, in_std: bool) -> String {
    match tree {
        syn::UseTree::Path(p) => {
            let raw = p.ident.to_string();
            let std_here = in_std
                || (prefix.is_empty()
                    && matches!(raw.as_str(), "std" | "core" | "alloc" | "rsharp_prelude"));
            let seg = if p.ident == "self" {
                "this".to_string()
            } else {
                escape_ident(&raw)
            };
            prefix.push(seg);
            fmt_use_tree_with_prefix(&p.tree, prefix, std_here)
        }
        syn::UseTree::Name(n) => {
            let seg = if n.ident == "self" {
                "this".to_string()
            } else if in_std {
                escape_ident(&map_rust_std_alias(&n.ident.to_string()))
            } else {
                escape_ident(&n.ident.to_string())
            };
            prefix.push(seg);
            prefix.join(".")
        }
        syn::UseTree::Rename(r) => {
            let seg = if r.ident == "self" {
                "this".to_string()
            } else if in_std {
                escape_ident(&map_rust_std_alias(&r.ident.to_string()))
            } else {
                escape_ident(&r.ident.to_string())
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
                .map(|sub| fmt_use_tree_with_prefix(sub, Vec::new(), in_std))
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

fn strip_leading_borrow_prefix(ty: &str, is_mut: bool) -> &str {
    let kw = if is_mut { "write" } else { "read" };
    if let Some(rest) = ty.strip_prefix(kw) {
        if let Some(after_sp) = rest.strip_prefix(' ') {
            return after_sp;
        }
        if let Some((_, tail)) = rest.strip_prefix('<').and_then(|s| s.split_once("> ")) {
            return tail;
        }
    }
    ty
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
    if let Some(inner) = ty
        .strip_prefix("List<")
        .and_then(|s| s.strip_suffix('>'))
        .filter(|s| *s != "_")
    {
        return Some(inner.to_string());
    }
    if let Some((elem, _)) = ty.strip_suffix(']').and_then(|s| s.rsplit_once('['))
        && elem != "_"
    {
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

fn has_cfg_attr(attrs: &[Attribute]) -> bool {
    attrs
        .iter()
        .any(|a| a.path().is_ident("cfg") || a.path().is_ident("cfg_attr"))
}

fn is_boxed_any(path: &Path) -> bool {
    let last = path.segments.last().expect("non-empty path");
    if last.ident != "Box" {
        return false;
    }
    if let PathArguments::AngleBracketed(ab) = &last.arguments
        && let Some(GenericArgument::Type(Type::TraitObject(to))) = ab.args.first()
        && to.bounds.len() == 1
    {
        return to.bounds.iter().any(|b| {
            matches!(
                b,
                TypeParamBound::Trait(tb) if tb.path.segments.last().is_some_and(|s| s.ident == "Any")
            )
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
    if stripped == "_" || stripped == "Self" {
        return stripped.to_string();
    }
    if was_raw || stripped == "global" || Keyword::parse(stripped).is_some() {
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
