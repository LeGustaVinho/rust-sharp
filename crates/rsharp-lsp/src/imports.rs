use rsharp_core::token::{Keyword, Token, TokenKind};
use std::ops::RangeInclusive;

pub(crate) fn segment(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Ident { name, .. } => Some(name),
        TokenKind::Kw(
            k @ (Keyword::Crate | Keyword::This | Keyword::Super | Keyword::SelfType),
        ) => Some(k.as_str()),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Import {
    pub path: Vec<String>,
    pub alias: Option<(String, usize)>,
    pub wildcard: bool,
    pub exported: bool,
    pub scope: RangeInclusive<usize>,
    pub segments: Vec<(usize, Vec<String>)>,
}

impl Import {
    pub fn name(&self) -> Option<&str> {
        self.alias
            .as_ref()
            .map(|a| a.0.as_str())
            .or_else(|| self.path.last().map(String::as_str))
    }
}

// Import segments retain their locations so a rename can edit the original name behind an alias.
pub(crate) fn collect(
    tokens: &[Token],
    scope: RangeInclusive<usize>,
    exported: bool,
    out: &mut Vec<Import>,
) {
    fn tree(
        tokens: &[Token],
        prefix: &[(usize, String)],
        scope: &RangeInclusive<usize>,
        exported: bool,
        out: &mut Vec<Import>,
    ) {
        if tokens.is_empty() {
            return;
        }
        let (alias, tokens) = match tokens {
            [name, eq, rest @ ..] if eq.kind == TokenKind::Eq => {
                (segment(name).map(|s| (s.to_owned(), name.span.start)), rest)
            }
            _ => (None, tokens),
        };
        let mut parts = prefix.to_vec();
        for (i, t) in tokens.iter().enumerate() {
            match t.kind {
                TokenKind::LBrace => {
                    let mut level = 0;
                    let mut start = i + 1;
                    for (j, item) in tokens.iter().enumerate().skip(start) {
                        match item.kind {
                            TokenKind::Comma | TokenKind::RBrace if level == 0 => {
                                tree(&tokens[start..j], &parts, scope, exported, out);
                                start = j + 1;
                                if item.kind == TokenKind::RBrace {
                                    break;
                                }
                            }
                            TokenKind::LBrace => level += 1,
                            TokenKind::RBrace => level -= 1,
                            _ => {}
                        }
                    }
                    return;
                }
                TokenKind::Dot | TokenKind::ColonColon | TokenKind::Star => {}
                _ => match segment(t) {
                    Some("this" | "self") if !prefix.is_empty() && i + 1 == tokens.len() => {}
                    Some(name) => parts.push((t.span.start, name.to_owned())),
                    None => return,
                },
            }
        }
        let mut path = Vec::new();
        let mut segments = Vec::new();
        for (pos, part) in parts {
            path.push(part);
            segments.push((pos, path.clone()));
        }
        out.push(Import {
            path,
            segments,
            alias,
            wildcard: tokens.last().is_some_and(|t| t.kind == TokenKind::Star),
            exported,
            scope: scope.clone(),
        });
    }
    tree(tokens, &[], &scope, exported, out);
}
