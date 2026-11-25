use crate::ast::*;
use crate::error::{RSharpError, Result, Span};
use crate::lexer::tokenize;
use crate::token::{InterpPart, Keyword, Token, TokenKind};

/// Parses R# source code into a `FileAst`.
pub fn parse_file(source: &str) -> Result<FileAst> {
    let tokens = tokenize(source)?;
    let mut parser = Parser::new(tokens);
    parser.parse_file()
}

/// Parses a standalone R# expression (used by interpolated strings and tests).
pub fn parse_expr_str(source: &str) -> Result<Expr> {
    let tokens = tokenize(source)?;
    let mut parser = Parser::new(tokens);
    let expr = parser.parse_expr()?;
    if !parser.is_eof() {
        return Err(parser.err_here("Unexpected trailing tokens in expression"));
    }
    Ok(expr)
}

#[derive(Debug, Clone, Copy)]
struct Checkpoint {
    pos: usize,
    pending_gt: bool,
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    /// Set when a `>>` (`Shr`) token was split to close a nested generic `<...<...>>`.
    pending_gt: bool,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            pending_gt: false,
        }
    }

    fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            pos: self.pos,
            pending_gt: self.pending_gt,
        }
    }

    fn restore(&mut self, cp: Checkpoint) {
        self.pos = cp.pos;
        self.pending_gt = cp.pending_gt;
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    fn peek_kind(&self) -> &TokenKind {
        if self.pending_gt {
            &TokenKind::Gt
        } else {
            &self.peek().kind
        }
    }

    fn peek_at(&self, offset: usize) -> &Token {
        let idx = (self.pos + offset).min(self.tokens.len() - 1);
        &self.tokens[idx]
    }

    fn is_eof(&self) -> bool {
        !self.pending_gt && matches!(self.peek_kind(), TokenKind::Eof)
    }

    fn bump(&mut self) -> Token {
        if self.pending_gt {
            self.pending_gt = false;
            let span = self.peek().span;
            self.pos += 1;
            return Token::new(TokenKind::Gt, span);
        }
        let tok = self.peek().clone();
        if !matches!(tok.kind, TokenKind::Eof) {
            self.pos += 1;
        }
        tok
    }

    fn check_gt(&self) -> bool {
        self.pending_gt || matches!(self.peek().kind, TokenKind::Gt | TokenKind::Shr)
    }

    fn eat_gt(&mut self) -> bool {
        if self.pending_gt {
            self.pending_gt = false;
            self.pos += 1;
            true
        } else if matches!(self.peek().kind, TokenKind::Gt) {
            self.pos += 1;
            true
        } else if matches!(self.peek().kind, TokenKind::Shr) {
            self.pending_gt = true;
            true
        } else {
            false
        }
    }

    fn expect_gt(&mut self) -> Result<()> {
        if self.eat_gt() {
            Ok(())
        } else {
            Err(self.err_here(format!("Expected '>', found {:?}", self.peek_kind())))
        }
    }

    fn span(&self) -> Span {
        self.peek().span
    }

    fn err_here(&self, msg: impl Into<String>) -> RSharpError {
        RSharpError::parse(self.span(), msg)
    }

    fn check(&self, kind: &TokenKind) -> bool {
        self.peek_kind() == kind
    }

    fn check_kw(&self, kw: Keyword) -> bool {
        matches!(self.peek_kind(), TokenKind::Kw(k) if *k == kw)
    }

    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.check(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn eat_kw(&mut self, kw: Keyword) -> bool {
        if self.check_kw(kw) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kind: &TokenKind, expected: &str) -> Result<Token> {
        if self.check(kind) {
            Ok(self.bump())
        } else {
            Err(self.err_here(format!(
                "Expected {}, found {:?}",
                expected,
                self.peek_kind()
            )))
        }
    }

    fn expect_kw(&mut self, kw: Keyword) -> Result<()> {
        if self.eat_kw(kw) {
            Ok(())
        } else {
            Err(self.err_here(format!(
                "Expected keyword '{}', found {:?}",
                kw.as_str(),
                self.peek_kind()
            )))
        }
    }

    fn expect_ident(&mut self) -> Result<String> {
        match self.peek_kind().clone() {
            TokenKind::Ident { name, .. } => {
                self.bump();
                Ok(name)
            }
            _ => Err(self.err_here(format!(
                "Expected identifier, found {:?}",
                self.peek_kind()
            ))),
        }
    }

    /// Accepts identifiers and contextual keywords after `.` or `::` (e.g. `Box.new`, `reader.read`).
    fn expect_member_ident(&mut self) -> Result<String> {
        match self.peek_kind().clone() {
            TokenKind::Ident { name, .. } => {
                self.bump();
                Ok(name)
            }
            TokenKind::Kw(kw) => {
                self.bump();
                Ok(kw.as_str().to_string())
            }
            _ => Err(self.err_here(format!(
                "Expected member identifier, found {:?}",
                self.peek_kind()
            ))),
        }
    }

    fn expect_path_segment_ident(&mut self) -> Result<String> {
        match self.peek_kind().clone() {
            TokenKind::Ident { name, .. } => {
                self.bump();
                Ok(name)
            }
            TokenKind::Kw(Keyword::SelfType) => {
                self.bump();
                Ok("Self".to_string())
            }
            TokenKind::Kw(Keyword::Super) => {
                self.bump();
                Ok("super".to_string())
            }
            TokenKind::Kw(Keyword::Crate) => {
                self.bump();
                Ok("crate".to_string())
            }
            _ => Err(self.err_here(format!(
                "Expected path identifier, found {:?}",
                self.peek_kind()
            ))),
        }
    }

    fn collect_docs(&mut self) -> Vec<String> {
        let mut docs = Vec::new();
        while let TokenKind::DocComment(text) = self.peek_kind().clone() {
            self.bump();
            docs.push(text);
        }
        docs
    }

    fn parse_attrs(&mut self) -> Result<Vec<Attribute>> {
        let mut attrs = Vec::new();
        while self.check(&TokenKind::LBracket) {
            attrs.push(self.parse_attr()?);
        }
        Ok(attrs)
    }

    fn parse_attr(&mut self) -> Result<Attribute> {
        self.expect(&TokenKind::LBracket, "'['")?;
        let inner = matches!(
            (self.peek_kind(), &self.peek_at(1).kind),
            (TokenKind::Ident { name, .. }, TokenKind::Colon) if name == "module" || name == "assembly"
        );
        if inner {
            self.bump();
            self.bump();
        }
        let mut name = self.expect_member_ident()?;
        while self.eat(&TokenKind::Dot) || self.eat(&TokenKind::ColonColon) {
            name.push_str("::");
            name.push_str(&self.expect_member_ident()?);
        }
        let args = if self.check(&TokenKind::LParen) {
            Some(self.collect_balanced_raw(TokenKind::LParen, TokenKind::RParen)?)
        } else if self.eat(&TokenKind::Eq) {
            let mut rhs = String::from("= ");
            while !self.check(&TokenKind::RBracket) && !self.is_eof() {
                let tok = self.bump();
                rhs.push_str(&self.token_to_raw_str(&tok));
            }
            Some(rhs)
        } else {
            None
        };
        self.expect(&TokenKind::RBracket, "']'")?;
        Ok(Attribute { inner, name, args })
    }

    fn collect_balanced_raw(&mut self, open: TokenKind, close: TokenKind) -> Result<String> {
        self.expect(&open, "opening delimiter")?;
        let mut depth = 1usize;
        let mut parts = Vec::new();
        while depth > 0 {
            if self.is_eof() {
                return Err(self.err_here("Unterminated delimiter group"));
            }
            let tok = self.bump();
            if tok.kind == open {
                depth += 1;
                parts.push(self.token_to_raw_str(&tok));
            } else if tok.kind == close {
                depth -= 1;
                if depth > 0 {
                    parts.push(self.token_to_raw_str(&tok));
                }
            } else {
                parts.push(self.token_to_raw_str(&tok));
            }
        }
        Ok(join_raw_tokens(&parts))
    }

    fn token_to_raw_str(&self, tok: &Token) -> String {
        match &tok.kind {
            TokenKind::Kw(kw) => kw.as_str().to_string(),
            TokenKind::Ident { name, .. } => name.clone(),
            TokenKind::IntLit(s) | TokenKind::FloatLit(s) => s.clone(),
            TokenKind::StringLit(s) => format!("\"{}\"", s),
            TokenKind::CharLit(c) => format!("'{}'", c),
            TokenKind::InterpString(parts) => format_interp_raw(parts),
            TokenKind::RustBlock(s) => format!("{{{}}}", s),
            TokenKind::DocComment(s) => format!("/// {}", s),
            TokenKind::LParen => "(".into(),
            TokenKind::RParen => ")".into(),
            TokenKind::LBrace => "{".into(),
            TokenKind::RBrace => "}".into(),
            TokenKind::LBracket => "[".into(),
            TokenKind::RBracket => "]".into(),
            TokenKind::Comma => ",".into(),
            TokenKind::Semicolon => ";".into(),
            TokenKind::Colon => ":".into(),
            TokenKind::ColonColon => "::".into(),
            TokenKind::Dot => ".".into(),
            TokenKind::DotDot => "..".into(),
            TokenKind::DotDotEq => "..=".into(),
            TokenKind::Question => "?".into(),
            TokenKind::QuestionQuestion => "??".into(),
            TokenKind::FatArrow => "=>".into(),
            TokenKind::ThinArrow => "->".into(),
            TokenKind::Plus => "+".into(),
            TokenKind::PlusPlus => "++".into(),
            TokenKind::PlusEq => "+=".into(),
            TokenKind::Minus => "-".into(),
            TokenKind::MinusMinus => "--".into(),
            TokenKind::MinusEq => "-=".into(),
            TokenKind::Star => "*".into(),
            TokenKind::StarEq => "*=".into(),
            TokenKind::Slash => "/".into(),
            TokenKind::SlashEq => "/=".into(),
            TokenKind::Percent => "%".into(),
            TokenKind::PercentEq => "%=".into(),
            TokenKind::Eq => "=".into(),
            TokenKind::EqEq => "==".into(),
            TokenKind::Bang => "!".into(),
            TokenKind::BangEq => "!=".into(),
            TokenKind::Lt => "<".into(),
            TokenKind::Le => "<=".into(),
            TokenKind::Gt => ">".into(),
            TokenKind::Ge => ">=".into(),
            TokenKind::Amp => "&".into(),
            TokenKind::AmpAmp => "&&".into(),
            TokenKind::AmpEq => "&=".into(),
            TokenKind::Pipe => "|".into(),
            TokenKind::PipePipe => "||".into(),
            TokenKind::PipeEq => "|=".into(),
            TokenKind::Caret => "^".into(),
            TokenKind::CaretEq => "^=".into(),
            TokenKind::Tilde => "~".into(),
            TokenKind::Shl => "<<".into(),
            TokenKind::ShlEq => "<<=".into(),
            TokenKind::Shr => ">>".into(),
            TokenKind::ShrEq => ">>=".into(),
            TokenKind::Dollar => "$".into(),
            TokenKind::Eof => String::new(),
        }
    }

    fn parse_visibility(&mut self) -> Result<Visibility> {
        if self.eat_kw(Keyword::Public) {
            if self.eat(&TokenKind::LParen) {
                self.expect_kw(Keyword::Super)?;
                self.expect(&TokenKind::RParen, "')'")?;
                Ok(Visibility::Super)
            } else {
                Ok(Visibility::Public)
            }
        } else if self.eat_kw(Keyword::Internal) {
            Ok(Visibility::Internal)
        } else {
            self.eat_kw(Keyword::Private);
            Ok(Visibility::Private)
        }
    }

    fn parse_file(&mut self) -> Result<FileAst> {
        let mut inner_attrs = Vec::new();
        let mut items = Vec::new();
        while !self.is_eof() {
            let docs = self.collect_docs();
            if self.is_eof() {
                break;
            }
            let attrs = self.parse_attrs()?;
            if docs.is_empty() && !attrs.is_empty() && attrs.iter().all(|a| a.inner) {
                inner_attrs.extend(attrs);
                continue;
            }
            let (inners, outers): (Vec<_>, Vec<_>) = attrs.into_iter().partition(|a| a.inner);
            inner_attrs.extend(inners);
            let item = self.parse_item_after_meta(docs, outers)?;
            items.push(item);
        }
        Ok(FileAst { inner_attrs, items })
    }

    fn parse_item(&mut self) -> Result<Item> {
        let docs = self.collect_docs();
        let attrs = self.parse_attrs()?;
        self.parse_item_after_meta(docs, attrs)
    }

    fn parse_item_after_meta(&mut self, docs: Vec<String>, attrs: Vec<Attribute>) -> Result<Item> {
        // Inline `rust { ... }` requires no visibility
        if let TokenKind::RustBlock(raw) = self.peek_kind().clone() {
            self.bump();
            return Ok(Item {
                docs,
                attrs,
                vis: Visibility::Private,
                kind: ItemKind::InlineRust(raw),
            });
        }

        let vis = self.parse_visibility()?;

        if self.check_kw(Keyword::Namespace) {
            let ns = self.parse_namespace()?;
            return Ok(Item {
                docs,
                attrs,
                vis,
                kind: ItemKind::Namespace(ns),
            });
        }
        if self.check_kw(Keyword::Using) {
            let u = self.parse_using()?;
            return Ok(Item {
                docs,
                attrs,
                vis,
                kind: ItemKind::Using(u),
            });
        }
        if self.check_kw(Keyword::Struct) {
            let s = self.parse_struct()?;
            return Ok(Item {
                docs,
                attrs,
                vis,
                kind: ItemKind::Struct(s),
            });
        }
        if self.check_kw(Keyword::Enum) {
            let e = self.parse_enum()?;
            return Ok(Item {
                docs,
                attrs,
                vis,
                kind: ItemKind::Enum(e),
            });
        }
        if self.check_kw(Keyword::Union) {
            let u = self.parse_union()?;
            return Ok(Item {
                docs,
                attrs,
                vis,
                kind: ItemKind::Union(u),
            });
        }
        if self.check_kw(Keyword::Trait) {
            let t = self.parse_trait(false)?;
            return Ok(Item {
                docs,
                attrs,
                vis,
                kind: ItemKind::Trait(t),
            });
        }
        if self.check_kw(Keyword::Impl) && self.is_impl_block() {
            let i = self.parse_impl(false)?;
            return Ok(Item {
                docs,
                attrs,
                vis,
                kind: ItemKind::Impl(i),
            });
        }
        if self.check_kw(Keyword::Alias) {
            let a = self.parse_alias()?;
            return Ok(Item {
                docs,
                attrs,
                vis,
                kind: ItemKind::Alias(a),
            });
        }
        if self.check_kw(Keyword::Macro) {
            let m = self.parse_macro_decl()?;
            return Ok(Item {
                docs,
                attrs,
                vis,
                kind: ItemKind::MacroDecl(m),
            });
        }
        if self.check_kw(Keyword::Unsafe) {
            if matches!(self.peek_at(1).kind, TokenKind::Kw(Keyword::Trait)) {
                self.bump();
                let t = self.parse_trait(true)?;
                return Ok(Item {
                    docs,
                    attrs,
                    vis,
                    kind: ItemKind::Trait(t),
                });
            }
            if matches!(self.peek_at(1).kind, TokenKind::Kw(Keyword::Impl)) {
                self.bump();
                let i = self.parse_impl(true)?;
                return Ok(Item {
                    docs,
                    attrs,
                    vis,
                    kind: ItemKind::Impl(i),
                });
            }
        }
        if self.check_kw(Keyword::Extern) && self.is_extern_block() {
            let eb = self.parse_extern_block()?;
            return Ok(Item {
                docs,
                attrs,
                vis,
                kind: ItemKind::ExternBlock(eb),
            });
        }

        // Distinguish `const Type Name = Expr;` vs `const ReturnType Fn(...)`
        if self.check_kw(Keyword::Const) && self.is_const_item() {
            let c = self.parse_const_decl()?;
            return Ok(Item {
                docs,
                attrs,
                vis,
                kind: ItemKind::Const(c),
            });
        }

        // Distinguish `static mut? Type Name = Expr;` vs `static ReturnType Fn(...)`
        if self.check_kw(Keyword::Static) && self.is_static_item() {
            let s = self.parse_static_decl()?;
            return Ok(Item {
                docs,
                attrs,
                vis,
                kind: ItemKind::Static(s),
            });
        }

        // Item-level macro call `foo!(...);`
        if let Some(mc) = self.try_parse_item_macro_call()? {
            return Ok(Item {
                docs,
                attrs,
                vis,
                kind: ItemKind::MacroCall(mc),
            });
        }

        // Otherwise, function/method declaration
        let func = self.parse_fn_decl()?;
        Ok(Item {
            docs,
            attrs,
            vis,
            kind: ItemKind::Fn(func),
        })
    }

    fn is_impl_block(&mut self) -> bool {
        let save = self.checkpoint();
        let is_block = if self.eat_kw(Keyword::Impl) {
            if self.check(&TokenKind::Lt) {
                true
            } else if self.parse_type().is_ok() {
                self.check_kw(Keyword::For)
                    || self.check_kw(Keyword::Where)
                    || self.check(&TokenKind::LBrace)
            } else {
                false
            }
        } else {
            false
        };
        self.restore(save);
        is_block
    }

    fn is_extern_block(&self) -> bool {
        if !self.check_kw(Keyword::Extern) {
            return false;
        }
        match &self.peek_at(1).kind {
            TokenKind::LBrace => true,
            TokenKind::StringLit(_) => matches!(self.peek_at(2).kind, TokenKind::LBrace),
            _ => false,
        }
    }

    fn is_const_item(&mut self) -> bool {
        let save = self.pos;
        let is_item = if self.eat_kw(Keyword::Const) {
            if self.parse_type().is_ok() && self.expect_ident().is_ok() {
                self.check(&TokenKind::Eq) || self.check(&TokenKind::Semicolon)
            } else {
                false
            }
        } else {
            false
        };
        self.pos = save;
        is_item
    }

    fn is_static_item(&mut self) -> bool {
        let save = self.pos;
        let is_item = if self.eat_kw(Keyword::Static) {
            let has_mut = self.eat_kw(Keyword::Mut);
            if has_mut {
                true
            } else if self.parse_type().is_ok() && self.expect_ident().is_ok() {
                self.check(&TokenKind::Eq) || self.check(&TokenKind::Semicolon)
            } else {
                false
            }
        } else {
            false
        };
        self.pos = save;
        is_item
    }

    fn try_parse_item_macro_call(&mut self) -> Result<Option<MacroCall>> {
        let save = self.pos;
        if let Ok(path) = self.parse_simple_path()
            && self.eat(&TokenKind::Bang) {
                let mc = self.parse_macro_call_body(path)?;
                if mc.delim != MacroDelim::Brace {
                    self.expect(&TokenKind::Semicolon, "';' after macro invocation")?;
                } else {
                    self.eat(&TokenKind::Semicolon);
                }
                return Ok(Some(mc));
            }
        self.pos = save;
        Ok(None)
    }

    fn parse_simple_path(&mut self) -> Result<Vec<String>> {
        let mut segs = vec![self.expect_path_segment_ident()?];
        while self.eat(&TokenKind::Dot) || self.eat(&TokenKind::ColonColon) {
            segs.push(self.expect_member_ident()?);
        }
        Ok(segs)
    }

    fn parse_namespace(&mut self) -> Result<NamespaceDecl> {
        self.expect_kw(Keyword::Namespace)?;
        let path = self.parse_simple_path()?;
        if self.eat(&TokenKind::Semicolon) {
            Ok(NamespaceDecl { path, items: None })
        } else {
            self.expect(&TokenKind::LBrace, "'{' or ';'")?;
            let mut items = Vec::new();
            while !self.check(&TokenKind::RBrace) && !self.is_eof() {
                items.push(self.parse_item()?);
            }
            self.expect(&TokenKind::RBrace, "'}'")?;
            Ok(NamespaceDecl {
                path,
                items: Some(items),
            })
        }
    }

    fn parse_using(&mut self) -> Result<UsingDecl> {
        self.expect_kw(Keyword::Using)?;
        // Check `using Alias = path;`
        let alias = if matches!(
            (self.peek_kind(), &self.peek_at(1).kind),
            (TokenKind::Ident { .. }, TokenKind::Eq)
        ) {
            let a = self.expect_ident()?;
            self.bump(); // '='
            Some(a)
        } else {
            None
        };

        let mut path = vec![self.expect_path_segment_ident()?];
        let mut wildcard = false;
        while self.eat(&TokenKind::Dot) || self.eat(&TokenKind::ColonColon) {
            if self.eat(&TokenKind::Star) {
                wildcard = true;
                break;
            }
            path.push(self.expect_member_ident()?);
        }
        self.expect(&TokenKind::Semicolon, "';' after using declaration")?;
        Ok(UsingDecl {
            path,
            wildcard,
            alias,
        })
    }

    fn parse_generic_params(&mut self) -> Result<Vec<GenericParam>> {
        if !self.eat(&TokenKind::Lt) {
            return Ok(Vec::new());
        }
        let mut params = Vec::new();
        while !self.check_gt() && !self.is_eof() {
            if self.eat_kw(Keyword::Life) {
                let name = self.expect_ident()?;
                params.push(GenericParam {
                    kind: GenericParamKind::Lifetime(name),
                });
            } else if self.eat_kw(Keyword::Const) {
                let ty = self.parse_type()?;
                let name = self.expect_ident()?;
                params.push(GenericParam {
                    kind: GenericParamKind::Const { ty, name },
                });
            } else {
                let name = self.expect_ident()?;
                let mut bounds = Vec::new();
                if self.eat(&TokenKind::Colon) {
                    bounds.push(self.parse_type_bound()?);
                    while self.eat(&TokenKind::Plus) {
                        bounds.push(self.parse_type_bound()?);
                    }
                }
                params.push(GenericParam {
                    kind: GenericParamKind::Type { name, bounds },
                });
            }
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect_gt()?;
        Ok(params)
    }

    fn parse_where_clause(&mut self) -> Result<Option<WhereClause>> {
        if !self.eat_kw(Keyword::Where) {
            return Ok(None);
        }
        let mut predicates = Vec::new();
        while !self.check(&TokenKind::LBrace)
            && !self.check(&TokenKind::Semicolon)
            && !self.is_eof()
        {
            if self.eat_kw(Keyword::Life) {
                let life = self.expect_ident()?;
                self.expect(&TokenKind::Colon, "':' in lifetime bound")?;
                let mut outlives = vec![self.expect_lifetime_name()?];
                while self.check(&TokenKind::Comma) && !self.next_starts_where_predicate() {
                    self.bump();
                    outlives.push(self.expect_lifetime_name()?);
                }
                predicates.push(WherePredicate::Lifetime { life, outlives });
            } else {
                let ty = self.parse_type()?;
                self.expect(&TokenKind::Colon, "':' in where constraint")?;
                let mut bounds = vec![self.parse_type_bound()?];
                while (self.check(&TokenKind::Comma) && !self.next_starts_where_predicate())
                    || self.check(&TokenKind::Plus)
                {
                    self.bump();
                    if self.check(&TokenKind::LBrace) || self.check(&TokenKind::Semicolon) {
                        break;
                    }
                    bounds.push(self.parse_type_bound()?);
                }
                predicates.push(WherePredicate::Type { ty, bounds });
            }
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        Ok(Some(WhereClause { predicates }))
    }

    /// Lookahead after a `,` inside `where` to distinguish `, Display` (another bound)
    /// from `, U : Clone` or `, life a : b` (a new predicate).
    fn next_starts_where_predicate(&mut self) -> bool {
        let save = self.pos;
        self.bump(); // skip ','
        let starts = if self.check(&TokenKind::LBrace) || self.check(&TokenKind::Semicolon) {
            true
        } else if self.eat_kw(Keyword::Life) {
            self.expect_ident().is_ok() && self.check(&TokenKind::Colon)
        } else {
            self.parse_type().is_ok() && self.check(&TokenKind::Colon)
        };
        self.pos = save;
        starts
    }

    fn expect_lifetime_name(&mut self) -> Result<String> {
        if self.eat_kw(Keyword::Static) {
            return Ok("static".to_string());
        }
        self.eat_kw(Keyword::Life);
        self.expect_ident()
    }

    fn parse_type_bound(&mut self) -> Result<TypeBound> {
        if self.eat_kw(Keyword::Life) {
            let name = self.expect_lifetime_name()?;
            return Ok(TypeBound::Lifetime(name));
        }
        if self.eat_kw(Keyword::Static) {
            return Ok(TypeBound::Lifetime("static".to_string()));
        }
        Ok(TypeBound::Trait(self.parse_type_path()?))
    }

    fn parse_struct(&mut self) -> Result<StructDecl> {
        self.expect_kw(Keyword::Struct)?;
        let name = self.expect_ident()?;
        let generics = self.parse_generic_params()?;

        // Tuple struct: `struct Meters(public double);`
        if self.eat(&TokenKind::LParen) {
            let mut fields = Vec::new();
            while !self.check(&TokenKind::RParen) && !self.is_eof() {
                let attrs = self.parse_attrs()?;
                let vis = self.parse_visibility()?;
                let ty = self.parse_type()?;
                fields.push(TupleField { attrs, vis, ty });
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::RParen, "')'")?;
            let where_clause = self.parse_where_clause()?;
            self.expect(&TokenKind::Semicolon, "';'")?;
            return Ok(StructDecl {
                name,
                generics,
                where_clause,
                kind: StructKind::Tuple(fields),
                methods: Vec::new(),
            });
        }

        let where_clause = self.parse_where_clause()?;

        // Unit struct: `struct Marker;`
        if self.eat(&TokenKind::Semicolon) {
            return Ok(StructDecl {
                name,
                generics,
                where_clause,
                kind: StructKind::Unit,
                methods: Vec::new(),
            });
        }

        // Named struct with optional inline methods
        self.expect(&TokenKind::LBrace, "'{'")?;
        let mut fields = Vec::new();
        let mut methods = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_eof() {
            let docs = self.collect_docs();
            if self.check(&TokenKind::RBrace) {
                break;
            }
            let attrs = self.parse_attrs()?;
            let vis = self.parse_visibility()?;
            if self.is_field_decl() {
                let ty = self.parse_type()?;
                let fname = self.expect_ident()?;
                self.expect(&TokenKind::Semicolon, "';' after struct field")?;
                fields.push(FieldDecl {
                    docs,
                    attrs,
                    vis,
                    ty,
                    name: fname,
                });
            } else {
                let func = self.parse_fn_decl()?;
                methods.push(Item {
                    docs,
                    attrs,
                    vis,
                    kind: ItemKind::Fn(func),
                });
            }
        }
        self.expect(&TokenKind::RBrace, "'}'")?;
        Ok(StructDecl {
            name,
            generics,
            where_clause,
            kind: StructKind::Named(fields),
            methods,
        })
    }

    fn is_field_decl(&mut self) -> bool {
        if self.check_kw(Keyword::Const)
            || self.check_kw(Keyword::Async)
            || self.check_kw(Keyword::Unsafe)
            || self.check_kw(Keyword::Static)
            || self.check_kw(Keyword::Extern)
        {
            return false;
        }
        let save = self.pos;
        let ok = self.parse_type().is_ok()
            && self.expect_ident().is_ok()
            && self.check(&TokenKind::Semicolon);
        self.pos = save;
        ok
    }

    fn parse_enum(&mut self) -> Result<EnumDecl> {
        self.expect_kw(Keyword::Enum)?;
        let name = self.expect_ident()?;
        let generics = self.parse_generic_params()?;
        let where_clause = self.parse_where_clause()?;
        self.expect(&TokenKind::LBrace, "'{'")?;
        let mut variants = Vec::new();
        let mut methods = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_eof() {
            let docs = self.collect_docs();
            if self.check(&TokenKind::RBrace) {
                break;
            }
            let attrs = self.parse_attrs()?;
            if self.is_enum_method_start() {
                let vis = self.parse_visibility()?;
                let func = self.parse_fn_decl()?;
                methods.push(Item {
                    docs,
                    attrs,
                    vis,
                    kind: ItemKind::Fn(func),
                });
                continue;
            }
            let vname = self.expect_ident()?;
            let kind = if self.eat(&TokenKind::LParen) {
                let mut tuple_fields = Vec::new();
                while !self.check(&TokenKind::RParen) && !self.is_eof() {
                    let ty = self.parse_type()?;
                    let opt_name = if matches!(self.peek_kind(), TokenKind::Ident { .. }) {
                        Some(self.expect_ident()?)
                    } else {
                        None
                    };
                    tuple_fields.push(VariantTupleField { ty, name: opt_name });
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RParen, "')'")?;
                VariantKind::Tuple(tuple_fields)
            } else if self.eat(&TokenKind::LBrace) {
                let mut fields = Vec::new();
                while !self.check(&TokenKind::RBrace) && !self.is_eof() {
                    let fdocs = self.collect_docs();
                    let fattrs = self.parse_attrs()?;
                    let ty = self.parse_type()?;
                    let fname = self.expect_ident()?;
                    if !self.eat(&TokenKind::Semicolon) {
                        self.eat(&TokenKind::Comma);
                    }
                    fields.push(FieldDecl {
                        docs: fdocs,
                        attrs: fattrs,
                        vis: Visibility::Private,
                        ty,
                        name: fname,
                    });
                }
                self.expect(&TokenKind::RBrace, "'}'")?;
                VariantKind::Named(fields)
            } else {
                VariantKind::Unit
            };
            let discriminant = if self.eat(&TokenKind::Eq) {
                Some(self.parse_expr()?)
            } else {
                None
            };
            self.eat(&TokenKind::Comma);
            variants.push(EnumVariant {
                docs,
                attrs,
                name: vname,
                kind,
                discriminant,
            });
        }
        self.expect(&TokenKind::RBrace, "'}'")?;
        Ok(EnumDecl {
            name,
            generics,
            where_clause,
            variants,
            methods,
        })
    }

    fn is_enum_method_start(&self) -> bool {
        matches!(
            self.peek_kind(),
            TokenKind::Kw(
                Keyword::Public
                    | Keyword::Internal
                    | Keyword::Private
                    | Keyword::Const
                    | Keyword::Async
                    | Keyword::Unsafe
                    | Keyword::Static
                    | Keyword::Extern
            )
        )
    }

    fn parse_union(&mut self) -> Result<UnionDecl> {
        self.expect_kw(Keyword::Union)?;
        let name = self.expect_ident()?;
        let generics = self.parse_generic_params()?;
        let where_clause = self.parse_where_clause()?;
        self.expect(&TokenKind::LBrace, "'{'")?;
        let mut fields = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_eof() {
            let docs = self.collect_docs();
            let attrs = self.parse_attrs()?;
            let vis = self.parse_visibility()?;
            let ty = self.parse_type()?;
            let fname = self.expect_ident()?;
            self.expect(&TokenKind::Semicolon, "';'")?;
            fields.push(FieldDecl {
                docs,
                attrs,
                vis,
                ty,
                name: fname,
            });
        }
        self.expect(&TokenKind::RBrace, "'}'")?;
        Ok(UnionDecl {
            name,
            generics,
            where_clause,
            fields,
        })
    }

    fn parse_trait(&mut self, is_unsafe: bool) -> Result<TraitDecl> {
        self.expect_kw(Keyword::Trait)?;
        let name = self.expect_ident()?;
        let generics = self.parse_generic_params()?;
        let mut supertraits = Vec::new();
        if self.eat(&TokenKind::Colon) {
            supertraits.push(self.parse_type_bound()?);
            while self.eat(&TokenKind::Comma) || self.eat(&TokenKind::Plus) {
                supertraits.push(self.parse_type_bound()?);
            }
        }
        let where_clause = self.parse_where_clause()?;
        self.expect(&TokenKind::LBrace, "'{'")?;
        let mut items = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_eof() {
            let docs = self.collect_docs();
            if self.check(&TokenKind::RBrace) {
                break;
            }
            let attrs = self.parse_attrs()?;
            if self.eat_kw(Keyword::Type) {
                let aname = self.expect_ident()?;
                let mut bounds = Vec::new();
                if self.eat(&TokenKind::Colon) {
                    bounds.push(self.parse_type_bound()?);
                    while self.eat(&TokenKind::Plus) || self.eat(&TokenKind::Comma) {
                        bounds.push(self.parse_type_bound()?);
                    }
                }
                let default = if self.eat(&TokenKind::Eq) {
                    Some(self.parse_type()?)
                } else {
                    None
                };
                self.expect(&TokenKind::Semicolon, "';'")?;
                items.push(TraitItem::AssocType {
                    attrs,
                    name: aname,
                    bounds,
                    default,
                });
            } else if self.check_kw(Keyword::Const) && self.is_const_item() {
                self.bump();
                let ty = self.parse_type()?;
                let cname = self.expect_ident()?;
                let default = if self.eat(&TokenKind::Eq) {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                self.expect(&TokenKind::Semicolon, "';'")?;
                items.push(TraitItem::AssocConst {
                    attrs,
                    ty,
                    name: cname,
                    default,
                });
            } else {
                let vis = self.parse_visibility()?;
                let func = self.parse_fn_decl()?;
                items.push(TraitItem::Fn(Item {
                    docs,
                    attrs,
                    vis,
                    kind: ItemKind::Fn(func),
                }));
            }
        }
        self.expect(&TokenKind::RBrace, "'}'")?;
        Ok(TraitDecl {
            is_unsafe,
            name,
            generics,
            supertraits,
            where_clause,
            items,
        })
    }

    fn parse_impl(&mut self, is_unsafe: bool) -> Result<ImplDecl> {
        self.expect_kw(Keyword::Impl)?;
        let generics = self.parse_generic_params()?;
        let first_ty = self.parse_type()?;
        let (trait_path, target_ty) = if self.eat_kw(Keyword::For) {
            let tp = match first_ty {
                Type::Path(p) => p,
                _ => return Err(self.err_here("Expected trait path before 'for' in impl")),
            };
            let target = self.parse_type()?;
            (Some(tp), target)
        } else {
            (None, first_ty)
        };
        let where_clause = self.parse_where_clause()?;
        self.expect(&TokenKind::LBrace, "'{'")?;
        let mut items = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_eof() {
            let docs = self.collect_docs();
            if self.check(&TokenKind::RBrace) {
                break;
            }
            let attrs = self.parse_attrs()?;
            if self.eat_kw(Keyword::Type) {
                let aname = self.expect_ident()?;
                self.expect(&TokenKind::Eq, "'='")?;
                let ty = self.parse_type()?;
                self.expect(&TokenKind::Semicolon, "';'")?;
                items.push(ImplItem::AssocType {
                    attrs,
                    name: aname,
                    ty,
                });
            } else {
                let vis = self.parse_visibility()?;
                if self.check_kw(Keyword::Const) && self.is_const_item() {
                    let c = self.parse_const_decl()?;
                    items.push(ImplItem::AssocConst(Item {
                        docs,
                        attrs,
                        vis,
                        kind: ItemKind::Const(c),
                    }));
                } else {
                    let func = self.parse_fn_decl()?;
                    items.push(ImplItem::Fn(Item {
                        docs,
                        attrs,
                        vis,
                        kind: ItemKind::Fn(func),
                    }));
                }
            }
        }
        self.expect(&TokenKind::RBrace, "'}'")?;
        Ok(ImplDecl {
            is_unsafe,
            generics,
            trait_path,
            target_ty,
            where_clause,
            items,
        })
    }

    fn parse_alias(&mut self) -> Result<AliasDecl> {
        self.expect_kw(Keyword::Alias)?;
        let name = self.expect_ident()?;
        let generics = self.parse_generic_params()?;
        self.expect(&TokenKind::Eq, "'='")?;
        let target = self.parse_type()?;
        self.expect(&TokenKind::Semicolon, "';'")?;
        Ok(AliasDecl {
            name,
            generics,
            target,
        })
    }

    fn parse_const_decl(&mut self) -> Result<ConstDecl> {
        self.expect_kw(Keyword::Const)?;
        let ty = self.parse_type()?;
        let name = self.expect_ident()?;
        self.expect(&TokenKind::Eq, "'='")?;
        let value = self.parse_expr()?;
        self.expect(&TokenKind::Semicolon, "';'")?;
        Ok(ConstDecl { ty, name, value })
    }

    fn parse_static_decl(&mut self) -> Result<StaticDecl> {
        self.expect_kw(Keyword::Static)?;
        let is_mut = self.eat_kw(Keyword::Mut);
        let ty = self.parse_type()?;
        let name = self.expect_ident()?;
        self.expect(&TokenKind::Eq, "'='")?;
        let value = self.parse_expr()?;
        self.expect(&TokenKind::Semicolon, "';'")?;
        Ok(StaticDecl {
            is_mut,
            ty,
            name,
            value,
        })
    }

    fn parse_extern_block(&mut self) -> Result<ExternBlock> {
        self.expect_kw(Keyword::Extern)?;
        let abi = if let TokenKind::StringLit(s) = self.peek_kind().clone() {
            self.bump();
            Some(s)
        } else {
            None
        };
        self.expect(&TokenKind::LBrace, "'{'")?;
        let mut items = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_eof() {
            items.push(self.parse_item()?);
        }
        self.expect(&TokenKind::RBrace, "'}'")?;
        Ok(ExternBlock { abi, items })
    }

    fn parse_macro_decl(&mut self) -> Result<MacroDecl> {
        self.expect_kw(Keyword::Macro)?;
        let name = self.expect_ident()?;
        let body = self.collect_balanced_raw(TokenKind::LBrace, TokenKind::RBrace)?;
        Ok(MacroDecl { name, body })
    }

    fn parse_fn_decl(&mut self) -> Result<FnDecl> {
        let mut is_const = false;
        let mut is_async = false;
        let mut is_unsafe = false;
        let mut is_static = false;
        let mut extern_abi = None;

        loop {
            if self.eat_kw(Keyword::Const) {
                is_const = true;
            } else if self.eat_kw(Keyword::Async) {
                is_async = true;
            } else if self.eat_kw(Keyword::Unsafe) {
                is_unsafe = true;
            } else if self.eat_kw(Keyword::Static) {
                is_static = true;
            } else if self.eat_kw(Keyword::Extern) {
                let abi = if let TokenKind::StringLit(s) = self.peek_kind().clone() {
                    self.bump();
                    s
                } else {
                    "C".to_string()
                };
                extern_abi = Some(abi);
            } else {
                break;
            }
        }

        let return_ty = self.parse_type()?;
        let name = self.expect_ident()?;
        let generics = self.parse_generic_params()?;
        let params = self.parse_params()?;
        let where_clause = self.parse_where_clause()?;
        let body = if self.eat(&TokenKind::Semicolon) {
            None
        } else {
            Some(self.parse_block()?)
        };

        Ok(FnDecl {
            is_const,
            is_async,
            is_unsafe,
            is_static,
            extern_abi,
            return_ty,
            name,
            generics,
            params,
            where_clause,
            body,
        })
    }

    fn parse_params(&mut self) -> Result<Vec<Param>> {
        self.expect(&TokenKind::LParen, "'('")?;
        let mut params = Vec::new();
        while !self.check(&TokenKind::RParen) && !self.is_eof() {
            if let Some(recv) = self.try_parse_receiver()? {
                params.push(Param::Receiver(recv));
            } else {
                let is_mut = self.eat_kw(Keyword::Mut);
                let ty = self.parse_type()?;
                let name = self.expect_ident()?;
                params.push(Param::Normal { is_mut, ty, name });
            }
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RParen, "')'")?;
        Ok(params)
    }

    fn try_parse_receiver(&mut self) -> Result<Option<ReceiverKind>> {
        let save = self.pos;
        // `read this` or `read<a> this`
        if self.eat_kw(Keyword::Read) {
            let life = self.try_parse_lifetime_angle()?;
            if self.eat_kw(Keyword::This) {
                return Ok(Some(ReceiverKind::Read(life)));
            }
            self.pos = save;
        }
        // `write this` or `write<a> this`
        if self.eat_kw(Keyword::Write) {
            let life = self.try_parse_lifetime_angle()?;
            if self.eat_kw(Keyword::This) {
                return Ok(Some(ReceiverKind::Write(life)));
            }
            self.pos = save;
        }
        // `take this` or `take Box<Self> this`
        if self.eat_kw(Keyword::Take) {
            if self.eat_kw(Keyword::This) {
                return Ok(Some(ReceiverKind::Take));
            }
            if let Ok(ty) = self.parse_type()
                && self.eat_kw(Keyword::This) {
                    return Ok(Some(ReceiverKind::Typed { ty }));
                }
            self.pos = save;
        }
        // Arbitrary typed receiver `Pin<write Self> this`
        if let Ok(ty) = self.parse_type()
            && self.eat_kw(Keyword::This) {
                return Ok(Some(ReceiverKind::Typed { ty }));
            }
        self.pos = save;
        Ok(None)
    }

    fn try_parse_lifetime_angle(&mut self) -> Result<Option<String>> {
        let save = self.pos;
        if self.eat(&TokenKind::Lt) {
            let life = if self.eat_kw(Keyword::Static) {
                "static".to_string()
            } else if let TokenKind::Ident { name, .. } = self.peek_kind().clone() {
                self.bump();
                name
            } else {
                self.pos = save;
                return Ok(None);
            };
            if self.eat(&TokenKind::Gt) {
                return Ok(Some(life));
            }
            self.pos = save;
        }
        Ok(None)
    }

    // ==================== TYPES ====================

    fn parse_type(&mut self) -> Result<Type> {
        let base = if self.eat_kw(Keyword::Void) {
            Type::Void
        } else if self.eat_kw(Keyword::Never) {
            Type::Never
        } else if self.eat_kw(Keyword::Dynamic) {
            Type::Dynamic
        } else if self.eat_kw(Keyword::Read) {
            let life = self.try_parse_lifetime_angle()?;
            if life.is_none() && self.eat_kw(Keyword::Ptr) {
                self.expect(&TokenKind::Lt, "'<'")?;
                let inner = self.parse_type()?;
                self.expect_gt()?;
                Type::ReadPtr(Box::new(inner))
            } else {
                let inner = self.parse_type()?;
                Type::Read {
                    life,
                    inner: Box::new(inner),
                }
            }
        } else if self.eat_kw(Keyword::Write) {
            let life = self.try_parse_lifetime_angle()?;
            if life.is_none() && self.eat_kw(Keyword::Ptr) {
                self.expect(&TokenKind::Lt, "'<'")?;
                let inner = self.parse_type()?;
                self.expect_gt()?;
                Type::WritePtr(Box::new(inner))
            } else {
                let inner = self.parse_type()?;
                Type::Write {
                    life,
                    inner: Box::new(inner),
                }
            }
        } else if self.eat_kw(Keyword::Slice) {
            self.expect(&TokenKind::Lt, "'<' after 'slice'")?;
            let inner = self.parse_type()?;
            self.expect_gt()?;
            Type::Slice(Box::new(inner))
        } else if self.eat_kw(Keyword::Dyn) {
            let mut bounds = vec![self.parse_type_bound()?];
            while self.eat(&TokenKind::Plus) {
                bounds.push(self.parse_type_bound()?);
            }
            Type::DynTrait(bounds)
        } else if self.eat_kw(Keyword::Impl) {
            let mut bounds = vec![self.parse_type_bound()?];
            while self.eat(&TokenKind::Plus) {
                bounds.push(self.parse_type_bound()?);
            }
            Type::ImplTrait(bounds)
        } else if self.eat(&TokenKind::LParen) {
            if self.eat(&TokenKind::RParen) {
                Type::Void
            } else {
                let mut elems = Vec::new();
                while !self.check(&TokenKind::RParen) && !self.is_eof() {
                    let ty = self.parse_type()?;
                    let name = if matches!(self.peek_kind(), TokenKind::Ident { .. }) {
                        Some(self.expect_ident()?)
                    } else {
                        None
                    };
                    elems.push(TupleTypeElem { ty, name });
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RParen, "')'")?;
                // Check for function pointer type `(int, int) -> int`
                if self.eat(&TokenKind::ThinArrow) {
                    let params = elems.into_iter().map(|e| e.ty).collect();
                    let ret = self.parse_type()?;
                    Type::FnPtr {
                        params,
                        ret: Box::new(ret),
                    }
                } else {
                    Type::Tuple(elems)
                }
            }
        } else {
            Type::Path(self.parse_type_path()?)
        };

        // Postfix array dims `T[N]` / `T[R, C]` and optional `T?`
        let mut ty = base;
        loop {
            if self.eat(&TokenKind::LBracket) {
                let mut dims = Vec::new();
                while !self.check(&TokenKind::RBracket) && !self.is_eof() {
                    dims.push(self.parse_expr()?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RBracket, "']'")?;
                if dims.is_empty() {
                    return Err(self.err_here(
                        "Array type requires explicit length(s), e.g. 'int[4]' or use 'read slice<int>'",
                    ));
                }
                ty = Type::Array {
                    elem: Box::new(ty),
                    dims,
                };
            } else if self.eat(&TokenKind::Question) {
                ty = Type::Optional(Box::new(ty));
            } else {
                break;
            }
        }
        Ok(ty)
    }

    fn parse_type_path(&mut self) -> Result<TypePath> {
        let mut segments = Vec::new();
        let first_name = self.expect_path_segment_ident()?;
        let first_args = self.parse_optional_generic_args()?;
        segments.push(TypePathSegment {
            name: first_name,
            args: first_args,
        });
        while self.eat(&TokenKind::Dot) || self.eat(&TokenKind::ColonColon) {
            let name = self.expect_member_ident()?;
            let args = self.parse_optional_generic_args()?;
            segments.push(TypePathSegment { name, args });
        }
        Ok(TypePath { segments })
    }

    fn parse_optional_generic_args(&mut self) -> Result<Vec<GenericArg>> {
        if !self.eat(&TokenKind::Lt) {
            return Ok(Vec::new());
        }
        let args = self.parse_generic_arg_list()?;
        self.expect_gt()?;
        Ok(args)
    }

    fn parse_generic_arg_list(&mut self) -> Result<Vec<GenericArg>> {
        let mut args = Vec::new();
        while !self.check_gt() && !self.is_eof() {
            if self.eat_kw(Keyword::Life) {
                args.push(GenericArg::Lifetime(self.expect_lifetime_name()?));
            } else if matches!(
                (self.peek_kind(), &self.peek_at(1).kind),
                (TokenKind::Ident { .. }, TokenKind::Eq)
            ) {
                let name = self.expect_ident()?;
                self.bump(); // '='
                let ty = self.parse_type()?;
                args.push(GenericArg::AssocBinding { name, ty });
            } else if matches!(
                self.peek_kind(),
                TokenKind::IntLit(_) | TokenKind::Kw(Keyword::True | Keyword::False)
            ) {
                let expr = self.parse_primary()?;
                args.push(GenericArg::ConstExpr(expr));
            } else {
                args.push(GenericArg::Type(self.parse_type()?));
            }
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        Ok(args)
    }

    // ==================== BLOCKS & STATEMENTS ====================

    fn parse_block(&mut self) -> Result<Block> {
        self.expect(&TokenKind::LBrace, "'{'")?;
        let mut stmts = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_eof() {
            stmts.push(self.parse_stmt()?);
        }
        self.expect(&TokenKind::RBrace, "'}'")?;
        Ok(Block { stmts })
    }

    fn parse_stmt(&mut self) -> Result<Stmt> {
        // Reject implicit typing explicitly
        if self.check_kw(Keyword::Var) {
            return Err(self.err_here(
                "Implicit typing with 'var' is forbidden in R#; specify an explicit type",
            ));
        }
        if self.check_kw(Keyword::Let) {
            return Err(self.err_here(
                "Rust-style 'let' is not used in R#; declare variables with an explicit type (e.g. 'int x = 10;')",
            ));
        }

        // Inline rust block statement
        if let TokenKind::RustBlock(raw) = self.peek_kind().clone() {
            self.bump();
            return Ok(Stmt::InlineRust(raw));
        }

        // Return / Break / Continue
        if self.eat_kw(Keyword::Return) {
            let expr = if self.check(&TokenKind::Semicolon) {
                None
            } else {
                Some(self.parse_expr()?)
            };
            self.expect(&TokenKind::Semicolon, "';' after return")?;
            return Ok(Stmt::Return(expr));
        }
        if self.eat_kw(Keyword::Break) {
            let (label, value) = self.parse_break_payload()?;
            self.expect(&TokenKind::Semicolon, "';' after break")?;
            return Ok(Stmt::Break { label, value });
        }
        if self.eat_kw(Keyword::Continue) {
            let label = if matches!(self.peek_kind(), TokenKind::Ident { .. }) {
                Some(self.expect_ident()?)
            } else {
                None
            };
            self.expect(&TokenKind::Semicolon, "';' after continue")?;
            return Ok(Stmt::Continue { label });
        }

        // Control flow statements
        if self.check_kw(Keyword::If) {
            return Ok(Stmt::If(self.parse_if()?));
        }
        if self.check_kw(Keyword::Require) {
            return self.parse_require();
        }
        if self.check_kw(Keyword::While) {
            return self.parse_while(None);
        }
        if self.check_kw(Keyword::For) {
            return self.parse_for(None);
        }
        if self.check_kw(Keyword::Foreach) {
            return self.parse_foreach(None);
        }
        if self.check_kw(Keyword::Loop) {
            return self.parse_loop(None);
        }
        if self.check_kw(Keyword::Match) {
            return Ok(Stmt::Match(self.parse_match()?));
        }
        if self.check_kw(Keyword::Unsafe) && matches!(self.peek_at(1).kind, TokenKind::LBrace) {
            self.bump();
            return Ok(Stmt::Unsafe(self.parse_block()?));
        }

        // Labeled loop: `outer: loop/while/for/foreach`
        if matches!(
            (self.peek_kind(), &self.peek_at(1).kind, &self.peek_at(2).kind),
            (
                TokenKind::Ident { .. },
                TokenKind::Colon,
                TokenKind::Kw(Keyword::Loop | Keyword::While | Keyword::For | Keyword::Foreach)
            )
        ) {
            let label = self.expect_ident()?;
            self.bump(); // ':'
            return match self.peek_kind() {
                TokenKind::Kw(Keyword::Loop) => self.parse_loop(Some(label)),
                TokenKind::Kw(Keyword::While) => self.parse_while(Some(label)),
                TokenKind::Kw(Keyword::For) => self.parse_for(Some(label)),
                _ => self.parse_foreach(Some(label)),
            };
        }

        // Discard statement: `_ = expr;`
        if matches!(
            (self.peek_kind(), &self.peek_at(1).kind),
            (TokenKind::Ident { name, .. }, TokenKind::Eq) if name == "_"
        ) {
            self.bump();
            self.bump();
            let expr = self.parse_expr()?;
            self.expect(&TokenKind::Semicolon, "';'")?;
            return Ok(Stmt::Discard(expr));
        }

        // Explicit local variable declaration starting with `impl Trait name = ...`
        if self.check_kw(Keyword::Impl)
            && let Some(stmt) = self.try_parse_local_decl(true)?
        {
            return Ok(stmt);
        }

        // Nested item statements (const, static, struct, enum, trait, impl, alias, using)
        if self.is_nested_item_start() {
            let item = self.parse_item()?;
            return Ok(Stmt::Item(Box::new(item)));
        }

        // Explicit tuple destructuring: `(int a, mut string b) = pair;`
        if let Some(stmt) = self.try_parse_tuple_destructure()? {
            return Ok(stmt);
        }

        // Explicit local variable declaration: `shadow? mut? Type name (= expr)?;`
        if let Some(stmt) = self.try_parse_local_decl(true)? {
            return Ok(stmt);
        }

        // Expression statement (enforce explicit return — disallow bare trailing expr before '}')
        let expr = self.parse_expr()?;
        if self.check(&TokenKind::RBrace) {
            return Err(self.err_here(
                "Implicit return is not allowed in R#; use an explicit 'return <expr>;' statement",
            ));
        }
        self.expect(&TokenKind::Semicolon, "';' after statement")?;
        Ok(Stmt::Expr(expr))
    }

    fn parse_break_payload(&mut self) -> Result<(Option<String>, Option<Expr>)> {
        if self.check(&TokenKind::Semicolon) {
            return Ok((None, None));
        }
        // `break label;` or `break label value;`
        if let TokenKind::Ident { .. } = self.peek_kind() {
            let next = &self.peek_at(1).kind;
            // If followed by `;` we treat a lone ident after `break` as label if it matches a label,
            // OR as a value? Wait: in R# `break outer;` vs `break outer expr;`.
            // If followed by `;`: treat as label when no operators follow? Actually wait:
            // `break;` -> (None, None)
            // `break outer;` -> label `Some("outer")` (standard loop label break)
            // `break outer expr;` -> label `Some("outer")`, value `Some(expr)`!
            // `break (expr);` or `break 42;` -> (None, Some(expr))
            if matches!(next, TokenKind::Semicolon) {
                let lbl = self.expect_ident()?;
                return Ok((Some(lbl), None));
            }
            if !is_binary_or_postfix_continuation(next) {
                let save = self.pos;
                let lbl = self.expect_ident()?;
                if let Ok(val) = self.parse_expr()
                    && self.check(&TokenKind::Semicolon) {
                        return Ok((Some(lbl), Some(val)));
                    }
                self.pos = save;
            }
        }
        let val = self.parse_expr()?;
        Ok((None, Some(val)))
    }

    fn is_nested_item_start(&self) -> bool {
        matches!(
            self.peek_kind(),
            TokenKind::Kw(
                Keyword::Const
                    | Keyword::Static
                    | Keyword::Struct
                    | Keyword::Enum
                    | Keyword::Union
                    | Keyword::Trait
                    | Keyword::Impl
                    | Keyword::Alias
                    | Keyword::Using
                    | Keyword::Macro
            )
        )
    }

    fn try_parse_tuple_destructure(&mut self) -> Result<Option<Stmt>> {
        if !self.check(&TokenKind::LParen) {
            return Ok(None);
        }
        let save = self.pos;
        self.bump(); // '('
        let mut bindings = Vec::new();
        while !self.check(&TokenKind::RParen) && !self.is_eof() {
            let is_mut = self.eat_kw(Keyword::Mut);
            let Ok(ty) = self.parse_type() else {
                self.pos = save;
                return Ok(None);
            };
            let Ok(name) = self.expect_ident() else {
                self.pos = save;
                return Ok(None);
            };
            bindings.push((is_mut, ty, name));
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        if !bindings.is_empty() && self.eat(&TokenKind::RParen) && self.eat(&TokenKind::Eq) {
            let init = self.parse_expr()?;
            self.expect(&TokenKind::Semicolon, "';'")?;
            return Ok(Some(Stmt::DestructureTuple { bindings, init }));
        }
        self.pos = save;
        Ok(None)
    }

    fn try_parse_local_decl(&mut self, expect_semi: bool) -> Result<Option<Stmt>> {
        let save = self.checkpoint();
        let is_shadow = self.eat_kw(Keyword::Shadow);
        let is_mut = self.eat_kw(Keyword::Mut);
        let forced = is_shadow || is_mut;

        let ty = match self.parse_type() {
            Ok(t) => t,
            Err(e) => {
                if e.to_string().contains("Array type requires explicit length") {
                    return Err(e);
                }
                self.restore(save);
                if forced {
                    return Err(e);
                }
                return Ok(None);
            }
        };

        let name = match self.expect_ident() {
            Ok(n) => n,
            Err(e) => {
                self.restore(save);
                if forced {
                    return Err(e);
                }
                return Ok(None);
            }
        };

        if !self.check(&TokenKind::Eq) && !self.check(&TokenKind::Semicolon) {
            self.restore(save);
            if forced {
                return Err(self.err_here("Expected '=' or ';' in variable declaration"));
            }
            return Ok(None);
        }

        let init = if self.eat(&TokenKind::Eq) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        if expect_semi {
            self.expect(&TokenKind::Semicolon, "';' after variable declaration")?;
        }
        Ok(Some(Stmt::Local {
            is_shadow,
            is_mut,
            ty,
            name,
            init,
        }))
    }

    fn parse_condition(&mut self) -> Result<Condition> {
        let expr = self.parse_expr()?;
        if let Expr::IsPattern {
            expr: inner,
            negated,
            pat,
        } = expr
        {
            Ok(Condition::IsPattern {
                expr: *inner,
                negated,
                pat,
            })
        } else {
            Ok(Condition::Bool(expr))
        }
    }

    fn parse_if(&mut self) -> Result<IfStmt> {
        self.expect_kw(Keyword::If)?;
        self.expect(&TokenKind::LParen, "'(' after 'if'")?;
        let cond = self.parse_condition()?;
        self.expect(&TokenKind::RParen, "')'")?;
        let then_block = self.parse_block()?;
        let else_branch = if self.eat_kw(Keyword::Else) {
            if self.check_kw(Keyword::If) {
                Some(ElseBranch::ElseIf(Box::new(self.parse_if()?)))
            } else {
                Some(ElseBranch::Else(self.parse_block()?))
            }
        } else {
            None
        };
        Ok(IfStmt {
            cond,
            then_block,
            else_branch,
        })
    }

    fn parse_require(&mut self) -> Result<Stmt> {
        self.expect_kw(Keyword::Require)?;
        self.expect(&TokenKind::LParen, "'(' after 'require'")?;
        let cond = self.parse_condition()?;
        self.expect(&TokenKind::RParen, "')'")?;
        self.expect_kw(Keyword::Else)?;
        let else_block = self.parse_block()?;
        match cond {
            Condition::IsPattern {
                expr,
                negated: false,
                pat,
            } => Ok(Stmt::Require {
                expr,
                pat,
                else_block,
            }),
            _ => Err(self.err_here("'require' expects a pattern condition '(expr is Pattern)'")),
        }
    }

    fn parse_while(&mut self, label: Option<String>) -> Result<Stmt> {
        self.expect_kw(Keyword::While)?;
        self.expect(&TokenKind::LParen, "'(' after 'while'")?;
        let cond = self.parse_condition()?;
        self.expect(&TokenKind::RParen, "')'")?;
        let body = self.parse_block()?;
        Ok(Stmt::While { label, cond, body })
    }

    fn parse_for(&mut self, label: Option<String>) -> Result<Stmt> {
        self.expect_kw(Keyword::For)?;
        self.expect(&TokenKind::LParen, "'(' after 'for'")?;
        let init = if self.eat(&TokenKind::Semicolon) {
            None
        } else if let Some(local) = self.try_parse_local_decl(true)? {
            Some(Box::new(local))
        } else {
            let e = self.parse_expr()?;
            self.expect(&TokenKind::Semicolon, "';' in for loop")?;
            Some(Box::new(Stmt::Expr(e)))
        };
        let cond = if self.eat(&TokenKind::Semicolon) {
            None
        } else {
            let c = self.parse_expr()?;
            self.expect(&TokenKind::Semicolon, "';' in for loop")?;
            Some(c)
        };
        let step = if self.check(&TokenKind::RParen) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.expect(&TokenKind::RParen, "')'")?;
        let body = self.parse_block()?;
        Ok(Stmt::For {
            label,
            init,
            cond,
            step,
            body,
        })
    }

    fn parse_foreach(&mut self, label: Option<String>) -> Result<Stmt> {
        self.expect_kw(Keyword::Foreach)?;
        self.expect(&TokenKind::LParen, "'(' after 'foreach'")?;
        let pat = self.parse_pattern()?;
        self.expect_kw(Keyword::In)?;
        let iter = self.parse_expr()?;
        self.expect(&TokenKind::RParen, "')'")?;
        let body = self.parse_block()?;
        Ok(Stmt::Foreach {
            label,
            pat,
            iter,
            body,
        })
    }

    fn parse_loop(&mut self, label: Option<String>) -> Result<Stmt> {
        self.expect_kw(Keyword::Loop)?;
        let body = self.parse_block()?;
        Ok(Stmt::Loop { label, body })
    }

    fn parse_match(&mut self) -> Result<MatchExpr> {
        self.expect_kw(Keyword::Match)?;
        self.expect(&TokenKind::LParen, "'(' after 'match'")?;
        let scrutinee = Box::new(self.parse_expr()?);
        self.expect(&TokenKind::RParen, "')'")?;
        self.expect(&TokenKind::LBrace, "'{'")?;
        let mut arms = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_eof() {
            let pat = self.parse_pattern()?;
            let guard = if self.eat_kw(Keyword::When) {
                Some(self.parse_expr()?)
            } else {
                None
            };
            self.expect(&TokenKind::FatArrow, "'=>' in match arm")?;
            let body = if self.check(&TokenKind::LBrace) {
                MatchArmBody::Block(self.parse_block()?)
            } else {
                let expr = self.parse_expr()?;
                MatchArmBody::Expr(expr)
            };
            self.eat(&TokenKind::Comma);
            arms.push(MatchArm { pat, guard, body });
        }
        self.expect(&TokenKind::RBrace, "'}'")?;
        Ok(MatchExpr { scrutinee, arms })
    }

    // ==================== PATTERNS ====================

    fn parse_pattern(&mut self) -> Result<Pattern> {
        let first = self.parse_single_pattern()?;
        if !self.check(&TokenKind::Pipe) {
            return Ok(first);
        }
        let mut alts = vec![first];
        while self.eat(&TokenKind::Pipe) {
            alts.push(self.parse_single_pattern()?);
        }
        Ok(Pattern::Or(alts))
    }

    fn parse_single_pattern(&mut self) -> Result<Pattern> {
        // Wildcard `_`
        if matches!(self.peek_kind(), TokenKind::Ident { name, .. } if name == "_") {
            self.bump();
            return Ok(Pattern::Wildcard);
        }

        // Literal or Range pattern (`1`, `-1`, `1..=5`, `'a'..='z'`, `"foo"`, `true`)
        if let Some(pat) = self.try_parse_lit_or_range_pattern()? {
            return Ok(pat);
        }

        // Tuple pattern `(int a, string b)`
        if self.eat(&TokenKind::LParen) {
            let mut pats = Vec::new();
            while !self.check(&TokenKind::RParen) && !self.is_eof() {
                pats.push(self.parse_pattern()?);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::RParen, "')'")?;
            return Ok(Pattern::Tuple(pats));
        }

        // Typed binding pattern: `mut? Type name` (e.g. `int x`, `mut int x`, `read Account acc`, `write int x`)
        if let Some(binding) = self.try_parse_typed_pattern_binding()? {
            return Ok(binding);
        }

        // Path / TupleStruct / Struct pattern: `Some(int v)`, `Shape.Rect { double W; double H; }`, `None`
        let path = self.parse_simple_path()?;
        if self.eat(&TokenKind::LParen) {
            let mut fields = Vec::new();
            while !self.check(&TokenKind::RParen) && !self.is_eof() {
                fields.push(self.parse_pattern()?);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::RParen, "')'")?;
            return Ok(Pattern::TupleStruct { path, fields });
        }
        if self.eat(&TokenKind::LBrace) {
            let mut fields = Vec::new();
            let mut rest = false;
            while !self.check(&TokenKind::RBrace) && !self.is_eof() {
                if self.eat(&TokenKind::DotDot) {
                    rest = true;
                    break;
                }
                let f = self.parse_struct_pat_field()?;
                fields.push(f);
                if !self.eat(&TokenKind::Semicolon) {
                    self.eat(&TokenKind::Comma);
                }
            }
            self.expect(&TokenKind::RBrace, "'}'")?;
            return Ok(Pattern::Struct { path, fields, rest });
        }
        Ok(Pattern::Path(path))
    }

    fn parse_struct_pat_field(&mut self) -> Result<StructPatField> {
        let save = self.pos;
        // `mut? Type FieldName`
        let is_mut = self.eat_kw(Keyword::Mut);
        if let Ok(ty) = self.parse_type()
            && let Ok(name) = self.expect_ident()
                && matches!(
                    self.peek_kind(),
                    TokenKind::Semicolon | TokenKind::Comma | TokenKind::RBrace
                ) {
                    return Ok(StructPatField {
                        ty: Some(ty),
                        is_mut,
                        name,
                        pat: None,
                    });
                }
        self.pos = save;
        let name = self.expect_ident()?;
        let pat = if self.eat(&TokenKind::Colon) || self.eat(&TokenKind::Eq) {
            Some(self.parse_pattern()?)
        } else {
            None
        };
        Ok(StructPatField {
            ty: None,
            is_mut: false,
            name,
            pat,
        })
    }

    fn try_parse_lit_or_range_pattern(&mut self) -> Result<Option<Pattern>> {
        let save = self.pos;
        let lit = match self.peek_kind().clone() {
            TokenKind::IntLit(s) => {
                self.bump();
                Literal::Int(s)
            }
            TokenKind::FloatLit(s) => {
                self.bump();
                Literal::Float(s)
            }
            TokenKind::StringLit(s) => {
                self.bump();
                Literal::Str(s)
            }
            TokenKind::CharLit(c) => {
                self.bump();
                Literal::Char(c)
            }
            TokenKind::Kw(Keyword::True) => {
                self.bump();
                Literal::Bool(true)
            }
            TokenKind::Kw(Keyword::False) => {
                self.bump();
                Literal::Bool(false)
            }
            TokenKind::Minus => {
                self.bump();
                match self.peek_kind().clone() {
                    TokenKind::IntLit(s) => {
                        self.bump();
                        Literal::Int(format!("-{}", s))
                    }
                    TokenKind::FloatLit(s) => {
                        self.bump();
                        Literal::Float(format!("-{}", s))
                    }
                    _ => {
                        self.pos = save;
                        return Ok(None);
                    }
                }
            }
            _ => return Ok(None),
        };

        if self.eat(&TokenKind::DotDotEq) || self.eat(&TokenKind::DotDot) {
            let inclusive = matches!(self.tokens[self.pos - 1].kind, TokenKind::DotDotEq);
            let end_expr = self.parse_primary()?;
            return Ok(Some(Pattern::Range {
                start: Box::new(Expr::Lit(lit)),
                end: Box::new(end_expr),
                inclusive,
            }));
        }

        Ok(Some(Pattern::Lit(lit)))
    }

    fn try_parse_typed_pattern_binding(&mut self) -> Result<Option<Pattern>> {
        let save = self.pos;
        let is_mut = self.eat_kw(Keyword::Mut);
        let Ok(ty) = self.parse_type() else {
            self.pos = save;
            return Ok(None);
        };
        let Ok(name) = self.expect_ident() else {
            self.pos = save;
            return Ok(None);
        };
        // Ensure we don't accidentally swallow a variant like `Some(x)`
        if self.check(&TokenKind::LParen) || self.check(&TokenKind::LBrace) {
            self.pos = save;
            return Ok(None);
        }
        let ref_mode = match &ty {
            Type::Read { .. } => Some(RefMode::Read),
            Type::Write { .. } => Some(RefMode::Write),
            _ => None,
        };
        Ok(Some(Pattern::Binding {
            is_mut,
            ref_mode,
            ty: Some(ty),
            name,
        }))
    }

    // ==================== EXPRESSIONS ====================

    fn parse_expr(&mut self) -> Result<Expr> {
        self.parse_assign()
    }

    fn parse_assign(&mut self) -> Result<Expr> {
        let lhs = self.parse_ternary()?;
        let op = match self.peek_kind() {
            TokenKind::Eq => Some(AssignOp::Assign),
            TokenKind::PlusEq => Some(AssignOp::AddAssign),
            TokenKind::MinusEq => Some(AssignOp::SubAssign),
            TokenKind::StarEq => Some(AssignOp::MulAssign),
            TokenKind::SlashEq => Some(AssignOp::DivAssign),
            TokenKind::PercentEq => Some(AssignOp::RemAssign),
            TokenKind::AmpEq => Some(AssignOp::BitAndAssign),
            TokenKind::PipeEq => Some(AssignOp::BitOrAssign),
            TokenKind::CaretEq => Some(AssignOp::BitXorAssign),
            TokenKind::ShlEq => Some(AssignOp::ShlAssign),
            TokenKind::ShrEq => Some(AssignOp::ShrAssign),
            _ => None,
        };
        if let Some(op) = op {
            self.bump();
            let rhs = self.parse_assign()?;
            return Ok(Expr::Assign {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            });
        }
        Ok(lhs)
    }

    fn parse_ternary(&mut self) -> Result<Expr> {
        let expr = self.parse_coalesce()?;
        if self.eat(&TokenKind::Question) {
            let then_expr = self.parse_expr()?;
            self.expect(&TokenKind::Colon, "':' in ternary expression")?;
            let else_expr = self.parse_ternary()?;
            return Ok(Expr::Ternary {
                cond: Box::new(expr),
                then_expr: Box::new(then_expr),
                else_expr: Box::new(else_expr),
            });
        }
        Ok(expr)
    }

    fn parse_coalesce(&mut self) -> Result<Expr> {
        let lhs = self.parse_range()?;
        if self.eat(&TokenKind::QuestionQuestion) {
            let rhs = self.parse_coalesce()?;
            return Ok(Expr::Coalesce {
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            });
        }
        Ok(lhs)
    }

    fn parse_range(&mut self) -> Result<Expr> {
        if self.eat(&TokenKind::DotDot) || self.eat(&TokenKind::DotDotEq) {
            let inclusive = matches!(self.tokens[self.pos - 1].kind, TokenKind::DotDotEq);
            let end = if self.starts_expr_after_range() {
                Some(Box::new(self.parse_or()?))
            } else {
                None
            };
            return Ok(Expr::Range {
                start: None,
                end,
                inclusive,
            });
        }
        let lhs = self.parse_or()?;
        if self.eat(&TokenKind::DotDot) || self.eat(&TokenKind::DotDotEq) {
            let inclusive = matches!(self.tokens[self.pos - 1].kind, TokenKind::DotDotEq);
            let end = if self.starts_expr_after_range() {
                Some(Box::new(self.parse_or()?))
            } else {
                None
            };
            return Ok(Expr::Range {
                start: Some(Box::new(lhs)),
                end,
                inclusive,
            });
        }
        Ok(lhs)
    }

    fn starts_expr_after_range(&self) -> bool {
        !matches!(
            self.peek_kind(),
            TokenKind::RBracket
                | TokenKind::RParen
                | TokenKind::RBrace
                | TokenKind::Comma
                | TokenKind::Semicolon
                | TokenKind::Colon
                | TokenKind::FatArrow
                | TokenKind::Eof
        )
    }

    fn parse_or(&mut self) -> Result<Expr> {
        let mut lhs = self.parse_and()?;
        while self.eat(&TokenKind::PipePipe) {
            let rhs = self.parse_and()?;
            lhs = Expr::Binary {
                op: BinaryOp::Or,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> Result<Expr> {
        let mut lhs = self.parse_bitor()?;
        while self.eat(&TokenKind::AmpAmp) {
            let rhs = self.parse_bitor()?;
            lhs = Expr::Binary {
                op: BinaryOp::And,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn parse_bitor(&mut self) -> Result<Expr> {
        let mut lhs = self.parse_bitxor()?;
        while self.eat(&TokenKind::Pipe) {
            let rhs = self.parse_bitxor()?;
            lhs = Expr::Binary {
                op: BinaryOp::BitOr,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn parse_bitxor(&mut self) -> Result<Expr> {
        let mut lhs = self.parse_bitand()?;
        while self.eat(&TokenKind::Caret) {
            let rhs = self.parse_bitand()?;
            lhs = Expr::Binary {
                op: BinaryOp::BitXor,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn parse_bitand(&mut self) -> Result<Expr> {
        let mut lhs = self.parse_equality()?;
        while self.eat(&TokenKind::Amp) {
            let rhs = self.parse_equality()?;
            lhs = Expr::Binary {
                op: BinaryOp::BitAnd,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn parse_equality(&mut self) -> Result<Expr> {
        let mut lhs = self.parse_comparison()?;
        loop {
            if self.eat(&TokenKind::EqEq) {
                let rhs = self.parse_comparison()?;
                lhs = Expr::Binary {
                    op: BinaryOp::Eq,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                };
            } else if self.eat(&TokenKind::BangEq) {
                let rhs = self.parse_comparison()?;
                lhs = Expr::Binary {
                    op: BinaryOp::Ne,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                };
            } else if self.eat_kw(Keyword::Is) {
                let negated = self.eat_kw(Keyword::Not);
                let pat = self.parse_pattern()?;
                lhs = Expr::IsPattern {
                    expr: Box::new(lhs),
                    negated,
                    pat,
                };
            } else {
                break;
            }
        }
        Ok(lhs)
    }

    fn parse_comparison(&mut self) -> Result<Expr> {
        let mut lhs = self.parse_shift()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::Lt => Some(BinaryOp::Lt),
                TokenKind::Le => Some(BinaryOp::Le),
                TokenKind::Gt => Some(BinaryOp::Gt),
                TokenKind::Ge => Some(BinaryOp::Ge),
                _ => None,
            };
            if let Some(op) = op {
                self.bump();
                let rhs = self.parse_shift()?;
                lhs = Expr::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                };
            } else {
                break;
            }
        }
        Ok(lhs)
    }

    fn parse_shift(&mut self) -> Result<Expr> {
        let mut lhs = self.parse_additive()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::Shl => Some(BinaryOp::Shl),
                TokenKind::Shr => Some(BinaryOp::Shr),
                _ => None,
            };
            if let Some(op) = op {
                self.bump();
                let rhs = self.parse_additive()?;
                lhs = Expr::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                };
            } else {
                break;
            }
        }
        Ok(lhs)
    }

    fn parse_additive(&mut self) -> Result<Expr> {
        let mut lhs = self.parse_multiplicative()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::Plus => Some(BinaryOp::Add),
                TokenKind::Minus => Some(BinaryOp::Sub),
                _ => None,
            };
            if let Some(op) = op {
                self.bump();
                let rhs = self.parse_multiplicative()?;
                lhs = Expr::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                };
            } else {
                break;
            }
        }
        Ok(lhs)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr> {
        let mut lhs = self.parse_cast()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::Star => Some(BinaryOp::Mul),
                TokenKind::Slash => Some(BinaryOp::Div),
                TokenKind::Percent => Some(BinaryOp::Rem),
                _ => None,
            };
            if let Some(op) = op {
                self.bump();
                let rhs = self.parse_cast()?;
                lhs = Expr::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                };
            } else {
                break;
            }
        }
        Ok(lhs)
    }

    fn parse_cast(&mut self) -> Result<Expr> {
        let mut expr = self.parse_unary()?;
        while self.eat_kw(Keyword::As) {
            let ty = self.parse_type()?;
            expr = Expr::Cast {
                expr: Box::new(expr),
                ty,
            };
        }
        Ok(expr)
    }

    fn parse_unary(&mut self) -> Result<Expr> {
        if self.eat(&TokenKind::Minus) {
            return Ok(Expr::Unary {
                op: UnaryOp::Neg,
                expr: Box::new(self.parse_unary()?),
            });
        }
        if self.eat(&TokenKind::Bang) {
            return Ok(Expr::Unary {
                op: UnaryOp::Not,
                expr: Box::new(self.parse_unary()?),
            });
        }
        if self.eat(&TokenKind::Tilde) {
            return Ok(Expr::Unary {
                op: UnaryOp::BitNot,
                expr: Box::new(self.parse_unary()?),
            });
        }
        if self.eat(&TokenKind::PlusPlus) {
            return Ok(Expr::Step {
                inc: true,
                target: Box::new(self.parse_unary()?),
            });
        }
        if self.eat(&TokenKind::MinusMinus) {
            return Ok(Expr::Step {
                inc: false,
                target: Box::new(self.parse_unary()?),
            });
        }
        if self.eat_kw(Keyword::Try) {
            return Ok(Expr::Try(Box::new(self.parse_unary()?)));
        }
        if self.eat_kw(Keyword::Await) {
            return Ok(Expr::Await(Box::new(self.parse_unary()?)));
        }
        if self.eat_kw(Keyword::Deref) {
            return Ok(Expr::Deref(Box::new(self.parse_unary()?)));
        }
        if self.eat_kw(Keyword::Read) {
            if self.eat_kw(Keyword::Slice) {
                return Ok(Expr::ReadSlice(Box::new(self.parse_unary()?)));
            }
            return Ok(Expr::Read(Box::new(self.parse_unary()?)));
        }
        if self.eat_kw(Keyword::Write) {
            if self.eat_kw(Keyword::Slice) {
                return Ok(Expr::WriteSlice(Box::new(self.parse_unary()?)));
            }
            return Ok(Expr::Write(Box::new(self.parse_unary()?)));
        }
        if self.check_kw(Keyword::Take) {
            // `take (int x) => ...` is a move lambda; `take expr` is explicit move
            if let Some(lambda) = self.try_parse_lambda()? {
                return Ok(lambda);
            }
            self.bump();
            return Ok(Expr::Take(Box::new(self.parse_unary()?)));
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Result<Expr> {
        let mut expr = self.parse_primary()?;
        loop {
            if self.eat(&TokenKind::Dot) || self.eat(&TokenKind::ColonColon) {
                if let TokenKind::IntLit(idx) = self.peek_kind().clone() {
                    self.bump();
                    expr = Expr::TupleIndex {
                        object: Box::new(expr),
                        index: idx,
                    };
                } else {
                    let field = self.expect_member_ident()?;
                    let type_args = self.try_parse_expr_generic_args();
                    expr = Expr::Member {
                        object: Box::new(expr),
                        field,
                        type_args,
                    };
                }
            } else if self.check(&TokenKind::Lt) {
                let type_args = self.try_parse_expr_generic_args();
                if !type_args.is_empty() {
                    if self.eat(&TokenKind::LParen) {
                        let args = self.parse_arg_list()?;
                        expr = Expr::Call {
                            callee: Box::new(expr),
                            type_args,
                            args,
                        };
                    } else {
                        // e.g. `List<int>.new()` -> wrap into a Path segment with generic args
                        expr = attach_type_args_to_expr(expr, type_args);
                    }
                } else {
                    break;
                }
            } else if self.eat(&TokenKind::LParen) {
                let args = self.parse_arg_list()?;
                expr = Expr::Call {
                    callee: Box::new(expr),
                    type_args: Vec::new(),
                    args,
                };
            } else if self.eat(&TokenKind::LBracket) {
                let mut indices = Vec::new();
                while !self.check(&TokenKind::RBracket) && !self.is_eof() {
                    indices.push(self.parse_expr()?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RBracket, "']'")?;
                expr = Expr::Index {
                    object: Box::new(expr),
                    indices,
                };
            } else if self.eat(&TokenKind::PlusPlus) {
                expr = Expr::Step {
                    inc: true,
                    target: Box::new(expr),
                };
            } else if self.eat(&TokenKind::MinusMinus) {
                expr = Expr::Step {
                    inc: false,
                    target: Box::new(expr),
                };
            } else if self.eat_kw(Keyword::With) {
                let fields = self.parse_struct_field_inits()?;
                expr = Expr::StructUpdate {
                    base: Box::new(expr),
                    fields,
                };
            } else if self.check(&TokenKind::Bang)
                && matches!(
                    self.peek_at(1).kind,
                    TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace
                )
            {
                if let Some(path) = expr_to_path(&expr) {
                    self.bump(); // '!'
                    let mc = self.parse_macro_call_body(path)?;
                    expr = Expr::MacroCall(mc);
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn try_parse_expr_generic_args(&mut self) -> Vec<GenericArg> {
        if !self.check(&TokenKind::Lt) {
            return Vec::new();
        }
        let save = self.checkpoint();
        self.bump(); // '<'
        let Ok(args) = self.parse_generic_arg_list() else {
            self.restore(save);
            return Vec::new();
        };
        if !args.is_empty()
            && self.eat_gt()
            && matches!(
                self.peek_kind(),
                TokenKind::LParen | TokenKind::Dot | TokenKind::ColonColon
            )
        {
            return args;
        }
        self.restore(save);
        Vec::new()
    }

    fn parse_arg_list(&mut self) -> Result<Vec<Expr>> {
        let mut args = Vec::new();
        while !self.check(&TokenKind::RParen) && !self.is_eof() {
            args.push(self.parse_expr()?);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RParen, "')'")?;
        Ok(args)
    }

    fn parse_macro_call_body(&mut self, path: Vec<String>) -> Result<MacroCall> {
        let (delim, open, close) = match self.peek_kind() {
            TokenKind::LParen => (MacroDelim::Paren, TokenKind::LParen, TokenKind::RParen),
            TokenKind::LBracket => (
                MacroDelim::Bracket,
                TokenKind::LBracket,
                TokenKind::RBracket,
            ),
            TokenKind::LBrace => (MacroDelim::Brace, TokenKind::LBrace, TokenKind::RBrace),
            _ => return Err(self.err_here("Expected '(', '[', or '{' after '!'")),
        };
        let body = self.collect_balanced_raw(open, close)?;
        Ok(MacroCall { path, delim, body })
    }

    fn parse_primary(&mut self) -> Result<Expr> {
        // Inline `rust { ... }` expression
        if let TokenKind::RustBlock(raw) = self.peek_kind().clone() {
            self.bump();
            return Ok(Expr::InlineRust(raw));
        }

        // Match expression
        if self.check_kw(Keyword::Match) {
            return Ok(Expr::Match(self.parse_match()?));
        }

        // Unsafe block expression
        if self.check_kw(Keyword::Unsafe) && matches!(self.peek_at(1).kind, TokenKind::LBrace) {
            self.bump();
            return Ok(Expr::UnsafeBlock(self.parse_block()?));
        }

        // Async block expression: `async { ... }` or `async take { ... }`
        if self.check_kw(Keyword::Async) {
            self.bump();
            let is_move = self.eat_kw(Keyword::Take);
            let block = self.parse_block()?;
            return Ok(Expr::AsyncBlock { is_move, block });
        }

        // Struct literal: `new Point { X = 1, Y = 2 }`
        if self.eat_kw(Keyword::New) {
            let path = self.parse_simple_path()?;
            let fields = self.parse_struct_field_inits()?;
            return Ok(Expr::StructLit { path, fields });
        }

        // Labeled value block: `b: { int a = 1; break b a + 2; }`
        if matches!(
            (self.peek_kind(), &self.peek_at(1).kind, &self.peek_at(2).kind),
            (TokenKind::Ident { .. }, TokenKind::Colon, TokenKind::LBrace)
        ) {
            let label = self.expect_ident()?;
            self.bump(); // ':'
            let block = self.parse_block()?;
            return Ok(Expr::LabeledBlock { label, block });
        }

        // Lambda `(int x) : int => ...` or `() => ...`
        if let Some(lambda) = self.try_parse_lambda()? {
            return Ok(lambda);
        }

        match self.peek_kind().clone() {
            TokenKind::IntLit(s) => {
                self.bump();
                Ok(Expr::Lit(Literal::Int(s)))
            }
            TokenKind::FloatLit(s) => {
                self.bump();
                Ok(Expr::Lit(Literal::Float(s)))
            }
            TokenKind::StringLit(s) => {
                self.bump();
                Ok(Expr::Lit(Literal::Str(s)))
            }
            TokenKind::CharLit(c) => {
                self.bump();
                Ok(Expr::Lit(Literal::Char(c)))
            }
            TokenKind::InterpString(parts) => {
                self.bump();
                let mut parsed = Vec::with_capacity(parts.len());
                for part in parts {
                    match part {
                        InterpPart::Literal(s) => parsed.push(InterpExprPart::Literal(s)),
                        InterpPart::Expr { source, format } => {
                            let expr = parse_expr_str(&source)?;
                            parsed.push(InterpExprPart::Expr { expr, format });
                        }
                    }
                }
                Ok(Expr::InterpString(parsed))
            }
            TokenKind::Kw(Keyword::True) => {
                self.bump();
                Ok(Expr::Lit(Literal::Bool(true)))
            }
            TokenKind::Kw(Keyword::False) => {
                self.bump();
                Ok(Expr::Lit(Literal::Bool(false)))
            }
            TokenKind::Kw(Keyword::This) => {
                self.bump();
                Ok(Expr::This)
            }
            TokenKind::Kw(Keyword::SelfType) => {
                self.bump();
                Ok(Expr::SelfType)
            }
            TokenKind::Kw(Keyword::Crate) => {
                self.bump();
                Ok(Expr::Ident("crate".to_string()))
            }
            TokenKind::Kw(Keyword::Super) => {
                self.bump();
                Ok(Expr::Ident("super".to_string()))
            }
            TokenKind::Ident { name, .. } => {
                self.bump();
                Ok(Expr::Ident(name))
            }
            TokenKind::LParen => {
                self.bump();
                if self.eat(&TokenKind::RParen) {
                    return Ok(Expr::TupleLit(Vec::new()));
                }
                let first = self.parse_expr()?;
                if self.eat(&TokenKind::Comma) {
                    let mut elems = vec![first];
                    while !self.check(&TokenKind::RParen) && !self.is_eof() {
                        elems.push(self.parse_expr()?);
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                    self.expect(&TokenKind::RParen, "')'")?;
                    Ok(Expr::TupleLit(elems))
                } else {
                    self.expect(&TokenKind::RParen, "')'")?;
                    Ok(first)
                }
            }
            TokenKind::LBracket => {
                self.bump();
                if self.eat(&TokenKind::RBracket) {
                    return Ok(Expr::ArrayLit(Vec::new()));
                }
                let first = self.parse_expr()?;
                if self.eat(&TokenKind::Semicolon) {
                    let count = self.parse_expr()?;
                    self.expect(&TokenKind::RBracket, "']'")?;
                    Ok(Expr::ArrayRepeat {
                        value: Box::new(first),
                        count: Box::new(count),
                    })
                } else {
                    let mut elems = vec![first];
                    while self.eat(&TokenKind::Comma) {
                        if self.check(&TokenKind::RBracket) {
                            break;
                        }
                        elems.push(self.parse_expr()?);
                    }
                    self.expect(&TokenKind::RBracket, "']'")?;
                    Ok(Expr::ArrayLit(elems))
                }
            }
            _ => Err(self.err_here(format!(
                "Expected expression, found {:?}",
                self.peek_kind()
            ))),
        }
    }

    fn parse_struct_field_inits(&mut self) -> Result<Vec<(String, Expr)>> {
        self.expect(&TokenKind::LBrace, "'{'")?;
        let mut fields = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_eof() {
            let name = self.expect_ident()?;
            self.expect(&TokenKind::Eq, "'=' in struct initializer")?;
            let val = self.parse_expr()?;
            fields.push((name, val));
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RBrace, "'}'")?;
        Ok(fields)
    }

    fn try_parse_lambda(&mut self) -> Result<Option<Expr>> {
        let save = self.pos;
        let is_move = self.eat_kw(Keyword::Take);
        if !self.eat(&TokenKind::LParen) {
            self.pos = save;
            return Ok(None);
        }
        let mut params = Vec::new();
        while !self.check(&TokenKind::RParen) && !self.is_eof() {
            let is_mut = self.eat_kw(Keyword::Mut);
            let Ok(ty) = self.parse_type() else {
                self.pos = save;
                return Ok(None);
            };
            let Ok(name) = self.expect_ident() else {
                self.pos = save;
                return Ok(None);
            };
            params.push((is_mut, ty, name));
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        if !self.eat(&TokenKind::RParen) {
            self.pos = save;
            return Ok(None);
        }
        let ret_ty = if self.eat(&TokenKind::Colon) {
            match self.parse_type() {
                Ok(t) => Some(t),
                Err(_) => {
                    self.pos = save;
                    return Ok(None);
                }
            }
        } else {
            None
        };
        if !self.eat(&TokenKind::FatArrow) {
            self.pos = save;
            return Ok(None);
        }
        let body = if self.check(&TokenKind::LBrace) {
            LambdaBody::Block(self.parse_block()?)
        } else {
            LambdaBody::Expr(Box::new(self.parse_expr()?))
        };
        Ok(Some(Expr::Lambda {
            is_move,
            params,
            ret_ty,
            body,
        }))
    }
}

fn is_binary_or_postfix_continuation(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Percent
            | TokenKind::EqEq
            | TokenKind::BangEq
            | TokenKind::Lt
            | TokenKind::Le
            | TokenKind::Gt
            | TokenKind::Ge
            | TokenKind::AmpAmp
            | TokenKind::PipePipe
            | TokenKind::Dot
            | TokenKind::ColonColon
            | TokenKind::LParen
            | TokenKind::LBracket
            | TokenKind::Question
            | TokenKind::QuestionQuestion
            | TokenKind::Kw(Keyword::As | Keyword::With | Keyword::Is)
    )
}

fn attach_type_args_to_expr(expr: Expr, type_args: Vec<GenericArg>) -> Expr {
    match expr {
        Expr::Ident(name) => Expr::Path {
            segments: vec![PathExprSegment { name, type_args }],
        },
        Expr::Member { object, field, .. } => Expr::Member {
            object,
            field,
            type_args,
        },
        other => other,
    }
}

fn expr_to_path(expr: &Expr) -> Option<Vec<String>> {
    match expr {
        Expr::Ident(name) => Some(vec![name.clone()]),
        Expr::Member { object, field, .. } => {
            let mut segs = expr_to_path(object)?;
            segs.push(field.clone());
            Some(segs)
        }
        _ => None,
    }
}

fn format_interp_raw(parts: &[InterpPart]) -> String {
    let mut s = String::from("$\"");
    for p in parts {
        match p {
            InterpPart::Literal(l) => s.push_str(l),
            InterpPart::Expr { source, format } => {
                s.push('{');
                s.push_str(source);
                if let Some(f) = format {
                    s.push(':');
                    s.push_str(f);
                }
                s.push('}');
            }
        }
    }
    s.push('"');
    s
}

fn join_raw_tokens(parts: &[String]) -> String {
    let mut out = String::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            let prev = &parts[i - 1];
            let no_space = matches!(
                part.as_str(),
                "," | ";" | ")" | "]" | "." | "::" | "!" | "("
            ) || matches!(prev.as_str(), "(" | "[" | "." | "::" | "!");
            if !no_space {
                out.push(' ');
            }
        }
        out.push_str(part);
    }
    out
}
