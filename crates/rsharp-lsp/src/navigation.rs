use crate::{
    analysis::{DeclKind as Kind, DeclSymbol},
    handlers::{DocumentState, ServerState},
    imports::segment,
};
use lsp_types::{Location, Position, TextDocumentPositionParams};
use rsharp_core::token::{Token, TokenKind as K};

// Stop recursive type aliases and self-referential initializers in incomplete editor buffers.
pub(crate) const RESOLUTION_DEPTH: usize = 32;

#[derive(Clone, Copy)]
pub(crate) struct Found<'a> {
    pub doc: &'a DocumentState,
    pub sym: &'a DeclSymbol,
    pub aliased: bool,
}

impl<'a> Found<'a> {
    fn new(doc: &'a DocumentState, sym: &'a DeclSymbol) -> Self {
        Self {
            doc,
            sym,
            aliased: false,
        }
    }
    pub fn location(self) -> Location {
        Location::new(
            self.doc.uri.clone(),
            self.doc
                .lines
                .span_to_range(&self.doc.text, self.sym.selection_span),
        )
    }
    pub fn same(self, other: Self) -> bool {
        self.doc.uri == other.doc.uri
            && self.sym.selection_span.start == other.sym.selection_span.start
    }
}

pub(crate) fn is_type(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::Enum
            | Kind::Struct
            | Kind::Union
            | Kind::Trait
            | Kind::TypeAlias
            | Kind::TypeParameter
    )
}

impl DocumentState {
    pub(crate) fn stream_at(&self, offset: usize) -> &[Token] {
        self.expressions
            .iter()
            .filter(|tokens| {
                tokens.first().is_some_and(|t| t.span.start <= offset)
                    && tokens.last().is_some_and(|t| offset <= t.span.end)
            })
            .min_by_key(|tokens| tokens.last().unwrap().span.end - tokens[0].span.start)
            .map_or(self.tokens.as_slice(), Vec::as_slice)
    }

    pub(crate) fn token_at(&self, pos: Position) -> Option<(&[Token], usize)> {
        if !self.fresh {
            return None;
        }
        let offset = self.lines.position_to_offset(&self.text, pos);
        let tokens = self.stream_at(offset);
        let i = tokens.partition_point(|t| t.span.end <= offset);
        if let Some(t) = tokens.get(i)
            && t.span.start <= offset
            && segment(t).is_some()
        {
            return Some((tokens, i));
        }
        let prev = i.checked_sub(1)?;
        (tokens[prev].span.end == offset && segment(&tokens[prev]).is_some())
            .then_some((tokens, prev))
    }
    pub(crate) fn token_start(&self, offset: usize) -> usize {
        self.tokens.partition_point(|t| t.span.start < offset)
    }
}

