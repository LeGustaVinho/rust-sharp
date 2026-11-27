use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use proc_macro2::{Delimiter, TokenStream, TokenTree};
use syn::punctuated::Punctuated;
use syn::visit::{self, Visit};
use syn::{Expr, ExprLit, ItemMacro, ItemMod, Lit, Meta, Token};

/// Resolves all compiled Rust module files reachable from a Cargo target root
/// (`lib.rs`, `main.rs`, `build.rs`, `tests/*.rs`, `benches/*.rs`, `examples/*.rs`)
/// by following Rust's `mod` / `#[path]` / `#[cfg_attr(..., path = ...)]` graph.
pub(crate) fn collect_target_module_graph(target_src: &Path, discovered: &mut BTreeSet<PathBuf>) {
    let mut state = GraphState {
        discovered,
        walked_includes: HashSet::new(),
        mod_macros: HashSet::new(),
    };
    state.visit_module_file(target_src, true);
}

struct GraphState<'a> {
    discovered: &'a mut BTreeSet<PathBuf>,
    walked_includes: HashSet<PathBuf>,
    mod_macros: HashSet<String>,
}

impl GraphState<'_> {
    fn visit_module_file(&mut self, file: &Path, parent_is_base: bool) {
        let Ok(canon) = std::fs::canonicalize(file) else {
            return;
        };
        if !canon.is_file() || !self.discovered.insert(canon.clone()) {
            return;
        }
        let file_dir = canon.parent().unwrap();
        let base_dir = if parent_is_base {
            file_dir.to_path_buf()
        } else {
            file_dir.join(canon.file_stem().unwrap())
        };
        let src = std::fs::read_to_string(&canon).unwrap_or_default();
        self.scan_source(&src, file_dir, &[base_dir]);
    }

    fn walk_included_file(&mut self, file_dir: &Path, base_dirs: &[PathBuf], rel: &str) {
        let candidate = file_dir.join(rel);
        let Ok(canon) = std::fs::canonicalize(&candidate) else {
            return;
        };
        if !canon.is_file() || !self.walked_includes.insert(canon.clone()) {
            return;
        }
        let inc_dir = canon.parent().unwrap();
        let src = std::fs::read_to_string(&canon).unwrap_or_default();
        self.scan_source(&src, inc_dir, base_dirs);
    }

    fn scan_source(&mut self, src: &str, file_dir: &Path, base_dirs: &[PathBuf]) {
        if let Ok(parsed) = syn::parse_file(src) {
            let mut visitor = ModVisitor {
                state: self,
                file_dir: file_dir.to_path_buf(),
                base_dirs: base_dirs.to_vec(),
            };
            visitor.visit_file(&parsed);
        } else if let Ok(ts) = src.parse::<TokenStream>() {
            self.scan_token_stream(&ts, file_dir, base_dirs);
        }
    }

    fn resolve_out_of_line_mod(
        &mut self,
        base_dirs: &[PathBuf],
        mod_name: &str,
        explicit_paths: &[String],
    ) {
        if !explicit_paths.is_empty() {
            for base in base_dirs {
                for rel in explicit_paths {
                    self.visit_module_file(&base.join(rel), true);
                }
            }
            return;
        }
        let stem = unescape_raw_ident(mod_name);
        for base in base_dirs {
            let file_cand = base.join(format!("{stem}.rs"));
            if file_cand.is_file() {
                self.visit_module_file(&file_cand, false);
            }
            let dir_cand = base.join(stem).join("mod.rs");
            if dir_cand.is_file() {
                self.visit_module_file(&dir_cand, true);
            }
        }
    }

    fn resolve_mod_macro_args(&mut self, tokens: &TokenStream, base_dirs: &[PathBuf]) {
        for tt in tokens.clone() {
            match tt {
                TokenTree::Ident(id) => {
                    let name = id.to_string();
                    self.resolve_out_of_line_mod(base_dirs, &name, &[]);
                }
                TokenTree::Group(g) => self.resolve_mod_macro_args(&g.stream(), base_dirs),
                _ => {}
            }
        }
    }

    fn scan_token_stream(&mut self, tokens: &TokenStream, file_dir: &Path, base_dirs: &[PathBuf]) {
        let tts: Vec<TokenTree> = tokens.clone().into_iter().collect();
        let mut pending_paths: Vec<String> = Vec::new();
        let mut i = 0usize;
        while i < tts.len() {
            match &tts[i] {
                TokenTree::Punct(p) if p.as_char() == '#' => {
                    if let Some(TokenTree::Group(g)) = tts.get(i + 1)
                        && g.delimiter() == Delimiter::Bracket
                    {
                        if let Ok(meta) = syn::parse2::<Meta>(g.stream()) {
                            collect_meta_paths(&meta, &mut pending_paths);
                        }
                        i += 2;
                        continue;
                    }
                    pending_paths.clear();
                    i += 1;
                }
                TokenTree::Ident(id) if id == "pub" || id == "unsafe" => {
                    i += 1;
                    if let Some(TokenTree::Group(g)) = tts.get(i)
                        && g.delimiter() == Delimiter::Parenthesis
                    {
                        i += 1;
                    }
                }
                TokenTree::Ident(id) if id == "mod" => {
                    let preceded_by_dollar =
                        i > 0 && matches!(&tts[i - 1], TokenTree::Punct(p) if p.as_char() == '$');
                    if !preceded_by_dollar && let Some(TokenTree::Ident(mod_id)) = tts.get(i + 1) {
                        let mod_name = mod_id.to_string();
                        match tts.get(i + 2) {
                            Some(TokenTree::Punct(p)) if p.as_char() == ';' => {
                                let paths = std::mem::take(&mut pending_paths);
                                self.resolve_out_of_line_mod(base_dirs, &mod_name, &paths);
                                i += 3;
                                continue;
                            }
                            Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Brace => {
                                let paths = std::mem::take(&mut pending_paths);
                                let child_dirs = child_base_dirs(base_dirs, &mod_name, &paths);
                                self.scan_token_stream(&g.stream(), file_dir, &child_dirs);
                                i += 3;
                                continue;
                            }
                            _ => {}
                        }
                    }
                    pending_paths.clear();
                    i += 1;
                }
                TokenTree::Ident(id) if id == "include" => {
                    if matches!(tts.get(i + 1), Some(TokenTree::Punct(p)) if p.as_char() == '!')
                        && let Some(TokenTree::Group(g)) = tts.get(i + 2)
                    {
                        if let Some(rel) = extract_single_str_lit(&g.stream()) {
                            self.walk_included_file(file_dir, base_dirs, &rel);
                        }
                        pending_paths.clear();
                        i += 3;
                        continue;
                    }
                    pending_paths.clear();
                    i += 1;
                }
                TokenTree::Group(g) => {
                    pending_paths.clear();
                    self.scan_token_stream(&g.stream(), file_dir, base_dirs);
                    i += 1;
                }
                _ => {
                    pending_paths.clear();
                    i += 1;
                }
            }
        }
    }
}

