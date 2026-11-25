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

    fn tokenize_all(&mut self) -> Result<Vec<Token>> {
        let mut tokens = Vec::new();
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
                } else {
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.bump();
                    }
                }
            } else if self.peek() == Some('/') && self.peek_at(1) == Some('*') {
                self.skip_block_comment()?;
            } else {
                return Ok(());
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

        // Verbatim identifier: @ident
        if ch == '@' {
            self.bump();
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
                        "Expected identifier after '@'",
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
                let raw = self.lex_balanced_rust_block(line, col, start)?;
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

        // String literal
        if ch == '"' {
            let s = self.lex_string(line, col, start)?;
            return Ok(Token::new(
                TokenKind::StringLit(s),
                self.span_from(line, col, start),
            ));
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

    fn lex_balanced_rust_block(&mut self, line: usize, col: usize, start: usize) -> Result<String> {
        while let Some(c) = self.peek() {
            if c.is_whitespace() {
                self.bump();
            } else {
                break;
            }
        }
        // Consume opening '{'
        self.bump();
        let body_start = self.pos;
        let mut depth = 1usize;
        while depth > 0 {
            let Some(c) = self.peek() else {
                return Err(RSharpError::lex(
                    self.span_from(line, col, start),
                    "Unterminated 'rust { ... }' block",
                ));
            };
            match c {
                '{' => {
                    self.bump();
                    depth += 1;
                }
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        let content = self.src[body_start..self.pos].to_string();
                        self.bump();
                        return Ok(content);
                    }
                    self.bump();
                }
                '"' => {
                    self.lex_string(self.line, self.col, self.pos)?;
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
        unreachable!("loop returns when depth hits 0")
    }

    fn lex_number(&mut self, line: usize, col: usize, start: usize) -> Result<Token> {
        if self.peek() == Some('0')
            && matches!(
                self.peek_at(1),
                Some('x' | 'X' | 'b' | 'B' | 'o' | 'O')
            )
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
        if self.peek() == Some('.')
            && self.peek_at(1).is_some_and(|c| c.is_ascii_digit())
        {
            is_float = true;
            self.bump();
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() || c == '_' {
                    self.bump();
                } else {
                    break;
                }
            }
        }

        if matches!(self.peek(), Some('e' | 'E')) {
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

        self.consume_num_suffix();
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

    fn lex_interp_string(&mut self, line: usize, col: usize, start: usize) -> Result<Vec<InterpPart>> {
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

    fn lex_interp_hole(&mut self, line: usize, col: usize, start: usize) -> Result<(String, Option<String>)> {
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
                ':' if depth == 1 && paren_depth == 0 && fmt_buf.is_none() && self.peek() != Some(':') => {
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
                    if self.peek() == Some('=') {
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
            other => {
                return Err(RSharpError::lex(
                    self.span_from(line, col, start),
                    format!("Unexpected character '{}'", other),
                ));
            }
        };
        Ok(Token::new(kind, self.span_from(line, col, start)))
    }
}

fn push_hole_char(expr_buf: &mut String, fmt_buf: &mut Option<String>, c: char) {
    if let Some(fmt) = fmt_buf {
        fmt.push(c);
    } else {
        expr_buf.push(c);
    }
}

fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

fn is_ident_continue(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}
