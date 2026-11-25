use lsp_types::{SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend};
use rsharp_core::token::{Keyword, Token, TokenKind};

use crate::analysis::{DeclKind, DocumentIndex};
use crate::builtins::{BuiltinKind, find_builtin};
use crate::position::LineIndex;

pub const TOKEN_TYPES: &[SemanticTokenType] = &[
    SemanticTokenType::NAMESPACE,      // 0
    SemanticTokenType::TYPE,           // 1
    SemanticTokenType::STRUCT,         // 2
    SemanticTokenType::ENUM,           // 3
    SemanticTokenType::INTERFACE,      // 4 (trait)
    SemanticTokenType::TYPE_PARAMETER, // 5
    SemanticTokenType::FUNCTION,       // 6
    SemanticTokenType::METHOD,         // 7
    SemanticTokenType::PROPERTY,       // 8 (struct/union field)
    SemanticTokenType::ENUM_MEMBER,    // 9
    SemanticTokenType::VARIABLE,       // 10
    SemanticTokenType::PARAMETER,      // 11
    SemanticTokenType::MACRO,          // 12
    SemanticTokenType::DECORATOR,      // 13 (attribute)
];

pub const TOKEN_MODIFIERS: &[SemanticTokenModifier] = &[
    SemanticTokenModifier::DECLARATION,     // bit 0
    SemanticTokenModifier::READONLY,        // bit 1
    SemanticTokenModifier::STATIC,          // bit 2
    SemanticTokenModifier::ASYNC,           // bit 3
    SemanticTokenModifier::DEFAULT_LIBRARY, // bit 4
];

const TY_NAMESPACE: u32 = 0;
const TY_TYPE: u32 = 1;
const TY_STRUCT: u32 = 2;
const TY_ENUM: u32 = 3;
const TY_INTERFACE: u32 = 4;
const TY_TYPE_PARAM: u32 = 5;
const TY_FUNCTION: u32 = 6;
const TY_METHOD: u32 = 7;
const TY_PROPERTY: u32 = 8;
const TY_ENUM_MEMBER: u32 = 9;
const TY_VARIABLE: u32 = 10;
const TY_PARAMETER: u32 = 11;
const TY_MACRO: u32 = 12;
const TY_DECORATOR: u32 = 13;

const MOD_DECLARATION: u32 = 1 << 0;
const MOD_READONLY: u32 = 1 << 1;
const MOD_STATIC: u32 = 1 << 2;
const MOD_ASYNC: u32 = 1 << 3;
const MOD_DEFAULT_LIB: u32 = 1 << 4;

pub fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: TOKEN_TYPES.to_vec(),
        token_modifiers: TOKEN_MODIFIERS.to_vec(),
    }
}

/// Computes C#/Roslyn-aligned semantic tokens layered on top of the TextMate grammar.
pub fn compute_semantic_tokens(
    source: &str,
    tokens: &[Token],
    lines: &LineIndex,
    index: &DocumentIndex,
) -> Vec<SemanticToken> {
    let mut raw: Vec<(u32, u32, u32, u32, u32)> = Vec::new();
    let mut attr_depth = 0usize;

    for (i, tok) in tokens.iter().enumerate() {
        match &tok.kind {
            TokenKind::LBracket if is_attr_start(tokens, i) => {
                attr_depth += 1;
            }
            TokenKind::RBracket if attr_depth > 0 => {
                attr_depth -= 1;
            }
            TokenKind::Ident { name, .. } => {
                let Some((token_type, modifiers)) =
                    classify_ident(tokens, i, name, attr_depth > 0, index)
                else {
                    continue;
                };
                let start_pos = lines.offset_to_position(source, tok.span.start);
                let end_pos = lines.offset_to_position(source, tok.span.end);
                raw.push((
                    start_pos.line,
                    start_pos.character,
                    end_pos.character - start_pos.character,
                    token_type,
                    modifiers,
                ));
            }
            _ => {}
        }
    }

    encode_delta_tokens(&raw)
}

fn is_attr_start(tokens: &[Token], idx: usize) -> bool {
    if idx == 0 {
        return true;
    }
    matches!(
        tokens[idx - 1].kind,
        TokenKind::Semicolon
            | TokenKind::LBrace
            | TokenKind::RBrace
            | TokenKind::RBracket
            | TokenKind::DocComment(_)
            | TokenKind::InnerDocComment(_)
            | TokenKind::Shebang(_)
            | TokenKind::Comma
    )
}