impl ServerState {
    fn lookup<'a>(
        &'a self,
        doc: &DocumentState,
        path: &[String],
        visiting: &mut Vec<(lsp_types::Uri, usize)>,
    ) -> Option<Found<'a>> {
        let root = &doc.module.root;
        if let Some(hits) = self.exports.get(&(root.clone(), path.to_vec())) {
            let [(uri, index)] = hits.as_slice() else {
                return None;
            };
            let defining = &self.docs[uri];
            return Some(Found::new(defining, &defining.index.symbols[*index]));
        }
        if self.reexports.is_empty() {
            return None;
        }
        for end in (1..=path.len()).rev() {
            let mut prefix = path[..end].to_vec();
            for wildcard in [false, true] {
                if wildcard {
                    prefix[end - 1] = "*".into();
                }
                let Some(imports) = self.reexports.get(&(root.clone(), prefix.clone())) else {
                    continue;
                };
                let mut resolved = None;
                for entry in imports {
                    if visiting.contains(entry) {
                        continue;
                    }
                    let (uri, i) = entry;
                    let defining = &self.docs[uri];
                    let import = &defining.index.imports[*i];
                    let mut next = import.path.clone();
                    let suffix = &path[if wildcard { end - 1 } else { end }..];
                    next.extend_from_slice(suffix);
                    visiting.push(entry.clone());
                    let result = self.relative_lookup(
                        defining,
                        &defining.namespace(*import.scope.start()),
                        &next,
                        visiting,
                    );
                    visiting.pop();
                    if let Some(mut hit) = result {
                        hit.aliased |= import.alias.is_some() && suffix.is_empty();
                        if resolved.is_some_and(|other| !hit.same(other)) {
                            return None;
                        }
                        resolved = Some(hit);
                    }
                }
                if resolved.is_some() {
                    return resolved;
                }
            }
        }
        None
    }

    fn path_from<'a>(
        &'a self,
        doc: &DocumentState,
        ns: &[String],
        path: &[String],
    ) -> Option<Found<'a>> {
        self.relative_lookup(doc, ns, path, &mut Vec::new())
    }

    fn relative_lookup<'a>(
        &'a self,
        doc: &DocumentState,
        ns: &[String],
        path: &[String],
        visiting: &mut Vec<(lsp_types::Uri, usize)>,
    ) -> Option<Found<'a>> {
        let mut base = ns.to_vec();
        let mut rest = path;
        while let Some((first, tail)) = rest.split_first() {
            match first.as_str() {
                "crate" | "global" => base.clear(),
                "this" | "self" => {}
                "super" => {
                    base.pop()?;
                }
                _ => break,
            }
            rest = tail;
        }
        base.extend_from_slice(rest);
        self.lookup(doc, &base, visiting).or_else(|| {
            if rest.len() == path.len() && !ns.is_empty() {
                self.lookup(doc, path, visiting)
            } else {
                None
            }
        })
    }

    pub(crate) fn names<'a>(
        &'a self,
        doc: &'a DocumentState,
        offset: usize,
        path: &[String],
        fuel: usize,
    ) -> Option<Found<'a>> {
        let (first, tail) = path.split_first()?;
        if fuel == 0 {
            return None;
        }
        let ns = doc.namespace(offset);
        if matches!(first.as_str(), "Self" | "this")
            && let Some(owner) = doc.index.enclosing_owner(offset)
        {
            let mut names = split_path(owner);
            names.extend_from_slice(tail);
            return self.names(doc, offset, &names, fuel - 1);
        }
        let matching = || {
            doc.index.imports.iter().filter(|imp| {
                imp.scope.contains(&offset) && !imp.wildcard && imp.name() == Some(first.as_str())
            })
        };
        let nearest = matching()
            .map(|imp| imp.scope.end() - imp.scope.start())
            .min();
        if let Some(sym) = doc.index.resolve_symbol(first, offset, None)
            && nearest.is_none_or(|span| sym.scope_end - sym.scope_start <= span)
        {
            let sym = Found::new(doc, sym);
            if tail.is_empty() {
                return Some(sym);
            }
            if sym.sym.kind != Kind::Namespace {
                return self.follow(sym, tail, fuel - 1);
            }
        }
        let mut imported = None;
        for import in matching().filter(|imp| Some(imp.scope.end() - imp.scope.start()) == nearest)
        {
            let mut full = import.path.clone();
            full.extend_from_slice(tail);
            let found = self.path_from(doc, &ns, &full).or_else(|| {
                self.follow(
                    self.path_from(doc, &doc.namespace(*import.scope.start()), &import.path)?,
                    tail,
                    fuel - 1,
                )
            });
            if let Some(mut f) = found {
                f.aliased |= tail.is_empty() && import.alias.is_some();
                if imported.is_some_and(|other| !f.same(other)) {
                    return None;
                }
                imported = Some(f);
            }
        }
        if nearest.is_some() {
            return imported;
        }
        if let Some(sym) = self.path_from(doc, &ns, path) {
            return Some(sym);
        }
        for import in doc
            .index
            .imports
            .iter()
            .filter(|imp| imp.scope.contains(&offset) && imp.wildcard)
        {
            let mut full = import.path.clone();
            full.extend_from_slice(path);
            if let Some(f) = self.path_from(doc, &doc.namespace(*import.scope.start()), &full) {
                if imported.is_some_and(|other| !f.same(other)) {
                    return None;
                }
                imported = Some(f);
            }
        }
        if imported.is_some() {
            return imported;
        }
        // Standalone R# files can refer to a unique top-level name without importing a Cargo module.
        let mut hits = self
            .docs
            .values()
            .filter(|d| d.module.root == doc.module.root && d.fresh)
            .flat_map(|d| {
                d.index
                    .outline
                    .iter()
                    .filter(|s| s.name == *first)
                    .map(move |s| Found::new(d, s))
            });
        let first = hits.next()?;
        if hits.next().is_some() {
            return None;
        }
        self.follow(first, tail, fuel - 1)
    }

    fn follow<'a>(
        &'a self,
        mut found: Found<'a>,
        parts: &[String],
        fuel: usize,
    ) -> Option<Found<'a>> {
        for name in parts {
            let owner = self.type_of(found, fuel)?;
            found = if let Some(child) = owner.sym.children.iter().find(|s| s.name == *name) {
                Found::new(owner.doc, child)
            } else {
                self.members(owner)
                    .into_iter()
                    .find(|s| s.sym.name == *name)?
            };
        }
        Some(found)
    }

    pub(crate) fn members<'a>(&'a self, owner: Found<'a>) -> Vec<Found<'a>> {
        let mut members: Vec<_> = owner
            .sym
            .children
            .iter()
            .filter(|s| s.kind != Kind::TypeParameter)
            .map(|s| Found::new(owner.doc, s))
            .collect();
        // An impl can live in a module that imports the type under another name.
        for d in self
            .docs
            .values()
            .filter(|d| d.fresh && d.module.root == owner.doc.module.root)
        {
            for block in d.index.symbols.iter().filter(|s| s.kind == Kind::Impl) {
                if let Some(name) = &block.owner
                    && self
                        .names(d, block.selection_span.start, &split_path(name), 1)
                        .is_some_and(|s| s.same(owner))
                {
                    members.extend(block.children.iter().map(|s| Found::new(d, s)));
                }
            }
        }
        members
    }

    pub(crate) fn expr_type<'a>(
        &'a self,
        doc: &'a DocumentState,
        pos: usize,
        expr: &rsharp_core::ast::Expr,
        fuel: usize,
    ) -> Option<Found<'a>> {
        use rsharp_core::ast::Expr;
        if fuel == 0 {
            return None;
        }
        fn path(expr: &Expr) -> Option<Vec<String>> {
            match expr {
                Expr::Ident(name) => Some(vec![name.clone()]),
                Expr::SelfType => Some(vec!["Self".into()]),
                Expr::This => Some(vec!["this".into()]),
                Expr::Path { segments, .. } => {
                    Some(segments.iter().map(|s| s.name.clone()).collect())
                }
                Expr::Member { object, field, .. } => {
                    let mut p = path(object)?;
                    p.push(field.clone());
                    Some(p)
                }
                _ => None,
            }
        }
        let found = match expr {
            Expr::Call { callee, .. }
            | Expr::Group(callee)
            | Expr::Read(callee)
            | Expr::Write(callee)
            | Expr::Take(callee)
            | Expr::Deref(callee)
            | Expr::Await(callee) => return self.expr_type(doc, pos, callee, fuel - 1),
            Expr::StructLit { path, .. } => self.names(
                doc,
                pos,
                &path
                    .segments
                    .iter()
                    .map(|s| s.name.clone())
                    .collect::<Vec<_>>(),
                fuel - 1,
            )?,
            Expr::Member { object, field, .. } if path(object).is_none() => self.follow(
                self.expr_type(doc, pos, object, fuel - 1)?,
                std::slice::from_ref(field),
                fuel - 1,
            )?,
            _ => self.names(doc, pos, &path(expr)?, fuel - 1)?,
        };
        self.type_of(found, fuel - 1)
    }

    pub(crate) fn type_of<'a>(&'a self, found: Found<'a>, fuel: usize) -> Option<Found<'a>> {
        if fuel == 0 {
            return None;
        }
        let Found { doc, sym, .. } = found;
        if is_type(sym.kind) && sym.kind != Kind::TypeAlias {
            return Some(found);
        }
        if sym.kind == Kind::EnumMember {
            return doc
                .index
                .symbols
                .iter()
                .find(|s| s.kind == Kind::Enum && s.full_span.start == sym.scope_start)
                .map(|s| Found::new(doc, s));
        }
        if let Some(ty) = &sym.ty_name {
            let target = self.names(doc, sym.selection_span.start, &split_path(ty), fuel - 1)?;
            if target.sym.kind == Kind::TypeAlias {
                return self.type_of(target, fuel - 1);
            }
            return is_type(target.sym.kind).then_some(target);
        }
        if sym.kind == Kind::Variable {
            let i = doc.token_start(sym.selection_span.start) + 1;
            if doc.tokens.get(i)?.kind == K::Eq {
                let mut depth = 0i32;
                let end = doc.tokens[i + 1..]
                    .iter()
                    .find(|t| {
                        match t.kind {
                            K::LBrace | K::LParen | K::LBracket => depth += 1,
                            K::RBrace | K::RParen | K::RBracket => depth -= 1,
                            _ => {}
                        }
                        depth < 0 || depth == 0 && t.kind == K::Semicolon
                    })?
                    .span
                    .start;
                let expr =
                    rsharp_core::parser::parse_expr_str(&doc.text[doc.tokens[i].span.end..end])
                        .ok()?;
                return self.expr_type(doc, sym.selection_span.start, &expr, fuel - 1);
            }
        }
        None
    }

    pub(crate) fn resolve_token<'a>(
        &'a self,
        doc: &'a DocumentState,
        tokens: &[Token],
        i: usize,
        fuel: usize,
    ) -> Option<Found<'a>> {
        if fuel == 0 || !doc.fresh {
            return None;
        }
        let t = tokens.get(i)?;
        segment(t)?;
        if let Some(sym) = doc.index.find_declaration_at(t.span.start) {
            return Some(Found::new(doc, sym));
        }
        for import in &doc.index.imports {
            if let Some((_, path)) = import.segments.iter().find(|(pos, _)| *pos == t.span.start) {
                return self.path_from(doc, &doc.namespace(t.span.start), path);
            }
            if import.alias.as_ref().is_some_and(|a| a.1 == t.span.start) {
                let mut target = self.path_from(doc, &doc.namespace(t.span.start), &import.path)?;
                target.aliased = true;
                return Some(target);
            }
        }
        let mut start = i;
        while start >= 2 && separator(&tokens[start - 1]) && segment(&tokens[start - 2]).is_some() {
            start -= 2;
        }
        let path: Vec<_> = (start..=i)
            .step_by(2)
            .map(|n| segment(&tokens[n]).unwrap().to_owned())
            .collect();
        if start >= 2 && separator(&tokens[start - 1]) {
            let recv = self.before_dot(doc, tokens, start - 1, fuel - 1)?;
            return self.follow(recv, &path, fuel - 1);
        }

        self.names(doc, t.span.start, &path, fuel)
    }

    pub(crate) fn before_dot<'a>(
        &'a self,
        doc: &'a DocumentState,
        tokens: &[Token],
        dot: usize,
        fuel: usize,
    ) -> Option<Found<'a>> {
        let mut i = dot.checked_sub(1)?;
        if tokens[i].kind == K::RParen {
            let mut depth = 1;
            while depth > 0 {
                i = i.checked_sub(1)?;
                match tokens[i].kind {
                    K::LParen => depth -= 1,
                    K::RParen => depth += 1,
                    _ => {}
                }
            }
            i = i.checked_sub(1)?;
        }
        if matches!(tokens[i].kind, K::Gt | K::Shr) {
            let mut depth = 0i32;
            loop {
                match tokens[i].kind {
                    K::Gt => depth += 1,
                    K::Shr => depth += 2,
                    K::Lt => depth -= 1,
                    _ => {}
                }
                i = i.checked_sub(1)?;
                if depth == 0 {
                    break;
                }
            }
        }
        self.resolve_token(doc, tokens, i, fuel)
    }

    pub(crate) fn at<'a>(&'a self, p: &TextDocumentPositionParams) -> Option<Found<'a>> {
        let doc = self.document(&p.text_document.uri)?;
        let (tokens, i) = doc.token_at(p.position)?;
        self.resolve_token(doc, tokens, i, RESOLUTION_DEPTH)
    }
}

pub(crate) fn split_path(path: &str) -> Vec<String> {
    path.split('.').map(str::to_owned).collect()
}
fn separator(t: &Token) -> bool {
    matches!(t.kind, K::Dot | K::ColonColon)
}
