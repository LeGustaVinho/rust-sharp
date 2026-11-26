use std::collections::{HashMap, HashSet};

use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionOptions, CompletionParams, CompletionResponse,
    Diagnostic, DidChangeConfigurationParams, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse,
    Hover, HoverContents, HoverParams, HoverProviderCapability, Location, MarkupContent,
    MarkupKind, OneOf, PublishDiagnosticsParams, SemanticTokens, SemanticTokensFullOptions,
    SemanticTokensOptions, SemanticTokensParams, SemanticTokensResult,
    SemanticTokensServerCapabilities, ServerCapabilities, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextDocumentSyncOptions, TextDocumentSyncSaveOptions, Uri,
};
use rsharp_core::lexer;
use rsharp_core::token::Token;
use serde_json::Value;

use crate::analysis::{DeclKind, DeclSymbol, DocumentIndex};
use crate::builtins::{BUILTINS, BuiltinKind, builtin_members_for, find_builtin};
use crate::diagnostics::{semantic_diagnostics, syntax_diagnostics};
use crate::position::{LineIndex, ident_at_position};
use crate::semantic_tokens::{compute_semantic_tokens, legend};

#[derive(Debug, Clone)]
pub struct DocumentState {
    pub text: String,
    pub lines: LineIndex,
    pub tokens: Vec<Token>,
    pub index: DocumentIndex,
}

impl DocumentState {
    fn new(text: String, prior: Option<&DocumentState>) -> Self {
        let lines = LineIndex::new(&text);
        match lexer::tokenize(&text) {
            Ok(tokens) => {
                let index = DocumentIndex::build(&text, &tokens);
                Self {
                    text,
                    lines,
                    tokens,
                    index,
                }
            }
            Err(_) => {
                let (tokens, index) = match prior {
                    Some(prev) => (prev.tokens.clone(), prev.index.clone()),
                    None => (Vec::new(), DocumentIndex::default()),
                };
                Self {
                    text,
                    lines,
                    tokens,
                    index,
                }
            }
        }
    }
}

/// Mutable state of an R# LSP server session.
#[derive(Debug)]
pub struct ServerState {
    pub docs: HashMap<Uri, DocumentState>,
    pub check_on_save: bool,
}

impl Default for ServerState {
    fn default() -> Self {
        Self {
            docs: HashMap::new(),
            check_on_save: true,
        }
    }
}

pub fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::FULL),
                save: Some(TextDocumentSyncSaveOptions::Supported(true)),
                ..TextDocumentSyncOptions::default()
            },
        )),
        document_symbol_provider: Some(OneOf::Left(true)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![".".to_string()]),
            ..CompletionOptions::default()
        }),
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                legend: legend(),
                full: Some(SemanticTokensFullOptions::Bool(true)),
                ..SemanticTokensOptions::default()
            },
        )),
        ..ServerCapabilities::default()
    }
}

impl ServerState {
    pub fn apply_settings(&mut self, value: &Value) {
        if let Some(flag) = value
            .get("rsharp")
            .and_then(|r| r.get("checkOnSave"))
            .and_then(Value::as_bool)
            .or_else(|| value.get("checkOnSave").and_then(Value::as_bool))
        {
            self.check_on_save = flag;
        }
    }

    pub fn did_open(&mut self, params: DidOpenTextDocumentParams) -> PublishDiagnosticsParams {
        let uri = params.text_document.uri;
        let doc = DocumentState::new(params.text_document.text, None);
        let diagnostics = syntax_diagnostics(&doc.text, &doc.lines);
        self.docs.insert(uri.clone(), doc);
        PublishDiagnosticsParams::new(uri, diagnostics, None)
    }

    pub fn did_change(
        &mut self,
        params: DidChangeTextDocumentParams,
    ) -> Option<PublishDiagnosticsParams> {
        let uri = params.text_document.uri;
        let text = params.content_changes.into_iter().last()?.text;
        let doc = DocumentState::new(text, self.docs.get(&uri));
        let diagnostics = syntax_diagnostics(&doc.text, &doc.lines);
        self.docs.insert(uri.clone(), doc);
        Some(PublishDiagnosticsParams::new(uri, diagnostics, None))
    }

    pub fn did_save(&self, params: DidSaveTextDocumentParams) -> Option<PublishDiagnosticsParams> {
        let uri = params.text_document.uri;
        let doc = self.docs.get(&uri)?;
        let mut diagnostics = syntax_diagnostics(&doc.text, &doc.lines);
        if diagnostics.is_empty() && self.check_on_save {
            diagnostics.extend(semantic_diagnostics(&doc.text, &doc.lines, &doc.index));
        }
        Some(PublishDiagnosticsParams::new(uri, diagnostics, None))
    }

