use rsharp_core::token::{Token, TokenKind};

use crate::handlers::DocumentState;
use crate::position::IdentAtCursor;

const METHODS: &str = include_str!("functional_methods.tsv");

pub(crate) fn call_separator(doc: &DocumentState, cursor: &IdentAtCursor) -> Option<usize> {
    let start = doc.lines.position_to_offset(&doc.text, cursor.range.start);
    let method = doc.tokens.partition_point(|token| token.span.start < start);
    let separator = method.checked_sub(1)?;
    let before = &doc.tokens[separator];
    let name = doc.tokens.get(method)?;
    let after = doc.tokens.get(method + 1)?;
    if name.span.start != start
        || token_name(name) != Some(cursor.name.as_str())
        || !matches!(before.kind, TokenKind::Dot | TokenKind::ColonColon)
        || !matches!(
            after.kind,
            TokenKind::LParen | TokenKind::Lt | TokenKind::ColonColon
        )
    {
        return None;
    }

    Some(separator)
}

pub(crate) fn documentation(doc: &DocumentState, method: &str, separator: usize) -> Option<String> {
    let owner = receiver_owner(doc, separator);
    let mut entries = Vec::new();
    for line in METHODS.lines() {
        let (key, description) = line.split_once('\t').expect("method description");
        let (owners, name) = key.split_once(' ').expect("method owner");
        if name == method {
            for documented in owners.split('|') {
                if owner.is_none_or(|actual| owner_matches(actual, documented)) {
                    let label = match documented {
                        "array" => "[T; N]",
                        "slice" => "[T]",
                        "Atomic" => owner.unwrap_or("Atomic types"),
                        _ => documented,
                    };
                    entries.push(format!("`{label}::{method}`\n\n{description}"));
                }
            }
        }
    }
    if entries.is_empty() {
        return None;
    }
    let heading = if owner.is_some() {
        "**Rust standard library**"
    } else {
        "**Possible Rust standard-library methods**"
    };
    Some(format!("{heading}\n\n{}", entries.join("\n\n---\n\n")))
}

fn receiver_owner(doc: &DocumentState, mut end: usize) -> Option<&str> {
    // shortcut: recognizes common iterator chains by syntax, upgrade when the LSP has expression types.
    loop {
        let receiver = doc.tokens.get(end.checked_sub(1)?)?;
        match receiver.kind {
            TokenKind::StringLit(_) | TokenKind::RawStringLit(_) => return Some("str"),
            TokenKind::InterpString(_) => return Some("String"),
            TokenKind::RParen => {
                let call = call_name_before(&doc.tokens[..end])?;
                let callee = &doc.tokens[call];
                let name = token_name(callee)?;
                if let [.., target, dot] = &doc.tokens[..call]
                    && dot.kind == TokenKind::Dot
                    && let Some(target) = token_name(target)
                    && let Some(member) =
                        doc.index
                            .resolve_symbol(name, callee.span.start, Some(target))
                {
                    return member.ty_name.as_deref();
                }
                match name {
                    "Some" => return Some("Option"),
                    "Ok" | "Err" => return Some("Result"),
                    "iter"
                    | "iter_mut"
                    | "into_iter"
                    | "chars"
                    | "bytes"
                    | "lines"
                    | "split_whitespace"
                    | "split_ascii_whitespace" => return Some("Iterator"),
                    "peekable" => return Some("Peekable"),
                    "by_ref" | "chain" | "cloned" | "copied" | "cycle" | "enumerate" | "filter"
                    | "filter_map" | "flat_map" | "flatten" | "fuse" | "inspect" | "map"
                    | "map_while" | "or_else" | "rev" | "scan" | "skip" | "skip_while"
                    | "step_by" | "take" | "take_while" | "zip" => {
                        end = call.checked_sub(1)?;
                        if doc.tokens[end].kind != TokenKind::Dot {
                            return None;
                        }
                    }
                    _ => {
                        return doc
                            .index
                            .resolve_symbol(name, receiver.span.start, None)?
                            .ty_name
                            .as_deref();
                    }
                }
            }
            _ => {
                let name = token_name(receiver)?;
                if matches!(name, "true" | "false") {
                    return Some("bool");
                }
                if let Some(symbol) = doc.index.resolve_symbol(name, receiver.span.start, None) {
                    let ty = doc
                        .text
                        .get(symbol.full_span.start..symbol.selection_span.start)?
                        .trim_end();
                    return match ty.as_bytes().last() {
                        Some(b'?') => Some("Option"),
                        Some(b']') => Some("array"),
                        _ => symbol.ty_name.as_deref().filter(|ty| *ty != "var"),
                    };
                }
                return name
                    .chars()
                    .next()
                    .is_some_and(char::is_uppercase)
                    .then_some(name);
            }
        }
    }
}

fn token_name(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Ident { name, .. } => Some(name),
        TokenKind::Kw(keyword) => Some(keyword.as_str()),
        _ => None,
    }
}

fn call_name_before(tokens: &[Token]) -> Option<usize> {
    let mut depth = 0;
    for (index, token) in tokens.iter().enumerate().rev() {
        match token.kind {
            TokenKind::RParen => depth += 1,
            TokenKind::LParen => {
                depth -= 1;
                if depth == 0 {
                    return index.checked_sub(1);
                }
            }
            _ => {}
        }
    }
    None
}

fn owner_matches(actual: &str, documented: &str) -> bool {
    let actual = actual.rsplit(['.', ':']).next().unwrap_or(actual);
    let actual = match actual {
        "List" => "Vec",
        "Queue" => "VecDeque",
        "Dictionary" => "HashMap",
        "SortedDictionary" => "BTreeMap",
        "SortedSet" => "BTreeSet",
        "PriorityQueue" => "BinaryHeap",
        "string" => "String",
        other => other,
    };
    actual == documented
        || matches!(
            (actual, documented),
            ("array" | "Vec", "slice")
                | ("String", "str")
                | ("Peekable" | "DoubleEndedIterator", "Iterator")
                | ("Peekable" | "Iterator", "DoubleEndedIterator")
        )
        || matches!(
            (actual, documented),
            (
                "Vec"
                    | "VecDeque"
                    | "LinkedList"
                    | "HashMap"
                    | "BTreeMap"
                    | "HashSet"
                    | "BTreeSet"
                    | "BinaryHeap",
                "IntoIterator" | "FromIterator" | "Extend"
            ) | (
                "array" | "Iterator" | "DoubleEndedIterator" | "Peekable",
                "IntoIterator"
            ) | ("Option" | "Result", "IntoIterator" | "FromIterator")
                | ("String", "FromIterator" | "Extend")
        )
        || (documented == "Atomic"
            && matches!(
                actual,
                "AtomicBool"
                    | "AtomicPtr"
                    | "AtomicI8"
                    | "AtomicI16"
                    | "AtomicI32"
                    | "AtomicI64"
                    | "AtomicIsize"
                    | "AtomicU8"
                    | "AtomicU16"
                    | "AtomicU32"
                    | "AtomicU64"
                    | "AtomicUsize"
            ))
}
