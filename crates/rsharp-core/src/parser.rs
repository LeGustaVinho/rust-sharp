use crate::ast::*;
use crate::error::{RSharpError, Result, Span};
use crate::lexer::tokenize;
use crate::token::{InterpPart, Keyword, Token, TokenKind};

/// Parses R# source code into a `FileAst`.
pub fn parse_file(source: &str) -> Result<FileAst> {
    let tokens = tokenize(source)?;
    let mut parser = Parser::new(source, tokens);
    parser.parse_file()
}

/// Parses a standalone R# expression (used by interpolated strings and tests).
pub fn parse_expr_str(source: &str) -> Result<Expr> {
    let tokens = tokenize(source)?;
    let mut parser = Parser::new(source, tokens);
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

struct Parser<'a> {
    src: &'a str,
    tokens: Vec<Token>,
    pos: usize,
    /// Set when a `>>` (`Shr`) token was split to close a nested generic `<...<...>>`.
    pending_gt: bool,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str, tokens: Vec<Token>) -> Self {
        Self {
            src,
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

    fn check_contextual(&self, word: &str) -> bool {
        matches!(self.peek_kind(), TokenKind::Ident { name, verbatim: false } if name == word)
    }

    fn eat_contextual(&mut self, word: &str) -> bool {
        if self.check_contextual(word) {
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
            _ => Err(self.err_here(format!("Expected identifier, found {:?}", self.peek_kind()))),
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
            TokenKind::Kw(Keyword::This) => {
                self.bump();
                Ok("self".to_string())
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

    fn eat_global_prefix(&mut self) -> bool {
        if self.check_contextual("global") && matches!(self.peek_at(1).kind, TokenKind::ColonColon)
        {
            self.bump();
            self.bump();
            true
        } else {
            self.eat(&TokenKind::ColonColon)
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

    fn collect_inner_meta(&mut self) -> Result<(Vec<String>, Vec<Attribute>)> {
        let mut inner_docs = Vec::new();
        let mut inner_attrs = Vec::new();
        loop {
            if let TokenKind::InnerDocComment(text) = self.peek_kind().clone() {
                self.bump();
                inner_docs.push(text);
                continue;
            }
            if self.check(&TokenKind::LBracket)
                && matches!(
                    (&self.peek_at(1).kind, &self.peek_at(2).kind),
                    (TokenKind::Ident { name, .. }, TokenKind::Colon)
                        if name == "module" || name == "assembly" || name == "inner"
                )
            {
                inner_attrs.push(self.parse_attr()?);
                continue;
            }
            break;
        }
        Ok((inner_docs, inner_attrs))
    }

    fn parse_attrs(&mut self) -> Result<Vec<Attribute>> {
        let mut attrs = Vec::new();
        while self.check(&TokenKind::LBracket) {
            attrs.push(self.parse_attr()?);
        }
        Ok(attrs)
    }

    /// Speculatively parses attributes in contexts where `[` could also start a slice pattern or array literal.
    fn try_parse_contextual_attrs(&mut self, is_match_arm: bool) -> Result<Vec<Attribute>> {
        let mut attrs = Vec::new();
        while self.check(&TokenKind::LBracket) {
            let save = self.checkpoint();
            if let Ok(attr) = self.parse_attr() {
                let not_attr = if is_match_arm {
                    matches!(
                        self.peek_kind(),
                        TokenKind::FatArrow
                            | TokenKind::Kw(Keyword::When | Keyword::As)
                            | TokenKind::Pipe
                    )
                } else {
                    matches!(
                        self.peek_kind(),
                        TokenKind::Eq
                            | TokenKind::Dot
                            | TokenKind::ColonColon
                            | TokenKind::LBracket
                            | TokenKind::Semicolon
                            | TokenKind::Comma
                            | TokenKind::RParen
                            | TokenKind::RBrace
                    )
                };
                if not_attr {
                    self.restore(save);
                    break;
                }
                attrs.push(attr);
            } else {
                self.restore(save);
                break;
            }
        }
        Ok(attrs)
    }

    fn parse_attr(&mut self) -> Result<Attribute> {
        self.expect(&TokenKind::LBracket, "'['")?;
        let inner = matches!(
            (self.peek_kind(), &self.peek_at(1).kind),
            (TokenKind::Ident { name, .. }, TokenKind::Colon)
                if name == "module" || name == "assembly" || name == "inner"
        );
        if inner {
            self.bump();
            self.bump();
        }
        let is_unsafe = self.eat_kw(Keyword::Unsafe);
        let mut name = self.expect_member_ident()?;
        while self.eat(&TokenKind::Dot) || self.eat(&TokenKind::ColonColon) {
            name.push_str("::");
            name.push_str(&self.expect_member_ident()?);
        }
        let args = if self.check(&TokenKind::LParen) {
            Some(self.collect_balanced_raw(TokenKind::LParen, TokenKind::RParen)?)
        } else if self.eat(&TokenKind::Eq) {
            let rhs_start = self.peek().span.start;
            let mut rhs_end = rhs_start;
            while !self.check(&TokenKind::RBracket) && !self.is_eof() {
                rhs_end = self.bump().span.end;
            }
            Some(format!("= {}", self.src[rhs_start..rhs_end].trim()))
        } else {
            None
        };
        self.expect(&TokenKind::RBracket, "']'")?;
        Ok(Attribute {
            inner,
            is_unsafe,
            name,
            args,
        })
    }

    fn collect_balanced_raw(&mut self, open: TokenKind, close: TokenKind) -> Result<String> {
        let open_tok = self.expect(&open, "opening delimiter")?;
        let body_start = open_tok.span.end;
        let mut depth = 1usize;
        while depth > 0 {
            if self.is_eof() {
                return Err(self.err_here("Unterminated delimiter group"));
            }
            let tok = self.bump();
            if tok.kind == open {
                depth += 1;
            } else if tok.kind == close {
                depth -= 1;
                if depth == 0 {
                    return Ok(self.src[body_start..tok.span.start].trim().to_string());
                }
            }
        }
        Ok(String::new())
    }

    fn parse_visibility(&mut self) -> Result<Visibility> {
        if self.eat_kw(Keyword::Public) {
            if self.check(&TokenKind::LParen)
                && matches!(
                    self.peek_at(1).kind,
                    TokenKind::Kw(Keyword::Super | Keyword::This | Keyword::In)
                )
            {
                self.bump(); // '('
                let vis = if self.eat_kw(Keyword::Super) {
                    Visibility::Super
                } else if self.eat_kw(Keyword::This) {
                    Visibility::This
                } else {
                    self.expect_kw(Keyword::In)?;
                    Visibility::InPath(self.parse_simple_path()?)
                };
                self.expect(&TokenKind::RParen, "')'")?;
                Ok(vis)
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
        let mut inner_docs = Vec::new();
        let mut inner_attrs = Vec::new();
        let mut items = Vec::new();
        while !self.is_eof() {
            let (idocs, iattrs) = self.collect_inner_meta()?;
            inner_docs.extend(idocs);
            inner_attrs.extend(iattrs);
            if self.is_eof() {
                break;
            }
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
            let item = self.parse_item_after_meta(docs, outers, false)?;
            items.push(item);
        }
        Ok(FileAst {
            inner_docs,
            inner_attrs,
            items,
        })
    }

    fn parse_item(&mut self) -> Result<Item> {
        let docs = self.collect_docs();
        let attrs = self.parse_attrs()?;
        self.parse_item_after_meta(docs, attrs, false)
    }

    fn parse_item_after_meta(
        &mut self,
        docs: Vec<String>,
        attrs: Vec<Attribute>,
        in_extern: bool,
    ) -> Result<Item> {
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
        if self.check_kw(Keyword::Extern)
            && matches!(self.peek_at(1).kind, TokenKind::Kw(Keyword::Crate))
        {
            self.bump();
            self.bump();
            let name = self.expect_ident()?;
            let alias = if self.eat_kw(Keyword::As) {
                Some(self.expect_ident()?)
            } else {
                None
            };
            self.expect(&TokenKind::Semicolon, "';' after extern crate")?;
            return Ok(Item {
                docs,
                attrs,
                vis,
                kind: ItemKind::ExternCrate(ExternCrateDecl { name, alias }),
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
            if self.is_extern_block_at(1) {
                self.bump();
                let eb = self.parse_extern_block(true)?;
                return Ok(Item {
                    docs,
                    attrs,
                    vis,
                    kind: ItemKind::ExternBlock(eb),
                });
            }
        }
        if self.is_extern_block_at(0) {
            let eb = self.parse_extern_block(false)?;
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

        // Distinguish `safe?/unsafe? static mut? Type Name (= Expr)?;` vs `static ReturnType Fn(...)`
        if self.is_static_item() {
            let s = self.parse_static_decl(in_extern)?;
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

    fn is_extern_block_at(&self, offset: usize) -> bool {
        if !matches!(self.peek_at(offset).kind, TokenKind::Kw(Keyword::Extern)) {
            return false;
        }
        match &self.peek_at(offset + 1).kind {
            TokenKind::LBrace => true,
            TokenKind::StringLit(_) => matches!(self.peek_at(offset + 2).kind, TokenKind::LBrace),
            _ => false,
        }
    }

    fn is_const_item(&mut self) -> bool {
        let save = self.checkpoint();
        let is_item = if self.eat_kw(Keyword::Const) {
            if self.parse_type().is_ok() && self.expect_ident().is_ok() {
                self.check(&TokenKind::Eq) || self.check(&TokenKind::Semicolon)
            } else {
                false
            }
        } else {
            false
        };
        self.restore(save);
        is_item
    }

    fn is_static_item(&mut self) -> bool {
        let save = self.checkpoint();
        let _ = self.eat_contextual("safe") || self.eat_kw(Keyword::Unsafe);
        let is_item = if self.eat_kw(Keyword::Static) {
            if self.eat_kw(Keyword::Mut) {
                true
            } else if self.parse_type().is_ok() && self.expect_ident().is_ok() {
                self.check(&TokenKind::Eq) || self.check(&TokenKind::Semicolon)
            } else {
                false
            }
        } else {
            false
        };
        self.restore(save);
        is_item
    }

    fn try_parse_item_macro_call(&mut self) -> Result<Option<MacroCall>> {
        let save = self.checkpoint();
        let global = self.eat_global_prefix();
        if let Ok(path) = self.parse_simple_path()
            && self.eat(&TokenKind::Bang)
        {
            let mc = self.parse_macro_call_body(global, path)?;
            if mc.delim != MacroDelim::Brace {
                self.expect(&TokenKind::Semicolon, "';' after macro invocation")?;
            } else {
                self.eat(&TokenKind::Semicolon);
            }
            return Ok(Some(mc));
        }
        self.restore(save);
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
            Ok(NamespaceDecl {
                path,
                inner_docs: Vec::new(),
                inner_attrs: Vec::new(),
                items: None,
            })
        } else {
            self.expect(&TokenKind::LBrace, "'{' or ';'")?;
            let (inner_docs, inner_attrs) = self.collect_inner_meta()?;
            let mut items = Vec::new();
            while !self.check(&TokenKind::RBrace) && !self.is_eof() {
                items.push(self.parse_item()?);
            }
            self.expect(&TokenKind::RBrace, "'}'")?;
            Ok(NamespaceDecl {
                path,
                inner_docs,
                inner_attrs,
                items: Some(items),
            })
        }
    }

    fn parse_using(&mut self) -> Result<UsingDecl> {
        self.expect_kw(Keyword::Using)?;
        let tree = self.parse_using_tree()?;
        self.expect(&TokenKind::Semicolon, "';' after using declaration")?;
        Ok(UsingDecl { tree })
    }

    fn parse_using_tree(&mut self) -> Result<UsingTree> {
        if self.eat(&TokenKind::Star) {
            return Ok(UsingTree::Path {
                global: false,
                prefix: Vec::new(),
                wildcard: true,
                alias: None,
            });
        }
        // Check `Alias = path`
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

        let global = self.eat_global_prefix();

        // Root grouped import: `using { std.fmt.Debug, std.fmt.Display };`
        if alias.is_none() && self.eat(&TokenKind::LBrace) {
            let mut items = Vec::new();
            while !self.check(&TokenKind::RBrace) && !self.is_eof() {
                items.push(self.parse_using_tree()?);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::RBrace, "'}'")?;
            return Ok(UsingTree::Group {
                global,
                prefix: Vec::new(),
                items,
            });
        }

        let first = if self.eat_kw(Keyword::This) {
            "self".to_string()
        } else {
            self.expect_path_segment_ident()?
        };
        let mut prefix = vec![first];
        let mut wildcard = false;
        while self.eat(&TokenKind::Dot) || self.eat(&TokenKind::ColonColon) {
            if self.eat(&TokenKind::Star) {
                wildcard = true;
                break;
            }
            if self.eat(&TokenKind::LBrace) {
                let mut items = Vec::new();
                while !self.check(&TokenKind::RBrace) && !self.is_eof() {
                    items.push(self.parse_using_tree()?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RBrace, "'}'")?;
                return Ok(UsingTree::Group {
                    global,
                    prefix,
                    items,
                });
            }
            if self.eat_kw(Keyword::This) {
                prefix.push("self".to_string());
            } else {
                prefix.push(self.expect_member_ident()?);
            }
        }
        Ok(UsingTree::Path {
            global,
            prefix,
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
            let attrs = self.parse_attrs()?;
            if self.eat_kw(Keyword::Life) {
                let name = self.expect_ident()?;
                let mut outlives = Vec::new();
                if self.eat(&TokenKind::Colon) {
                    outlives.push(self.expect_lifetime_name()?);
                    while self.eat(&TokenKind::Plus) {
                        outlives.push(self.expect_lifetime_name()?);
                    }
                }
                params.push(GenericParam {
                    attrs,
                    kind: GenericParamKind::Lifetime { name, outlives },
                });
            } else if self.eat_kw(Keyword::Const) {
                let ty = self.parse_type()?;
                let name = self.expect_ident()?;
                let default = if self.eat(&TokenKind::Eq) {
                    Some(Box::new(self.parse_const_generic_expr()?))
                } else {
                    None
                };
                params.push(GenericParam {
                    attrs,
                    kind: GenericParamKind::Const { ty, name, default },
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
                let default = if self.eat(&TokenKind::Eq) {
                    Some(self.parse_type()?)
                } else {
                    None
                };
                params.push(GenericParam {
                    attrs,
                    kind: GenericParamKind::Type {
                        name,
                        bounds,
                        default,
                    },
                });
            }
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect_gt()?;
        Ok(params)
    }

    fn parse_const_generic_expr(&mut self) -> Result<Expr> {
        if self.eat(&TokenKind::LBrace) {
            let expr = self.parse_expr()?;
            self.expect(&TokenKind::RBrace, "'}'")?;
            Ok(expr)
        } else if self.eat(&TokenKind::Minus) {
            let inner = self.parse_primary()?;
            Ok(Expr::Unary {
                op: UnaryOp::Neg,
                expr: Box::new(inner),
            })
        } else {
            self.parse_primary()
        }
    }

    fn parse_where_clause(&mut self) -> Result<Option<WhereClause>> {
        if !self.eat_kw(Keyword::Where) {
            return Ok(None);
        }
        let mut predicates = Vec::new();
        while !self.check(&TokenKind::LBrace)
            && !self.check(&TokenKind::Semicolon)
            && !self.check(&TokenKind::Eq)
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
                let lifetimes = if self.eat_kw(Keyword::Forall) {
                    self.parse_forall_lifetimes()?
                } else {
                    Vec::new()
                };
                let ty = self.parse_type()?;
                self.expect(&TokenKind::Colon, "':' in where constraint")?;
                let mut bounds = vec![self.parse_type_bound()?];
                while (self.check(&TokenKind::Comma) && !self.next_starts_where_predicate())
                    || self.check(&TokenKind::Plus)
                {
                    self.bump();
                    if self.check(&TokenKind::LBrace)
                        || self.check(&TokenKind::Semicolon)
                        || self.check(&TokenKind::Eq)
                    {
                        break;
                    }
                    bounds.push(self.parse_type_bound()?);
                }
                predicates.push(WherePredicate::Type {
                    lifetimes,
                    ty,
                    bounds,
                });
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
        let save = self.checkpoint();
        self.bump(); // skip ','
        let starts = if self.check(&TokenKind::LBrace)
            || self.check(&TokenKind::Semicolon)
            || self.check(&TokenKind::Eq)
        {
            true
        } else if self.eat_kw(Keyword::Life) {
            self.expect_ident().is_ok() && self.check(&TokenKind::Colon)
        } else if self.eat_kw(Keyword::Forall) {
            self.parse_forall_lifetimes().is_ok()
                && self.parse_type().is_ok()
                && self.check(&TokenKind::Colon)
        } else {
            self.parse_type().is_ok() && self.check(&TokenKind::Colon)
        };
        self.restore(save);
        starts
    }

    fn parse_forall_lifetimes(&mut self) -> Result<Vec<String>> {
        self.expect(&TokenKind::Lt, "'<' after 'forall'")?;
        let mut lts = Vec::new();
        while !self.check_gt() && !self.is_eof() {
            self.eat_kw(Keyword::Life);
            lts.push(self.expect_ident()?);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect_gt()?;
        Ok(lts)
    }

    fn expect_lifetime_name(&mut self) -> Result<String> {
        if self.eat_kw(Keyword::Static) {
            return Ok("static".to_string());
        }
        self.eat_kw(Keyword::Life);
        self.expect_ident()
    }

    fn parse_type_bound(&mut self) -> Result<TypeBound> {
        if self.eat_contextual("maybe") {
            return Ok(TypeBound::Relaxed(self.parse_type_path(false)?));
        }
        if self.eat_kw(Keyword::Forall) {
            let lifetimes = self.parse_forall_lifetimes()?;
            let trait_path = self.parse_type_path(false)?;
            return Ok(TypeBound::Forall {
                lifetimes,
                trait_path,
            });
        }
        if self.check_contextual("capture") && matches!(self.peek_at(1).kind, TokenKind::Lt) {
            self.bump();
            self.expect(&TokenKind::Lt, "'<' after 'capture'")?;
            let mut args = Vec::new();
            while !self.check_gt() && !self.is_eof() {
                if self.eat_kw(Keyword::Life) {
                    args.push(CaptureArg::Lifetime(self.expect_lifetime_name()?));
                } else if self.eat_kw(Keyword::SelfType) {
                    args.push(CaptureArg::TypeOrConst("Self".to_string()));
                } else {
                    args.push(CaptureArg::TypeOrConst(self.expect_ident()?));
                }
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect_gt()?;
            return Ok(TypeBound::PreciseCapture(args));
        }
        if self.eat_kw(Keyword::Life) {
            let name = self.expect_lifetime_name()?;
            return Ok(TypeBound::Lifetime(name));
        }
        if self.eat_kw(Keyword::Static) {
            return Ok(TypeBound::Lifetime("static".to_string()));
        }
        Ok(TypeBound::Trait(self.parse_type_path(false)?))
    }

    fn parse_struct(&mut self) -> Result<StructDecl> {
        self.expect_kw(Keyword::Struct)?;
        let name = self.expect_ident()?;
        let generics = self.parse_generic_params()?;

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

        if self.eat(&TokenKind::Semicolon) {
            return Ok(StructDecl {
                name,
                generics,
                where_clause,
                kind: StructKind::Unit,
                methods: Vec::new(),
            });
        }

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
            || self.check_kw(Keyword::Static)
        {
            return false;
        }
        let save = self.checkpoint();
        let ok = self.parse_type().is_ok()
            && self.expect_ident().is_ok()
            && self.check(&TokenKind::Semicolon);
        self.restore(save);
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
                    let fattrs = self.parse_attrs()?;
                    let ty = self.parse_type()?;
                    let opt_name = if matches!(self.peek_kind(), TokenKind::Ident { .. }) {
                        Some(self.expect_ident()?)
                    } else {
                        None
                    };
                    tuple_fields.push(VariantTupleField {
                        attrs: fattrs,
                        ty,
                        name: opt_name,
                    });
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
                self.expect(&TokenKind::Semicolon, "';'")?;
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
        Ok(UnionDecl {
            name,
            generics,
            where_clause,
            fields,
            methods,
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
        let (inner_docs, inner_attrs) = self.collect_inner_meta()?;
        let mut items = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_eof() {
            let docs = self.collect_docs();
            if self.check(&TokenKind::RBrace) {
                break;
            }
            let attrs = self.parse_attrs()?;
            if self.eat_kw(Keyword::Type) {
                let aname = self.expect_ident()?;
                let agens = self.parse_generic_params()?;
                let mut bounds = Vec::new();
                if self.eat(&TokenKind::Colon) {
                    bounds.push(self.parse_type_bound()?);
                    while self.eat(&TokenKind::Plus) || self.eat(&TokenKind::Comma) {
                        bounds.push(self.parse_type_bound()?);
                    }
                }
                let mut awhere = self.parse_where_clause()?;
                let default = if self.eat(&TokenKind::Eq) {
                    Some(self.parse_type()?)
                } else {
                    None
                };
                if awhere.is_none() {
                    awhere = self.parse_where_clause()?;
                }
                self.expect(&TokenKind::Semicolon, "';'")?;
                items.push(TraitItem::AssocType {
                    attrs,
                    name: aname,
                    generics: agens,
                    bounds,
                    where_clause: awhere,
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
            } else if let Some(mc) = self.try_parse_item_macro_call()? {
                items.push(TraitItem::MacroCall(mc));
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
            inner_docs,
            inner_attrs,
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
        let (inner_docs, inner_attrs) = self.collect_inner_meta()?;
        let mut items = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_eof() {
            let docs = self.collect_docs();
            if self.check(&TokenKind::RBrace) {
                break;
            }
            let attrs = self.parse_attrs()?;
            if self.eat_kw(Keyword::Type) {
                let aname = self.expect_ident()?;
                let agens = self.parse_generic_params()?;
                let mut awhere = self.parse_where_clause()?;
                self.expect(&TokenKind::Eq, "'='")?;
                let ty = self.parse_type()?;
                if awhere.is_none() {
                    awhere = self.parse_where_clause()?;
                }
                self.expect(&TokenKind::Semicolon, "';'")?;
                items.push(ImplItem::AssocType {
                    attrs,
                    name: aname,
                    generics: agens,
                    where_clause: awhere,
                    ty,
                });
            } else if let Some(mc) = self.try_parse_item_macro_call()? {
                items.push(ImplItem::MacroCall(mc));
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
            inner_docs,
            inner_attrs,
            items,
        })
    }

    fn parse_alias(&mut self) -> Result<AliasDecl> {
        self.expect_kw(Keyword::Alias)?;
        let name = self.expect_ident()?;
        let generics = self.parse_generic_params()?;
        let mut where_clause = self.parse_where_clause()?;
        self.expect(&TokenKind::Eq, "'='")?;
        let target = self.parse_type()?;
        if where_clause.is_none() {
            where_clause = self.parse_where_clause()?;
        }
        self.expect(&TokenKind::Semicolon, "';'")?;
        Ok(AliasDecl {
            name,
            generics,
            where_clause,
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

    fn parse_static_decl(&mut self, in_extern: bool) -> Result<StaticDecl> {
        let is_safe = self.eat_contextual("safe");
        let is_unsafe = !is_safe && self.eat_kw(Keyword::Unsafe);
        self.expect_kw(Keyword::Static)?;
        let is_mut = self.eat_kw(Keyword::Mut);
        let ty = self.parse_type()?;
        let name = self.expect_ident()?;
        let value = if in_extern && self.check(&TokenKind::Semicolon) {
            None
        } else {
            self.expect(&TokenKind::Eq, "'='")?;
            Some(self.parse_expr()?)
        };
        self.expect(&TokenKind::Semicolon, "';'")?;
        Ok(StaticDecl {
            is_safe,
            is_unsafe,
            is_mut,
            ty,
            name,
            value,
        })
    }

    fn parse_extern_block(&mut self, is_unsafe: bool) -> Result<ExternBlock> {
        self.expect_kw(Keyword::Extern)?;
        let abi = if let TokenKind::StringLit(s) = self.peek_kind().clone() {
            self.bump();
            Some(s)
        } else {
            None
        };
        self.expect(&TokenKind::LBrace, "'{'")?;
        let (inner_docs, inner_attrs) = self.collect_inner_meta()?;
        let mut items = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_eof() {
            let docs = self.collect_docs();
            if self.check(&TokenKind::RBrace) {
                break;
            }
            let attrs = self.parse_attrs()?;
            items.push(self.parse_item_after_meta(docs, attrs, true)?);
        }
        self.expect(&TokenKind::RBrace, "'}'")?;
        Ok(ExternBlock {
            is_unsafe,
            abi,
            inner_docs,
            inner_attrs,
            items,
        })
    }

    fn parse_macro_decl(&mut self) -> Result<MacroDecl> {
        self.expect_kw(Keyword::Macro)?;
        let name = self.expect_ident()?;
        let (delim, open, close) = match self.peek_kind() {
            TokenKind::LParen => (MacroDelim::Paren, TokenKind::LParen, TokenKind::RParen),
            TokenKind::LBracket => (
                MacroDelim::Bracket,
                TokenKind::LBracket,
                TokenKind::RBracket,
            ),
            _ => (MacroDelim::Brace, TokenKind::LBrace, TokenKind::RBrace),
        };
        let body = self.collect_balanced_raw(open, close)?;
        if delim != MacroDelim::Brace {
            self.expect(&TokenKind::Semicolon, "';' after macro declaration")?;
        } else {
            self.eat(&TokenKind::Semicolon);
        }
        Ok(MacroDecl { name, delim, body })
    }

    fn parse_fn_decl(&mut self) -> Result<FnDecl> {
        let mut is_const = false;
        let mut is_async = false;
        let mut is_safe = false;
        let mut is_unsafe = false;
        let mut is_static = false;
        let mut extern_abi = None;

        loop {
            if self.eat_kw(Keyword::Const) {
                is_const = true;
            } else if self.eat_kw(Keyword::Async) {
                is_async = true;
            } else if self.check_contextual("safe") && !self.check(&TokenKind::LParen) {
                let save = self.checkpoint();
                self.bump();
                if self.parse_type().is_ok() && self.expect_ident().is_ok() {
                    self.restore(save);
                    self.bump();
                    is_safe = true;
                } else {
                    self.restore(save);
                    break;
                }
            } else if self.check_kw(Keyword::Unsafe) && !self.starts_fn_ptr_type() {
                self.bump();
                is_unsafe = true;
            } else if self.eat_kw(Keyword::Static) {
                is_static = true;
            } else if self.check_kw(Keyword::Extern) && !self.starts_fn_ptr_type() {
                self.bump();
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
        let (params, variadic) = self.parse_params()?;
        let where_clause = self.parse_where_clause()?;
        let body = if self.eat(&TokenKind::Semicolon) {
            None
        } else {
            Some(self.parse_block()?)
        };

        Ok(FnDecl {
            is_const,
            is_async,
            is_safe,
            is_unsafe,
            is_static,
            extern_abi,
            return_ty,
            name,
            generics,
            params,
            variadic,
            where_clause,
            body,
        })
    }

    /// Distinguishes `unsafe extern "C" (int) -> int foo()` (fn-pointer return type) from
    /// `unsafe extern "C" int foo()` (function modifiers).
    fn starts_fn_ptr_type(&mut self) -> bool {
        let save = self.checkpoint();
        let _ = self.eat_kw(Keyword::Unsafe);
        if self.eat_kw(Keyword::Extern) && matches!(self.peek_kind(), TokenKind::StringLit(_)) {
            self.bump();
        }
        let is_ptr = self.check(&TokenKind::LParen);
        self.restore(save);
        is_ptr
    }

    fn parse_params(&mut self) -> Result<(Vec<Param>, Option<VariadicParam>)> {
        self.expect(&TokenKind::LParen, "'('")?;
        let mut params = Vec::new();
        let mut variadic = None;
        while !self.check(&TokenKind::RParen) && !self.is_eof() {
            let attrs = self.try_parse_contextual_attrs(false)?;
            // Variadic parameter: `...`, `... args`, or `mut ... args`
            if self.eat(&TokenKind::DotDotDot) {
                let name = if matches!(self.peek_kind(), TokenKind::Ident { .. }) {
                    Some(self.expect_ident()?)
                } else {
                    None
                };
                variadic = Some(VariadicParam {
                    attrs,
                    is_mut: false,
                    name,
                });
                self.eat(&TokenKind::Comma);
                break;
            }
            if self.check_kw(Keyword::Mut) && matches!(self.peek_at(1).kind, TokenKind::DotDotDot) {
                self.bump(); // mut
                self.bump(); // ...
                let name = Some(self.expect_ident()?);
                variadic = Some(VariadicParam {
                    attrs,
                    is_mut: true,
                    name,
                });
                self.eat(&TokenKind::Comma);
                break;
            }
            if attrs.is_empty()
                && let Some(recv) = self.try_parse_receiver()?
            {
                params.push(Param::Receiver(recv));
            } else if let Some(normal) = self.try_parse_normal_param(&attrs) {
                params.push(normal);
            } else {
                let pat = self.parse_pattern()?;
                let explicit_ty = if self.eat(&TokenKind::Colon) {
                    Some(self.parse_type()?)
                } else {
                    None
                };
                params.push(Param::Pattern {
                    attrs,
                    pat,
                    explicit_ty,
                });
            }
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RParen, "')'")?;
        Ok((params, variadic))
    }

    fn try_parse_normal_param(&mut self, attrs: &[Attribute]) -> Option<Param> {
        let save = self.checkpoint();
        let is_mut = self.eat_kw(Keyword::Mut);
        if let Ok(ty) = self.parse_type()
            && let Ok(name) = self.expect_ident()
            && matches!(self.peek_kind(), TokenKind::Comma | TokenKind::RParen)
        {
            return Some(Param::Normal {
                attrs: attrs.to_vec(),
                is_mut,
                ty,
                name,
            });
        }
        self.restore(save);
        None
    }

    fn try_parse_receiver(&mut self) -> Result<Option<ReceiverKind>> {
        let save = self.checkpoint();
        if self.eat_kw(Keyword::Read) {
            let life = self.try_parse_lifetime_angle()?;
            if self.eat_kw(Keyword::This) {
                return Ok(Some(ReceiverKind::Read(life)));
            }
            self.restore(save);
        }
        if self.eat_kw(Keyword::Write) {
            let life = self.try_parse_lifetime_angle()?;
            if self.eat_kw(Keyword::This) {
                return Ok(Some(ReceiverKind::Write(life)));
            }
            self.restore(save);
        }
        let is_mut = self.eat_kw(Keyword::Mut);
        if self.eat_kw(Keyword::Take) {
            if self.eat_kw(Keyword::This) {
                return Ok(Some(ReceiverKind::Take { is_mut }));
            }
            if let Ok(ty) = self.parse_type()
                && self.eat_kw(Keyword::This)
            {
                return Ok(Some(ReceiverKind::Typed { is_mut, ty }));
            }
            self.restore(save);
            return Ok(None);
        }
        if let Ok(ty) = self.parse_type()
            && self.eat_kw(Keyword::This)
        {
            return Ok(Some(ReceiverKind::Typed { is_mut, ty }));
        }
        self.restore(save);
        Ok(None)
    }

    fn try_parse_lifetime_angle(&mut self) -> Result<Option<String>> {
        let save = self.checkpoint();
        if self.eat(&TokenKind::Lt) {
            let life = if self.eat_kw(Keyword::Static) {
                "static".to_string()
            } else if let TokenKind::Ident { name, .. } = self.peek_kind().clone() {
                self.bump();
                name
            } else {
                self.restore(save);
                return Ok(None);
            };
            if self.eat_gt() {
                return Ok(Some(life));
            }
            self.restore(save);
        }
        Ok(None)
    }

    // ==================== TYPES ====================

    fn parse_type(&mut self) -> Result<Type> {
        self.parse_type_inner(false)
    }

    fn parse_type_inner(&mut self, allow_infer: bool) -> Result<Type> {
        let base = if allow_infer
            && matches!(self.peek_kind(), TokenKind::Ident { name, .. } if name == "_")
        {
            self.bump();
            Type::Infer
        } else if self.eat_kw(Keyword::Void) {
            Type::Void
        } else if self.eat_kw(Keyword::Never) {
            Type::Never
        } else if self.eat_kw(Keyword::Dynamic) {
            Type::Dynamic
        } else if self.check_kw(Keyword::Forall)
            || self.check_kw(Keyword::Unsafe)
            || self.check_kw(Keyword::Extern)
        {
            self.parse_fn_ptr_type()?
        } else if self.eat_kw(Keyword::Read) {
            let life = self.try_parse_lifetime_angle()?;
            if life.is_none() && self.eat_kw(Keyword::Ptr) {
                self.expect(&TokenKind::Lt, "'<'")?;
                let inner = self.parse_type_inner(allow_infer)?;
                self.expect_gt()?;
                Type::ReadPtr(Box::new(inner))
            } else {
                let inner = self.parse_type_inner(allow_infer)?;
                Type::Read {
                    life,
                    inner: Box::new(inner),
                }
            }
        } else if self.eat_kw(Keyword::Write) {
            let life = self.try_parse_lifetime_angle()?;
            if life.is_none() && self.eat_kw(Keyword::Ptr) {
                self.expect(&TokenKind::Lt, "'<'")?;
                let inner = self.parse_type_inner(allow_infer)?;
                self.expect_gt()?;
                Type::WritePtr(Box::new(inner))
            } else {
                let inner = self.parse_type_inner(allow_infer)?;
                Type::Write {
                    life,
                    inner: Box::new(inner),
                }
            }
        } else if self.eat_kw(Keyword::Slice) {
            self.expect(&TokenKind::Lt, "'<' after 'slice'")?;
            let inner = self.parse_type_inner(allow_infer)?;
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
                if self.eat(&TokenKind::ThinArrow) {
                    let ret = self.parse_type_inner(allow_infer)?;
                    Type::FnPtr {
                        lifetimes: Vec::new(),
                        is_unsafe: false,
                        extern_abi: None,
                        params: Vec::new(),
                        is_variadic: false,
                        ret: Box::new(ret),
                    }
                } else {
                    Type::Void
                }
            } else {
                let first_ty = self.parse_type_inner(allow_infer)?;
                // Qualified associated type `(I as Iterator).Item`
                if self.eat_kw(Keyword::As) {
                    let trait_path = self.parse_type_path(allow_infer)?;
                    self.expect(&TokenKind::RParen, "')'")?;
                    if !self.eat(&TokenKind::Dot) {
                        self.expect(&TokenKind::ColonColon, "'.' after '(Type as Trait)'")?;
                    }
                    let item = self.expect_member_ident()?;
                    let args = self.parse_optional_generic_args()?;
                    Type::QualifiedPath {
                        self_ty: Box::new(first_ty),
                        trait_path,
                        item,
                        args,
                    }
                } else {
                    let first_name = if matches!(self.peek_kind(), TokenKind::Ident { .. }) {
                        Some(self.expect_ident()?)
                    } else {
                        None
                    };
                    let saw_comma = self.eat(&TokenKind::Comma);
                    let mut elems = vec![TupleTypeElem {
                        ty: first_ty,
                        name: first_name,
                    }];
                    let mut is_variadic = false;
                    if saw_comma {
                        while !self.check(&TokenKind::RParen) && !self.is_eof() {
                            if self.eat(&TokenKind::DotDotDot) {
                                is_variadic = true;
                                self.eat(&TokenKind::Comma);
                                break;
                            }
                            let ty = self.parse_type_inner(allow_infer)?;
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
                    }
                    self.expect(&TokenKind::RParen, "')'")?;
                    if self.eat(&TokenKind::ThinArrow) {
                        let params = elems
                            .into_iter()
                            .map(|e| FnPtrParam {
                                name: e.name,
                                ty: e.ty,
                            })
                            .collect();
                        let ret = self.parse_type_inner(allow_infer)?;
                        Type::FnPtr {
                            lifetimes: Vec::new(),
                            is_unsafe: false,
                            extern_abi: None,
                            params,
                            is_variadic,
                            ret: Box::new(ret),
                        }
                    } else if is_variadic {
                        return Err(self.err_here(
                            "Variadic '...' is only valid in function pointer types with '->'",
                        ));
                    } else if !saw_comma && elems.len() == 1 && elems[0].name.is_none() {
                        Type::Grouped(Box::new(elems.pop().unwrap().ty))
                    } else {
                        Type::Tuple(elems)
                    }
                }
            }
        } else {
            let tp = self.parse_type_path(allow_infer)?;
            if self.check(&TokenKind::Bang)
                && matches!(
                    self.peek_at(1).kind,
                    TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace
                )
                && tp
                    .segments
                    .iter()
                    .all(|s| matches!(s.args, TypePathArgs::None))
            {
                self.bump(); // '!'
                let path = tp.segments.into_iter().map(|s| s.name).collect();
                Type::Macro(self.parse_macro_call_body(tp.global, path)?)
            } else {
                Type::Path(tp)
            }
        };

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

    fn parse_fn_ptr_type(&mut self) -> Result<Type> {
        let lifetimes = if self.eat_kw(Keyword::Forall) {
            self.parse_forall_lifetimes()?
        } else {
            Vec::new()
        };
        let is_unsafe = self.eat_kw(Keyword::Unsafe);
        let extern_abi = if self.eat_kw(Keyword::Extern) {
            if let TokenKind::StringLit(s) = self.peek_kind().clone() {
                self.bump();
                Some(s)
            } else {
                Some("C".to_string())
            }
        } else {
            None
        };
        self.expect(&TokenKind::LParen, "'(' in function pointer type")?;
        let mut params = Vec::new();
        let mut is_variadic = false;
        while !self.check(&TokenKind::RParen) && !self.is_eof() {
            if self.eat(&TokenKind::DotDotDot) {
                is_variadic = true;
                self.eat(&TokenKind::Comma);
                break;
            }
            let ty = self.parse_type()?;
            let name = if matches!(self.peek_kind(), TokenKind::Ident { .. }) {
                Some(self.expect_ident()?)
            } else {
                None
            };
            params.push(FnPtrParam { name, ty });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RParen, "')'")?;
        let ret = if self.eat(&TokenKind::ThinArrow) {
            self.parse_type()?
        } else {
            Type::Void
        };
        Ok(Type::FnPtr {
            lifetimes,
            is_unsafe,
            extern_abi,
            params,
            is_variadic,
            ret: Box::new(ret),
        })
    }

    fn parse_type_path(&mut self, allow_infer: bool) -> Result<TypePath> {
        let global = self.eat_global_prefix();
        let mut segments = Vec::new();
        let first_name = self.expect_path_segment_ident()?;
        let first_args = self.parse_type_path_args(allow_infer)?;
        segments.push(TypePathSegment {
            name: first_name,
            args: first_args,
        });
        while self.eat(&TokenKind::Dot) || self.eat(&TokenKind::ColonColon) {
            let name = self.expect_member_ident()?;
            let args = self.parse_type_path_args(allow_infer)?;
            segments.push(TypePathSegment { name, args });
        }
        Ok(TypePath { global, segments })
    }

    fn parse_type_path_args(&mut self, allow_infer: bool) -> Result<TypePathArgs> {
        if self.eat(&TokenKind::Lt) {
            let args = self.parse_generic_arg_list()?;
            self.expect_gt()?;
            Ok(TypePathArgs::Angle(args))
        } else if self.check(&TokenKind::LParen) && self.is_paren_trait_args() {
            self.bump(); // '('
            let mut params = Vec::new();
            while !self.check(&TokenKind::RParen) && !self.is_eof() {
                params.push(self.parse_type_inner(allow_infer)?);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::RParen, "')'")?;
            let ret = if self.eat(&TokenKind::ThinArrow) {
                Some(Box::new(self.parse_type_inner(allow_infer)?))
            } else {
                None
            };
            Ok(TypePathArgs::Parenthesized { params, ret })
        } else {
            Ok(TypePathArgs::None)
        }
    }

    /// Distinguishes `Fn(int) -> bool` from a function signature `ReturnType FnName(int x)`.
    fn is_paren_trait_args(&mut self) -> bool {
        let save = self.checkpoint();
        self.bump(); // '('
        if self.eat(&TokenKind::RParen) {
            let ok = self.check(&TokenKind::ThinArrow);
            self.restore(save);
            return ok;
        }
        let mut all_types_no_names = true;
        while !self.check(&TokenKind::RParen) && !self.is_eof() {
            if self.parse_type().is_err() {
                all_types_no_names = false;
                break;
            }
            // If followed by an identifier, it's a parameter `int x` rather than trait arg `int`
            if matches!(
                self.peek_kind(),
                TokenKind::Ident { .. } | TokenKind::Kw(Keyword::This)
            ) {
                all_types_no_names = false;
                break;
            }
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        let ok = all_types_no_names && self.eat(&TokenKind::RParen);
        self.restore(save);
        ok
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
            } else if self.eat_kw(Keyword::Static) {
                args.push(GenericArg::Lifetime("static".to_string()));
            } else if self.eat(&TokenKind::LBrace) {
                let expr = self.parse_expr()?;
                self.expect(&TokenKind::RBrace, "'}'")?;
                args.push(GenericArg::ConstBlock(expr));
            } else if matches!(self.peek_kind(), TokenKind::Ident { .. })
                && let Some(assoc) = self.try_parse_assoc_generic_arg()?
            {
                args.push(assoc);
            } else if self.check(&TokenKind::Minus)
                || matches!(
                    self.peek_kind(),
                    TokenKind::IntLit(_) | TokenKind::Kw(Keyword::True | Keyword::False)
                )
            {
                let expr = self.parse_const_generic_expr()?;
                args.push(GenericArg::ConstExpr(expr));
            } else {
                args.push(GenericArg::Type(self.parse_type_inner(true)?));
            }
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        Ok(args)
    }

    fn try_parse_assoc_generic_arg(&mut self) -> Result<Option<GenericArg>> {
        let save = self.checkpoint();
        let name = self.expect_ident()?;
        let generics = if self.check(&TokenKind::Lt) {
            match self.parse_optional_generic_args() {
                Ok(g) => g,
                Err(_) => {
                    self.restore(save);
                    return Ok(None);
                }
            }
        } else {
            Vec::new()
        };
        if self.eat(&TokenKind::Eq) {
            let ty = self.parse_type_inner(true)?;
            return Ok(Some(GenericArg::AssocBinding { name, generics, ty }));
        }
        if self.eat(&TokenKind::Colon) {
            let mut bounds = vec![self.parse_type_bound()?];
            while self.eat(&TokenKind::Plus) {
                bounds.push(self.parse_type_bound()?);
            }
            return Ok(Some(GenericArg::AssocBound {
                name,
                generics,
                bounds,
            }));
        }
        self.restore(save);
        Ok(None)
    }

    // ==================== BLOCKS & STATEMENTS ====================

    fn parse_block(&mut self) -> Result<Block> {
        self.expect(&TokenKind::LBrace, "'{'")?;
        let (inner_docs, inner_attrs) = self.collect_inner_meta()?;
        let mut stmts = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_eof() {
            stmts.push(self.parse_stmt()?);
        }
        self.expect(&TokenKind::RBrace, "'}'")?;
        Ok(Block {
            inner_docs,
            inner_attrs,
            stmts,
        })
    }

    fn parse_stmt(&mut self) -> Result<Stmt> {
        let attrs = self.try_parse_contextual_attrs(false)?;

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

        if let TokenKind::RustBlock(raw) = self.peek_kind().clone() {
            self.bump();
            return Ok(Stmt {
                attrs,
                kind: StmtKind::InlineRust(raw),
            });
        }

        if self.eat_kw(Keyword::Return) {
            let expr = if self.check(&TokenKind::Semicolon) {
                None
            } else {
                Some(self.parse_expr()?)
            };
            self.expect(&TokenKind::Semicolon, "';' after return")?;
            return Ok(Stmt {
                attrs,
                kind: StmtKind::Return(expr),
            });
        }
        if self.eat_kw(Keyword::Break) {
            let (label, value) = self.parse_break_payload()?;
            self.expect(&TokenKind::Semicolon, "';' after break")?;
            return Ok(Stmt {
                attrs,
                kind: StmtKind::Break { label, value },
            });
        }
        if self.eat_kw(Keyword::Continue) {
            let label = if matches!(self.peek_kind(), TokenKind::Ident { .. }) {
                Some(self.expect_ident()?)
            } else {
                None
            };
            self.expect(&TokenKind::Semicolon, "';' after continue")?;
            return Ok(Stmt {
                attrs,
                kind: StmtKind::Continue { label },
            });
        }

        if self.check_kw(Keyword::If) {
            return Ok(Stmt {
                attrs,
                kind: StmtKind::If(self.parse_if()?),
            });
        }
        if self.check_kw(Keyword::Require) {
            let mut st = self.parse_require()?;
            st.attrs = attrs;
            return Ok(st);
        }
        if self.check_kw(Keyword::While) {
            let mut st = self.parse_while(None)?;
            st.attrs = attrs;
            return Ok(st);
        }
        if self.check_kw(Keyword::For) {
            let mut st = self.parse_for(None)?;
            st.attrs = attrs;
            return Ok(st);
        }
        if self.check_kw(Keyword::Foreach) {
            let mut st = self.parse_foreach(None)?;
            st.attrs = attrs;
            return Ok(st);
        }
        if self.check_kw(Keyword::Loop) {
            let mut st = self.parse_loop(None)?;
            st.attrs = attrs;
            return Ok(st);
        }
        if self.check_kw(Keyword::Match) {
            return Ok(Stmt {
                attrs,
                kind: StmtKind::Match(self.parse_match()?),
            });
        }
        if self.check_kw(Keyword::Unsafe) && matches!(self.peek_at(1).kind, TokenKind::LBrace) {
            self.bump();
            return Ok(Stmt {
                attrs,
                kind: StmtKind::Unsafe(self.parse_block()?),
            });
        }

        // Labeled loop statement: `outer: loop/while/for/foreach`
        if matches!(
            (
                self.peek_kind(),
                &self.peek_at(1).kind,
                &self.peek_at(2).kind
            ),
            (
                TokenKind::Ident { .. },
                TokenKind::Colon,
                TokenKind::Kw(Keyword::Loop | Keyword::While | Keyword::For | Keyword::Foreach)
            )
        ) {
            let label = self.expect_ident()?;
            self.bump(); // ':'
            let mut st = match self.peek_kind() {
                TokenKind::Kw(Keyword::Loop) => self.parse_loop(Some(label))?,
                TokenKind::Kw(Keyword::While) => self.parse_while(Some(label))?,
                TokenKind::Kw(Keyword::For) => self.parse_for(Some(label))?,
                _ => self.parse_foreach(Some(label))?,
            };
            st.attrs = attrs;
            return Ok(st);
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
            return Ok(Stmt {
                attrs,
                kind: StmtKind::Discard(expr),
            });
        }

        // Explicit local variable starting with `impl Trait name = ...`
        if self.check_kw(Keyword::Impl)
            && let Some(mut stmt) = self.try_parse_local_decl(true)?
        {
            stmt.attrs = attrs;
            return Ok(stmt);
        }

        // Nested item statements (const item vs const expr block handled carefully!)
        if self.is_nested_item_start() {
            let mut item = self.parse_item_after_meta(Vec::new(), attrs, false)?;
            if item.docs.is_empty() {
                item.docs = Vec::new();
            }
            return Ok(Stmt::plain(StmtKind::Item(Box::new(item))));
        }

        // General destructuring statement: `(int a, int b) = pair;`, `[int a, int b] = arr;`, `Point { int X; int Y; } = pt;`
        if let Some(mut stmt) = self.try_parse_destructure_stmt()? {
            stmt.attrs = attrs;
            return Ok(stmt);
        }

        // Explicit local variable declaration: `shadow? mut? Type name (= expr)?;`
        if let Some(mut stmt) = self.try_parse_local_decl(true)? {
            stmt.attrs = attrs;
            return Ok(stmt);
        }

        let expr = self.parse_expr()?;
        if self.check(&TokenKind::RBrace) {
            return Err(self.err_here(
                "Implicit return is not allowed in R#; use an explicit 'return <expr>;' statement",
            ));
        }
        self.expect(&TokenKind::Semicolon, "';' after statement")?;
        Ok(Stmt {
            attrs,
            kind: StmtKind::Expr(expr),
        })
    }

    fn parse_break_payload(&mut self) -> Result<(Option<String>, Option<Expr>)> {
        if self.check(&TokenKind::Semicolon) {
            return Ok((None, None));
        }
        if let TokenKind::Ident { .. } = self.peek_kind() {
            let lbl_span = self.peek().span;
            let next_tok = self.peek_at(1);
            let next = &next_tok.kind;
            if matches!(next, TokenKind::Semicolon) {
                let lbl = self.expect_ident()?;
                return Ok((Some(lbl), None));
            }
            let separated_lparen =
                matches!(next, TokenKind::LParen) && lbl_span.end < next_tok.span.start;
            if separated_lparen || !is_binary_or_postfix_continuation(next) {
                let save = self.checkpoint();
                let lbl = self.expect_ident()?;
                if let Ok(val) = self.parse_expr()
                    && self.check(&TokenKind::Semicolon)
                {
                    return Ok((Some(lbl), Some(val)));
                }
                self.restore(save);
            }
        }
        let val = self.parse_expr()?;
        Ok((None, Some(val)))
    }

    fn is_nested_item_start(&mut self) -> bool {
        if self.check_kw(Keyword::Const) {
            return self.is_const_item();
        }
        if self.check_kw(Keyword::Static) {
            return self.is_static_item();
        }
        matches!(
            self.peek_kind(),
            TokenKind::Kw(
                Keyword::Struct
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

    fn try_parse_destructure_stmt(&mut self) -> Result<Option<Stmt>> {
        // Only try on `(`, `[`, or `Ident ... {` (struct pattern)
        if !matches!(
            self.peek_kind(),
            TokenKind::LParen | TokenKind::LBracket | TokenKind::Ident { .. }
        ) {
            return Ok(None);
        }
        let save = self.checkpoint();
        if let Ok(pat) = self.parse_single_pattern()
            && matches!(
                pat,
                Pattern::Tuple(_) | Pattern::Slice(_) | Pattern::Struct { .. }
            )
            && self.eat(&TokenKind::Eq)
        {
            let init = self.parse_expr()?;
            self.expect(&TokenKind::Semicolon, "';'")?;
            let kind = if pattern_has_explicit_type(&pat) {
                StmtKind::Destructure { pat, init }
            } else {
                StmtKind::DestructureAssign { pat, value: init }
            };
            return Ok(Some(Stmt::plain(kind)));
        }
        self.restore(save);
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
                if e.to_string()
                    .contains("Array type requires explicit length")
                {
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
        Ok(Some(Stmt::plain(StmtKind::Local {
            is_shadow,
            is_mut,
            ty,
            name,
            init,
        })))
    }

    fn parse_condition(&mut self) -> Result<Condition> {
        let expr = self.parse_expr()?;
        Ok(expr_to_condition(expr))
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
            } => Ok(Stmt::plain(StmtKind::Require {
                expr,
                pat,
                else_block,
            })),
            _ => Err(self.err_here("'require' expects a pattern condition '(expr is Pattern)'")),
        }
    }

    fn parse_while(&mut self, label: Option<String>) -> Result<Stmt> {
        self.expect_kw(Keyword::While)?;
        self.expect(&TokenKind::LParen, "'(' after 'while'")?;
        let cond = self.parse_condition()?;
        self.expect(&TokenKind::RParen, "')'")?;
        let body = self.parse_block()?;
        Ok(Stmt::plain(StmtKind::While { label, cond, body }))
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
            Some(Box::new(Stmt::plain(StmtKind::Expr(e))))
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
        Ok(Stmt::plain(StmtKind::For {
            label,
            init,
            cond,
            step,
            body,
        }))
    }

    fn parse_foreach(&mut self, label: Option<String>) -> Result<Stmt> {
        self.expect_kw(Keyword::Foreach)?;
        self.expect(&TokenKind::LParen, "'(' after 'foreach'")?;
        let pat = self.parse_pattern()?;
        self.expect_kw(Keyword::In)?;
        let iter = self.parse_expr()?;
        self.expect(&TokenKind::RParen, "')'")?;
        let body = self.parse_block()?;
        Ok(Stmt::plain(StmtKind::Foreach {
            label,
            pat,
            iter,
            body,
        }))
    }

    fn parse_loop(&mut self, label: Option<String>) -> Result<Stmt> {
        self.expect_kw(Keyword::Loop)?;
        let body = self.parse_block()?;
        Ok(Stmt::plain(StmtKind::Loop { label, body }))
    }

    fn parse_match(&mut self) -> Result<MatchExpr> {
        self.expect_kw(Keyword::Match)?;
        self.expect(&TokenKind::LParen, "'(' after 'match'")?;
        let scrutinee = Box::new(self.parse_expr()?);
        self.expect(&TokenKind::RParen, "')'")?;
        self.expect(&TokenKind::LBrace, "'{'")?;
        let mut arms = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_eof() {
            let attrs = self.try_parse_contextual_attrs(true)?;
            let pat = self.parse_pattern()?;
            let guard = if self.eat_kw(Keyword::When) {
                Some(self.parse_condition()?)
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
            arms.push(MatchArm {
                attrs,
                pat,
                guard,
                body,
            });
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
        let base = self.parse_atom_pattern()?;
        // Optional subpattern binding: `<pattern> as mut? <Type> <name>`
        if self.eat_kw(Keyword::As) {
            let is_mut = self.eat_kw(Keyword::Mut);
            let ty = self.parse_type()?;
            let name = self.expect_ident()?;
            return Ok(Pattern::BindAt {
                is_mut,
                ty,
                name,
                subpat: Box::new(base),
            });
        }
        Ok(base)
    }

    fn parse_atom_pattern(&mut self) -> Result<Pattern> {
        // Wildcard `_`
        if matches!(self.peek_kind(), TokenKind::Ident { name, .. } if name == "_") {
            self.bump();
            return Ok(Pattern::Wildcard);
        }

        // Half-open range `..=59` / `..60` OR standalone rest `..`
        if self.check(&TokenKind::DotDotEq) || self.check(&TokenKind::DotDot) {
            let inclusive = self.check(&TokenKind::DotDotEq);
            self.bump();
            if inclusive || self.starts_pattern_literal() {
                let end_expr = self.parse_range_pat_endpoint()?;
                return Ok(Pattern::Range {
                    start: None,
                    end: Some(Box::new(end_expr)),
                    inclusive,
                });
            }
            return Ok(Pattern::Rest);
        }

        // Borrow-out pattern: `borrow read T x` / `borrow write T x`
        if self.eat_kw(Keyword::Borrow) {
            let mode = if self.eat_kw(Keyword::Write) {
                RefMode::Write
            } else {
                self.expect_kw(Keyword::Read)?;
                RefMode::Read
            };
            let ty = self.parse_type()?;
            let name = self.expect_ident()?;
            return Ok(Pattern::BorrowRef { mode, ty, name });
        }

        // Reference-stripping pattern: `deref int val` (`&val`) or `deref write int val` (`&mut val`)
        if self.eat_kw(Keyword::Deref) {
            let is_mut = if self.eat_kw(Keyword::Write) {
                true
            } else {
                self.eat_kw(Keyword::Read);
                false
            };
            let inner = self.parse_single_pattern()?;
            return Ok(Pattern::DerefPat {
                is_mut,
                inner: Box::new(inner),
            });
        }

        // Inline const pattern: `const(MAX - 1)`
        if self.eat_kw(Keyword::Const) {
            self.expect(&TokenKind::LParen, "'(' after 'const' in pattern")?;
            let expr = self.parse_expr()?;
            self.expect(&TokenKind::RParen, "')'")?;
            return Ok(Pattern::Const(Box::new(expr)));
        }

        // Slice / list pattern: `[]`, `[int first, .. read slice<int> mid, int last]`
        if self.eat(&TokenKind::LBracket) {
            let mut elems = Vec::new();
            while !self.check(&TokenKind::RBracket) && !self.is_eof() {
                if self.eat(&TokenKind::DotDot) {
                    if self.check(&TokenKind::Comma) || self.check(&TokenKind::RBracket) {
                        elems.push(SlicePatElem::Rest);
                    } else if let Some(Pattern::Binding { is_mut, ty, name }) =
                        self.try_parse_typed_pattern_binding()?
                    {
                        elems.push(SlicePatElem::RestBind { is_mut, ty, name });
                    } else {
                        let is_mut = self.eat_kw(Keyword::Mut);
                        let name = self.expect_ident()?;
                        elems.push(SlicePatElem::RestBind {
                            is_mut,
                            ty: None,
                            name,
                        });
                    }
                } else {
                    elems.push(SlicePatElem::Pat(self.parse_pattern()?));
                }
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::RBracket, "']'")?;
            return Ok(Pattern::Slice(elems));
        }

        // Literal or range pattern (`1`, `-1`, `1..=5`, `90..`, `'a'..='z'`, `"foo"`, `b'a'`, `"hi"u8`)
        if let Some(pat) = self.try_parse_lit_or_range_pattern()? {
            return Ok(pat);
        }

        // Parenthesized pattern `(1 | 2)` or tuple pattern `()`, `(int a,)`, `(int a, .., string b)`
        if self.eat(&TokenKind::LParen) {
            if self.eat(&TokenKind::RParen) {
                return Ok(Pattern::Tuple(Vec::new()));
            }
            let first = self.parse_pattern()?;
            if self.eat(&TokenKind::Comma) {
                let mut pats = vec![first];
                while !self.check(&TokenKind::RParen) && !self.is_eof() {
                    pats.push(self.parse_pattern()?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RParen, "')'")?;
                return Ok(Pattern::Tuple(pats));
            }
            self.expect(&TokenKind::RParen, "')'")?;
            return Ok(Pattern::Paren(Box::new(first)));
        }

        // Typed binding pattern: `mut? Type name`
        if let Some(binding) = self.try_parse_typed_pattern_binding()? {
            return Ok(binding);
        }

        // Path / Macro / TupleStruct / Struct pattern
        let global = self.eat_global_prefix();
        let path = self.parse_simple_path()?;
        if self.check(&TokenKind::Bang)
            && matches!(
                self.peek_at(1).kind,
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace
            )
        {
            self.bump(); // '!'
            return Ok(Pattern::Macro(self.parse_macro_call_body(global, path)?));
        }
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

    fn starts_pattern_literal(&self) -> bool {
        matches!(
            self.peek_kind(),
            TokenKind::IntLit(_)
                | TokenKind::FloatLit(_)
                | TokenKind::CharLit(_)
                | TokenKind::ByteLit(_)
                | TokenKind::Minus
        )
    }

    fn parse_range_pat_endpoint(&mut self) -> Result<Expr> {
        if self.eat(&TokenKind::Minus) {
            let inner = self.parse_primary()?;
            Ok(Expr::Unary {
                op: UnaryOp::Neg,
                expr: Box::new(inner),
            })
        } else {
            self.parse_primary()
        }
    }

    fn parse_struct_pat_field(&mut self) -> Result<StructPatField> {
        let save = self.checkpoint();
        let is_mut = self.eat_kw(Keyword::Mut);
        if let Ok(ty) = self.parse_type()
            && let Ok(name) = self.expect_ident()
            && matches!(
                self.peek_kind(),
                TokenKind::Semicolon | TokenKind::Comma | TokenKind::RBrace
            )
        {
            return Ok(StructPatField {
                ty: Some(ty),
                is_mut,
                name,
                pat: None,
            });
        }
        self.restore(save);
        let name = match self.peek_kind().clone() {
            TokenKind::IntLit(idx) => {
                self.bump();
                idx
            }
            _ => self.expect_ident()?,
        };
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
        let save = self.checkpoint();
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
            TokenKind::RawStringLit(s) => {
                self.bump();
                Literal::RawStr(s)
            }
            TokenKind::ByteLit(b) => {
                self.bump();
                Literal::Byte(b)
            }
            TokenKind::ByteStringLit { value, raw } => {
                self.bump();
                Literal::ByteStr { value, raw }
            }
            TokenKind::CStringLit { value, raw } => {
                self.bump();
                Literal::CStr { value, raw }
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
                        self.restore(save);
                        return Ok(None);
                    }
                }
            }
            _ => return Ok(None),
        };

        if self.eat(&TokenKind::DotDotEq) || self.eat(&TokenKind::DotDot) {
            let inclusive = matches!(self.tokens[self.pos - 1].kind, TokenKind::DotDotEq);
            let end = if inclusive || self.starts_pattern_literal() {
                Some(Box::new(self.parse_range_pat_endpoint()?))
            } else {
                None
            };
            return Ok(Some(Pattern::Range {
                start: Some(Box::new(Expr::Lit(lit))),
                end,
                inclusive,
            }));
        }

        Ok(Some(Pattern::Lit(lit)))
    }

    fn try_parse_typed_pattern_binding(&mut self) -> Result<Option<Pattern>> {
        let save = self.checkpoint();
        let is_mut = self.eat_kw(Keyword::Mut);
        let Ok(ty) = self.parse_type() else {
            self.restore(save);
            return Ok(None);
        };
        let Ok(name) = self.expect_ident() else {
            self.restore(save);
            return Ok(None);
        };
        if self.check(&TokenKind::LParen) || self.check(&TokenKind::LBrace) {
            self.restore(save);
            return Ok(None);
        }
        Ok(Some(Pattern::Binding {
            is_mut,
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
            let ty = self.parse_type_inner(true)?;
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
            if self.eat_kw(Keyword::Ptr) {
                return Ok(Expr::RawReadPtr(Box::new(self.parse_unary()?)));
            }
            return Ok(Expr::Read(Box::new(self.parse_unary()?)));
        }
        if self.eat_kw(Keyword::Write) {
            if self.eat_kw(Keyword::Slice) {
                return Ok(Expr::WriteSlice(Box::new(self.parse_unary()?)));
            }
            if self.eat_kw(Keyword::Ptr) {
                return Ok(Expr::RawWritePtr(Box::new(self.parse_unary()?)));
            }
            return Ok(Expr::Write(Box::new(self.parse_unary()?)));
        }
        if self.check_kw(Keyword::Take) {
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
                if let Some((global, path)) = expr_to_path(&expr) {
                    self.bump(); // '!'
                    let mc = self.parse_macro_call_body(global, path)?;
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
                TokenKind::LParen
                    | TokenKind::Dot
                    | TokenKind::ColonColon
                    | TokenKind::Kw(Keyword::Is)
                    | TokenKind::Comma
                    | TokenKind::Semicolon
                    | TokenKind::RParen
                    | TokenKind::RBrace
                    | TokenKind::RBracket
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

    fn parse_macro_call_body(&mut self, global: bool, path: Vec<String>) -> Result<MacroCall> {
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
        Ok(MacroCall {
            global,
            path,
            delim,
            body,
        })
    }

    fn parse_primary(&mut self) -> Result<Expr> {
        // Expression-level attributes: `[Allow(unused)] expr` (avoid stealing array literals `[x]`)
        if self.check(&TokenKind::LBracket) {
            let save = self.checkpoint();
            if let Ok(attr) = self.parse_attr()
                && starts_expr_after_attr(self.peek_kind())
            {
                let mut attrs = vec![attr];
                while self.check(&TokenKind::LBracket) {
                    let s2 = self.checkpoint();
                    if let Ok(a) = self.parse_attr()
                        && starts_expr_after_attr(self.peek_kind())
                    {
                        attrs.push(a);
                    } else {
                        self.restore(s2);
                        break;
                    }
                }
                let inner = self.parse_unary()?;
                return Ok(Expr::Attributed {
                    attrs,
                    expr: Box::new(inner),
                });
            }
            self.restore(save);
        }

        if let TokenKind::RustBlock(raw) = self.peek_kind().clone() {
            self.bump();
            return Ok(Expr::InlineRust(raw));
        }

        // Control-flow expressions: `return`, `break`, `continue`
        if self.eat_kw(Keyword::Return) {
            let val = if starts_expr_after_control_flow(self.peek_kind()) {
                Some(Box::new(self.parse_expr()?))
            } else {
                None
            };
            return Ok(Expr::Return(val));
        }
        if self.eat_kw(Keyword::Break) {
            let (label, value) = self.parse_break_expr_payload()?;
            return Ok(Expr::Break {
                label,
                value: value.map(Box::new),
            });
        }
        if self.eat_kw(Keyword::Continue) {
            let label = if matches!(self.peek_kind(), TokenKind::Ident { .. }) {
                Some(self.expect_ident()?)
            } else {
                None
            };
            return Ok(Expr::Continue { label });
        }

        // Expression-bodied `if (cond) => expr else if (...) => expr else => expr`
        if self.check_kw(Keyword::If) {
            return self.parse_if_expr();
        }

        if self.check_kw(Keyword::Match) {
            return Ok(Expr::Match(self.parse_match()?));
        }

        if self.eat_kw(Keyword::Loop) {
            let block = self.parse_block()?;
            return Ok(Expr::Loop { label: None, block });
        }
        if self.check_kw(Keyword::While) {
            self.bump();
            self.expect(&TokenKind::LParen, "'(' after 'while'")?;
            let cond = self.parse_condition()?;
            self.expect(&TokenKind::RParen, "')'")?;
            let body = self.parse_block()?;
            return Ok(Expr::While {
                label: None,
                cond: Box::new(cond),
                body,
            });
        }
        if self.check_kw(Keyword::Foreach) {
            self.bump();
            self.expect(&TokenKind::LParen, "'(' after 'foreach'")?;
            let pat = self.parse_pattern()?;
            self.expect_kw(Keyword::In)?;
            let iter = self.parse_expr()?;
            self.expect(&TokenKind::RParen, "')'")?;
            let body = self.parse_block()?;
            return Ok(Expr::Foreach {
                label: None,
                pat,
                iter: Box::new(iter),
                body,
            });
        }

        // Inline `const(expr)` or `const { ... }`
        if self.eat_kw(Keyword::Const) {
            if self.eat(&TokenKind::LParen) {
                let expr = self.parse_expr()?;
                self.expect(&TokenKind::RParen, "')'")?;
                return Ok(Expr::ConstBlock(ConstBody::Expr(Box::new(expr))));
            }
            let block = self.parse_block()?;
            return Ok(Expr::ConstBlock(ConstBody::Block(block)));
        }

        if self.check_kw(Keyword::Unsafe) && matches!(self.peek_at(1).kind, TokenKind::LBrace) {
            self.bump();
            return Ok(Expr::UnsafeBlock(self.parse_block()?));
        }

        // Async lambda `async (int x) => ...` or async block `async take? { ... }`
        if self.check_kw(Keyword::Async) {
            if let Some(lambda) = self.try_parse_lambda()? {
                return Ok(lambda);
            }
            self.bump();
            let is_move = self.eat_kw(Keyword::Take);
            let block = self.parse_block()?;
            return Ok(Expr::AsyncBlock { is_move, block });
        }

        // Struct literal: `new Point { X = 1, Y = 2 }` or shorthand `new Point { x, y }`
        if self.eat_kw(Keyword::New) {
            let path = self.parse_simple_path()?;
            let fields = self.parse_struct_field_inits()?;
            return Ok(Expr::StructLit { path, fields });
        }

        // Labeled loop expression `a: loop { break a 42; }` or labeled block `b: { break b 42; }`
        if matches!(
            (self.peek_kind(), &self.peek_at(1).kind),
            (TokenKind::Ident { .. }, TokenKind::Colon)
        ) {
            if matches!(self.peek_at(2).kind, TokenKind::LBrace) {
                let label = self.expect_ident()?;
                self.bump(); // ':'
                let block = self.parse_block()?;
                return Ok(Expr::LabeledBlock { label, block });
            }
            if matches!(self.peek_at(2).kind, TokenKind::Kw(Keyword::Loop)) {
                let label = self.expect_ident()?;
                self.bump(); // ':'
                self.bump(); // 'loop'
                let block = self.parse_block()?;
                return Ok(Expr::Loop {
                    label: Some(label),
                    block,
                });
            }
            if matches!(self.peek_at(2).kind, TokenKind::Kw(Keyword::While)) {
                let label = self.expect_ident()?;
                self.bump(); // ':'
                self.bump(); // 'while'
                self.expect(&TokenKind::LParen, "'(' after 'while'")?;
                let cond = self.parse_condition()?;
                self.expect(&TokenKind::RParen, "')'")?;
                let body = self.parse_block()?;
                return Ok(Expr::While {
                    label: Some(label),
                    cond: Box::new(cond),
                    body,
                });
            }
            if matches!(self.peek_at(2).kind, TokenKind::Kw(Keyword::Foreach)) {
                let label = self.expect_ident()?;
                self.bump(); // ':'
                self.bump(); // 'foreach'
                self.expect(&TokenKind::LParen, "'(' after 'foreach'")?;
                let pat = self.parse_pattern()?;
                self.expect_kw(Keyword::In)?;
                let iter = self.parse_expr()?;
                self.expect(&TokenKind::RParen, "')'")?;
                let body = self.parse_block()?;
                return Ok(Expr::Foreach {
                    label: Some(label),
                    pat,
                    iter: Box::new(iter),
                    body,
                });
            }
        }

        // Global path expression: `global::std.mem.size_of<int>()` or `::std.mem.size_of<int>()`
        if self.eat_global_prefix() {
            let name = self.expect_path_segment_ident()?;
            let type_args = self.try_parse_expr_generic_args();
            return Ok(Expr::Path {
                global: true,
                segments: vec![PathExprSegment { name, type_args }],
            });
        }

        // Lambda `(int x) : int => ...` or `() => ...`
        if let Some(lambda) = self.try_parse_lambda()? {
            return Ok(lambda);
        }

        // UFCS `(Type as TraitPath).member` when `Type` is not a plain cast expression
        if self.check(&TokenKind::LParen)
            && let Some(ufcs) = self.try_parse_ufcs()?
        {
            return Ok(ufcs);
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
            TokenKind::RawStringLit(s) => {
                self.bump();
                Ok(Expr::Lit(Literal::RawStr(s)))
            }
            TokenKind::ByteLit(b) => {
                self.bump();
                Ok(Expr::Lit(Literal::Byte(b)))
            }
            TokenKind::ByteStringLit { value, raw } => {
                self.bump();
                Ok(Expr::Lit(Literal::ByteStr { value, raw }))
            }
            TokenKind::CStringLit { value, raw } => {
                self.bump();
                Ok(Expr::Lit(Literal::CStr { value, raw }))
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
                            parsed.push(InterpExprPart::Expr {
                                expr: Box::new(expr),
                                format,
                            });
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
                if name == "_" {
                    Ok(Expr::Infer)
                } else {
                    Ok(Expr::Ident(name))
                }
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
            _ => Err(self.err_here(format!("Expected expression, found {:?}", self.peek_kind()))),
        }
    }

    fn try_parse_ufcs(&mut self) -> Result<Option<Expr>> {
        let save = self.checkpoint();
        self.bump(); // '('
        if let Ok(self_ty) = self.parse_type()
            && self.eat_kw(Keyword::As)
            && let Ok(trait_path) = self.parse_type_path(false)
            && self.eat(&TokenKind::RParen)
            && (self.eat(&TokenKind::Dot) || self.eat(&TokenKind::ColonColon))
            && let Ok(member) = self.expect_member_ident()
        {
            // Avoid stealing casts like `(x as int).to_string()` when target is a primitive alias!
            let target_is_primitive = trait_path.segments.len() == 1
                && matches!(trait_path.segments[0].args, TypePathArgs::None)
                && is_primitive_or_scalar_name(&trait_path.segments[0].name);
            if !target_is_primitive {
                let type_args = self.try_parse_expr_generic_args();
                return Ok(Some(Expr::QualifiedMember {
                    self_ty: Box::new(self_ty),
                    trait_path,
                    member,
                    type_args,
                }));
            }
        }
        self.restore(save);
        Ok(None)
    }

    fn parse_if_expr(&mut self) -> Result<Expr> {
        self.expect_kw(Keyword::If)?;
        self.expect(&TokenKind::LParen, "'(' after 'if'")?;
        let cond = self.parse_condition()?;
        self.expect(&TokenKind::RParen, "')'")?;
        self.expect(&TokenKind::FatArrow, "'=>' in expression-bodied 'if'")?;
        let then_expr = self.parse_expr()?;
        self.expect_kw(Keyword::Else)?;
        let else_expr = if self.check_kw(Keyword::If) {
            self.parse_if_expr()?
        } else {
            self.expect(&TokenKind::FatArrow, "'=>' after 'else' in 'if' expression")?;
            self.parse_expr()?
        };
        Ok(Expr::IfExpr {
            cond: Box::new(cond),
            then_expr: Box::new(then_expr),
            else_expr: Box::new(else_expr),
        })
    }

    fn parse_break_expr_payload(&mut self) -> Result<(Option<String>, Option<Expr>)> {
        if !starts_expr_after_control_flow(self.peek_kind()) {
            return Ok((None, None));
        }
        if let TokenKind::Ident { .. } = self.peek_kind() {
            let lbl_span = self.peek().span;
            let next_tok = self.peek_at(1);
            let next = &next_tok.kind;
            if !starts_expr_after_control_flow(next) && !is_binary_or_postfix_continuation(next) {
                let lbl = self.expect_ident()?;
                return Ok((Some(lbl), None));
            }
            let separated_lparen =
                matches!(next, TokenKind::LParen) && lbl_span.end < next_tok.span.start;
            if separated_lparen || !is_binary_or_postfix_continuation(next) {
                let save = self.checkpoint();
                let lbl = self.expect_ident()?;
                if starts_expr_after_control_flow(self.peek_kind())
                    && let Ok(val) = self.parse_expr()
                {
                    return Ok((Some(lbl), Some(val)));
                }
                self.restore(save);
            }
        }
        let val = self.parse_expr()?;
        Ok((None, Some(val)))
    }

    fn parse_struct_field_inits(&mut self) -> Result<Vec<StructFieldInit>> {
        self.expect(&TokenKind::LBrace, "'{'")?;
        let mut fields = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_eof() {
            let attrs = self.parse_attrs()?;
            let name = match self.peek_kind().clone() {
                TokenKind::IntLit(idx) => {
                    self.bump();
                    idx
                }
                _ => self.expect_ident()?,
            };
            let value = if self.eat(&TokenKind::Eq) {
                Some(self.parse_expr()?)
            } else {
                None
            };
            fields.push(StructFieldInit { attrs, name, value });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RBrace, "'}'")?;
        Ok(fields)
    }

    fn try_parse_lambda(&mut self) -> Result<Option<Expr>> {
        let save = self.checkpoint();
        let is_async = self.eat_kw(Keyword::Async);
        let is_move = self.eat_kw(Keyword::Take);
        if !self.check(&TokenKind::LParen) {
            self.restore(save);
            return Ok(None);
        }
        let Ok((params, variadic)) = self.parse_params() else {
            self.restore(save);
            return Ok(None);
        };
        if variadic.is_some() || !params.iter().all(lambda_param_is_typed) {
            self.restore(save);
            return Ok(None);
        }
        let ret_ty = if self.eat(&TokenKind::Colon) {
            match self.parse_type() {
                Ok(t) => Some(t),
                Err(_) => {
                    self.restore(save);
                    return Ok(None);
                }
            }
        } else {
            None
        };
        if !self.eat(&TokenKind::FatArrow) {
            self.restore(save);
            return Ok(None);
        }
        let body = if self.check(&TokenKind::LBrace) {
            LambdaBody::Block(self.parse_block()?)
        } else {
            LambdaBody::Expr(Box::new(self.parse_expr()?))
        };
        Ok(Some(Expr::Lambda {
            is_async,
            is_move,
            params,
            ret_ty,
            body,
        }))
    }
}

fn lambda_param_is_typed(param: &Param) -> bool {
    match param {
        Param::Normal { .. } => true,
        Param::Pattern {
            pat, explicit_ty, ..
        } => {
            explicit_ty.is_some()
                || matches!(
                    pat,
                    Pattern::Tuple(_) | Pattern::Struct { .. } | Pattern::Slice(_)
                )
        }
        Param::Receiver(_) => false,
    }
}

fn pattern_has_explicit_type(pat: &Pattern) -> bool {
    match pat {
        Pattern::Binding { ty, .. } => ty.is_some(),
        Pattern::BorrowRef { .. } | Pattern::BindAt { .. } => true,
        Pattern::DerefPat { inner, .. } | Pattern::Paren(inner) => pattern_has_explicit_type(inner),
        Pattern::Tuple(ps) | Pattern::TupleStruct { fields: ps, .. } | Pattern::Or(ps) => {
            ps.iter().any(pattern_has_explicit_type)
        }
        Pattern::Slice(elems) => elems.iter().any(|e| match e {
            SlicePatElem::Pat(p) => pattern_has_explicit_type(p),
            SlicePatElem::RestBind { ty, .. } => ty.is_some(),
            SlicePatElem::Rest => false,
        }),
        Pattern::Struct { fields, .. } => fields
            .iter()
            .any(|f| f.ty.is_some() || f.pat.as_ref().is_some_and(pattern_has_explicit_type)),
        _ => false,
    }
}

fn starts_expr_after_attr(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::LParen
            | TokenKind::LBrace
            | TokenKind::LBracket
            | TokenKind::Ident { .. }
            | TokenKind::IntLit(_)
            | TokenKind::FloatLit(_)
            | TokenKind::StringLit(_)
            | TokenKind::RawStringLit(_)
            | TokenKind::ByteLit(_)
            | TokenKind::ByteStringLit { .. }
            | TokenKind::CStringLit { .. }
            | TokenKind::CharLit(_)
            | TokenKind::InterpString(_)
            | TokenKind::Minus
            | TokenKind::Bang
            | TokenKind::Tilde
            | TokenKind::ColonColon
            | TokenKind::Kw(
                Keyword::True
                    | Keyword::False
                    | Keyword::This
                    | Keyword::SelfType
                    | Keyword::Crate
                    | Keyword::Super
                    | Keyword::New
                    | Keyword::Read
                    | Keyword::Write
                    | Keyword::Take
                    | Keyword::Deref
                    | Keyword::Try
                    | Keyword::Await
                    | Keyword::Match
                    | Keyword::If
                    | Keyword::Loop
                    | Keyword::While
                    | Keyword::Foreach
                    | Keyword::Const
                    | Keyword::Async
                    | Keyword::Unsafe
                    | Keyword::Return
                    | Keyword::Break
                    | Keyword::Continue
            )
    )
}

fn starts_expr_after_control_flow(kind: &TokenKind) -> bool {
    !matches!(
        kind,
        TokenKind::Comma
            | TokenKind::Semicolon
            | TokenKind::RParen
            | TokenKind::RBrace
            | TokenKind::RBracket
            | TokenKind::Colon
            | TokenKind::FatArrow
            | TokenKind::Kw(Keyword::Else)
            | TokenKind::Eof
    )
}

fn expr_to_condition(expr: Expr) -> Condition {
    match expr {
        Expr::IsPattern {
            expr: inner,
            negated,
            pat,
        } => Condition::IsPattern {
            expr: *inner,
            negated,
            pat,
        },
        Expr::Binary {
            op: BinaryOp::And, ..
        } => {
            let mut parts = Vec::new();
            flatten_and_chain(expr, &mut parts);
            let has_let = parts
                .iter()
                .any(|p| matches!(p, ChainItem::IsPattern { .. }));
            if has_let {
                Condition::LetChain(parts)
            } else {
                // Reconstruct boolean conjunction
                let mut iter = parts.into_iter().map(|p| match p {
                    ChainItem::Bool(e) => e,
                    ChainItem::IsPattern { expr, pat } => Expr::IsPattern {
                        expr: Box::new(expr),
                        negated: false,
                        pat,
                    },
                });
                let first = iter.next().unwrap();
                let combined = iter.fold(first, |acc, rhs| Expr::Binary {
                    op: BinaryOp::And,
                    lhs: Box::new(acc),
                    rhs: Box::new(rhs),
                });
                Condition::Bool(combined)
            }
        }
        other => Condition::Bool(other),
    }
}

fn flatten_and_chain(expr: Expr, out: &mut Vec<ChainItem>) {
    match expr {
        Expr::Binary {
            op: BinaryOp::And,
            lhs,
            rhs,
        } => {
            flatten_and_chain(*lhs, out);
            flatten_and_chain(*rhs, out);
        }
        Expr::IsPattern {
            expr: inner,
            negated: false,
            pat,
        } => {
            out.push(ChainItem::IsPattern { expr: *inner, pat });
        }
        other => out.push(ChainItem::Bool(other)),
    }
}

fn is_primitive_or_scalar_name(name: &str) -> bool {
    matches!(
        name,
        "sbyte"
            | "byte"
            | "short"
            | "ushort"
            | "int"
            | "uint"
            | "long"
            | "ulong"
            | "int128"
            | "uint128"
            | "nint"
            | "nuint"
            | "float"
            | "double"
            | "bool"
            | "char"
            | "i8"
            | "u8"
            | "i16"
            | "u16"
            | "i32"
            | "u32"
            | "i64"
            | "u64"
            | "i128"
            | "u128"
            | "isize"
            | "usize"
            | "f32"
            | "f64"
    )
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
            global: false,
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

fn expr_to_path(expr: &Expr) -> Option<(bool, Vec<String>)> {
    match expr {
        Expr::Ident(name) => Some((false, vec![name.clone()])),
        Expr::This => Some((false, vec!["self".to_string()])),
        Expr::Path { global, segments } => {
            if segments.iter().all(|s| s.type_args.is_empty()) {
                Some((*global, segments.iter().map(|s| s.name.clone()).collect()))
            } else {
                None
            }
        }
        Expr::Member { object, field, .. } => {
            let (global, mut segs) = expr_to_path(object)?;
            segs.push(field.clone());
            Some((global, segs))
        }
        _ => None,
    }
}