    pub fn did_close(&mut self, params: DidCloseTextDocumentParams) -> PublishDiagnosticsParams {
        let uri = params.text_document.uri;
        self.docs.remove(&uri);
        PublishDiagnosticsParams::new(uri, Vec::<Diagnostic>::new(), None)
    }

    pub fn did_change_configuration(&mut self, params: DidChangeConfigurationParams) {
        self.apply_settings(&params.settings);
    }

    pub fn document_symbol(&self, params: DocumentSymbolParams) -> Option<DocumentSymbolResponse> {
        let doc = self.docs.get(&params.text_document.uri)?;
        let symbols = doc.index.to_document_symbols(&doc.text, &doc.lines);
        Some(DocumentSymbolResponse::Nested(symbols))
    }

    pub fn hover(&self, params: HoverParams) -> Option<Hover> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let doc = self.docs.get(uri)?;
        let cursor = ident_at_position(&doc.text, &doc.lines, pos)?;
        if cursor.name.is_empty() {
            return None;
        }
        let offset = doc.lines.position_to_offset(&doc.text, pos);

        let method_call = crate::functional_methods::call_separator(doc, &cursor);

        // 1. Resolve in current document (scoped or receiver member).
        if (method_call.is_none() || cursor.receiver.is_some())
            && let Some(sym) =
                doc.index
                    .resolve_symbol(&cursor.name, offset, cursor.receiver.as_deref())
        {
            return Some(hover_for_symbol(sym, cursor.range));
        }

        // 2. Built-in receiver member (e.g., `accounts.Add`, `text.Length`).
        if let Some(recv) = cursor.receiver.as_deref()
            && let Some(ty_name) = doc.index.resolve_receiver_type(recv, offset)
            && let Some(member) = builtin_members_for(&ty_name)
                .into_iter()
                .find(|m| m.name == cursor.name)
        {
            return Some(make_hover(
                format!("```rsharp\n{}\n```\n\n{}", member.detail, member.doc),
                cursor.range,
            ));
        }

        // 3. Cross-open-document top-level symbol lookup.
        if cursor.receiver.is_none() && method_call.is_none() {
            for (other_uri, other_doc) in &self.docs {
                if other_uri != uri
                    && let Some(sym) = other_doc.index.resolve_symbol(&cursor.name, 0, None)
                {
                    return Some(hover_for_symbol(sym, cursor.range));
                }
            }
        }

        if let Some(separator) = method_call
            && let Some(markdown) =
                crate::functional_methods::documentation(doc, &cursor.name, separator)
        {
            return Some(make_hover(markdown, cursor.range));
        }

