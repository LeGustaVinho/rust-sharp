use crate::error::{RSharpError, Result, Span};
use crate::token::{InterpPart, Keyword, Token, TokenKind};

/// Lexes an R# source string into a token stream terminated by `TokenKind::Eof`.
pub fn tokenize(source: &str) -> Result<Vec<Token>> {
    Lexer::new(source).tokenize_all()
}

struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    line: usize,
    col: usize,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn peek_at(&self, byte_offset: usize) -> Option<char> {
        let idx = self.pos + byte_offset;
        if idx >= self.bytes.len() {
            None
        } else {
            self.src[idx..].chars().next()
        }
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += ch.len_utf8();
        if ch == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(ch)
    }

    fn span_from(&self, line: usize, col: usize, start: usize) -> Span {
        Span::new(line, col, start, self.pos)
    }

    fn eat_u8_suffix(&mut self) -> bool {
        if self.peek() == Some('u')
            && self.peek_at(1) == Some('8')
            && !self.peek_at(2).is_some_and(is_ident_continue)
        {
            self.bump();
            self.bump();
            true
        } else {
            false
        }
    }

    fn tokenize_all(&mut self) -> Result<Vec<Token>> {
        let mut tokens = Vec::new();
        if self.src.starts_with("#!") && !self.src.starts_with("#![") {
            let line_end = self.src.find('\n').unwrap_or(self.src.len());
            let raw = self.src[..line_end].trim_end_matches('\r').to_string();
            while self.pos < line_end {
                self.bump();
            }
            tokens.push(Token::new(
                TokenKind::Shebang(raw),
                Span::new(1, 1, 0, line_end),
            ));
        }
        loop {
            self.skip_whitespace_and_comments(&mut tokens)?;
            if self.is_eof() {
                tokens.push(Token::new(
                    TokenKind::Eof,
                    Span::new(self.line, self.col, self.pos, self.pos),
                ));
                return Ok(tokens);
            }
            let tok = self.next_token()?;
            tokens.push(tok);
            if should_lex_raw_macro_body(&tokens)
                && let Some((open, close)) = self.peek_macro_delim()
            {
                let (line, col, start) = self.skip_ascii_ws_pos();
                let raw = self.lex_balanced_rust_group(
                    open,
                    close,
                    line,
                    col,
                    start,
                    "Unterminated macro delimiter group",
                )?;
                tokens.push(Token::new(
                    TokenKind::RawMacroBody {
                        delim: open,
                        body: raw.trim().to_string(),
                    },
                    self.span_from(line, col, start),
                ));
            } else if is_attr_arg_prefix(&tokens)
                && let Some((open, close)) = self.peek_macro_delim()
            {
                let saved_pos = self.pos;
                let saved_line = self.line;
                let saved_col = self.col;
                let (line, col, start) = self.skip_ascii_ws_pos();
                if let Ok(raw) = self.lex_balanced_rust_group(
                    open,
                    close,
                    line,
                    col,
                    start,
                    "Unterminated attribute delimiter group",
                ) && self.peek_next_non_ws() == Some(']')
                    && tokenize(&raw).is_err()
                {
                    tokens.push(Token::new(
                        TokenKind::RawMacroBody {
                            delim: open,
                            body: raw.trim().to_string(),
                        },
                        self.span_from(line, col, start),
                    ));
                } else {
                    self.pos = saved_pos;
                    self.line = saved_line;
                    self.col = saved_col;
                }
            }
        }
    }

    fn skip_whitespace_and_comments(&mut self, tokens: &mut Vec<Token>) -> Result<()> {
        loop {
            while let Some(c) = self.peek() {
                if c.is_whitespace() {
                    self.bump();
                } else {
                    break;
                }
            }
            if self.peek() == Some('/') && self.peek_at(1) == Some('/') {
                let line = self.line;
                let col = self.col;
                let start = self.pos;
                self.bump();
                self.bump();
                if self.peek() == Some('/') && self.peek_at(1) != Some('/') {
                    self.bump();
                    let text_start = self.pos;
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.bump();
                    }
                    let text = self.src[text_start..self.pos].trim_end_matches('\r');
                    let cleaned = text.strip_prefix(' ').unwrap_or(text).to_string();
                    tokens.push(Token::new(
                        TokenKind::DocComment(cleaned),
                        self.span_from(line, col, start),
                    ));
                } else if self.peek() == Some('!') {
                    self.bump();
                    let text_start = self.pos;
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.bump();
                    }
                    let text = self.src[text_start..self.pos].trim_end_matches('\r');
                    let cleaned = text.strip_prefix(' ').unwrap_or(text).to_string();
                    tokens.push(Token::new(
                        TokenKind::InnerDocComment(cleaned),
                        self.span_from(line, col, start),
                    ));
                } else {
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.bump();
                    }
                }
            } else if self.peek() == Some('/') && self.peek_at(1) == Some('*') {
                self.lex_block_comment_or_doc(tokens)?;
            } else {
                return Ok(());
            }
        }
    }

    fn lex_block_comment_or_doc(&mut self, tokens: &mut Vec<Token>) -> Result<()> {
        let line = self.line;
        let col = self.col;
        let start = self.pos;
        let doc_kind = match (self.peek_at(2), self.peek_at(3)) {
            (Some('!'), _) => Some(true),
            (Some('*'), Some(c)) if c != '*' && c != '/' => Some(false),
            _ => None,
        };
        let Some(is_inner) = doc_kind else {
            return self.skip_block_comment();
        };
        self.bump(); // '/'
        self.bump(); // '*'
        self.bump(); // '*' or '!'
        let body_start = self.pos;
        let mut depth = 1usize;
        loop {
            match (self.peek(), self.peek_at(1)) {
                (Some('/'), Some('*')) => {
                    self.bump();
                    self.bump();
                    depth += 1;
                }
                (Some('*'), Some('/')) => {
                    depth -= 1;
                    if depth == 0 {
                        let raw = &self.src[body_start..self.pos];
                        self.bump();
                        self.bump();
                        let span = self.span_from(line, col, start);
                        for doc_line in clean_block_doc(raw) {
                            let kind = if is_inner {
                                TokenKind::InnerDocComment(doc_line)
                            } else {
                                TokenKind::DocComment(doc_line)
                            };
                            tokens.push(Token::new(kind, span));
                        }
                        return Ok(());
                    }
                    self.bump();
                    self.bump();
                }
                (Some(_), _) => {
                    self.bump();
                }
                (None, _) => {
                    return Err(RSharpError::lex(
                        self.span_from(line, col, start),
                        "Unterminated block comment",
                    ));
                }
            }
        }
    }

    fn skip_block_comment(&mut self) -> Result<()> {
        let line = self.line;
        let col = self.col;
        let start = self.pos;
        self.bump();
        self.bump();
        let mut depth = 1usize;
        while depth > 0 {
            match (self.peek(), self.peek_at(1)) {
                (Some('/'), Some('*')) => {
                    self.bump();
                    self.bump();
                    depth += 1;
                }
                (Some('*'), Some('/')) => {
                    self.bump();
                    self.bump();
                    depth -= 1;
                }
                (Some(_), _) => {
                    self.bump();
                }
                (None, _) => {
                    return Err(RSharpError::lex(
                        self.span_from(line, col, start),
                        "Unterminated block comment",
                    ));
                }
            }
        }
        Ok(())
    }

    fn next_token(&mut self) -> Result<Token> {
        let line = self.line;
        let col = self.col;
        let start = self.pos;
        let ch = self.peek().unwrap();

        // Interpolated string: $"..."
        if ch == '$' && self.peek_at(1) == Some('"') {
            self.bump();
            self.bump();
            let parts = self.lex_interp_string(line, col, start)?;
            return Ok(Token::new(
                TokenKind::InterpString(parts),
                self.span_from(line, col, start),
            ));
        }

        // Byte char literal: b'a'
        if ch == 'b' && self.peek_at(1) == Some('\'') {
            self.bump();
            let c = self.lex_char(line, col, start)?;
            return Ok(Token::new(
                TokenKind::ByteLit(c),
                self.span_from(line, col, start),
            ));
        }

        // C-string literal: c"...", c@"...", c"""..."""
        if ch == 'c'
            && (self.peek_at(1) == Some('"')
                || (self.peek_at(1) == Some('@') && self.peek_at(2) == Some('"')))
        {
            self.bump(); // 'c'
            let (value, raw) = if self.peek() == Some('@') {
                self.bump();
                (self.lex_verbatim_string(line, col, start)?, true)
            } else if self.is_triple_quote() {
                (self.lex_triple_raw_string(line, col, start)?, true)
            } else {
                (self.lex_string(line, col, start)?, false)
            };
            return Ok(Token::new(
                TokenKind::CStringLit { value, raw },
                self.span_from(line, col, start),
            ));
        }

        // Verbatim string `@"..."` (`@"..."u8`) or verbatim identifier `@ident`
        if ch == '@' {
            self.bump();
            if self.peek() == Some('"') {
                let value = self.lex_verbatim_string(line, col, start)?;
                let kind = if self.eat_u8_suffix() {
                    TokenKind::ByteStringLit { value, raw: true }
                } else {
                    TokenKind::RawStringLit(value)
                };
                return Ok(Token::new(kind, self.span_from(line, col, start)));
            }
            let id_start = self.pos;
            match self.peek() {
                Some(c) if is_ident_start(c) => {
                    self.bump();
                    while let Some(c) = self.peek() {
                        if is_ident_continue(c) {
                            self.bump();
                        } else {
                            break;
                        }
                    }
                    let name = self.src[id_start..self.pos].to_string();
                    return Ok(Token::new(
                        TokenKind::Ident {
                            name,
                            verbatim: true,
                        },
                        self.span_from(line, col, start),
                    ));
                }
                _ => {
                    return Err(RSharpError::lex(
                        self.span_from(line, col, start),
                        "Expected identifier or string after '@'",
                    ));
                }
            }
        }

        // Identifiers & keywords (including inline `rust { ... }`)
        if is_ident_start(ch) {
            self.bump();
            while let Some(c) = self.peek() {
                if is_ident_continue(c) {
                    self.bump();
                } else {
                    break;
                }
            }
            let text = &self.src[start..self.pos];
            if text == "rust" && self.is_followed_by_lbrace() {
                let raw = self.lex_balanced_rust_group(
                    '{',
                    '}',
                    line,
                    col,
                    start,
                    "Unterminated 'rust { ... }' block",
                )?;
                return Ok(Token::new(
                    TokenKind::RustBlock(raw),
                    self.span_from(line, col, start),
                ));
            }
            if let Some(kw) = Keyword::parse(text) {
                return Ok(Token::new(
                    TokenKind::Kw(kw),
                    self.span_from(line, col, start),
                ));
            }
            return Ok(Token::new(
                TokenKind::Ident {
                    name: text.to_string(),
                    verbatim: false,
                },
                self.span_from(line, col, start),
            ));
        }

        // Numeric literals
        if ch.is_ascii_digit() {
            return self.lex_number(line, col, start);
        }

        // Triple-quote raw string `"""..."""` (`"""..."""u8`) or standard string `"..."` (`"..."u8`)
        if ch == '"' {
            if self.is_triple_quote() {
                let value = self.lex_triple_raw_string(line, col, start)?;
                let kind = if self.eat_u8_suffix() {
                    TokenKind::ByteStringLit { value, raw: true }
                } else {
                    TokenKind::RawStringLit(value)
                };
                return Ok(Token::new(kind, self.span_from(line, col, start)));
            }
            let value = self.lex_string(line, col, start)?;
            let kind = if self.eat_u8_suffix() {
                TokenKind::ByteStringLit { value, raw: false }
            } else {
                TokenKind::StringLit(value)
            };
            return Ok(Token::new(kind, self.span_from(line, col, start)));
        }

        // Char literal
        if ch == '\'' {
            let c = self.lex_char(line, col, start)?;
            return Ok(Token::new(
                TokenKind::CharLit(c),
                self.span_from(line, col, start),
            ));
        }

        // Punctuation & operators
        self.lex_punct(line, col, start)
    }

    fn is_triple_quote(&self) -> bool {
        self.peek() == Some('"') && self.peek_at(1) == Some('"') && self.peek_at(2) == Some('"')
    }

    /// C#-style verbatim string `@"..."` where `""` represents a literal `"`.
    fn lex_verbatim_string(&mut self, line: usize, col: usize, start: usize) -> Result<String> {
        self.bump(); // opening "
        let mut out = String::new();
        while let Some(c) = self.bump() {
            if c == '"' {
                if self.peek() == Some('"') {
                    self.bump();
                    out.push('"');
                } else {
                    return Ok(out);
                }
            } else {
                out.push(c);
            }
        }
        Err(RSharpError::lex(
            self.span_from(line, col, start),
            "Unterminated verbatim string literal",
        ))
    }

    /// C# 11 raw string literal `"""..."""` (supports 3+ matching quotes).
    fn lex_triple_raw_string(&mut self, line: usize, col: usize, start: usize) -> Result<String> {
        let mut quotes = 0usize;
        while self.peek() == Some('"') {
            self.bump();
            quotes += 1;
        }
        let mut out = String::new();
        while !self.is_eof() {
            if self.peek() == Some('"') {
                let mut run = 0usize;
                while self.peek_at(run) == Some('"') {
                    run += 1;
                }
                if run >= quotes {
                    for _ in 0..(run - quotes) {
                        self.bump();
                        out.push('"');
                    }
                    for _ in 0..quotes {
                        self.bump();
                    }
                    return Ok(out);
                }
            }
            out.push(self.bump().unwrap());
        }
        Err(RSharpError::lex(
            self.span_from(line, col, start),
            "Unterminated raw string literal",
        ))
    }

    fn is_followed_by_lbrace(&self) -> bool {
        let mut idx = self.pos;
        while idx < self.bytes.len() {
            let b = self.bytes[idx];
            if b.is_ascii_whitespace() {
                idx += 1;
            } else {
                return b == b'{';
            }
        }
        false
    }

    fn peek_macro_delim(&self) -> Option<(char, char)> {
        let mut idx = self.pos;
        while idx < self.bytes.len() && self.bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }
        match self.bytes.get(idx).copied() {
            Some(b'(') => Some(('(', ')')),
            Some(b'[') => Some(('[', ']')),
            Some(b'{') => Some(('{', '}')),
            _ => None,
        }
    }

    fn skip_ascii_ws_pos(&mut self) -> (usize, usize, usize) {
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() {
                self.bump();
            } else {
                break;
            }
        }
        (self.line, self.col, self.pos)
    }

    fn lex_balanced_rust_group(
        &mut self,
        open: char,
        close: char,
        line: usize,
        col: usize,
        start: usize,
        err_msg: &str,
    ) -> Result<String> {
        while let Some(c) = self.peek() {
            if c.is_whitespace() {
                self.bump();
            } else {
                break;
            }
        }
        self.bump(); // opening delimiter
        let body_start = self.pos;
        let mut depth = 1usize;
        loop {
            let Some(c) = self.peek() else {
                return Err(RSharpError::lex(self.span_from(line, col, start), err_msg));
            };
            if c == open {
                self.bump();
                depth += 1;
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    let content = self.src[body_start..self.pos].to_string();
                    self.bump();
                    return Ok(content);
                }
                self.bump();
            } else {
                match c {
                    '"' => {
                        self.lex_string(self.line, self.col, self.pos)?;
                    }
                    'r' if self.starts_rust_raw_str() => {
                        self.skip_rust_raw_str(line, col, start)?;
                    }
                    '\'' => {
                        self.skip_rust_char_or_lifetime();
                    }
                    '/' if self.peek_at(1) == Some('/') => {
                        while let Some(nc) = self.peek() {
                            if nc == '\n' {
                                break;
                            }
                            self.bump();
                        }
                    }
                    '/' if self.peek_at(1) == Some('*') => {
                        self.skip_block_comment()?;
                    }
                    _ => {
                        self.bump();
                    }
                }
            }
        }
    }

    fn starts_rust_raw_str(&self) -> bool {
        let mut off = 1usize;
        while self.peek_at(off) == Some('#') {
            off += 1;
        }
        self.peek_at(off) == Some('"')
    }

    fn skip_rust_raw_str(&mut self, line: usize, col: usize, start: usize) -> Result<()> {
        self.bump(); // 'r'
        let mut hashes = 0usize;
        while self.peek() == Some('#') {
            self.bump();
            hashes += 1;
        }
        self.bump(); // '"'
        loop {
            let Some(ch) = self.bump() else {
                return Err(RSharpError::lex(
                    self.span_from(line, col, start),
                    "Unterminated raw string in 'rust { ... }' block",
                ));
            };
            if ch == '"' && (0..hashes).all(|i| self.peek_at(i) == Some('#')) {
                for _ in 0..hashes {
                    self.bump();
                }
                return Ok(());
            }
        }
    }

    fn skip_rust_char_or_lifetime(&mut self) {
        self.bump(); // '\''
        if self.peek() == Some('\\') {
            self.bump(); // '\\'
            let _ = self.bump();
            while let Some(ch) = self.bump() {
                if ch == '\'' {
                    break;
                }
            }
        } else if let Some(ch) = self.peek()
            && self.peek_at(ch.len_utf8()) == Some('\'')
        {
            self.bump();
            self.bump();
        }
    }

    fn lex_number(&mut self, line: usize, col: usize, start: usize) -> Result<Token> {
        if self.peek() == Some('0')
            && matches!(self.peek_at(1), Some('x' | 'X' | 'b' | 'B' | 'o' | 'O'))
        {
            self.bump();
            self.bump();
            while let Some(c) = self.peek() {
                if c.is_ascii_hexdigit() || c == '_' {
                    self.bump();
                } else {
                    break;
                }
            }
            self.consume_num_suffix();
            return Ok(Token::new(
                TokenKind::IntLit(self.src[start..self.pos].to_string()),
                self.span_from(line, col, start),
            ));
        }

        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || c == '_' {
                self.bump();
            } else {
                break;
            }
        }

        let mut is_float = false;
        let mut trailing_dot = false;
        if self.peek() == Some('.') {
            if self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
                is_float = true;
                self.bump();
                while let Some(c) = self.peek() {
                    if c.is_ascii_digit() || c == '_' {
                        self.bump();
                    } else {
                        break;
                    }
                }
            } else if !matches!(self.peek_at(1), Some('.' | '@'))
                && !self.peek_at(1).is_some_and(is_ident_start)
            {
                is_float = true;
                trailing_dot = true;
                self.bump();
            }
        }

        if !trailing_dot && matches!(self.peek(), Some('e' | 'E')) {
            is_float = true;
            self.bump();
            if matches!(self.peek(), Some('+' | '-')) {
                self.bump();
            }
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() || c == '_' {
                    self.bump();
                } else {
                    break;
                }
            }
        }

        if !trailing_dot {
            self.consume_num_suffix();
        }
        let text = self.src[start..self.pos].to_string();
        let kind = if is_float || text.ends_with("f32") || text.ends_with("f64") {
            TokenKind::FloatLit(text)
        } else {
            TokenKind::IntLit(text)
        };
        Ok(Token::new(kind, self.span_from(line, col, start)))
    }

    fn consume_num_suffix(&mut self) {
        if self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
            while let Some(c) = self.peek() {
                if c.is_ascii_alphanumeric() || c == '_' {
                    self.bump();
                } else {
                    break;
                }
            }
        }
    }

    fn lex_string(&mut self, line: usize, col: usize, start: usize) -> Result<String> {
        self.bump(); // opening "
        let mut out = String::new();
        while let Some(c) = self.bump() {
            match c {
                '"' => return Ok(out),
                '\\' => {
                    let Some(esc) = self.bump() else {
                        return Err(RSharpError::lex(
                            self.span_from(line, col, start),
                            "Unterminated escape sequence in string literal",
                        ));
                    };
                    out.push('\\');
                    out.push(esc);
                }
                other => out.push(other),
            }
        }
        Err(RSharpError::lex(
            self.span_from(line, col, start),
            "Unterminated string literal",
        ))
    }

    fn lex_char(&mut self, line: usize, col: usize, start: usize) -> Result<String> {
        self.bump(); // opening '
        let mut out = String::new();
        while let Some(c) = self.bump() {
            match c {
                '\'' => {
                    if out.is_empty() {
                        return Err(RSharpError::lex(
                            self.span_from(line, col, start),
                            "Empty char literal",
                        ));
                    }
                    return Ok(out);
                }
                '\\' => {
                    let Some(esc) = self.bump() else {
                        return Err(RSharpError::lex(
                            self.span_from(line, col, start),
                            "Unterminated escape sequence in char literal",
                        ));
                    };
                    out.push('\\');
                    out.push(esc);
                }
                other => out.push(other),
            }
        }
        Err(RSharpError::lex(
            self.span_from(line, col, start),
            "Unterminated char literal",
        ))
    }

    fn lex_interp_string(
        &mut self,
        line: usize,
        col: usize,
        start: usize,
    ) -> Result<Vec<InterpPart>> {
        let mut parts = Vec::new();
        let mut lit = String::new();
        while let Some(c) = self.bump() {
            match c {
                '"' => {
                    if !lit.is_empty() {
                        parts.push(InterpPart::Literal(lit));
                    }
                    return Ok(parts);
                }
                '\\' => {
                    let Some(esc) = self.bump() else {
                        return Err(RSharpError::lex(
                            self.span_from(line, col, start),
                            "Unterminated escape in interpolated string",
                        ));
                    };
                    lit.push('\\');
                    lit.push(esc);
                }
                '{' if self.peek() == Some('{') => {
                    self.bump();
                    lit.push_str("{{");
                }
                '}' if self.peek() == Some('}') => {
                    self.bump();
                    lit.push_str("}}");
                }
                '{' => {
                    if !lit.is_empty() {
                        parts.push(InterpPart::Literal(std::mem::take(&mut lit)));
                    }
                    let (expr_src, fmt) = self.lex_interp_hole(line, col, start)?;
                    parts.push(InterpPart::Expr {
                        source: expr_src,
                        format: fmt,
                    });
                }
                other => lit.push(other),
            }
        }
        Err(RSharpError::lex(
            self.span_from(line, col, start),
            "Unterminated interpolated string literal",
        ))
    }

    fn lex_interp_hole(
        &mut self,
        line: usize,
        col: usize,
        start: usize,
    ) -> Result<(String, Option<String>)> {
        let mut depth = 1usize;
        let mut paren_depth = 0usize;
        let mut expr_buf = String::new();
        let mut fmt_buf: Option<String> = None;
        while let Some(c) = self.bump() {
            match c {
                '{' => {
                    depth += 1;
                    push_hole_char(&mut expr_buf, &mut fmt_buf, c);
                }
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        let expr_trimmed = expr_buf.trim().to_string();
                        if expr_trimmed.is_empty() {
                            return Err(RSharpError::lex(
                                self.span_from(line, col, start),
                                "Empty interpolation expression '{}'",
                            ));
                        }
                        return Ok((expr_trimmed, fmt_buf));
                    }
                    push_hole_char(&mut expr_buf, &mut fmt_buf, c);
                }
                '(' | '[' => {
                    paren_depth += 1;
                    push_hole_char(&mut expr_buf, &mut fmt_buf, c);
                }
                ')' | ']' => {
                    paren_depth = paren_depth.saturating_sub(1);
                    push_hole_char(&mut expr_buf, &mut fmt_buf, c);
                }
                ':' if depth == 1
                    && paren_depth == 0
                    && fmt_buf.is_none()
                    && self.peek() != Some(':') =>
                {
                    fmt_buf = Some(String::new());
                }
                '"' => {
                    push_hole_char(&mut expr_buf, &mut fmt_buf, '"');
                    while let Some(sc) = self.bump() {
                        push_hole_char(&mut expr_buf, &mut fmt_buf, sc);
                        if sc == '\\' {
                            if let Some(esc) = self.bump() {
                                push_hole_char(&mut expr_buf, &mut fmt_buf, esc);
                            }
                        } else if sc == '"' {
                            break;
                        }
                    }
                }
                other => push_hole_char(&mut expr_buf, &mut fmt_buf, other),
            }
        }
        Err(RSharpError::lex(
            self.span_from(line, col, start),
            "Unterminated interpolation expression '{...}'",
        ))
    }

    fn lex_punct(&mut self, line: usize, col: usize, start: usize) -> Result<Token> {
        let c = self.bump().unwrap();
        let kind = match c {
            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            '{' => TokenKind::LBrace,
            '}' => TokenKind::RBrace,
            '[' => TokenKind::LBracket,
            ']' => TokenKind::RBracket,
            ',' => TokenKind::Comma,
            ';' => TokenKind::Semicolon,
            '~' => TokenKind::Tilde,
            ':' => {
                if self.peek() == Some(':') {
                    self.bump();
                    TokenKind::ColonColon
                } else {
                    TokenKind::Colon
                }
            }
            '.' => {
                if self.peek() == Some('.') {
                    self.bump();
                    if self.peek() == Some('.') {
                        self.bump();
                        TokenKind::DotDotDot
                    } else if self.peek() == Some('=') {
                        self.bump();
                        TokenKind::DotDotEq
                    } else {
                        TokenKind::DotDot
                    }
                } else {
                    TokenKind::Dot
                }
            }
            '?' => {
                if self.peek() == Some('?') {
                    self.bump();
                    TokenKind::QuestionQuestion
                } else {
                    TokenKind::Question
                }
            }
            '+' => match self.peek() {
                Some('+') => {
                    self.bump();
                    TokenKind::PlusPlus
                }
                Some('=') => {
                    self.bump();
                    TokenKind::PlusEq
                }
                _ => TokenKind::Plus,
            },
            '-' => match self.peek() {
                Some('-') => {
                    self.bump();
                    TokenKind::MinusMinus
                }
                Some('=') => {
                    self.bump();
                    TokenKind::MinusEq
                }
                Some('>') => {
                    self.bump();
                    TokenKind::ThinArrow
                }
                _ => TokenKind::Minus,
            },
            '*' => {
                if self.peek() == Some('=') {
                    self.bump();
                    TokenKind::StarEq
                } else {
                    TokenKind::Star
                }
            }
            '/' => {
                if self.peek() == Some('=') {
                    self.bump();
                    TokenKind::SlashEq
                } else {
                    TokenKind::Slash
                }
            }
            '%' => {
                if self.peek() == Some('=') {
                    self.bump();
                    TokenKind::PercentEq
                } else {
                    TokenKind::Percent
                }
            }
            '=' => match self.peek() {
                Some('=') => {
                    self.bump();
                    TokenKind::EqEq
                }
                Some('>') => {
                    self.bump();
                    TokenKind::FatArrow
                }
                _ => TokenKind::Eq,
            },
            '!' => {
                if self.peek() == Some('=') {
                    self.bump();
                    TokenKind::BangEq
                } else {
                    TokenKind::Bang
                }
            }
            '<' => match self.peek() {
                Some('=') => {
                    self.bump();
                    TokenKind::Le
                }
                Some('<') => {
                    self.bump();
                    if self.peek() == Some('=') {
                        self.bump();
                        TokenKind::ShlEq
                    } else {
                        TokenKind::Shl
                    }
                }
                _ => TokenKind::Lt,
            },
            '>' => match self.peek() {
                Some('=') => {
                    self.bump();
                    TokenKind::Ge
                }
                Some('>') => {
                    self.bump();
                    if self.peek() == Some('=') {
                        self.bump();
                        TokenKind::ShrEq
                    } else {
                        TokenKind::Shr
                    }
                }
                _ => TokenKind::Gt,
            },
            '&' => match self.peek() {
                Some('&') => {
                    self.bump();
                    TokenKind::AmpAmp
                }
                Some('=') => {
                    self.bump();
                    TokenKind::AmpEq
                }
                _ => TokenKind::Amp,
            },
            '|' => match self.peek() {
                Some('|') => {
                    self.bump();
                    TokenKind::PipePipe
                }
                Some('=') => {
                    self.bump();
                    TokenKind::PipeEq
                }
                _ => TokenKind::Pipe,
            },
            '^' => {
                if self.peek() == Some('=') {
                    self.bump();
                    TokenKind::CaretEq
                } else {
                    TokenKind::Caret
                }
            }
            '$' => TokenKind::Dollar,
            '#' => TokenKind::Hash,
            other => {
                return Err(RSharpError::lex(
                    self.span_from(line, col, start),
                    format!("Unexpected character '{}'", other),
                ));
            }
        };
        Ok(Token::new(kind, self.span_from(line, col, start)))
    }
    fn peek_next_non_ws(&self) -> Option<char> {
        self.src[self.pos..].chars().find(|c| !c.is_whitespace())
    }
}

