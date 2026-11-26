use rsharp_core::{
    lexer,
    token::{InterpPart, Token, TokenKind},
};

// The lexer keeps literal segments and formatting text verbatim, but trims expression source.
pub(crate) fn expression_tokens(text: &str, tokens: &[Token]) -> Vec<Vec<Token>> {
    let mut result = Vec::new();
    fn strings(
        tokens: &[Token],
    ) -> impl Iterator<Item = (rsharp_core::Span, Vec<InterpPart>)> + '_ {
        tokens.iter().filter_map(|t| match &t.kind {
            TokenKind::InterpString(parts) => Some((t.span, parts.clone())),
            _ => None,
        })
    }
    let mut pending: Vec<_> = strings(tokens).collect();
    while let Some((span, parts)) = pending.pop() {
        let mut offset = span.start + 2;
        for part in parts {
            match part {
                InterpPart::Literal(literal) => offset += literal.len(),
                InterpPart::Expr { source, format } => {
                    let rest = &text[offset + 1..span.end];
                    let start = offset + 1 + rest.len() - rest.trim_start().len();
                    let end = start + source.len();
                    let tail = &text[end..span.end];
                    offset = end + tail.len() - tail.trim_start().len()
                        + format.map_or(0, |s| s.len() + 1)
                        + 1;
                    if let Ok(mut inner) = lexer::tokenize(&source) {
                        for t in &mut inner {
                            t.span.start += start;
                            t.span.end += start;
                        }
                        pending.extend(strings(&inner));
                        result.push(inner);
                    }
                }
            }
        }
    }
    result
}
