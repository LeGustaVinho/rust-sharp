use lsp_types::{Position, Range};
use rsharp_core::Span;

/// Precomputed line start byte offsets for fast UTF-8 <-> LSP UTF-16 conversion.
#[derive(Debug, Clone)]
pub struct LineIndex {
    line_starts: Vec<usize>,
}

/// Identifier context resolved at a cursor position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentAtCursor {
    pub name: String,
    pub range: Range,
    pub receiver: Option<String>,
    pub dot_triggered: bool,
}

impl LineIndex {
    pub fn new(source: &str) -> Self {
        let mut line_starts = vec![0];
        for (idx, b) in source.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(idx + 1);
            }
        }
        Self { line_starts }
    }

    pub fn offset_to_position(&self, source: &str, byte_offset: usize) -> Position {
        let clamped = clamp_char_boundary(source, byte_offset.min(source.len()));
        let line = self.line_starts.partition_point(|&start| start <= clamped) - 1;
        let line_start = self.line_starts[line];
        let slice = &source[line_start..clamped];
        let character = slice.encode_utf16().count() as u32;
        Position::new(line as u32, character)
    }

    pub fn position_to_offset(&self, source: &str, pos: Position) -> usize {
        let line = pos.line as usize;
        if line >= self.line_starts.len() {
            return source.len();
        }
        let line_start = self.line_starts[line];
        let line_end = self
            .line_starts
            .get(line + 1)
            .copied()
            .unwrap_or(source.len());
        let line_slice = source[line_start..line_end]
            .trim_end_matches('\n')
            .trim_end_matches('\r');

        let mut utf16_col = 0u32;
        for (rel_idx, ch) in line_slice.char_indices() {
            if utf16_col >= pos.character {
                return line_start + rel_idx;
            }
            utf16_col += ch.len_utf16() as u32;
        }
        line_start + line_slice.len()
    }

    pub fn span_to_range(&self, source: &str, span: Span) -> Range {
        let start = self.offset_to_position(source, span.start);
        let end = if span.end > span.start {
            self.offset_to_position(source, span.end)
        } else {
            Position::new(start.line, start.character + 1)
        };
        Range::new(start, end)
    }
}

