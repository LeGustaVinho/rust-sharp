use lsp_types::{DocumentSymbol, SymbolKind};
use rsharp_core::Span;
use rsharp_core::token::{Keyword, Token, TokenKind};

use crate::position::LineIndex;

/// Kind of symbol extracted by the R# token-based document analyzer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclKind {
    Namespace,
    Struct,
    Enum,
    Union,
    Trait,
    Impl,
    Function,
    Method,
    Field,
    EnumMember,
    Constant,
    Static,
    TypeAlias,
    Macro,
    Parameter,
    Variable,
    TypeParameter,
}

impl DeclKind {
    pub fn to_lsp_symbol_kind(self) -> SymbolKind {
        match self {
            Self::Namespace => SymbolKind::NAMESPACE,
            Self::Struct => SymbolKind::STRUCT,
            Self::Enum => SymbolKind::ENUM,
            Self::Union => SymbolKind::STRUCT,
            Self::Trait => SymbolKind::INTERFACE,
            Self::Impl => SymbolKind::CLASS,
            Self::Function => SymbolKind::FUNCTION,
            Self::Method => SymbolKind::METHOD,
            Self::Field => SymbolKind::FIELD,
            Self::EnumMember => SymbolKind::ENUM_MEMBER,
            Self::Constant => SymbolKind::CONSTANT,
            Self::Static | Self::Parameter | Self::Variable => SymbolKind::VARIABLE,
            Self::TypeAlias | Self::TypeParameter => SymbolKind::TYPE_PARAMETER,
            Self::Macro => SymbolKind::FUNCTION,
        }
    }
}

/// Declaration indexed from an `.rsharp` token stream.
#[derive(Debug, Clone)]
pub struct DeclSymbol {
    pub name: String,
    pub kind: DeclKind,
    pub selection_span: Span,
    pub full_span: Span,
    pub scope_start: usize,
    pub scope_end: usize,
    pub owner: Option<String>,
    pub ty_name: Option<String>,
    pub signature: String,
    pub docs: String,
    pub is_mut: bool,
    pub is_static: bool,
    pub is_async: bool,
    pub children: Vec<DeclSymbol>,
}

/// Indexed symbols for a single R# document.
#[derive(Debug, Clone, Default)]
pub struct DocumentIndex {
    pub outline: Vec<DeclSymbol>,
    pub symbols: Vec<DeclSymbol>,
    pub(crate) imports: Vec<crate::imports::Import>,
}

impl DocumentIndex {
    pub fn build(source: &str, tokens: &[Token]) -> Self {
        let mut scanner = Scanner {
            source,
            tokens,
            pos: 0,
            all: Vec::new(),
            imports: Vec::new(),
        };
        let end = tokens.len().saturating_sub(1);
        let outline = scanner.scan_items(end, None, 0, source.len());
        Self {
            outline,
            symbols: scanner.all,
            imports: scanner.imports,
        }
    }

    #[allow(deprecated)]
    pub fn to_document_symbols(&self, source: &str, lines: &LineIndex) -> Vec<DocumentSymbol> {
        self.outline
            .iter()
            .map(|sym| to_lsp_doc_symbol(sym, source, lines))
            .collect()
    }

    pub fn find_declaration_at(&self, selection_start: usize) -> Option<&DeclSymbol> {
        self.symbols
            .iter()
            .find(|sym| sym.selection_span.start == selection_start)
    }

    pub fn enclosing_owner(&self, offset: usize) -> Option<&str> {
        self.symbols
            .iter()
            .filter(|sym| {
                matches!(
                    sym.kind,
                    DeclKind::Struct
                        | DeclKind::Enum
                        | DeclKind::Union
                        | DeclKind::Trait
                        | DeclKind::Impl
                ) && sym.full_span.start <= offset
                    && offset <= sym.full_span.end
            })
            .min_by_key(|sym| sym.full_span.end - sym.full_span.start)
            .and_then(|sym| sym.owner.as_deref().or(Some(sym.name.as_str())))
    }

    pub fn members_of(&self, type_name: &str) -> Vec<&DeclSymbol> {
        self.symbols
            .iter()
            .filter(|sym| {
                sym.owner.as_deref() == Some(type_name)
                    && matches!(
                        sym.kind,
                        DeclKind::Field
                            | DeclKind::Method
                            | DeclKind::EnumMember
                            | DeclKind::Constant
                            | DeclKind::TypeAlias
                    )
            })
            .collect()
    }

    pub fn resolve_receiver_type(&self, receiver: &str, offset: usize) -> Option<String> {
        if receiver == "this" || receiver == "Self" {
            return self.enclosing_owner(offset).map(str::to_string);
        }
        if receiver == "string" {
            return Some("string".to_string());
        }
        if self.symbols.iter().any(|sym| {
            sym.name == receiver
                && matches!(
                    sym.kind,
                    DeclKind::Struct
                        | DeclKind::Enum
                        | DeclKind::Union
                        | DeclKind::Trait
                        | DeclKind::TypeAlias
                )
        }) {
            return Some(receiver.to_string());
        }
        self.resolve_unqualified(receiver, offset)
            .and_then(|sym| sym.ty_name.clone())
    }

    pub fn resolve_symbol(
        &self,
        name: &str,
        offset: usize,
        receiver: Option<&str>,
    ) -> Option<&DeclSymbol> {
        if let Some(recv) = receiver {
            let owner_ty = self.resolve_receiver_type(recv, offset)?;
            return self
                .members_of(&owner_ty)
                .into_iter()
                .find(|sym| sym.name == name);
        }
        self.resolve_unqualified(name, offset)
    }

    fn resolve_unqualified(&self, name: &str, offset: usize) -> Option<&DeclSymbol> {
        self.symbols
            .iter()
            .filter(|sym| {
                sym.name == name
                    && sym.scope_start <= offset
                    && offset <= sym.scope_end
                    && !matches!(
                        sym.kind,
                        DeclKind::Field | DeclKind::EnumMember | DeclKind::Impl
                    )
            })
            .min_by_key(|sym| (sym.scope_end - sym.scope_start, sym.selection_span.start))
    }

    pub fn visible_symbols(&self, offset: usize) -> Vec<&DeclSymbol> {
        self.symbols
            .iter()
            .filter(|sym| {
                sym.scope_start <= offset
                    && offset <= sym.scope_end
                    && !matches!(
                        sym.kind,
                        DeclKind::Field | DeclKind::EnumMember | DeclKind::Impl
                    )
            })
            .collect()
    }
}

#[allow(deprecated)]
fn to_lsp_doc_symbol(sym: &DeclSymbol, source: &str, lines: &LineIndex) -> DocumentSymbol {
    let range = lines.span_to_range(source, sym.full_span);
    let selection_range = lines.span_to_range(source, sym.selection_span);
    let children = if sym.children.is_empty() {
        None
    } else {
        Some(
            sym.children
                .iter()
                .map(|child| to_lsp_doc_symbol(child, source, lines))
                .collect(),
        )
    };
    DocumentSymbol {
        name: sym.name.clone(),
        detail: Some(sym.signature.clone()),
        kind: sym.kind.to_lsp_symbol_kind(),
        tags: None,
        deprecated: None,
        range,
        selection_range,
        children,
    }
}