struct ModVisitor<'a, 'b> {
    state: &'a mut GraphState<'b>,
    file_dir: PathBuf,
    base_dirs: Vec<PathBuf>,
}

impl<'ast> Visit<'ast> for ModVisitor<'_, '_> {
    fn visit_item_mod(&mut self, node: &'ast ItemMod) {
        let mut explicit_paths = Vec::new();
        for attr in &node.attrs {
            collect_meta_paths(&attr.meta, &mut explicit_paths);
        }
        let mod_name = node.ident.to_string();
        if let Some((_, items)) = &node.content {
            let child_dirs = child_base_dirs(&self.base_dirs, &mod_name, &explicit_paths);
            let prev = std::mem::replace(&mut self.base_dirs, child_dirs);
            for item in items {
                self.visit_item(item);
            }
            self.base_dirs = prev;
        } else {
            self.state
                .resolve_out_of_line_mod(&self.base_dirs, &mod_name, &explicit_paths);
        }
    }

    fn visit_item_macro(&mut self, node: &'ast ItemMacro) {
        if node.mac.path.is_ident("macro_rules")
            && let Some(id) = &node.ident
            && tokens_define_mod_macro(&node.mac.tokens)
        {
            self.state.mod_macros.insert(id.to_string());
        }
        self.visit_macro(&node.mac);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        let name = mac
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default();
        if name == "include"
            && let Some(rel) = extract_single_str_lit(&mac.tokens)
        {
            self.state
                .walk_included_file(&self.file_dir, &self.base_dirs, &rel);
            return;
        }
        if self.state.mod_macros.contains(&name) {
            self.state
                .resolve_mod_macro_args(&mac.tokens, &self.base_dirs);
        }
        self.state
            .scan_token_stream(&mac.tokens, &self.file_dir, &self.base_dirs);
        visit::visit_macro(self, mac);
    }
}

fn unescape_raw_ident(ident: &str) -> &str {
    ident.strip_prefix("r#").unwrap_or(ident)
}

fn child_base_dirs(
    base_dirs: &[PathBuf],
    mod_name: &str,
    explicit_paths: &[String],
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if !explicit_paths.is_empty() {
        for base in base_dirs {
            for rel in explicit_paths {
                out.push(base.join(rel));
            }
        }
    } else {
        let stem = unescape_raw_ident(mod_name);
        for base in base_dirs {
            out.push(base.join(stem));
        }
    }
    out
}

fn collect_meta_paths(meta: &Meta, out: &mut Vec<String>) {
    match meta {
        Meta::NameValue(nv) if nv.path.is_ident("path") => {
            if let Expr::Lit(ExprLit {
                lit: Lit::Str(s), ..
            }) = &nv.value
            {
                out.push(s.value());
            }
        }
        Meta::List(ml) if ml.path.is_ident("cfg_attr") => {
            if let Ok(nested) = ml.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
            {
                for item in nested.iter().skip(1) {
                    collect_meta_paths(item, out);
                }
            }
        }
        _ => {}
    }
}

fn extract_single_str_lit(tokens: &TokenStream) -> Option<String> {
    syn::parse2::<syn::LitStr>(tokens.clone())
        .ok()
        .map(|s| s.value())
}

fn tokens_define_mod_macro(tokens: &TokenStream) -> bool {
    let tts: Vec<TokenTree> = tokens.clone().into_iter().collect();
    for (i, tt) in tts.iter().enumerate() {
        match tt {
            TokenTree::Ident(id) if id == "mod" => {
                if matches!(tts.get(i + 1), Some(TokenTree::Punct(p)) if p.as_char() == '$') {
                    return true;
                }
            }
            TokenTree::Group(g) if tokens_define_mod_macro(&g.stream()) => {
                return true;
            }
            _ => {}
        }
    }
    false
}
