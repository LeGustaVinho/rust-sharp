use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range};
use rsharp_core::{RSharpError, TranspileOptions, check_rsharp_json, codegen_rust, parser};
use serde::Deserialize;

use crate::analysis::DocumentIndex;
use crate::position::LineIndex;

#[derive(Debug, Deserialize)]
struct RustcDiagnosticJson {
    #[serde(default)]
    message: String,
    #[serde(default)]
    level: String,
    code: Option<RustcCode>,
    #[serde(default)]
    spans: Vec<RustcSpan>,
    #[serde(default)]
    children: Vec<RustcChild>,
}

#[derive(Debug, Deserialize)]
struct RustcCode {
    code: String,
}

#[derive(Debug, Deserialize)]
struct RustcSpan {
    line_start: usize,
    #[serde(default)]
    is_primary: bool,
    label: Option<String>,
    #[serde(default)]
    text: Vec<RustcSpanText>,
}

#[derive(Debug, Deserialize)]
struct RustcSpanText {
    text: String,
    highlight_start: usize,
    highlight_end: usize,
}

#[derive(Debug, Deserialize)]
struct RustcChild {
    #[serde(default)]
    message: String,
    #[serde(default)]
    level: String,
}

/// Runs fast in-memory lex, parse, and codegen validation for immediate editor feedback.
pub fn syntax_diagnostics(source: &str, lines: &LineIndex) -> Vec<Diagnostic> {
    let ast = match parser::parse_file(source) {
        Ok(ast) => ast,
        Err(err) => return vec![rsharp_error_to_diagnostic(err, source, lines)],
    };
    if let Err(err) = codegen_rust::generate_rust(&ast, TranspileOptions::default()) {
        return vec![rsharp_error_to_diagnostic(err, source, lines)];
    }
    Vec::new()
}

fn rsharp_error_to_diagnostic(err: RSharpError, source: &str, lines: &LineIndex) -> Diagnostic {
    let (range, code, message) = match err {
        RSharpError::Lex { span, message } => {
            (lines.span_to_range(source, span), "lex", message)
        }
        RSharpError::Parse { span, message } => {
            (lines.span_to_range(source, span), "parse", message)
        }
        other => (
            Range::new(Position::new(0, 0), Position::new(0, 1)),
            "codegen",
            other.to_string(),
        ),
    };
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::ERROR),
        code: Some(NumberOrString::String(code.to_string())),
        source: Some("rsharp".to_string()),
        message,
        ..Diagnostic::default()
    }
}

/// Runs full `rustc --emit=metadata --error-format=json` semantic & borrow-checker diagnostics.
pub fn semantic_diagnostics(
    source: &str,
    lines: &LineIndex,
    index: &DocumentIndex,
) -> Vec<Diagnostic> {
    let Ok((rust_src, output)) = check_rsharp_json(source) else {
        return Vec::new();
    };
    parse_rustc_json_diagnostics(&rust_src, &output.messages, source, lines, index)
}

/// Parses `rustc --error-format=json` lines and maps spans back to the `.rsharp` document.
pub fn parse_rustc_json_diagnostics(
    rust_src: &str,
    json_lines: &[String],
    rsharp_src: &str,
    lines: &LineIndex,
    index: &DocumentIndex,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for raw in json_lines {
        let Ok(msg) = serde_json::from_str::<RustcDiagnosticJson>(raw) else {
            continue;
        };
        let severity = match msg.level.as_str() {
            "error" => DiagnosticSeverity::ERROR,
            "warning" => DiagnosticSeverity::WARNING,
            _ => continue,
        };
        if msg.spans.is_empty()
            && (msg.message.starts_with("aborting due to") || msg.message.ends_with("emitted"))
        {
            continue;
        }
        let primary = msg.spans.iter().find(|s| s.is_primary).or(msg.spans.first());
        let range = primary
            .map(|span| map_rustc_span_to_range(rust_src, span, rsharp_src, lines, index))
            .unwrap_or_else(|| Range::new(Position::new(0, 0), Position::new(0, 1)));

        let mut text = msg.message;
        if let Some(label) = primary.and_then(|s| s.label.as_deref())
            && !label.is_empty()
            && !text.contains(label)
        {
            text.push_str(": ");
            text.push_str(label);
        }
        for child in &msg.children {
            if matches!(child.level.as_str(), "help" | "note") && !child.message.is_empty() {
                text.push('\n');
                text.push_str(&child.level);
                text.push_str(": ");
                text.push_str(&child.message);
            }
        }

        diagnostics.push(Diagnostic {
            range,
            severity: Some(severity),
            code: msg.code.map(|c| NumberOrString::String(c.code)),
            source: Some("rsharp (rustc)".to_string()),
            message: text,
            ..Diagnostic::default()
        });
    }
    diagnostics
}

// shortcut: best-effort enclosing-item + highlighted-snippet span mapping without AST spans;
// upgrade to exact source maps when rsharp-core AST nodes carry per-node Spans.
fn map_rustc_span_to_range(
    rust_src: &str,
    span: &RustcSpan,
    rsharp_src: &str,
    lines: &LineIndex,
    index: &DocumentIndex,
) -> Range {
    let enclosing_item = find_enclosing_rust_item(rust_src, span.line_start);
    let target_sym = enclosing_item
        .as_deref()
        .and_then(|name| index.symbols.iter().find(|sym| sym.name == name));

    let (search_start, search_end, fallback_range) = match target_sym {
        Some(sym) => (
            sym.full_span.start.min(rsharp_src.len()),
            sym.full_span.end.min(rsharp_src.len()),
            lines.span_to_range(rsharp_src, sym.selection_span),
        ),
        None => (
            0,
            rsharp_src.len(),
            Range::new(Position::new(0, 0), Position::new(0, 1)),
        ),
    };

    if let Some(snippet) = extract_highlight_snippet(span)
        && let Some(rel) = rsharp_src[search_start..search_end].find(&snippet)
    {
        let abs_start = search_start + rel;
        let abs_end = abs_start + snippet.len();
        return Range::new(
            lines.offset_to_position(rsharp_src, abs_start),
            lines.offset_to_position(rsharp_src, abs_end),
        );
    }

    fallback_range
}

fn find_enclosing_rust_item(rust_src: &str, line_start: usize) -> Option<String> {
    let rust_lines: Vec<&str> = rust_src.lines().collect();
    if rust_lines.is_empty() || line_start == 0 {
        return None;
    }
    let start_idx = (line_start - 1).min(rust_lines.len() - 1);
    for line in rust_lines[..=start_idx].iter().rev() {
        let trimmed = line.trim();
        for kw in ["fn ", "struct ", "enum ", "union ", "trait ", "const ", "static "] {
            if let Some(pos) = trimmed.find(kw) {
                let after = &trimmed[pos + kw.len()..];
                let ident: String = after
                    .chars()
                    .take_while(|c| *c == '_' || c.is_alphanumeric())
                    .collect();
                if !ident.is_empty() {
                    return Some(ident);
                }
            }
        }
    }
    None
}

fn extract_highlight_snippet(span: &RustcSpan) -> Option<String> {
    let first = span.text.first()?;
    let chars: Vec<char> = first.text.chars().collect();
    let start = first.highlight_start.saturating_sub(1).min(chars.len());
    let end = first.highlight_end.saturating_sub(1).min(chars.len());
    if end <= start {
        return None;
    }
    let snippet: String = chars[start..end].iter().collect();
    let trimmed = snippet.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