fn clamp_char_boundary(source: &str, mut offset: usize) -> usize {
    while offset > 0 && !source.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn is_ident_char(ch: char) -> bool {
    ch == '_' || ch.is_alphanumeric()
}

/// Extracts the identifier and optional `.` receiver around `pos`.
pub fn ident_at_position(source: &str, lines: &LineIndex, pos: Position) -> Option<IdentAtCursor> {
    let offset = lines.position_to_offset(source, pos);

    // Find start of identifier (walking backwards from offset).
    let mut start = offset;
    for (idx, ch) in source[..offset].char_indices().rev() {
        if is_ident_char(ch) {
            start = idx;
        } else {
            break;
        }
    }

    // Find end of identifier (walking forwards from offset).
    let mut end = offset;
    for (rel, ch) in source[offset..].char_indices() {
        if is_ident_char(ch) {
            end = offset + rel + ch.len_utf8();
        } else {
            break;
        }
    }

    // Include verbatim `@` prefix in range while stripping it from logical name.
    let raw_start = if start > 0 && source.as_bytes()[start - 1] == b'@' {
        start - 1
    } else {
        start
    };

    let name = source[start..end].to_string();
    let (receiver, dot_triggered) = extract_dot_receiver(source, raw_start);

    if name.is_empty() && !dot_triggered {
        return None;
    }

    let range = Range::new(
        lines.offset_to_position(source, raw_start),
        lines.offset_to_position(source, end),
    );
    Some(IdentAtCursor {
        name,
        range,
        receiver,
        dot_triggered,
    })
}

fn extract_dot_receiver(source: &str, before_ident: usize) -> (Option<String>, bool) {
    let prefix = source[..before_ident].trim_end();
    if !prefix.ends_with('.') || prefix.ends_with("..") {
        return (None, false);
    }
    let before_dot = prefix[..prefix.len() - 1].trim_end();
    if before_dot.ends_with('"') {
        return (Some("string".to_string()), true);
    }
    let mut recv_start = before_dot.len();
    for (idx, ch) in before_dot.char_indices().rev() {
        if is_ident_char(ch) {
            recv_start = idx;
        } else {
            break;
        }
    }
    let recv = &before_dot[recv_start..];
    if recv.is_empty() {
        (None, true)
    } else {
        (Some(recv.to_string()), true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_index_handles_ascii_unicode_crlf_and_bounds() {
        let src = "int a = 1;\r\nstring s = \"😀\";\nfoo";
        let idx = LineIndex::new(src);

        // ASCII line 0
        assert_eq!(idx.offset_to_position(src, 4), Position::new(0, 4));
        assert_eq!(idx.position_to_offset(src, Position::new(0, 4)), 4);

        // Emoji '😀' is 4 bytes in UTF-8 and 2 code units in UTF-16.
        let after_emoji_byte = src.find('😀').unwrap() + '😀'.len_utf8();
        let pos = idx.offset_to_position(src, after_emoji_byte);
        assert_eq!(pos, Position::new(1, 14));
        assert_eq!(idx.position_to_offset(src, pos), after_emoji_byte);

        // Mid-UTF-8 byte offset clamps down cleanly to char boundary.
        let mid_emoji = src.find('😀').unwrap() + 2;
        assert_eq!(idx.offset_to_position(src, mid_emoji), Position::new(1, 12));

        // Past-EOL and past-EOF clamping
        assert_eq!(
            idx.position_to_offset(src, Position::new(0, 999)),
            10 // before \r\n
        );
        assert_eq!(idx.position_to_offset(src, Position::new(99, 0)), src.len());

        // Span conversion: normal and zero-width
        assert_eq!(
            idx.span_to_range(src, Span::new(1, 1, 0, 3)),
            Range::new(Position::new(0, 0), Position::new(0, 3))
        );
        assert_eq!(
            idx.span_to_range(src, Span::new(3, 4, src.len(), src.len())),
            Range::new(Position::new(2, 3), Position::new(2, 4))
        );
    }

    #[test]
    fn ident_at_position_extracts_plain_verbatim_and_dot_receivers() {
        let src = "alice.Deposit(1); @type x; alice.; a..b; (x).y;";
        let idx = LineIndex::new(src);

        // Plain member after dot
        let dep = ident_at_position(src, &idx, Position::new(0, 9)).unwrap();
        assert_eq!(dep.name, "Deposit");
        assert_eq!(dep.receiver.as_deref(), Some("alice"));
        assert!(dep.dot_triggered);

        // Verbatim identifier @type
        let verb = ident_at_position(src, &idx, Position::new(0, 21)).unwrap();
        assert_eq!(verb.name, "type");
        assert_eq!(verb.receiver, None);
        assert!(!verb.dot_triggered);

        // Cursor right after trailing dot `alice.`
        let dot_only = ident_at_position(src, &idx, Position::new(0, 33)).unwrap();
        assert_eq!(dot_only.name, "");
        assert_eq!(dot_only.receiver.as_deref(), Some("alice"));
        assert!(dot_only.dot_triggered);

        // Range `..` is not treated as member access
        let range_rhs = ident_at_position(src, &idx, Position::new(0, 38)).unwrap();
        assert_eq!(range_rhs.name, "b");
        assert_eq!(range_rhs.receiver, None);
        assert!(!range_rhs.dot_triggered);

        // Non-ident receiver `(x).y`
        let paren_mem = ident_at_position(src, &idx, Position::new(0, 45)).unwrap();
        assert_eq!(paren_mem.name, "y");
        assert_eq!(paren_mem.receiver, None);
        assert!(paren_mem.dot_triggered);

        // Whitespace with no ident or dot returns None
        assert!(ident_at_position(src, &idx, Position::new(0, 17)).is_none());

        // Identifier starting at byte 0 and reaching EOF
        let edge = ident_at_position("foo", &LineIndex::new("foo"), Position::new(0, 1)).unwrap();
        assert_eq!(edge.name, "foo");

        // String literal dot receiver `"hi".ToString()`
        let ssrc = "\"hi\".ToString()";
        let str_mem = ident_at_position(ssrc, &LineIndex::new(ssrc), Position::new(0, 6)).unwrap();
        assert_eq!(str_mem.name, "ToString");
        assert_eq!(str_mem.receiver.as_deref(), Some("string"));
        assert!(str_mem.dot_triggered);
    }
}