        // 4. Built-in keyword / primitive / prelude symbol.
        let builtin = find_builtin(&cursor.name)?;
        Some(make_hover(
            format!(
                "```rsharp\n{} // {}\n```\n\n{}",
                builtin.name, builtin.detail, builtin.doc
            ),
            cursor.range,
        ))
    }

    pub fn goto_definition(&self, params: GotoDefinitionParams) -> Option<GotoDefinitionResponse> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let doc = self.docs.get(uri)?;
        let cursor = ident_at_position(&doc.text, &doc.lines, pos)?;
        if cursor.name.is_empty() {
            return None;
        }
        let offset = doc.lines.position_to_offset(&doc.text, pos);

        if let Some(sym) =
            doc.index
                .resolve_symbol(&cursor.name, offset, cursor.receiver.as_deref())
        {
            let range = doc.lines.span_to_range(&doc.text, sym.selection_span);
            return Some(GotoDefinitionResponse::Scalar(Location::new(
                uri.clone(),
                range,
            )));
        }

        // Cross-open-document lookup.
        if cursor.receiver.is_none() {
            for (other_uri, other_doc) in &self.docs {
                if other_uri != uri
                    && let Some(sym) = other_doc.index.resolve_symbol(&cursor.name, 0, None)
                {
                    let range = other_doc
                        .lines
                        .span_to_range(&other_doc.text, sym.selection_span);
                    return Some(GotoDefinitionResponse::Scalar(Location::new(
                        other_uri.clone(),
                        range,
                    )));
                }
            }
        }
        None
    }

    pub fn completion(&self, params: CompletionParams) -> Option<CompletionResponse> {
        let uri = &params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;
        let doc = self.docs.get(uri)?;
        let offset = doc.lines.position_to_offset(&doc.text, pos);
        let cursor = ident_at_position(&doc.text, &doc.lines, pos);

        let mut items = Vec::new();
        let mut seen = HashSet::new();

        if let Some(c) = &cursor
            && c.dot_triggered
        {
            if let Some(recv) = c.receiver.as_deref()
                && let Some(ty_name) = doc.index.resolve_receiver_type(recv, offset)
            {
                for member in doc.index.members_of(&ty_name) {
                    if seen.insert(member.name.clone()) {
                        items.push(symbol_to_completion(member));
                    }
                }
                for builtin_member in builtin_members_for(&ty_name) {
                    if seen.insert(builtin_member.name.to_string()) {
                        items.push(CompletionItem {
                            label: builtin_member.name.to_string(),
                            kind: Some(CompletionItemKind::METHOD),
                            detail: Some(builtin_member.detail.to_string()),
                            documentation: Some(lsp_types::Documentation::String(
                                builtin_member.doc.to_string(),
                            )),
                            ..CompletionItem::default()
                        });
                    }
                }
            }
            return Some(CompletionResponse::Array(items));
        }

        // Visible symbols from current document.
        for sym in doc.index.visible_symbols(offset) {
            if seen.insert(sym.name.clone()) {
                items.push(symbol_to_completion(sym));
            }
        }

        // Built-in keywords, primitives, and prelude items.
        for entry in BUILTINS {
            if seen.insert(entry.name.to_string()) {
                let kind = match entry.kind {
                    BuiltinKind::Keyword => CompletionItemKind::KEYWORD,
                    BuiltinKind::PrimitiveType => CompletionItemKind::KEYWORD,
                    BuiltinKind::StdType => CompletionItemKind::STRUCT,
                    BuiltinKind::Function => CompletionItemKind::FUNCTION,
                };
                items.push(CompletionItem {
                    label: entry.name.to_string(),
                    kind: Some(kind),
                    detail: Some(entry.detail.to_string()),
                    documentation: Some(lsp_types::Documentation::String(entry.doc.to_string())),
                    ..CompletionItem::default()
                });
            }
        }

        Some(CompletionResponse::Array(items))
    }

    pub fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Option<SemanticTokensResult> {
        let doc = self.docs.get(&params.text_document.uri)?;
        let data = compute_semantic_tokens(&doc.text, &doc.tokens, &doc.lines, &doc.index);
        Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data,
        }))
    }
}

fn hover_for_symbol(sym: &DeclSymbol, range: lsp_types::Range) -> Hover {
    let mut md = format!("```rsharp\n{}\n```", sym.signature);
    if !sym.docs.is_empty() {
        md.push_str("\n\n");
        md.push_str(&sym.docs);
    }
    make_hover(md, range)
}

fn make_hover(markdown: String, range: lsp_types::Range) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: markdown,
        }),
        range: Some(range),
    }
}

fn symbol_to_completion(sym: &DeclSymbol) -> CompletionItem {
    let kind = match sym.kind {
        DeclKind::Namespace => CompletionItemKind::MODULE,
        DeclKind::Struct | DeclKind::Union => CompletionItemKind::STRUCT,
        DeclKind::Enum => CompletionItemKind::ENUM,
        DeclKind::Trait => CompletionItemKind::INTERFACE,
        DeclKind::Function => CompletionItemKind::FUNCTION,
        DeclKind::Method => CompletionItemKind::METHOD,
        DeclKind::Field => CompletionItemKind::FIELD,
        DeclKind::EnumMember => CompletionItemKind::ENUM_MEMBER,
        DeclKind::Constant => CompletionItemKind::CONSTANT,
        DeclKind::TypeAlias | DeclKind::TypeParameter => CompletionItemKind::TYPE_PARAMETER,
        DeclKind::Macro => CompletionItemKind::SNIPPET,
        DeclKind::Static | DeclKind::Parameter | DeclKind::Variable | DeclKind::Impl => {
            CompletionItemKind::VARIABLE
        }
    };
    CompletionItem {
        label: sym.name.clone(),
        kind: Some(kind),
        detail: Some(sym.signature.clone()),
        documentation: if sym.docs.is_empty() {
            None
        } else {
            Some(lsp_types::Documentation::String(sym.docs.clone()))
        },
        ..CompletionItem::default()
    }
}
