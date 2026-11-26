use crate::{
    analysis::{DeclKind, DeclSymbol},
    handlers::{DocumentState, ServerState},
};
use lsp_types::{DidChangeWatchedFilesParams, DidChangeWorkspaceFoldersParams, Uri};
use rsharp_core::ra_oracle::absolute_file_uri;
use std::{
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

pub(crate) fn file_path(uri: &Uri) -> Option<PathBuf> {
    if uri.scheme()?.as_str() != "file" {
        return None;
    }
    let path = uri.path().as_estr().decode().into_string().ok()?;
    let host = uri.authority().map(|a| a.as_str()).unwrap_or("");
    if !host.is_empty() && host != "localhost" {
        return cfg!(windows).then(|| PathBuf::from(format!("//{host}{path}")));
    }
    let path = if cfg!(windows) && path.as_bytes().get(2) == Some(&b':') {
        &path[1..]
    } else {
        &path
    };
    Some(PathBuf::from(path))
}

fn disk_uri(path: &Path) -> Uri {
    Uri::from_str(&absolute_file_uri(path)).expect("percent-encoded file URI")
}

pub(crate) fn key(uri: &Uri) -> Uri {
    file_path(uri).map_or_else(|| uri.clone(), |p| disk_uri(&p))
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Module {
    pub root: PathBuf,
    pub path: Vec<String>,
}

impl Module {
    pub fn new(uri: &Uri, roots: &[PathBuf]) -> Self {
        let Some(path) = file_path(uri) else {
            return Self {
                root: uri.as_str().into(),
                path: vec![],
            };
        };
        let parent = path.parent().unwrap_or(&path);
        let folder = roots
            .iter()
            .filter(|r| path.starts_with(r))
            .max_by_key(|r| r.components().count());
        let parents = parent
            .ancestors()
            .take_while(|dir| folder.is_none_or(|r| dir.starts_with(r)));
        let src = parents
            .clone()
            .find(|p| p.file_name().is_some_and(|n| n == "src"));
        let conversion = parents.clone().find(|dir| {
            dir.file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_prefix("rsharp-"))
                .is_some_and(|id| id.len() == 6 && id.bytes().all(|b| b.is_ascii_alphanumeric()))
        });
        let base = src
            .or(conversion)
            .or(folder.map(PathBuf::as_path))
            .unwrap_or(parent);
        let root = src.and_then(Path::parent).unwrap_or(base).to_owned();
        let mut parts: Vec<String> = parent
            .strip_prefix(base)
            .unwrap_or(parent)
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        if let Some(name) = path.file_stem().and_then(|s| s.to_str())
            && !matches!(name, "mod" | "main" | "lib")
        {
            parts.push(name.to_owned());
        }
        Self { root, path: parts }
    }
}

impl DocumentState {
    pub(crate) fn namespace(&self, pos: usize) -> Vec<String> {
        let mut path = self.module.path.clone();
        let mut items = &self.index.outline;
        while let Some(ns) = items.iter().find(|s| {
            s.kind == DeclKind::Namespace && s.selection_span.end <= pos && pos < s.full_span.end
        }) {
            path.extend(ns.name.split('.').map(str::to_owned));
            items = &ns.children;
        }
        path
    }

    pub(crate) fn symbol_path(&self, s: &DeclSymbol) -> Option<Vec<String>> {
        use DeclKind::*;
        if matches!(s.kind, Impl | Parameter | Variable | TypeParameter) {
            return None;
        }
        let mut path = self.namespace(s.selection_span.start);
        if matches!(s.kind, Method | Field | EnumMember | Constant | TypeAlias)
            && let Some(owner) = &s.owner
        {
            path.extend(owner.split('.').map(str::to_owned));
        }
        path.extend(s.name.split('.').map(str::to_owned));
        Some(path)
    }
}

pub(crate) type SymbolMap = std::collections::HashMap<(PathBuf, Vec<String>), Vec<(Uri, usize)>>;

impl DocumentState {
    fn exported_imports(&self) -> impl Iterator<Item = (usize, Vec<String>)> + '_ {
        self.index
            .imports
            .iter()
            .enumerate()
            .filter(|(_, imp)| imp.exported)
            .filter_map(|(i, imp)| {
                let mut path = self.namespace(*imp.scope.start());
                path.push(if imp.wildcard { "*" } else { imp.name()? }.to_owned());
                Some((i, path))
            })
    }

    fn export_symbols(&self) -> impl Iterator<Item = (usize, Vec<String>)> + '_ {
        self.index
            .symbols
            .iter()
            .enumerate()
            .filter_map(|(i, s)| Some((i, self.symbol_path(s)?)))
    }
}

impl ServerState {
    pub(crate) fn document(&self, uri: &Uri) -> Option<&DocumentState> {
        self.docs.get(&key(uri))
    }