#[derive(Default)]
struct Modifiers {
    visible: bool,
    is_mut: bool,
    is_static: bool,
    is_async: bool,
    is_const: bool,
    is_extern: bool,
}

struct Scanner<'a> {
    source: &'a str,
    tokens: &'a [Token],
    pos: usize,
    all: Vec<DeclSymbol>,
    imports: Vec<crate::imports::Import>,
}

impl<'a> Scanner<'a> {
    fn record(&mut self, sym: DeclSymbol) {
        self.all.push(sym);
    }

    fn slice_text(&self, start: usize, end: usize) -> String {
        let clamped_start = start.min(self.source.len());
        let clamped_end = end.min(self.source.len()).max(clamped_start);
        self.source[clamped_start..clamped_end]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn find_matching_delim(&self, open_idx: usize, open: &TokenKind, close: &TokenKind) -> usize {
        let mut depth = 0usize;
        let mut i = open_idx;
        let last = self.tokens.len().saturating_sub(1);
        while i < last {
            let kind = &self.tokens[i].kind;
            if kind == open {
                depth += 1;
            } else if kind == close {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return i;
                }
            }
            i += 1;
        }
        last
    }

    fn find_matching_angle(&self, open_idx: usize) -> usize {
        let mut depth = 0usize;
        let mut i = open_idx;
        let last = self.tokens.len().saturating_sub(1);
        while i < last {
            match &self.tokens[i].kind {
                TokenKind::Lt => depth += 1,
                TokenKind::Gt => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return i;
                    }
                }
                TokenKind::Shr => {
                    depth = depth.saturating_sub(2);
                    if depth == 0 {
                        return i;
                    }
                }
                TokenKind::Semicolon | TokenKind::LBrace => return i,
                _ => {}
            }
            i += 1;
        }
        last
    }

    fn skip_to_semicolon(&mut self, end: usize) {
        while self.pos < end {
            if matches!(self.tokens[self.pos].kind, TokenKind::Semicolon) {
                self.pos += 1;
                return;
            }
            self.pos += 1;
        }
    }

    fn skip_attrs_and_collect_docs(&mut self, end: usize) -> String {
        let mut docs = Vec::new();
        while self.pos < end {
            match &self.tokens[self.pos].kind {
                TokenKind::DocComment(text) => {
                    docs.push(text.clone());
                    self.pos += 1;
                }
                TokenKind::InnerDocComment(_) | TokenKind::Shebang(_) => {
                    self.pos += 1;
                }
                TokenKind::LBracket => {
                    self.pos = self.find_matching_delim(
                        self.pos,
                        &TokenKind::LBracket,
                        &TokenKind::RBracket,
                    ) + 1;
                }
                _ => break,
            }
        }
        docs.join("\n")
    }

    fn scan_modifiers(&mut self, end: usize) -> Modifiers {
        let mut mods = Modifiers::default();
        while self.pos < end {
            match &self.tokens[self.pos].kind {
                TokenKind::Kw(Keyword::Public) => {
                    mods.visible = true;
                    self.pos += 1;
                    if self.pos + 1 < end
                        && matches!(self.tokens[self.pos].kind, TokenKind::LParen)
                        && matches!(
                            self.tokens[self.pos + 1].kind,
                            TokenKind::Kw(
                                Keyword::In | Keyword::Super | Keyword::Crate | Keyword::This
                            )
                        )
                    {
                        self.pos = self.find_matching_delim(
                            self.pos,
                            &TokenKind::LParen,
                            &TokenKind::RParen,
                        ) + 1;
                    }
                }
                TokenKind::Kw(Keyword::Internal | Keyword::Private | Keyword::Unsafe) => {
                    mods.visible |= self.tokens[self.pos].kind == TokenKind::Kw(Keyword::Internal);
                    self.pos += 1;
                }
                TokenKind::Kw(Keyword::Const) => {
                    mods.is_const = true;
                    self.pos += 1;
                }
                TokenKind::Kw(Keyword::Static) => {
                    mods.is_static = true;
                    self.pos += 1;
                }
                TokenKind::Kw(Keyword::Mut) => {
                    mods.is_mut = true;
                    self.pos += 1;
                }
                TokenKind::Kw(Keyword::Async) => {
                    mods.is_async = true;
                    self.pos += 1;
                }
                TokenKind::Kw(Keyword::Extern) => {
                    mods.is_extern = true;
                    self.pos += 1;
                    if self.pos < end
                        && matches!(self.tokens[self.pos].kind, TokenKind::StringLit(_))
                    {
                        self.pos += 1;
                    }
                }
                TokenKind::Ident {
                    name,
                    verbatim: false,
                } if name == "safe"
                    || (name == "auto"
                        && self.pos + 1 < end
                        && matches!(
                            self.tokens[self.pos + 1].kind,
                            TokenKind::Kw(Keyword::Trait)
                        ))
                    || (name == "default"
                        && self.pos + 1 < end
                        && matches!(
                            self.tokens[self.pos + 1].kind,
                            TokenKind::Kw(Keyword::Impl | Keyword::Unsafe)
                        )) =>
                {
                    self.pos += 1;
                }
                _ => break,
            }
        }
        mods
    }

    fn scan_items(
        &mut self,
        end: usize,
        owner: Option<&str>,
        scope_start: usize,
        scope_end: usize,
    ) -> Vec<DeclSymbol> {
        let mut items = Vec::new();
        while self.pos < end {
            let docs = self.skip_attrs_and_collect_docs(end);
            if self.pos >= end {
                break;
            }
            let item_start = self.tokens[self.pos].span;
            if matches!(
                self.tokens[self.pos].kind,
                TokenKind::RustBlock(_) | TokenKind::Semicolon
            ) {
                self.pos += 1;
                continue;
            }

            let mods = self.scan_modifiers(end);
            if self.pos >= end {
                break;
            }

            // `extern crate foo;` or `extern "C" { ... }`
            if mods.is_extern {
                if matches!(self.tokens[self.pos].kind, TokenKind::Kw(Keyword::Crate)) {
                    self.skip_to_semicolon(end);
                    continue;
                }
                if matches!(self.tokens[self.pos].kind, TokenKind::LBrace) {
                    let close =
                        self.find_matching_delim(self.pos, &TokenKind::LBrace, &TokenKind::RBrace);
                    self.pos += 1;
                    let nested = self.scan_items(close, owner, scope_start, scope_end);
                    items.extend(nested);
                    self.pos = (close + 1).min(end);
                    continue;
                }
            }

            match &self.tokens[self.pos].kind {
                TokenKind::Kw(Keyword::Using) => {
                    let start = self.pos + 1;
                    self.skip_to_semicolon(end);
                    if self.tokens[self.pos - 1].kind == TokenKind::Semicolon {
                        crate::imports::collect(
                            &self.tokens[start..self.pos - 1],
                            scope_start..=scope_end,
                            mods.visible,
                            &mut self.imports,
                        );
                    }
                }
                TokenKind::Kw(Keyword::Namespace) => {
                    if let Some(sym) =
                        self.scan_namespace(end, item_start, docs, scope_start, scope_end)
                    {
                        self.record(sym.clone());
                        items.push(sym);
                    }
                }
                TokenKind::Kw(Keyword::Struct) => {
                    if let Some(sym) = self.scan_struct_or_union(
                        end,
                        item_start,
                        docs,
                        DeclKind::Struct,
                        scope_start,
                        scope_end,
                    ) {
                        self.record(sym.clone());
                        items.push(sym);
                    }
                }
                TokenKind::Kw(Keyword::Union) => {
                    if let Some(sym) = self.scan_struct_or_union(
                        end,
                        item_start,
                        docs,
                        DeclKind::Union,
                        scope_start,
                        scope_end,
                    ) {
                        self.record(sym.clone());
                        items.push(sym);
                    }
                }
                TokenKind::Kw(Keyword::Enum) => {
                    if let Some(sym) = self.scan_enum(end, item_start, docs, scope_start, scope_end)
                    {
                        self.record(sym.clone());
                        items.push(sym);
                    }
                }
                TokenKind::Kw(Keyword::Trait) => {
                    if let Some(sym) =
                        self.scan_trait(end, item_start, docs, scope_start, scope_end)
                    {
                        self.record(sym.clone());
                        items.push(sym);
                    }
                }
                TokenKind::Kw(Keyword::Impl) => {
                    let sym = self.scan_impl(end, item_start, docs, scope_start, scope_end);
                    self.record(sym.clone());
                    items.push(sym);
                }
                TokenKind::Kw(Keyword::Alias | Keyword::Type) => {
                    if let Some(sym) =
                        self.scan_type_alias(end, item_start, docs, owner, scope_start, scope_end)
                    {
                        self.record(sym.clone());
                        items.push(sym);
                    }
                }
                TokenKind::Kw(Keyword::Macro) => {
                    if let Some(sym) =
                        self.scan_macro(end, item_start, docs, scope_start, scope_end)
                    {
                        self.record(sym.clone());
                        items.push(sym);
                    }
                }
                _ => {
                    if let Some(sym) = self.scan_fn_or_value_item(
                        end,
                        item_start,
                        docs,
                        &mods,
                        owner,
                        scope_start,
                        scope_end,
                    ) {
                        self.record(sym.clone());
                        items.push(sym);
                    }
                }
            }
        }
        items
    }

    fn scan_namespace(
        &mut self,
        end: usize,
        item_start: Span,
        docs: String,
        scope_start: usize,
        scope_end: usize,
    ) -> Option<DeclSymbol> {
        self.pos += 1; // 'namespace'
        let name_start = self.pos;
        let mut parts = Vec::new();
        let mut sel_span = item_start;
        while self.pos < end {
            if let TokenKind::Ident { name, .. } = &self.tokens[self.pos].kind {
                if parts.is_empty() {
                    sel_span = self.tokens[self.pos].span;
                }
                parts.push(name.clone());
                self.pos += 1;
                if self.pos < end && matches!(self.tokens[self.pos].kind, TokenKind::Dot) {
                    self.pos += 1;
                    continue;
                }
            }
            break;
        }
        if parts.is_empty() {
            return None;
        }
        let full_name = parts.join(".");
        let sig_end = self.tokens[self.pos.saturating_sub(1).max(name_start)]
            .span
            .end;
        let mut children = Vec::new();
        let full_end = if self.pos < end && matches!(self.tokens[self.pos].kind, TokenKind::LBrace)
        {
            let close = self.find_matching_delim(self.pos, &TokenKind::LBrace, &TokenKind::RBrace);
            let body_start_byte = self.tokens[self.pos].span.start;
            let body_end_byte = self.tokens[close].span.end;
            self.pos += 1;
            children = self.scan_items(close, None, body_start_byte, body_end_byte);
            self.pos = (close + 1).min(end);
            body_end_byte
        } else {
            self.skip_to_semicolon(end);
            self.tokens[self.pos.saturating_sub(1)].span.end
        };
        Some(DeclSymbol {
            name: full_name,
            kind: DeclKind::Namespace,
            selection_span: sel_span,
            full_span: Span::new(item_start.line, item_start.col, item_start.start, full_end),
            scope_start,
            scope_end,
            owner: None,
            ty_name: None,
            signature: self.slice_text(item_start.start, sig_end),
            docs,
            is_mut: false,
            is_static: false,
            is_async: false,
            children,
        })
    }

    fn scan_generic_params(&mut self, end: usize, body_scope_start: usize, body_scope_end: usize) {
        if self.pos >= end || !matches!(self.tokens[self.pos].kind, TokenKind::Lt) {
            return;
        }
        let close = self.find_matching_angle(self.pos);
        let mut i = self.pos + 1;
        while i < close {
            match &self.tokens[i].kind {
                TokenKind::Kw(Keyword::Life) => {
                    i += 2;
                }
                TokenKind::Kw(Keyword::Const) => {
                    while i < close && !matches!(self.tokens[i].kind, TokenKind::Comma) {
                        i += 1;
                    }
                }
                TokenKind::Ident { name, .. } => {
                    let span = self.tokens[i].span;
                    self.record(DeclSymbol {
                        name: name.clone(),
                        kind: DeclKind::TypeParameter,
                        selection_span: span,
                        full_span: span,
                        scope_start: body_scope_start,
                        scope_end: body_scope_end,
                        owner: None,
                        ty_name: None,
                        signature: format!("type parameter {}", name),
                        docs: String::new(),
                        is_mut: false,
                        is_static: false,
                        is_async: false,
                        children: Vec::new(),
                    });
                    while i < close && !matches!(self.tokens[i].kind, TokenKind::Comma) {
                        if matches!(self.tokens[i].kind, TokenKind::Lt) {
                            i = self.find_matching_angle(i);
                        }
                        i += 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        self.pos = (close + 1).min(end);
    }

    fn scan_struct_or_union(
        &mut self,
        end: usize,
        item_start: Span,
        docs: String,
        kind: DeclKind,
        scope_start: usize,
        scope_end: usize,
    ) -> Option<DeclSymbol> {
        self.pos += 1; // 'struct' | 'union'
        let (name, sel_span) = self.take_ident(end)?;
        let generic_pos = self.pos;
        if self.pos < end && matches!(self.tokens[self.pos].kind, TokenKind::Lt) {
            self.pos = self.find_matching_angle(self.pos) + 1;
        }
        // Skip tuple fields `struct Pair(int, int);` or `where ...` up to `{` or `;`
        while self.pos < end
            && !matches!(
                self.tokens[self.pos].kind,
                TokenKind::LBrace | TokenKind::Semicolon
            )
        {
            if matches!(self.tokens[self.pos].kind, TokenKind::LParen) {
                self.pos =
                    self.find_matching_delim(self.pos, &TokenKind::LParen, &TokenKind::RParen) + 1;
            } else {
                self.pos += 1;
            }
        }
        let header_end = self.tokens[self.pos.saturating_sub(1)].span.end;
        let mut children = Vec::new();
        let full_end = if self.pos < end && matches!(self.tokens[self.pos].kind, TokenKind::LBrace)
        {
            let close = self.find_matching_delim(self.pos, &TokenKind::LBrace, &TokenKind::RBrace);
            let body_end_byte = self.tokens[close].span.end;
            let saved = self.pos;
            self.pos = generic_pos;
            self.scan_generic_params(end, item_start.start, body_end_byte);
            self.pos = saved + 1;
            children = self.scan_struct_body(close, &name, item_start.start, body_end_byte);
            self.pos = (close + 1).min(end);
            body_end_byte
        } else {
            self.skip_to_semicolon(end);
            let end_byte = self.tokens[self.pos.saturating_sub(1)].span.end;
            let saved = self.pos;
            self.pos = generic_pos;
            self.scan_generic_params(end, item_start.start, end_byte);
            self.pos = saved;
            end_byte
        };

        Some(DeclSymbol {
            name: name.clone(),
            kind,
            selection_span: sel_span,
            full_span: Span::new(item_start.line, item_start.col, item_start.start, full_end),
            scope_start,
            scope_end,
            owner: Some(name),
            ty_name: None,
            signature: self.slice_text(item_start.start, header_end),
            docs,
            is_mut: false,
            is_static: false,
            is_async: false,
            children,
        })
    }

    fn scan_struct_body(
        &mut self,
        end: usize,
        owner: &str,
        scope_start: usize,
        scope_end: usize,
    ) -> Vec<DeclSymbol> {
        let mut children = Vec::new();
        while self.pos < end {
            let docs = self.skip_attrs_and_collect_docs(end);
            if self.pos >= end {
                break;
            }
            let member_start = self.tokens[self.pos].span;
            let mods = self.scan_modifiers(end);
            if self.pos >= end {
                break;
            }
            if self.looks_like_fn_ahead(end) {
                if let Some(sym) = self.scan_fn_or_value_item(
                    end,
                    member_start,
                    docs,
                    &mods,
                    Some(owner),
                    scope_start,
                    scope_end,
                ) {
                    self.record(sym.clone());
                    children.push(sym);
                }
            } else {
                // Field: `Type Name;` (or comma-separated)
                let ty_start = self.pos;
                let mut end_tok = self.pos;
                while end_tok < end
                    && !matches!(
                        self.tokens[end_tok].kind,
                        TokenKind::Semicolon | TokenKind::Comma
                    )
                {
                    end_tok += 1;
                }
                if end_tok > ty_start
                    && let TokenKind::Ident { name, .. } = &self.tokens[end_tok - 1].kind
                {
                    let name_span = self.tokens[end_tok - 1].span;
                    let ty_name = self.extract_base_type_name(ty_start, end_tok - 1, Some(owner));
                    let field_end = self.tokens[end_tok.min(end.saturating_sub(1))].span.end;
                    let sym = DeclSymbol {
                        name: name.clone(),
                        kind: DeclKind::Field,
                        selection_span: name_span,
                        full_span: Span::new(
                            member_start.line,
                            member_start.col,
                            member_start.start,
                            field_end,
                        ),
                        scope_start,
                        scope_end,
                        owner: Some(owner.to_string()),
                        ty_name,
                        signature: self.slice_text(member_start.start, name_span.end),
                        docs,
                        is_mut: mods.is_mut,
                        is_static: false,
                        is_async: false,
                        children: Vec::new(),
                    };
                    self.record(sym.clone());
                    children.push(sym);
                }
                self.pos = (end_tok + 1).min(end);
            }
        }
        children
    }

    fn scan_enum(
        &mut self,
        end: usize,
        item_start: Span,
        docs: String,
        scope_start: usize,
        scope_end: usize,
    ) -> Option<DeclSymbol> {
        self.pos += 1; // 'enum'
        let (name, sel_span) = self.take_ident(end)?;
        let generic_pos = self.pos;
        while self.pos < end && !matches!(self.tokens[self.pos].kind, TokenKind::LBrace) {
            self.pos += 1;
        }
        let header_end = self.tokens[self.pos.saturating_sub(1)].span.end;
        let close = self.find_matching_delim(self.pos, &TokenKind::LBrace, &TokenKind::RBrace);
        let full_end = self.tokens[close].span.end;
        let saved = self.pos;
        self.pos = generic_pos;
        self.scan_generic_params(end, item_start.start, full_end);
        self.pos = (saved + 1).min(close);

        let mut children = Vec::new();
        while self.pos < close {
            let vdocs = self.skip_attrs_and_collect_docs(close);
            if self.pos >= close {
                break;
            }
            let vstart = self.tokens[self.pos].span;
            let save_pos = self.pos;
            let mods = self.scan_modifiers(close);
            if self.pos > save_pos && self.looks_like_fn_ahead(close) {
                if let Some(method) = self.scan_fn_or_value_item(
                    close,
                    vstart,
                    vdocs,
                    &mods,
                    Some(&name),
                    item_start.start,
                    full_end,
                ) {
                    self.record(method.clone());
                    children.push(method);
                }
                continue;
            }
            self.pos = save_pos;
            if let Some((vname, vspan)) = self.take_ident(close) {
                if self.pos < close && matches!(self.tokens[self.pos].kind, TokenKind::LParen) {
                    self.pos =
                        self.find_matching_delim(self.pos, &TokenKind::LParen, &TokenKind::RParen)
                            + 1;
                } else if self.pos < close
                    && matches!(self.tokens[self.pos].kind, TokenKind::LBrace)
                {
                    self.pos =
                        self.find_matching_delim(self.pos, &TokenKind::LBrace, &TokenKind::RBrace)
                            + 1;
                }
                if self.pos < close && matches!(self.tokens[self.pos].kind, TokenKind::Eq) {
                    while self.pos < close
                        && !matches!(self.tokens[self.pos].kind, TokenKind::Comma)
                    {
                        self.pos += 1;
                    }
                }
                let vend = self.tokens[self.pos.saturating_sub(1)].span.end;
                if self.pos < close && matches!(self.tokens[self.pos].kind, TokenKind::Comma) {
                    self.pos += 1;
                }
                let sym = DeclSymbol {
                    name: vname,
                    kind: DeclKind::EnumMember,
                    selection_span: vspan,
                    full_span: Span::new(vstart.line, vstart.col, vstart.start, vend),
                    scope_start: item_start.start,
                    scope_end: full_end,
                    owner: Some(name.clone()),
                    ty_name: Some(name.clone()),
                    signature: self.slice_text(vstart.start, vend),
                    docs: vdocs,
                    is_mut: false,
                    is_static: true,
                    is_async: false,
                    children: Vec::new(),
                };
                self.record(sym.clone());
                children.push(sym);
            } else {
                self.pos += 1;
            }
        }
        self.pos = (close + 1).min(end);
        Some(DeclSymbol {
            name: name.clone(),
            kind: DeclKind::Enum,
            selection_span: sel_span,
            full_span: Span::new(item_start.line, item_start.col, item_start.start, full_end),
            scope_start,
            scope_end,
            owner: Some(name),
            ty_name: None,
            signature: self.slice_text(item_start.start, header_end),
            docs,
            is_mut: false,
            is_static: false,
            is_async: false,
            children,
        })
    }

    fn scan_trait(
        &mut self,
        end: usize,
        item_start: Span,
        docs: String,
        scope_start: usize,
        scope_end: usize,
    ) -> Option<DeclSymbol> {
        self.pos += 1; // 'trait'
        let (name, sel_span) = self.take_ident(end)?;
        let generic_pos = self.pos;
        while self.pos < end
            && !matches!(
                self.tokens[self.pos].kind,
                TokenKind::LBrace | TokenKind::Semicolon
            )
        {
            self.pos += 1;
        }
        let header_end = self.tokens[self.pos.saturating_sub(1)].span.end;
        let (full_end, children) = if self.pos < end
            && matches!(self.tokens[self.pos].kind, TokenKind::LBrace)
        {
            let close = self.find_matching_delim(self.pos, &TokenKind::LBrace, &TokenKind::RBrace);
            let full_end = self.tokens[close].span.end;
            let saved = self.pos;
            self.pos = generic_pos;
            self.scan_generic_params(end, item_start.start, full_end);
            self.pos = (saved + 1).min(close);
            let children = self.scan_items(close, Some(&name), item_start.start, full_end);
            self.pos = (close + 1).min(end);
            (full_end, children)
        } else {
            let full_end = self.tokens[self.pos.min(end.saturating_sub(1))].span.end;
            let saved = self.pos;
            self.pos = generic_pos;
            self.scan_generic_params(end, item_start.start, full_end);
            self.pos = (saved + 1).min(end);
            (full_end, Vec::new())
        };
        Some(DeclSymbol {
            name: name.clone(),
            kind: DeclKind::Trait,
            selection_span: sel_span,
            full_span: Span::new(item_start.line, item_start.col, item_start.start, full_end),
            scope_start,
            scope_end,
            owner: Some(name),
            ty_name: None,
            signature: self.slice_text(item_start.start, header_end),
            docs,
            is_mut: false,
            is_static: false,
            is_async: false,
            children,
        })
    }

    fn scan_impl(
        &mut self,
        end: usize,
        item_start: Span,
        docs: String,
        scope_start: usize,
        scope_end: usize,
    ) -> DeclSymbol {
        let impl_span = self.tokens[self.pos].span;
        self.pos += 1; // 'impl'
        let generic_pos = self.pos;
        if self.pos < end && matches!(self.tokens[self.pos].kind, TokenKind::Lt) {
            self.pos = self.find_matching_angle(self.pos) + 1;
        }
        let header_start = self.pos;
        let mut for_idx = None;
        let mut where_idx = None;
        while self.pos < end && !matches!(self.tokens[self.pos].kind, TokenKind::LBrace) {
            if matches!(self.tokens[self.pos].kind, TokenKind::Kw(Keyword::For))
                && for_idx.is_none()
            {
                for_idx = Some(self.pos);
            } else if matches!(self.tokens[self.pos].kind, TokenKind::Kw(Keyword::Where))
                && where_idx.is_none()
            {
                where_idx = Some(self.pos);
            }
            self.pos += 1;
        }
        let target_start = for_idx.map(|i| i + 1).unwrap_or(header_start);
        let target_end = where_idx.unwrap_or(self.pos);
        let target_ty = self.extract_base_type_name(target_start, target_end, None);
        let header_end = self.tokens[self.pos.saturating_sub(1)].span.end;
        let close = self.find_matching_delim(self.pos, &TokenKind::LBrace, &TokenKind::RBrace);
        let full_end = self.tokens[close].span.end;
        let saved = self.pos;
        self.pos = generic_pos;
        self.scan_generic_params(end, item_start.start, full_end);
        self.pos = (saved + 1).min(close);
        let children = self.scan_items(close, target_ty.as_deref(), item_start.start, full_end);
        self.pos = (close + 1).min(end);
        let sig = self.slice_text(item_start.start, header_end);
        DeclSymbol {
            name: sig.clone(),
            kind: DeclKind::Impl,
            selection_span: impl_span,
            full_span: Span::new(item_start.line, item_start.col, item_start.start, full_end),
            scope_start,
            scope_end,
            owner: target_ty,
            ty_name: None,
            signature: sig,
            docs,
            is_mut: false,
            is_static: false,
            is_async: false,
            children,
        }
    }

    fn scan_type_alias(
        &mut self,
        end: usize,
        item_start: Span,
        docs: String,
        owner: Option<&str>,
        scope_start: usize,
        scope_end: usize,
    ) -> Option<DeclSymbol> {
        self.pos += 1; // 'alias' | 'type'
        let (name, sel_span) = self.take_ident(end)?;
        let start = self.pos;
        self.skip_to_semicolon(end);
        let ty_name = (start..self.pos)
            .find(|i| self.tokens[*i].kind == TokenKind::Eq)
            .and_then(|i| self.extract_base_type_name(i + 1, self.pos, owner));
        let full_end = self.tokens[self.pos.saturating_sub(1)].span.end;
        Some(DeclSymbol {
            name,
            kind: DeclKind::TypeAlias,
            selection_span: sel_span,
            full_span: Span::new(item_start.line, item_start.col, item_start.start, full_end),
            scope_start,
            scope_end,
            owner: owner.map(str::to_string),
            ty_name,
            signature: self.slice_text(item_start.start, full_end),
            docs,
            is_mut: false,
            is_static: false,
            is_async: false,
            children: Vec::new(),
        })
    }

    fn scan_macro(
        &mut self,
        end: usize,
        item_start: Span,
        docs: String,
        scope_start: usize,
        scope_end: usize,
    ) -> Option<DeclSymbol> {
        self.pos += 1; // 'macro'
        let (name, sel_span) = self.take_ident(end)?;
        let sig_end = sel_span.end;
        if self.pos < end {
            match &self.tokens[self.pos].kind {
                TokenKind::RawMacroBody { delim: '{', .. } => {
                    self.pos += 1;
                }
                TokenKind::RawMacroBody { .. } => {
                    self.pos += 1;
                    self.skip_to_semicolon(end);
                }
                _ => self.skip_to_semicolon(end),
            }
        }
        let full_end = self.tokens[self.pos.saturating_sub(1)].span.end;
        Some(DeclSymbol {
            name,
            kind: DeclKind::Macro,
            selection_span: sel_span,
            full_span: Span::new(item_start.line, item_start.col, item_start.start, full_end),
            scope_start,
            scope_end,
            owner: None,
            ty_name: None,
            signature: self.slice_text(item_start.start, sig_end),
            docs,
            is_mut: false,
            is_static: false,
            is_async: false,
            children: Vec::new(),
        })
    }

    fn find_fn_lparen(&self, end: usize) -> Option<usize> {
        let mut i = self.pos;
        while i < end {
            match &self.tokens[i].kind {
                TokenKind::Semicolon | TokenKind::Eq | TokenKind::LBrace => return None,
                TokenKind::LParen => {
                    if i > self.pos
                        && matches!(
                            self.tokens[i - 1].kind,
                            TokenKind::Ident { .. } | TokenKind::Gt | TokenKind::Shr
                        )
                    {
                        return Some(i);
                    }
                    i = self.find_matching_delim(i, &TokenKind::LParen, &TokenKind::RParen);
                }
                TokenKind::LBracket => {
                    i = self.find_matching_delim(i, &TokenKind::LBracket, &TokenKind::RBracket);
                }
                TokenKind::Lt => {
                    i = self.find_matching_angle(i);
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    fn looks_like_fn_ahead(&self, end: usize) -> bool {
        self.find_fn_lparen(end).is_some()
    }

    #[allow(clippy::too_many_arguments)]
    fn scan_fn_or_value_item(
        &mut self,
        end: usize,
        item_start: Span,
        docs: String,
        mods: &Modifiers,
        owner: Option<&str>,
        scope_start: usize,
        scope_end: usize,
    ) -> Option<DeclSymbol> {
        let start_tok = self.pos;
        // Macro call at item level: `foo!(...);`
        if self.pos + 1 < end
            && matches!(self.tokens[self.pos].kind, TokenKind::Ident { .. })
            && matches!(self.tokens[self.pos + 1].kind, TokenKind::Bang)
        {
            self.pos += 2;
            if self.pos < end
                && matches!(
                    self.tokens[self.pos].kind,
                    TokenKind::RawMacroBody { delim: '{', .. }
                )
            {
                self.pos += 1;
            } else {
                self.skip_to_semicolon(end);
            }
            return None;
        }

        if let Some(lparen) = self.find_fn_lparen(end) {
            return self.scan_fn(
                lparen,
                end,
                item_start,
                docs,
                mods,
                owner,
                scope_start,
                scope_end,
            );
        }

        // `const Type Name = expr;` or `static [mut] Type Name = expr;`
        let mut eq_or_semi = self.pos;
        while eq_or_semi < end
            && !matches!(
                self.tokens[eq_or_semi].kind,
                TokenKind::Eq | TokenKind::Semicolon
            )
        {
            eq_or_semi += 1;
        }
        if (mods.is_const || mods.is_static)
            && eq_or_semi > start_tok
            && let TokenKind::Ident { name, .. } = &self.tokens[eq_or_semi - 1].kind
        {
            let name_span = self.tokens[eq_or_semi - 1].span;
            let ty_name = self.extract_base_type_name(start_tok, eq_or_semi - 1, owner);
            self.pos = eq_or_semi;
            self.skip_to_semicolon(end);
            let full_end = self.tokens[self.pos.saturating_sub(1)].span.end;
            let kind = if mods.is_const {
                DeclKind::Constant
            } else {
                DeclKind::Static
            };
            return Some(DeclSymbol {
                name: name.clone(),
                kind,
                selection_span: name_span,
                full_span: Span::new(item_start.line, item_start.col, item_start.start, full_end),
                scope_start,
                scope_end,
                owner: owner.map(str::to_string),
                ty_name,
                signature: self.slice_text(item_start.start, name_span.end),
                docs,
                is_mut: mods.is_mut,
                is_static: mods.is_static,
                is_async: false,
                children: Vec::new(),
            });
        }

        self.pos = (eq_or_semi + 1).min(end);
        None
    }

    #[allow(clippy::too_many_arguments)]
    fn scan_fn(
        &mut self,
        lparen: usize,
        end: usize,
        item_start: Span,
        docs: String,
        mods: &Modifiers,
        owner: Option<&str>,
        scope_start: usize,
        scope_end: usize,
    ) -> Option<DeclSymbol> {
        let ret_start = self.pos;

        // Locate function name immediately before optional `<generics>` and `(`.
        let mut name_idx = lparen - 1;
        let mut generic_open = None;
        if matches!(self.tokens[name_idx].kind, TokenKind::Gt | TokenKind::Shr) {
            let mut depth = 0usize;
            let mut j = name_idx;
            loop {
                match &self.tokens[j].kind {
                    TokenKind::Gt => depth += 1,
                    TokenKind::Shr => depth += 2,
                    TokenKind::Lt => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            generic_open = Some(j);
                            name_idx = j.saturating_sub(1);
                            break;
                        }
                    }
                    _ => {}
                }
                if j == ret_start {
                    break;
                }
                j -= 1;
            }
        }
        let TokenKind::Ident { name, .. } = &self.tokens[name_idx].kind else {
            self.pos = (lparen + 1).min(end);
            return None;
        };
        let name = name.clone();
        let sel_span = self.tokens[name_idx].span;
        let ret_ty = self.extract_base_type_name(ret_start, name_idx, owner);

        let rparen = self.find_matching_delim(lparen, &TokenKind::LParen, &TokenKind::RParen);
        self.pos = (rparen + 1).min(end);

        // Skip optional `where ...` clause before `{` or `;`.
        while self.pos < end
            && !matches!(
                self.tokens[self.pos].kind,
                TokenKind::LBrace | TokenKind::Semicolon
            )
        {
            self.pos += 1;
        }
        let sig_end = self.tokens[self.pos.saturating_sub(1)].span.end;
        let (body_range, full_end) = if self.pos < end
            && matches!(self.tokens[self.pos].kind, TokenKind::LBrace)
        {
            let lbrace = self.pos;
            let rbrace = self.find_matching_delim(lbrace, &TokenKind::LBrace, &TokenKind::RBrace);
            self.pos = (rbrace + 1).min(end);
            (Some((lbrace, rbrace)), self.tokens[rbrace].span.end)
        } else {
            self.skip_to_semicolon(end);
            (None, self.tokens[self.pos.saturating_sub(1)].span.end)
        };

        if let Some(gen_idx) = generic_open {
            let saved = self.pos;
            self.pos = gen_idx;
            self.scan_generic_params(lparen, item_start.start, full_end);
            self.pos = saved;
        }

        let param_scope_start = self.tokens[lparen].span.start;
        self.scan_params(lparen + 1, rparen, owner, param_scope_start, full_end);

        if let Some((lbrace, rbrace)) = body_range {
            self.scan_body_locals(lbrace, rbrace, owner);
        }

        let kind = if owner.is_some() {
            DeclKind::Method
        } else {
            DeclKind::Function
        };
        Some(DeclSymbol {
            name,
            kind,
            selection_span: sel_span,
            full_span: Span::new(item_start.line, item_start.col, item_start.start, full_end),
            scope_start,
            scope_end,
            owner: owner.map(str::to_string),
            ty_name: ret_ty,
            signature: self.slice_text(item_start.start, sig_end),
            docs,
            is_mut: false,
            is_static: mods.is_static,
            is_async: mods.is_async,
            children: Vec::new(),
        })
    }

    fn scan_params(
        &mut self,
        start: usize,
        end: usize,
        owner: Option<&str>,
        scope_start: usize,
        scope_end: usize,
    ) {
        let mut p_start = start;
        let mut i = start;
        let mut angle_depth = 0usize;
        let mut paren_depth = 0usize;
        while i <= end {
            let at_sep = i == end
                || (angle_depth == 0
                    && paren_depth == 0
                    && matches!(self.tokens[i].kind, TokenKind::Comma));
            if at_sep {
                if i > p_start {
                    self.record_single_param(p_start, i, owner, scope_start, scope_end);
                }
                p_start = i + 1;
            } else {
                match &self.tokens[i].kind {
                    TokenKind::Lt => angle_depth += 1,
                    TokenKind::Gt => angle_depth = angle_depth.saturating_sub(1),
                    TokenKind::LParen => paren_depth += 1,
                    TokenKind::RParen => paren_depth = paren_depth.saturating_sub(1),
                    _ => {}
                }
            }
            i += 1;
        }
    }

    fn record_single_param(
        &mut self,
        mut start: usize,
        end: usize,
        owner: Option<&str>,
        scope_start: usize,
        scope_end: usize,
    ) {
        while start < end && matches!(self.tokens[start].kind, TokenKind::LBracket) {
            start = self.find_matching_delim(start, &TokenKind::LBracket, &TokenKind::RBracket) + 1;
        }
        if start >= end {
            return;
        }
        let sig_start = self.tokens[start].span.start;
        let is_mut = matches!(self.tokens[start].kind, TokenKind::Kw(Keyword::Mut));
        let last = end - 1;
        match &self.tokens[last].kind {
            TokenKind::Kw(Keyword::This) => {
                let span = self.tokens[last].span;
                self.record(DeclSymbol {
                    name: "this".to_string(),
                    kind: DeclKind::Parameter,
                    selection_span: span,
                    full_span: Span::new(
                        self.tokens[start].span.line,
                        self.tokens[start].span.col,
                        sig_start,
                        span.end,
                    ),
                    scope_start,
                    scope_end,
                    owner: None,
                    ty_name: owner.map(str::to_string),
                    signature: self.slice_text(sig_start, span.end),
                    docs: String::new(),
                    is_mut,
                    is_static: false,
                    is_async: false,
                    children: Vec::new(),
                });
            }
            TokenKind::Ident { name, .. } if last > start => {
                let span = self.tokens[last].span;
                let ty_name = self.extract_base_type_name(start, last, owner);
                self.record(DeclSymbol {
                    name: name.clone(),
                    kind: DeclKind::Parameter,
                    selection_span: span,
                    full_span: Span::new(
                        self.tokens[start].span.line,
                        self.tokens[start].span.col,
                        sig_start,
                        span.end,
                    ),
                    scope_start,
                    scope_end,
                    owner: None,
                    ty_name,
                    signature: self.slice_text(sig_start, span.end),
                    docs: String::new(),
                    is_mut,
                    is_static: false,
                    is_async: false,
                    children: Vec::new(),
                });
            }
            _ => {}
        }
    }

    fn scan_body_locals(&mut self, lbrace: usize, rbrace: usize, owner: Option<&str>) {
        // Maintain stack of enclosing block end byte offsets.
        let mut blocks = vec![(self.tokens[lbrace].span.start, self.tokens[rbrace].span.end)];
        let mut i = lbrace + 1;
        let mut at_stmt_start = true;

        while i < rbrace {
            match &self.tokens[i].kind {
                TokenKind::LBrace => {
                    let close = self.find_matching_delim(i, &TokenKind::LBrace, &TokenKind::RBrace);
                    blocks.push((self.tokens[i].span.start, self.tokens[close].span.end));
                    at_stmt_start = true;
                    i += 1;
                }
                TokenKind::RBrace => {
                    blocks.pop();
                    at_stmt_start = true;
                    i += 1;
                }
                TokenKind::Semicolon => {
                    at_stmt_start = true;
                    i += 1;
                }
                TokenKind::Kw(Keyword::Using) => {
                    let end = (i + 1..rbrace)
                        .find(|j| self.tokens[*j].kind == TokenKind::Semicolon)
                        .unwrap_or(rbrace);
                    let (start_pos, end_pos) = blocks.last().unwrap();
                    crate::imports::collect(
                        &self.tokens[i + 1..end],
                        *start_pos..=*end_pos,
                        false,
                        &mut self.imports,
                    );
                    i = end;
                }
                TokenKind::Kw(Keyword::Foreach) => {
                    if i + 1 < rbrace && matches!(self.tokens[i + 1].kind, TokenKind::LParen) {
                        let rp =
                            self.find_matching_delim(i + 1, &TokenKind::LParen, &TokenKind::RParen);
                        let loop_end = self.block_end_after(rp + 1, blocks.last().unwrap().1);
                        // `foreach ([mut] Type name in iter)`
                        let mut in_idx = i + 2;
                        while in_idx < rp
                            && !matches!(self.tokens[in_idx].kind, TokenKind::Kw(Keyword::In))
                        {
                            in_idx += 1;
                        }
                        if in_idx > i + 2 && in_idx < rp {
                            self.try_record_typed_binding(
                                i + 2,
                                in_idx,
                                owner,
                                self.tokens[i].span.start,
                                loop_end,
                            );
                        }
                        i = rp + 1;
                        at_stmt_start = false;
                    } else {
                        i += 1;
                    }
                }
                TokenKind::Kw(Keyword::For) => {
                    if i + 1 < rbrace && matches!(self.tokens[i + 1].kind, TokenKind::LParen) {
                        let rp =
                            self.find_matching_delim(i + 1, &TokenKind::LParen, &TokenKind::RParen);
                        let loop_end = self.block_end_after(rp + 1, blocks.last().unwrap().1);
                        let mut eq_idx = i + 2;
                        while eq_idx < rp
                            && !matches!(
                                self.tokens[eq_idx].kind,
                                TokenKind::Eq | TokenKind::Semicolon
                            )
                        {
                            eq_idx += 1;
                        }
                        if eq_idx > i + 2 && eq_idx < rp {
                            self.try_record_typed_binding(
                                i + 2,
                                eq_idx,
                                owner,
                                self.tokens[i].span.start,
                                loop_end,
                            );
                        }
                        i = rp + 1;
                        at_stmt_start = false;
                    } else {
                        i += 1;
                    }
                }
                TokenKind::Kw(Keyword::Is) => {
                    // Pattern binding inside `if (x is Some(int n))` or `while (x is Ok(read int v))`.
                    let scope_end = blocks.last().unwrap().1;
                    let mut j = i + 1;
                    while j < rbrace
                        && !matches!(
                            self.tokens[j].kind,
                            TokenKind::RParen | TokenKind::LBrace | TokenKind::Semicolon
                        )
                    {
                        if matches!(self.tokens[j].kind, TokenKind::LParen) {
                            let inner_rp =
                                self.find_matching_delim(j, &TokenKind::LParen, &TokenKind::RParen);
                            self.try_record_typed_binding(
                                j + 1,
                                inner_rp,
                                owner,
                                self.tokens[j].span.start,
                                scope_end,
                            );
                            j = inner_rp + 1;
                            continue;
                        }
                        j += 1;
                    }
                    i += 1;
                    at_stmt_start = false;
                }
                _ if at_stmt_start => {
                    let scope_end = blocks.last().unwrap().1;
                    if let Some(next_i) = self.try_scan_local_stmt(i, rbrace, owner, scope_end) {
                        i = next_i;
                        at_stmt_start = false;
                    } else {
                        at_stmt_start = false;
                        i += 1;
                    }
                }
                _ => {
                    i += 1;
                }
            }
        }
    }

    fn block_end_after(&self, idx: usize, fallback: usize) -> usize {
        if idx < self.tokens.len() && matches!(self.tokens[idx].kind, TokenKind::LBrace) {
            let close = self.find_matching_delim(idx, &TokenKind::LBrace, &TokenKind::RBrace);
            self.tokens[close].span.end
        } else {
            fallback
        }
    }

    fn try_scan_local_stmt(
        &mut self,
        start: usize,
        end: usize,
        owner: Option<&str>,
        scope_end: usize,
    ) -> Option<usize> {
        let mut cur = start;
        while cur < end
            && matches!(
                self.tokens[cur].kind,
                TokenKind::Kw(Keyword::Shadow | Keyword::Mut)
            )
        {
            cur += 1;
        }
        // Statement keywords like `if`, `return`, `while`, etc. cannot start a local decl.
        if cur >= end || is_non_type_stmt_start(&self.tokens[cur].kind) {
            return None;
        }
        // Scan until `=` or `;` at bracket depth 0.
        let mut scan = cur;
        let mut paren = 0usize;
        let mut bracket = 0usize;
        let mut angle = 0usize;
        while scan < end {
            match &self.tokens[scan].kind {
                TokenKind::LParen => paren += 1,
                TokenKind::RParen => paren = paren.saturating_sub(1),
                TokenKind::LBracket => bracket += 1,
                TokenKind::RBracket => bracket = bracket.saturating_sub(1),
                TokenKind::Lt => angle += 1,
                TokenKind::Gt => angle = angle.saturating_sub(1),
                TokenKind::Eq | TokenKind::Semicolon
                    if paren == 0 && bracket == 0 && angle == 0 =>
                {
                    break;
                }
                TokenKind::LBrace | TokenKind::RBrace if paren == 0 && bracket == 0 => {
                    return None;
                }
                _ => {}
            }
            scan += 1;
        }
        if scan > cur + 1
            && scan < end
            && self.try_record_typed_binding(
                start,
                scan,
                owner,
                self.tokens[start].span.start,
                scope_end,
            )
        {
            return Some(scan + 1);
        }
        None
    }

    fn try_record_typed_binding(
        &mut self,
        start: usize,
        end: usize,
        owner: Option<&str>,
        scope_start: usize,
        scope_end: usize,
    ) -> bool {
        if end < start + 2 {
            return false;
        }
        let last = end - 1;
        let TokenKind::Ident { name, .. } = &self.tokens[last].kind else {
            return false;
        };
        if name == "_" {
            return false;
        }
        // Ensure what precedes `name` looks like a type (not an operator like `+` or `.`).
        let prev = &self.tokens[last - 1].kind;
        if !matches!(
            prev,
            TokenKind::Ident { .. }
                | TokenKind::Gt
                | TokenKind::Shr
                | TokenKind::RBracket
                | TokenKind::RParen
                | TokenKind::Question
                | TokenKind::Kw(
                    Keyword::Void
                        | Keyword::Never
                        | Keyword::Dynamic
                        | Keyword::SelfType
                        | Keyword::Var
                )
        ) {
            return false;
        }
        let mut is_mut = false;
        let mut ty_start = start;
        while ty_start < last {
            match &self.tokens[ty_start].kind {
                TokenKind::Kw(Keyword::Shadow) => ty_start += 1,
                TokenKind::Kw(Keyword::Mut) => {
                    is_mut = true;
                    ty_start += 1;
                }
                _ => break,
            }
        }
        let ty_name = self.extract_base_type_name(ty_start, last, owner);
        let name_span = self.tokens[last].span;
        let decl_start = self.tokens[start].span;
        self.record(DeclSymbol {
            name: name.clone(),
            kind: DeclKind::Variable,
            selection_span: name_span,
            full_span: Span::new(
                decl_start.line,
                decl_start.col,
                decl_start.start,
                name_span.end,
            ),
            scope_start,
            scope_end,
            owner: None,
            ty_name,
            signature: self.slice_text(decl_start.start, name_span.end),
            docs: String::new(),
            is_mut,
            is_static: false,
            is_async: false,
            children: Vec::new(),
        });
        true
    }

    fn extract_base_type_name(
        &self,
        start: usize,
        end: usize,
        owner: Option<&str>,
    ) -> Option<String> {
        let mut i = start;
        while i < end {
            match &self.tokens[i].kind {
                TokenKind::Kw(
                    Keyword::Read
                    | Keyword::Write
                    | Keyword::Take
                    | Keyword::Borrow
                    | Keyword::Mut
                    | Keyword::Dyn
                    | Keyword::Impl
                    | Keyword::Shadow,
                ) => {
                    i += 1;
                    // Skip lifetime arg `read<a>` / `write<static>`
                    if i < end && matches!(self.tokens[i].kind, TokenKind::Lt) {
                        i = self.find_matching_angle(i) + 1;
                    }
                }
                _ => break,
            }
        }
        if i >= end {
            return None;
        }
        match &self.tokens[i].kind {
            TokenKind::Kw(Keyword::Slice) => return Some("slice".to_string()),
            TokenKind::Kw(Keyword::Ptr) => return Some("ptr".to_string()),
            TokenKind::Kw(Keyword::Dynamic) => return Some("dynamic".to_string()),
            TokenKind::Kw(Keyword::SelfType) => {
                return Some(owner.unwrap_or("Self").to_string());
            }
            _ => {}
        }
        // Keep the module path so types with the same name remain distinguishable.
        let mut parts = Vec::new();
        let mut j = i;
        while j < end {
            if let Some(part) = crate::imports::segment(&self.tokens[j]) {
                parts.push(part);
            } else if !matches!(self.tokens[j].kind, TokenKind::Dot | TokenKind::ColonColon) {
                break;
            }
            j += 1;
        }
        (!parts.is_empty()).then(|| parts.join("."))
    }

    fn take_ident(&mut self, end: usize) -> Option<(String, Span)> {
        if self.pos < end
            && let TokenKind::Ident { name, .. } = &self.tokens[self.pos].kind
        {
            let span = self.tokens[self.pos].span;
            let name = name.clone();
            self.pos += 1;
            return Some((name, span));
        }
        None
    }
}

fn is_non_type_stmt_start(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Kw(
            Keyword::If
                | Keyword::Else
                | Keyword::While
                | Keyword::For
                | Keyword::Foreach
                | Keyword::Loop
                | Keyword::Match
                | Keyword::Require
                | Keyword::Return
                | Keyword::Break
                | Keyword::Continue
                | Keyword::Unsafe
                | Keyword::Rust
                | Keyword::Using
                | Keyword::Struct
                | Keyword::Enum
                | Keyword::Union
                | Keyword::Trait
                | Keyword::Impl
                | Keyword::Const
                | Keyword::Static
                | Keyword::Macro
                | Keyword::Namespace
        )
    )
}
