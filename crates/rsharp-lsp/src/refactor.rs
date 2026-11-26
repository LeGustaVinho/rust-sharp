use crate::{
    analysis::DeclKind,
    handlers::{DocumentState, ServerState},
    navigation::{Found, RESOLUTION_DEPTH},
    workspace::file_path,
};
use lsp_types::{
    DocumentChanges, Location, OneOf, OptionalVersionedTextDocumentIdentifier,
    PrepareRenameResponse, Range, ReferenceParams, RenameParams, TextDocumentEdit,
    TextDocumentPositionParams, TextEdit, WorkspaceEdit,
};
use rsharp_core::{lexer, token::TokenKind};

impl DocumentState {
    pub(crate) fn edits(&self, edits: Vec<TextEdit>) -> TextDocumentEdit {
        TextDocumentEdit {
            text_document: OptionalVersionedTextDocumentIdentifier {
                uri: self.uri.clone(),
                version: self.version,
            },
            edits: edits.into_iter().map(OneOf::Left).collect(),
        }
    }
}

pub(crate) fn workspace_edit(documents: Vec<TextDocumentEdit>) -> WorkspaceEdit {
    WorkspaceEdit {
        document_changes: Some(DocumentChanges::Edits(documents)),
        ..Default::default()
    }
}

impl ServerState {
    fn usages<'a>(
        &'a self,
        target: Found<'a>,
        include_decl: bool,
        rename: bool,
    ) -> Vec<(&'a DocumentState, Vec<Range>)> {
        let project = || {
            self.docs
                .values()
                .filter(|d| d.fresh && d.module.root == target.doc.module.root)
        };
        let mut spellings = std::collections::HashSet::from([target.sym.name.as_str()]);
        if !rename {
            spellings.extend(
                project()
                    .flat_map(|d| d.index.imports.iter())
                    .filter_map(|i| i.alias.as_ref().map(|alias| alias.0.as_str())),
            );
        }
        let mut found = Vec::new();
        for doc in project() {
            let mut ranges = Vec::new();
            for tokens in std::iter::once(&doc.tokens).chain(&doc.expressions) {
                for (i, t) in tokens.iter().enumerate() {
                    let TokenKind::Ident { name, .. } = &t.kind else {
                        continue;
                    };
                    if !spellings.contains(name.as_str()) {
                        continue;
                    }
                    if !include_decl
                        && doc.uri == target.doc.uri
                        && t.span.start == target.sym.selection_span.start
                    {
                        continue;
                    }
                    if self
                        .resolve_token(doc, tokens, i, RESOLUTION_DEPTH)
                        .is_some_and(|s| s.same(target) && !(rename && s.aliased))
                    {
                        ranges.push(doc.lines.span_to_range(&doc.text, t.span));
                    }
                }
            }
            if !ranges.is_empty() {
                found.push((doc, ranges));
            }
        }
        found.sort_by(|a, b| a.0.uri.cmp(&b.0.uri));
        found
    }

    pub(crate) fn references(&self, params: ReferenceParams) -> Option<Vec<Location>> {
        let target = self.at(&params.text_document_position)?;
        Some(
            self.usages(target, params.context.include_declaration, false)
                .into_iter()
                .flat_map(|(d, ranges)| ranges.into_iter().map(|r| Location::new(d.uri.clone(), r)))
                .collect(),
        )
    }

    pub(crate) fn prepare_rename(
        &self,
        p: TextDocumentPositionParams,
    ) -> Option<PrepareRenameResponse> {
        let found = self.at(&p)?;
        if found.aliased || matches!(found.sym.kind, DeclKind::Namespace | DeclKind::Impl) {
            return None;
        }
        let doc = self.document(&p.text_document.uri)?;
        let (tokens, i) = doc.token_at(p.position)?;
        let t = &tokens[i];
        if !matches!(t.kind, TokenKind::Ident { .. }) {
            return None;
        }
        Some(PrepareRenameResponse::Range(
            doc.lines.span_to_range(&doc.text, t.span),
        ))
    }

    pub(crate) fn rename(&self, params: RenameParams) -> Result<WorkspaceEdit, String> {
        let p = &params.text_document_position;
        self.prepare_rename(p.clone())
            .ok_or("Select a symbol declaration or a reference to its original name.")?;
        let target = self.at(p).unwrap();
        let tokens =
            lexer::tokenize(&params.new_name).map_err(|_| "Enter a valid R# identifier.")?;
        let name = match &tokens[..] {
            [id, _] if id.span.start == 0 && id.span.end == params.new_name.len() => match &id.kind
            {
                TokenKind::Ident { name, .. } if name != "_" => name,
                _ => return Err("Enter a valid R# identifier.".into()),
            },
            _ => return Err("Enter a single R# identifier.".into()),
        };
        if target.doc.index.symbols.iter().any(|s| {
            s.name == *name
                && s.scope_start == target.sym.scope_start
                && s.scope_end == target.sym.scope_end
                && s.selection_span != target.sym.selection_span
        }) {
            return Err(format!("'{name}' is already declared in this scope."));
        }
        if self.docs.values().any(|d| {
            d.module.root == target.doc.module.root && !d.fresh && d.text.contains(&target.sym.name)
        }) {
            return Err("Fix the lexical error in the project before renaming this symbol.".into());
        }
        let mut documents = Vec::new();
        for (doc, ranges) in self.usages(target, true, true) {
            if doc.version.is_none()
                && file_path(&doc.uri)
                    .and_then(|p| std::fs::read_to_string(p).ok())
                    .as_deref()
                    != Some(&doc.text)
            {
                return Err(
                    "An R# file changed on disk. Retry after the workspace refreshes.".into(),
                );
            }
            let mut edits = Vec::new();
            for range in ranges {
                let (tokens, i) = doc.token_at(range.start).unwrap();
                let t = &tokens[i];
                let member_declaration = doc.uri == target.doc.uri
                    && t.span == target.sym.selection_span
                    && matches!(
                        target.sym.kind,
                        DeclKind::EnumMember | DeclKind::Field | DeclKind::Method
                    );
                if !member_declaration
                    && i.checked_sub(1).is_none_or(|j| {
                        !matches!(tokens[j].kind, TokenKind::Dot | TokenKind::ColonColon)
                    })
                    && self
                        .names(
                            doc,
                            t.span.start,
                            std::slice::from_ref(name),
                            RESOLUTION_DEPTH,
                        )
                        .is_some_and(|other| !other.same(target))
                {
                    return Err(format!(
                        "Renaming to '{name}' would conflict with another symbol."
                    ));
                }
                edits.push(TextEdit::new(range, params.new_name.clone()));
            }
            documents.push(doc.edits(edits));
        }
        Ok(workspace_edit(documents))
    }
}
