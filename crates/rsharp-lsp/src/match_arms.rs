use crate::{
    analysis::DeclKind,
    handlers::{DocumentState, ServerState},
    navigation::{Found, RESOLUTION_DEPTH},
    refactor::workspace_edit,
};
use lsp_types::{
    CodeAction, CodeActionKind, CodeActionParams, CodeActionResponse, Range, TextEdit,
};
use rsharp_core::{
    ast::{Expr, Pattern},
    parser,
    token::{Keyword, Token, TokenKind as K},
};

pub(crate) fn closing(tokens: &[Token], open: usize, left: K, right: K) -> Option<usize> {
    if tokens.get(open)?.kind != left {
        return None;
    }
    let mut depth = 0;
    for (i, t) in tokens.iter().enumerate().skip(open) {
        if t.kind == left {
            depth += 1;
        } else if t.kind == right {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

fn covers_any(state: &ServerState, doc: &DocumentState, pos: usize, p: &Pattern) -> bool {
    let any = |p: &Pattern| covers_any(state, doc, pos, p);
    match p {
        Pattern::Wildcard | Pattern::Binding { .. } | Pattern::BorrowRef { .. } | Pattern::Rest => {
            true
        }
        Pattern::Paren(p)
        | Pattern::DerefPat { inner: p, .. }
        | Pattern::BindAt { subpat: p, .. } => any(p),
        Pattern::Or(ps) => ps.iter().any(any),
        Pattern::Tuple(ps) => ps.iter().all(any),
        Pattern::Path(path) if !path.global && path.qself.is_none() && path.segments.len() == 1 => {
            let name = &path.segments[0].name;
            state
                .names(doc, pos, std::slice::from_ref(name), RESOLUTION_DEPTH)
                .is_none_or(|s| matches!(s.sym.kind, DeclKind::Variable | DeclKind::Parameter))
        }
        _ => false,
    }
}

fn covers(
    state: &ServerState,
    doc: &DocumentState,
    pos: usize,
    variant: Found<'_>,
    p: &Pattern,
) -> bool {
    let any = |p: &Pattern| covers_any(state, doc, pos, p);
    let path = match p {
        _ if any(p) => return true,
        Pattern::Or(ps) => return ps.iter().any(|p| covers(state, doc, pos, variant, p)),
        Pattern::Paren(p)
        | Pattern::DerefPat { inner: p, .. }
        | Pattern::BindAt { subpat: p, .. } => return covers(state, doc, pos, variant, p),
        Pattern::Path(path) => path,
        Pattern::TupleStruct { path, fields } if fields.iter().all(any) => path,
        Pattern::Struct { path, fields, .. }
            if fields.iter().all(|f| f.pat.as_ref().is_none_or(any)) =>
        {
            path
        }
        _ => return false,
    };
    let names: Vec<_> = path.segments.iter().map(|s| s.name.clone()).collect();
    state
        .names(doc, pos, &names, RESOLUTION_DEPTH)
        .is_some_and(|s| s.same(variant))
}

fn escaped(name: &str) -> String {
    if Keyword::parse(name).is_some() {
        format!("@{name}")
    } else {
        name.to_owned()
    }
}

fn indentation(text: &str, offset: usize) -> &str {
    let line = &text[text[..offset].rfind('\n').map_or(0, |i| i + 1)..offset];
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

impl ServerState {
    pub(crate) fn match_arms(&self, params: CodeActionParams) -> Option<CodeActionResponse> {
        if params
            .context
            .only
            .as_ref()
            .is_some_and(|only| !only.contains(&CodeActionKind::QUICKFIX))
        {
            return None;
        }
        let doc = self.document(&params.text_document.uri)?;
        if !doc.fresh {
            return None;
        }
        let offset = doc.lines.position_to_offset(&doc.text, params.range.start);
        let tokens = &doc.tokens;
        let before = tokens.partition_point(|t| t.span.start <= offset);
        let (start, open, close) =
            tokens[..before]
                .iter()
                .enumerate()
                .rev()
                .find_map(|(i, token)| {
                    if token.kind != K::Kw(Keyword::Match) {
                        return None;
                    }
                    let open = closing(tokens, i + 1, K::LParen, K::RParen)? + 1;
                    let close = closing(tokens, open, K::LBrace, K::RBrace)?;
                    (offset <= tokens[close].span.end).then_some((i, open, close))
                })?;
        let pos = tokens[start].span.start;
        let Expr::Match(m) = parser::parse_expr_str(&doc.text[pos..tokens[close].span.end]).ok()?
        else {
            return None;
        };
        let enum_type = self.expr_type(doc, pos, &m.scrutinee, RESOLUTION_DEPTH)?;
        if enum_type.sym.kind != DeclKind::Enum {
            return None;
        }
        let qualifier = doc
            .index
            .imports
            .iter()
            .filter(|imp| imp.scope.contains(&pos))
            .filter_map(|imp| imp.name())
            .chain([enum_type.sym.name.as_str()])
            .find(|name| {
                self.names(doc, pos, &[name.to_string()], RESOLUTION_DEPTH)
                    .is_some_and(|s| s.same(enum_type))
            })
            .map(escaped)
            .unwrap_or_else(|| {
                format!(
                    "@crate.{}",
                    enum_type
                        .doc
                        .symbol_path(enum_type.sym)
                        .unwrap()
                        .iter()
                        .map(|s| escaped(s))
                        .collect::<Vec<_>>()
                        .join(".")
                )
            });
        let mut arms = Vec::new();
        for sym in &enum_type.sym.children {
            if sym.kind != DeclKind::EnumMember {
                continue;
            }
            let variant = Found {
                doc: enum_type.doc,
                sym,
                aliased: false,
            };
            if m.arms
                .iter()
                .any(|a| a.guard.is_none() && covers(self, doc, pos, variant, &a.pat))
            {
                continue;
            }
            let t = enum_type.doc.token_start(sym.selection_span.start);
            let payload = match enum_type.doc.tokens[t + 1].kind {
                K::LParen => "(..)",
                K::LBrace => " { .. }",
                _ => "",
            };
            arms.push(format!(
                "{qualifier}.{}{payload} => todo!(),",
                escaped(&sym.name)
            ));
        }
        if arms.is_empty() {
            return None;
        }
        let nl = if doc.text.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let end = tokens[close].span.start;
        let line_start = doc.text[..end].rfind('\n').map_or(0, |i| i + 1);
        let own_line = doc.text[line_start..end].trim().is_empty();
        let base = if own_line {
            &doc.text[line_start..end]
        } else {
            indentation(&doc.text, pos)
        };
        let first = indentation(&doc.text, tokens[open + 1].span.start);
        let indent = if open + 1 < close && first.len() > base.len() {
            first.to_owned()
        } else {
            format!("{base}{}", if base.contains('\t') { "\t" } else { "    " })
        };
        let body = arms
            .iter()
            .map(|arm| format!("{indent}{arm}{nl}"))
            .collect::<String>();
        let (at, new_text) = if own_line {
            (line_start, body)
        } else {
            (end, format!("{nl}{body}{base}"))
        };
        let insert = |offset, text| {
            let p = doc.lines.offset_to_position(&doc.text, offset);
            TextEdit::new(Range::new(p, p), text)
        };
        let mut edits = vec![insert(at, new_text)];
        let prev = &tokens[close - 1];
        if !matches!(prev.kind, K::Comma | K::LBrace | K::RBrace) {
            if prev.span.end == at {
                edits[0].new_text.insert(0, ',');
            } else {
                edits.push(insert(prev.span.end, ",".into()));
            }
        }
        Some(vec![
            CodeAction {
                title: "Add missing enum variants".into(),
                kind: Some(CodeActionKind::QUICKFIX),
                edit: Some(workspace_edit(vec![doc.edits(edits)])),
                ..Default::default()
            }
            .into(),
        ])
    }
}