    pub(crate) fn remove(&mut self, key: &Uri) {
        if let Some(doc) = self.docs.remove(key) {
            let remove = |table: &mut SymbolMap, path| {
                let id = (doc.module.root.clone(), path);
                if let Some(hits) = table.get_mut(&id) {
                    hits.retain(|(uri, _)| uri != key);
                    if hits.is_empty() {
                        table.remove(&id);
                    }
                }
            };
            for (_, path) in doc.export_symbols() {
                remove(&mut self.exports, path);
            }
            for (_, path) in doc.exported_imports() {
                remove(&mut self.reexports, path);
            }
        }
    }

    pub(crate) fn put(&mut self, doc: DocumentState) {
        let key = key(&doc.uri);
        self.remove(&key);
        if doc.fresh {
            let add = |table: &mut SymbolMap, i, path| {
                table
                    .entry((doc.module.root.clone(), path))
                    .or_default()
                    .push((key.clone(), i))
            };
            for (i, path) in doc.export_symbols() {
                add(&mut self.exports, i, path);
            }
            for (i, path) in doc.exported_imports() {
                add(&mut self.reexports, i, path);
            }
        }
        self.docs.insert(key, doc);
    }

    pub(crate) fn set_folders(&mut self, roots: Vec<PathBuf>) {
        self.roots = roots;
        let cached: Vec<_> = self.docs.keys().cloned().collect();
        for id in cached {
            if self.docs[&id].version.is_none() {
                self.remove(&id);
            } else {
                let mut doc = self.docs[&id].clone();
                doc.module = Module::new(&doc.uri, &self.roots);
                self.put(doc);
            }
        }
        let mut pending = self.roots.clone();
        while let Some(dir) = pending.pop() {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let Ok(kind) = entry.file_type() else {
                        continue;
                    };
                    let path = entry.path();
                    if kind.is_dir() && !excluded(&path) {
                        pending.push(path);
                    } else if kind.is_file() && path.extension().is_some_and(|s| s == "rsharp") {
                        self.reload(&disk_uri(&path));
                    }
                }
            }
        }
    }

    pub(crate) fn reload(&mut self, uri: &Uri) {
        let Some(path) = file_path(uri) else {
            return;
        };
        if !self.roots.iter().any(|r| {
            path.strip_prefix(r)
                .is_ok_and(|rel| !rel.ancestors().any(excluded))
        }) || !path.extension().is_some_and(|s| s == "rsharp")
            || self.document(uri).is_some_and(|d| d.version.is_some())
        {
            return;
        }
        let id = key(uri);
        // A file watcher can report a symlink or a deletion after the initial scan.
        let text = path
            .canonicalize()
            .ok()
            .filter(|p| {
                self.roots
                    .iter()
                    .any(|r| r.canonicalize().is_ok_and(|base| p.starts_with(base)))
            })
            .and_then(|_| fs::read_to_string(&path).ok());
        if let Some(text) = text {
            self.put(DocumentState::new(
                uri.clone(),
                text,
                None,
                &self.roots,
                None,
            ));
        } else {
            self.remove(&id);
        }
    }

    pub fn watched_files(&mut self, params: DidChangeWatchedFilesParams) {
        for change in params.changes {
            self.reload(&change.uri);
        }
    }

    pub fn workspace_folders(&mut self, params: DidChangeWorkspaceFoldersParams) {
        let mut roots = self.roots.clone();
        for f in params.event.removed {
            if let Some(p) = file_path(&f.uri) {
                roots.retain(|r| r != &p);
            }
        }
        roots.extend(params.event.added.iter().filter_map(|f| file_path(&f.uri)));
        roots.sort();
        roots.dedup();
        self.set_folders(roots);
    }
}

fn excluded(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|s| s.to_str()),
        Some(".git" | "target" | "node_modules")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_uris_keep_identity_across_deletions_and_handle_authorities() {
        assert!(file_path(&"untitled:Document".parse().unwrap()).is_none());
        let local = file_path(&"file://localhost/a%20b%23c".parse().unwrap()).unwrap();
        assert_eq!(local, PathBuf::from("/a b#c"));
        let share = file_path(&"file://machine/share/a%20b.rsharp".parse().unwrap());
        if cfg!(windows) {
            let p = share.unwrap();
            assert_eq!(disk_uri(&p).as_str(), "file://machine/share/a%20b.rsharp");
            assert_eq!(
                disk_uri(Path::new(r"\\?\UNC\machine\share\a b.rsharp")),
                disk_uri(&p)
            );
            assert_eq!(
                key(&"file:///d%3A/no%20file.rsharp".parse().unwrap()),
                key(&"file:///D:/no%20file.rsharp".parse().unwrap())
            );
        } else {
            assert!(share.is_none());
        }
    }
}
