use ra_ap_syntax::ast::{self, AstNode, HasModuleItem};
use ra_ap_syntax::{Edition, NodeOrToken, SourceFile, SyntaxKind, SyntaxNode, SyntaxToken};

use crate::error::{RSharpError, Result};
use crate::from_rust::{FromRustOptions, convert_parsed_file};
use crate::ra_oracle::TypeHintMap;

/// Fallback Rust -> R# converter powered by `rust-analyzer`'s lossless `ra_ap_syntax` CST.
/// Invoked only when `syn::parse_file` fails (e.g. Rust nightly syntax such as `gen fn`,
/// `gen { ... }`, `const trait`, `impl const`, or postfix `.match { ... }`).
pub(crate) fn convert_nightly_from_rust(
    rust_source: &str,
    options: FromRustOptions,
    hints: &TypeHintMap,
    strict_unnameable: bool,
    syn_err: syn::Error,
) -> Result<String> {
    let parse = SourceFile::parse(rust_source, Edition::Edition2024);
    if !parse.errors().is_empty() {
        return Err(RSharpError::from_rust(format!(
            "Invalid Rust syntax: {syn_err}"
        )));
    }
    let file = parse.tree();
    let mut normalized = String::with_capacity(rust_source.len() + 64);
    render_node(file.syntax(), &mut normalized);
    if let Ok(parsed) = syn::parse_file(&normalized) {
        return convert_parsed_file(&normalized, &parsed, options, hints, strict_unnameable);
    }

    // Per-item lossless fallback for future nightly items accepted by `ra_ap_syntax`
    // that still exceed `syn` 2.0 after normalization.
    let mut out = String::new();
    for item in file.items() {
        let mut item_norm = String::new();
        render_node(item.syntax(), &mut item_norm);
        if let Ok(parsed_item) = syn::parse_file(&item_norm) {
            let rsh =
                convert_parsed_file(&item_norm, &parsed_item, options, hints, strict_unnameable)?;
            out.push_str(&rsh);
        } else {
            out.push_str(&format!("rust {{ {} }}\n", item.syntax().text()));
        }
    }
    Ok(out)
}

fn render_node(node: &SyntaxNode, out: &mut String) {
    if let Some(tr) = ast::Trait::cast(node.clone())
        && let Some(const_tok) = direct_token(tr.syntax(), SyntaxKind::CONST_KW)
    {
        out.push_str("#[__rsharp_const_trait] ");
        render_children_skipping(tr.syntax(), &[&const_tok], out);
        return;
    }

    if let Some(im) = ast::Impl::cast(node.clone())
        && let Some(const_tok) = im.const_token()
    {
        out.push_str("#[__rsharp_const_impl] ");
        render_children_skipping(im.syntax(), &[&const_tok], out);
        return;
    }

    if let Some(f) = ast::Fn::cast(node.clone())
        && let Some(gen_tok) = f.gen_token()
    {
        out.push_str("#[__rsharp_gen_fn] ");
        render_children_skipping(f.syntax(), &[&gen_tok], out);
        return;
    }

    if let Some(blk) = ast::BlockExpr::cast(node.clone())
        && let Some(gen_tok) = blk.gen_token()
    {
        out.push_str("#[__rsharp_gen_block] ");
        if blk.async_token().is_none()
            && let Some(move_tok) = blk.move_token()
        {
            out.push_str("#[__rsharp_gen_move] ");
            render_children_skipping(blk.syntax(), &[&gen_tok, &move_tok], out);
        } else {
            render_children_skipping(blk.syntax(), &[&gen_tok], out);
        }
        return;
    }

    for child in node.children_with_tokens() {
        render_element(&child, out);
    }
}

fn direct_token(node: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .find(|t| t.kind() == kind)
}

fn render_children_skipping(node: &SyntaxNode, skip: &[&SyntaxToken], out: &mut String) {
    for child in node.children_with_tokens() {
        if let NodeOrToken::Token(tok) = &child
            && skip.iter().any(|s| s.text_range() == tok.text_range())
        {
            continue;
        }
        render_element(&child, out);
    }
}

fn render_element(el: &NodeOrToken<SyntaxNode, SyntaxToken>, out: &mut String) {
    match el {
        NodeOrToken::Node(n) => render_node(n, out),
        NodeOrToken::Token(t) => out.push_str(t.text()),
    }
}