fn classify_ident(
    tokens: &[Token],
    idx: usize,
    name: &str,
    in_attr: bool,
    index: &DocumentIndex,
) -> Option<(u32, u32)> {
    // Macro invocation `println!(...)`
    if idx + 1 < tokens.len() && matches!(tokens[idx + 1].kind, TokenKind::Bang) {
        return Some((TY_MACRO, 0));
    }

    // Attribute name `[Derive(...)]`, `[unsafe NoMangle]`, `[module: Allow(...)]`
    if in_attr && idx > 0 {
        let prev = &tokens[idx - 1].kind;
        if matches!(
            prev,
            TokenKind::LBracket | TokenKind::Colon | TokenKind::Kw(Keyword::Unsafe)
        ) {
            return Some((TY_DECORATOR, 0));
        }
    }

    // Exact declaration site
    if let Some(decl) = index.find_declaration_at(tokens[idx].span.start) {
        let ty = decl_kind_to_token_type(decl.kind);
        let mut mods = MOD_DECLARATION;
        if !decl.is_mut
            && matches!(
                decl.kind,
                DeclKind::Variable | DeclKind::Field | DeclKind::Constant
            )
        {
            mods |= MOD_READONLY;
        }
        if decl.is_static {
            mods |= MOD_STATIC;
        }
        if decl.is_async {
            mods |= MOD_ASYNC;
        }
        return Some((ty, mods));
    }

    // Member access receiver `recv.ident`
    let (after_dot, receiver) = dot_receiver_before(tokens, idx);
    if let Some(resolved) = index.resolve_symbol(name, tokens[idx].span.start, receiver.as_deref())
    {
        let ty = decl_kind_to_token_type(resolved.kind);
        let mut mods = 0;
        if !resolved.is_mut
            && matches!(
                resolved.kind,
                DeclKind::Variable | DeclKind::Field | DeclKind::Constant
            )
        {
            mods |= MOD_READONLY;
        }
        if resolved.is_static {
            mods |= MOD_STATIC;
        }
        if resolved.is_async {
            mods |= MOD_ASYNC;
        }
        return Some((ty, mods));
    }

    // Built-in prelude functions & types (leave primitive types & keywords to TextMate C# scopes)
    if let Some(builtin) = find_builtin(name) {
        return match builtin.kind {
            BuiltinKind::Function => Some((TY_FUNCTION, MOD_DEFAULT_LIB)),
            BuiltinKind::StdType => Some((TY_TYPE, MOD_DEFAULT_LIB)),
            BuiltinKind::Keyword | BuiltinKind::PrimitiveType => None,
        };
    }

    // Heuristic call or member highlight when unresolved (e.g., external stdlib method)
    if is_followed_by_call(tokens, idx) {
        return Some((if after_dot { TY_METHOD } else { TY_FUNCTION }, 0));
    }
    if after_dot {
        return Some((TY_PROPERTY, 0));
    }
    None
}

fn decl_kind_to_token_type(kind: DeclKind) -> u32 {
    match kind {
        DeclKind::Namespace => TY_NAMESPACE,
        DeclKind::Struct | DeclKind::Union | DeclKind::Impl => TY_STRUCT,
        DeclKind::Enum => TY_ENUM,
        DeclKind::Trait => TY_INTERFACE,
        DeclKind::TypeParameter => TY_TYPE_PARAM,
        DeclKind::TypeAlias => TY_TYPE,
        DeclKind::Function => TY_FUNCTION,
        DeclKind::Method => TY_METHOD,
        DeclKind::Field => TY_PROPERTY,
        DeclKind::EnumMember => TY_ENUM_MEMBER,
        DeclKind::Constant | DeclKind::Static | DeclKind::Variable => TY_VARIABLE,
        DeclKind::Parameter => TY_PARAMETER,
        DeclKind::Macro => TY_MACRO,
    }
}

fn dot_receiver_before(tokens: &[Token], idx: usize) -> (bool, Option<String>) {
    if idx == 0 || !matches!(tokens[idx - 1].kind, TokenKind::Dot) {
        return (false, None);
    }
    if idx >= 2 {
        match &tokens[idx - 2].kind {
            TokenKind::Ident { name, .. } => return (true, Some(name.clone())),
            TokenKind::Kw(Keyword::This) => return (true, Some("this".to_string())),
            TokenKind::Kw(Keyword::SelfType) => return (true, Some("Self".to_string())),
            _ => {}
        }
    }
    (true, None)
}

fn is_followed_by_call(tokens: &[Token], idx: usize) -> bool {
    let mut j = idx + 1;
    if j < tokens.len() && matches!(tokens[j].kind, TokenKind::Lt) {
        let mut depth = 0usize;
        while j < tokens.len() {
            match tokens[j].kind {
                TokenKind::Lt => depth += 1,
                TokenKind::Gt => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        j += 1;
                        break;
                    }
                }
                TokenKind::Semicolon | TokenKind::LBrace | TokenKind::Eof => return false,
                _ => {}
            }
            j += 1;
        }
    }
    j < tokens.len() && matches!(tokens[j].kind, TokenKind::LParen)
}

fn encode_delta_tokens(raw: &[(u32, u32, u32, u32, u32)]) -> Vec<SemanticToken> {
    let mut out = Vec::with_capacity(raw.len());
    let mut prev_line = 0u32;
    let mut prev_start = 0u32;
    for &(line, start, length, token_type, token_modifiers_bitset) in raw {
        let delta_line = line - prev_line;
        let delta_start = if delta_line == 0 {
            start - prev_start
        } else {
            start
        };
        out.push(SemanticToken {
            delta_line,
            delta_start,
            length,
            token_type,
            token_modifiers_bitset,
        });
        prev_line = line;
        prev_start = start;
    }
    out
}