fn is_attr_arg_prefix(tokens: &[Token]) -> bool {
    let mut i = tokens.len();
    if i < 2 || !matches!(tokens[i - 1].kind, TokenKind::Ident { .. }) {
        return false;
    }
    i -= 1;
    while i >= 2
        && matches!(tokens[i - 1].kind, TokenKind::Dot | TokenKind::ColonColon)
        && matches!(tokens[i - 2].kind, TokenKind::Ident { .. })
    {
        i -= 2;
    }
    if i >= 1 && matches!(tokens[i - 1].kind, TokenKind::Kw(Keyword::Unsafe)) {
        i -= 1;
    }
    if i >= 2
        && matches!(tokens[i - 1].kind, TokenKind::Colon)
        && matches!(
            &tokens[i - 2].kind,
            TokenKind::Ident { name, .. } if matches!(name.as_str(), "module" | "assembly" | "inner")
        )
    {
        i -= 2;
    }
    i >= 1 && matches!(tokens[i - 1].kind, TokenKind::LBracket)
}

fn push_hole_char(expr_buf: &mut String, fmt_buf: &mut Option<String>, c: char) {
    if let Some(fmt) = fmt_buf {
        fmt.push(c);
    } else {
        expr_buf.push(c);
    }
}

fn clean_block_doc(raw: &str) -> Vec<String> {
    let lines: Vec<String> = raw
        .lines()
        .map(|l| {
            let trimmed = l.trim();
            if let Some(rest) = trimmed.strip_prefix('*') {
                rest.strip_prefix(' ').unwrap_or(rest).to_string()
            } else {
                trimmed.to_string()
            }
        })
        .collect();
    let start = lines.iter().position(|l| !l.is_empty()).unwrap_or(0);
    let end = lines
        .iter()
        .rposition(|l| !l.is_empty())
        .map(|i| i + 1)
        .unwrap_or(start);
    if start >= end {
        vec![String::new()]
    } else {
        lines[start..end].to_vec()
    }
}

fn should_lex_raw_macro_body(tokens: &[Token]) -> bool {
    let n = tokens.len();
    if n >= 2
        && matches!(tokens[n - 2].kind, TokenKind::Kw(Keyword::Macro))
        && matches!(tokens[n - 1].kind, TokenKind::Ident { .. })
    {
        return true;
    }
    if n >= 2
        && matches!(tokens[n - 2].kind, TokenKind::Ident { .. })
        && matches!(tokens[n - 1].kind, TokenKind::Bang)
        && !(n >= 3 && matches!(tokens[n - 3].kind, TokenKind::Kw(Keyword::Break)))
    {
        return true;
    }
    false
}

fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

fn is_ident_continue(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}
