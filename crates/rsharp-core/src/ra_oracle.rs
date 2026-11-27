use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use serde_json::{Value, json};

use crate::error::{RSharpError, Result};

/// Per-file map of inferred Rust types indexed by `(1-based line, 0-based UTF-16 column)`
/// of the end of each binding token (matching LSP `textDocument/inlayHint` positions).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TypeHintMap {
    /// Maps `(1-based line, 0-based UTF-16 col)` to `(rust_type_str, has_verified_edit)`.
    by_end: HashMap<(u32, u32), (String, bool)>,
}

impl TypeHintMap {
    pub fn insert(&mut self, line_1based: u32, utf16_col: u32, ty: String) {
        self.by_end.insert((line_1based, utf16_col), (ty, true));
    }

    pub fn insert_with_verified(
        &mut self,
        line_1based: u32,
        utf16_col: u32,
        ty: String,
        verified_edit: bool,
    ) {
        self.by_end
            .insert((line_1based, utf16_col), (ty, verified_edit));
    }

    pub fn get(&self, line_1based: u32, utf16_col: u32) -> Option<&str> {
        self.by_end
            .get(&(line_1based, utf16_col))
            .map(|(ty, _)| ty.as_str())
    }

    pub fn get_entry(&self, line_1based: u32, utf16_col: u32) -> Option<(&str, bool)> {
        self.by_end
            .get(&(line_1based, utf16_col))
            .map(|(ty, verified)| (ty.as_str(), *verified))
    }

    pub fn is_empty(&self) -> bool {
        self.by_end.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_end.len()
    }
}

/// Finds the enclosing Cargo workspace root (preferring the outermost ancestor `Cargo.toml`
/// containing `[workspace]`, falling back to the nearest ancestor `Cargo.toml`).
pub fn find_cargo_workspace_root(path: &Path) -> Option<PathBuf> {
    let start = if path.is_file() {
        path.parent().unwrap_or(path)
    } else {
        path
    };
    let mut nearest_manifest: Option<PathBuf> = None;
    let mut workspace_root: Option<PathBuf> = None;
    for dir in start.ancestors() {
        let manifest = dir.join("Cargo.toml");
        if manifest.is_file() {
            if nearest_manifest.is_none() {
                nearest_manifest = Some(dir.to_path_buf());
            }
            if let Ok(text) = std::fs::read_to_string(&manifest)
                && text.contains("[workspace]")
            {
                workspace_root = Some(dir.to_path_buf());
            }
        }
    }
    workspace_root.or(nearest_manifest)
}

/// Converts a filesystem path into a standard `file://` URI accepted by `rust-analyzer`.
pub fn path_to_file_uri(path: &Path) -> String {
    let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    absolute_file_uri(&abs)
}

/// Encodes an absolute path, including paths for files deleted since an editor indexed them.
pub fn absolute_file_uri(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    let mut forward = stripped.replace('\\', "/");
    if cfg!(windows) {
        if let Some(unc) = forward.strip_prefix("UNC/") {
            forward = format!("//{unc}");
        }
        if forward.as_bytes().get(1) == Some(&b':') {
            forward[..1].make_ascii_uppercase();
        }
    }
    let (prefix, forward) = match forward.strip_prefix("//") {
        Some(unc) if cfg!(windows) => ("file://", unc),
        _ if forward.starts_with('/') => ("file://", forward.as_str()),
        _ => ("file:///", forward.as_str()),
    };
    let mut encoded = String::with_capacity(forward.len() + prefix.len());
    encoded.push_str(prefix);
    for b in forward.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' => {
                encoded.push(b as char);
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", b));
            }
        }
    }
    encoded
}

/// Cargo feature activation mode shared by `rust-analyzer` and `--verify-cargo` so both
/// evaluate the exact same `#[cfg(feature = "...")]` graph.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum CargoFeaturesMode {
    #[default]
    Default,
    All,
    List(Vec<String>),
}

impl CargoFeaturesMode {
    /// Parses a CLI `--cargo-features` value (`default`, `all`, or comma-separated feature names).
    pub fn parse_cli(raw: &str) -> std::result::Result<Self, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed == "default" {
            Ok(Self::Default)
        } else if trimmed == "all" {
            Ok(Self::All)
        } else {
            let list: Vec<String> = trimmed
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            if list.is_empty() {
                Err("Invalid --cargo-features value: expected 'default', 'all', or comma-separated feature names".to_string())
            } else {
                Ok(Self::List(list))
            }
        }
    }

    pub(crate) fn to_ra_json(&self) -> Option<Value> {
        match self {
            Self::Default => None,
            Self::All => Some(Value::String("all".to_string())),
            Self::List(fs) => Some(json!(fs)),
        }
    }

    /// Returns matching `cargo check` CLI flags for this feature mode.
    pub fn cargo_args(&self) -> Vec<String> {
        match self {
            Self::Default => Vec::new(),
            Self::All => vec!["--all-features".to_string()],
            Self::List(fs) => vec!["--features".to_string(), fs.join(",")],
        }
    }
}

/// Minimal stdio JSON-RPC client driving `rust-analyzer` once per Cargo workspace to collect
/// full Cargo-aware type hints (`textDocument/inlayHint`) across all workspace `.rs` files.
pub struct RustAnalyzerOracle {
    reader: Box<dyn BufRead>,
    writer: Box<dyn Write>,
    child: Option<Child>,
    next_id: i64,
    optional_deps: HashSet<String>,
    cfg_gated_mods: HashSet<String>,
    cfg_test_reexports: HashSet<String>,
    is_no_std: bool,
}

impl RustAnalyzerOracle {
    /// Spawns `rust-analyzer` (or `RUST_ANALYZER` env override) rooted at `workspace_root`
    /// and waits for initial workspace indexing (`experimental/serverStatus` quiescent).
    pub fn open(workspace_root: &Path) -> Result<Self> {
        Self::open_with_features(workspace_root, &CargoFeaturesMode::Default)
    }

    /// Spawns `rust-analyzer` rooted at `workspace_root` with explicit Cargo feature configuration.
    pub fn open_with_features(workspace_root: &Path, features: &CargoFeaturesMode) -> Result<Self> {
        let bin = std::env::var("RUST_ANALYZER").unwrap_or_else(|_| "rust-analyzer".to_string());
        Self::open_with_bin_and_features(&bin, workspace_root, features)
    }

    /// Spawns a specific `rust-analyzer` executable path for `workspace_root`.
    pub fn open_with_bin(bin: &str, workspace_root: &Path) -> Result<Self> {
        Self::open_with_bin_and_features(bin, workspace_root, &CargoFeaturesMode::Default)
    }

    /// Spawns a specific `rust-analyzer` executable path with explicit Cargo feature configuration.
    pub fn open_with_bin_and_features(
        bin: &str,
        workspace_root: &Path,
        features: &CargoFeaturesMode,
    ) -> Result<Self> {
        let mut child = Command::new(bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                RSharpError::io(format!(
                    "Failed to launch '{}' for Cargo workspace type inference: {}",
                    bin, e
                ))
            })?;
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut oracle = Self {
            reader: Box::new(BufReader::new(stdout)),
            writer: Box::new(BufWriter::new(stdin)),
            child: Some(child),
            next_id: 1,
            optional_deps: HashSet::new(),
            cfg_gated_mods: HashSet::new(),
            cfg_test_reexports: HashSet::new(),
            is_no_std: false,
        };
        oracle.initialize(workspace_root, features)?;
        Ok(oracle)
    }

    /// Creates an oracle over arbitrary stdio streams (used for deterministic unit testing).
    pub fn from_io(
        reader: impl BufRead + 'static,
        writer: impl Write + 'static,
        workspace_root: &Path,
    ) -> Result<Self> {
        Self::from_io_with_features(reader, writer, workspace_root, &CargoFeaturesMode::Default)
    }

    /// Creates an oracle over arbitrary stdio streams with explicit Cargo feature configuration.
    pub fn from_io_with_features(
        reader: impl BufRead + 'static,
        writer: impl Write + 'static,
        workspace_root: &Path,
        features: &CargoFeaturesMode,
    ) -> Result<Self> {
        let mut oracle = Self {
            reader: Box::new(reader),
            writer: Box::new(writer),
            child: None,
            next_id: 1,
            optional_deps: HashSet::new(),
            cfg_gated_mods: HashSet::new(),
            cfg_test_reexports: HashSet::new(),
            is_no_std: false,
        };
        oracle.initialize(workspace_root, features)?;
        Ok(oracle)
    }

    fn initialize(&mut self, workspace_root: &Path, features: &CargoFeaturesMode) -> Result<()> {
        if workspace_root.join("Cargo.toml").exists() {
            collect_workspace_cfg_gated_names(
                workspace_root,
                &mut self.optional_deps,
                &mut self.cfg_gated_mods,
                &mut self.cfg_test_reexports,
                &mut self.is_no_std,
            );
        }
        let root_uri = path_to_file_uri(workspace_root);
        let ws_name = workspace_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("workspace");
        let mut cargo_cfg = json!({
            "allTargets": true,
            "buildScripts": { "enable": true }
        });
        if let Some(feat_val) = features.to_ra_json() {
            cargo_cfg
                .as_object_mut()
                .unwrap()
                .insert("features".to_string(), feat_val);
        }
        let init_params = json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "workspaceFolders": [{ "uri": root_uri, "name": ws_name }],
            "capabilities": {
                "experimental": {
                    "serverStatusNotification": true
                },
                "window": {
                    "workDoneProgress": true
                },
                "textDocument": {
                    "inlayHint": {
                        // Let rust-analyzer include type edits in the initial response.
                        "dynamicRegistration": false
                    }
                }
            },
            "initializationOptions": {
                "cargo": cargo_cfg,
                "procMacro": { "enable": true },
                "checkOnSave": false,
                "diagnostics": { "enable": false },
                "inlayHints": {
                    "typeHints": {
                        "enable": true,
                        "hideNamedConstructor": false,
                        "hideClosureInitialization": false,
                        "hideClosureParameter": false
                    },
                    "closureReturnTypeHints": { "enable": "always" },
                    "chainingHints": { "enable": false },
                    "parameterHints": { "enable": false },
                    "bindingModeHints": { "enable": false },
                    "discriminantHints": { "enable": "never" },
                    "genericParameterHints": {
                        "type": { "enable": false },
                        "const": { "enable": false },
                        "lifetime": { "enable": false }
                    },
                    "lifetimeElisionHints": { "enable": "never" },
                    "maxLength": null
                }
            }
        });

        let id = self.send_request("initialize", init_params)?;
        let _ = self.read_response(id)?;
        self.send_notification("initialized", json!({}))?;
        self.wait_until_quiescent()?;
        Ok(())
    }

    fn wait_until_quiescent(&mut self) -> Result<()> {
        loop {
            let msg = read_lsp_message(&mut *self.reader)?;
            if self.try_reply_server_request(&msg)? {
                continue;
            }
            if msg.get("method").and_then(Value::as_str) == Some("experimental/serverStatus")
                && msg
                    .get("params")
                    .and_then(|p| p.get("quiescent"))
                    .and_then(Value::as_bool)
                    == Some(true)
            {
                return Ok(());
            }
        }
    }

    /// Queries `rust-analyzer` for all inferred type inlay hints in `file_path`.
    pub fn type_hints_for_file(&mut self, file_path: &Path, text: &str) -> Result<TypeHintMap> {
        // Files that swap imports/types between `#[cfg(test)]` and `#[cfg(not(test))]` (e.g.
        // mock `JoinHandle` in `tokio/src/fs/file.rs`) have incompatible types under `--lib` vs
        // `--tests`, so skip RA hints and let `var` preserve dual-target compilation.
        if text.contains("cfg(test)") && text.contains("cfg(not(test))") {
            return Ok(TypeHintMap::default());
        }
        let uri = path_to_file_uri(file_path);
        self.send_notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "rust",
                    "version": 1,
                    "text": text
                }
            }),
        )?;

        let mut end_line: u32 = 0;
        let mut last_line_start: usize = 0;
        for (idx, b) in text.bytes().enumerate() {
            if b == b'\n' {
                end_line += 1;
                last_line_start = idx + 1;
            }
        }
        let last_line = text[last_line_start..]
            .strip_suffix('\r')
            .unwrap_or(&text[last_line_start..]);
        let end_char = last_line.encode_utf16().count() as u32;
        let mut resp = self.request_with_retry(
            "textDocument/inlayHint",
            &json!({
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": end_line, "character": end_char }
                }
            }),
        )?;
        if let Some(items) = resp.as_array_mut() {
            self.resolve_hints(items);
        }

        self.send_notification(
            "textDocument/didClose",
            json!({
                "textDocument": { "uri": uri }
            }),
        )?;

        let mut hints = parse_inlay_hint_response(&resp);
        if !self.optional_deps.is_empty()
            || !self.cfg_gated_mods.is_empty()
            || !self.cfg_test_reexports.is_empty()
            || self.is_no_std
        {
            for (ty, verified) in hints.by_end.values_mut() {
                if hint_uses_cfg_gated_item(
                    ty,
                    &self.optional_deps,
                    &self.cfg_gated_mods,
                    &self.cfg_test_reexports,
                ) || (self.is_no_std && !*verified && hint_uses_unqualified_alloc_type(ty))
                {
                    *ty = "{unknown}".to_string();
                    *verified = false;
                }
            }
        }
        Ok(hints)
    }

    fn resolve_hints(&mut self, items: &mut [Value]) {
        const WINDOW: usize = 8;
        let mut pending = HashMap::with_capacity(WINDOW);
        let mut remaining = items.iter_mut();
        loop {
            while pending.len() < WINDOW {
                let Some(item) = remaining.next() else { break };
                if item.get("kind").and_then(Value::as_u64) != Some(1) || has_text_edits(item) {
                    continue;
                }
                let raw_label = extract_inlay_label(item);
                let trimmed = raw_label.trim();
                let Some(ty_str) = trimmed.strip_prefix(':').map(str::trim) else {
                    continue;
                };
                if is_trivial_primitive_type(ty_str) {
                    continue;
                }
                if item.get("data").is_none() {
                    mark_no_edit_if_empty(item);
                    continue;
                }
                match self.send_request("inlayHint/resolve", item.clone()) {
                    Ok(id) => {
                        pending.insert(id, (item, 0));
                    }
                    Err(_) => mark_no_edit_if_empty(item),
                }
            }
            if pending.is_empty() {
                return;
            }
            let Ok((id, response)) = self.next_response() else {
                for (_, (item, _)) in pending.drain() {
                    mark_no_edit_if_empty(item);
                }
                continue;
            };
            let Some((item, attempt)) = pending.remove(&id) else {
                continue;
            };
            match response {
                Ok(resolved) => *item = resolved,
                Err(err) if retry_content_modified(&err, attempt) => {
                    if let Ok(id) = self.send_request("inlayHint/resolve", item.clone()) {
                        pending.insert(id, (item, attempt + 1));
                        continue;
                    }
                }
                Err(_) => {}
            }
            mark_no_edit_if_empty(item);
        }
    }

    /// Gracefully shuts down the `rust-analyzer` server process.
    pub fn close(mut self) {
        if let Ok(id) = self.send_request("shutdown", Value::Null) {
            let _ = self.read_response(id);
        }
        let _ = self.send_notification("exit", Value::Null);
        if let Some(mut child) = self.child.take() {
            let _ = child.wait();
        }
    }

    fn send_request(&mut self, method: &str, params: Value) -> Result<i64> {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        write_lsp_message(&mut *self.writer, &msg)?;
        Ok(id)
    }

    fn send_notification(&mut self, method: &str, params: Value) -> Result<()> {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });
        write_lsp_message(&mut *self.writer, &msg)
    }

    fn try_reply_server_request(&mut self, msg: &Value) -> Result<bool> {
        if let Some(req_id) = msg.get("id")
            && msg.get("method").is_some()
        {
            let reply = json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "result": Value::Null
            });
            write_lsp_message(&mut *self.writer, &reply)?;
            return Ok(true);
        }
        Ok(false)
    }

    fn request_with_retry(&mut self, method: &str, params: &Value) -> Result<Value> {
        let mut attempt = 0usize;
        loop {
            let id = self.send_request(method, params.clone())?;
            match self.read_response_raw(id)? {
                Ok(val) => return Ok(val),
                Err(err) if retry_content_modified(&err, attempt) => {
                    attempt += 1;
                }
                Err(err) => {
                    return Err(RSharpError::from_rust(format!(
                        "rust-analyzer LSP error: {}",
                        err
                    )));
                }
            }
        }
    }

    fn read_response(&mut self, expected_id: i64) -> Result<Value> {
        match self.read_response_raw(expected_id)? {
            Ok(val) => Ok(val),
            Err(err) => Err(RSharpError::from_rust(format!(
                "rust-analyzer LSP error: {}",
                err
            ))),
        }
    }

    fn read_response_raw(&mut self, expected_id: i64) -> Result<std::result::Result<Value, Value>> {
        loop {
            let (id, response) = self.next_response()?;
            if id == expected_id {
                return Ok(response);
            }
        }
    }

    fn next_response(&mut self) -> Result<(i64, std::result::Result<Value, Value>)> {
        loop {
            let mut msg = read_lsp_message(&mut *self.reader)?;
            if self.try_reply_server_request(&msg)? {
                continue;
            }
            if let Some(id) = msg.get("id").and_then(Value::as_i64) {
                let result = match msg.get_mut("error") {
                    Some(err) => Err(err.take()),
                    None => Ok(msg["result"].take()),
                };
                return Ok((id, result));
            }
        }
    }
}

fn retry_content_modified(error: &Value, attempt: usize) -> bool {
    if attempt >= 5 || error.get("code").and_then(Value::as_i64) != Some(-32801) {
        return false;
    }
    std::thread::sleep(std::time::Duration::from_millis(20 * (attempt + 1) as u64));
    true
}

impl Drop for RustAnalyzerOracle {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn write_lsp_message(writer: &mut dyn Write, msg: &Value) -> Result<()> {
    let body = msg.to_string();
    write!(writer, "Content-Length: {}\r\n\r\n{}", body.len(), body)
        .and_then(|()| writer.flush())
        .map_err(|e| {
            RSharpError::io(format!(
                "Failed writing LSP message to rust-analyzer: {}",
                e
            ))
        })
}

fn read_lsp_message(reader: &mut dyn BufRead) -> Result<Value> {
    let mut content_len: Option<usize> = None;
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .map_err(|e| RSharpError::io(format!("Failed reading rust-analyzer header: {}", e)))?;
        if n == 0 {
            return Err(RSharpError::io(
                "rust-analyzer exited unexpectedly before responding",
            ));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some(rest) = trimmed
            .strip_prefix("Content-Length:")
            .or_else(|| trimmed.strip_prefix("content-length:"))
        {
            content_len = rest.trim().parse::<usize>().ok();
        }
    }
    let len = content_len
        .ok_or_else(|| RSharpError::io("Missing Content-Length header from rust-analyzer"))?;
    let mut buf = vec![0u8; len];
    reader
        .read_exact(&mut buf)
        .map_err(|e| RSharpError::io(format!("Failed reading rust-analyzer body: {}", e)))?;
    serde_json::from_slice(&buf)
        .map_err(|e| RSharpError::io(format!("Invalid JSON from rust-analyzer: {}", e)))
}

fn has_text_edits(item: &Value) -> bool {
    item.get("textEdits")
        .and_then(Value::as_array)
        .is_some_and(|edits| !edits.is_empty())
}

fn mark_no_edit_if_empty(item: &mut Value) {
    if !has_text_edits(item)
        && let Some(obj) = item.as_object_mut()
    {
        obj.insert("_rsharp_no_edit".to_string(), Value::Bool(true));
    }
}

fn extract_inlay_label(item: &Value) -> String {
    match item.get("label") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("value").and_then(Value::as_str))
            .collect::<String>(),
        _ => String::new(),
    }
}

fn is_trivial_primitive_type(ty: &str) -> bool {
    matches!(
        ty,
        "bool"
            | "char"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "f32"
            | "f64"
            | "&str"
            | "&mut str"
            | "()"
            | "!"
    )
}

/// Extracts `InlayHintKind::TYPE` (`kind == 1`) labels from a `textDocument/inlayHint` result,
/// preferring resolved `textEdits` (which carry fully-qualified, import-aware paths) when present.
pub fn parse_inlay_hint_response(result: &Value) -> TypeHintMap {
    let mut map = TypeHintMap::default();
    let Some(items) = result.as_array() else {
        return map;
    };
    for item in items {
        if item.get("kind").and_then(Value::as_u64) != Some(1) {
            continue;
        }
        let Some(pos) = item.get("position") else {
            continue;
        };
        let Some(line0) = pos.get("line").and_then(Value::as_u64) else {
            continue;
        };
        let Some(col0) = pos.get("character").and_then(Value::as_u64) else {
            continue;
        };
        let edit_ty = item
            .get("textEdits")
            .and_then(Value::as_array)
            .and_then(|edits| {
                let mut found = None;
                for e in edits {
                    let nt = e.get("newText").and_then(Value::as_str)?.trim();
                    if let Some(rest) = nt.strip_prefix(':') {
                        found = Some(rest.trim().to_string());
                    } else if !nt.is_empty() {
                        // Extra edit (e.g. auto-import `use ...;`) means type is not standalone.
                        return None;
                    }
                }
                found
            });
        let (cleaned, verified) = if let Some(ty) = edit_ty {
            (ty, true)
        } else {
            let label = extract_inlay_label(item);
            let trimmed = label.trim();
            if trimmed.starts_with('=') {
                continue;
            }
            let ty = trimmed
                .strip_prefix(':')
                .unwrap_or(trimmed)
                .trim()
                .to_string();
            let no_edit = item
                .get("_rsharp_no_edit")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            (ty, !no_edit)
        };
        if !cleaned.is_empty() {
            map.insert_with_verified(line0 as u32 + 1, col0 as u32, cleaned, verified);
        }
    }
    map
}

/// Discovers compiled Rust source files in a Cargo project/workspace via `cargo metadata`,
/// avoiding non-member fixture and template `.rs` files. Returns `None` if `project_dir` is
/// not a Cargo project or `cargo metadata` fails.
pub fn discover_cargo_rs_files(project_dir: &Path) -> Option<Vec<PathBuf>> {
    discover_cargo_rs_files_with_bin(project_dir, "cargo")
}

/// Discovers compiled Rust source files using an explicit `cargo` binary path (used by tests).
pub fn discover_cargo_rs_files_with_bin(
    project_dir: &Path,
    cargo_bin: &str,
) -> Option<Vec<PathBuf>> {
    let manifest = project_dir.join("Cargo.toml");
    let manifest = if manifest.is_file() {
        manifest
    } else {
        find_cargo_workspace_root(project_dir)?.join("Cargo.toml")
    };
    let output = Command::new(cargo_bin)
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(&manifest)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let json = String::from_utf8_lossy(&output.stdout);
    parse_cargo_metadata_rs_files(&json, project_dir)
}

/// Parses `cargo metadata --no-deps --format-version 1` JSON and returns deduplicated, sorted
/// `.rs` source files belonging to targets inside `project_dir`.
pub fn parse_cargo_metadata_rs_files(
    metadata_json: &str,
    project_dir: &Path,
) -> Option<Vec<PathBuf>> {
    let val: Value = serde_json::from_str(metadata_json).ok()?;
    let pkgs = val.get("packages")?.as_array()?;
    let proj_canon =
        std::fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
    let mut found = std::collections::BTreeSet::new();
    for pkg in pkgs {
        let Some(manifest_str) = pkg.get("manifest_path").and_then(Value::as_str) else {
            continue;
        };
        let Some(pkg_dir) = Path::new(manifest_str).parent() else {
            continue;
        };
        let pkg_canon = std::fs::canonicalize(pkg_dir).unwrap_or_else(|_| pkg_dir.to_path_buf());
        if !pkg_canon.starts_with(&proj_canon) {
            continue;
        }
        let Some(targets) = pkg.get("targets").and_then(Value::as_array) else {
            continue;
        };
        for target in targets {
            let Some(src_str) = target.get("src_path").and_then(Value::as_str) else {
                continue;
            };
            let src_path = Path::new(src_str);
            let Ok(canon) = std::fs::canonicalize(src_path) else {
                continue;
            };
            if !canon.is_file() {
                continue;
            }
            crate::module_graph::collect_target_module_graph(&canon, &mut found);
        }
    }
    let files: Vec<PathBuf> = found.into_iter().collect();
    if files.is_empty() { None } else { Some(files) }
}

fn collect_workspace_cfg_gated_names(
    root: &Path,
    optional_deps: &mut HashSet<String>,
    cfg_gated_mods: &mut HashSet<String>,
    cfg_test_reexports: &mut HashSet<String>,
    is_no_std: &mut bool,
) {
    for entry in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let path = entry.path();
        let os_name = entry.file_name();
        let name = os_name.to_string_lossy();
        let name = name.as_ref();
        if path.is_dir() {
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            collect_workspace_cfg_gated_names(
                &path,
                optional_deps,
                cfg_gated_mods,
                cfg_test_reexports,
                is_no_std,
            );
        } else if name == "Cargo.toml"
            && let Ok(content) = std::fs::read_to_string(&path)
        {
            for line in content.lines() {
                if line.contains("optional")
                    && line.contains("true")
                    && let Some((key, _)) = line.split_once('=')
                {
                    let dep = key.trim().replace('-', "_");
                    if !dep.is_empty() && dep.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    {
                        optional_deps.insert(dep);
                    }
                }
            }
        } else if name.ends_with(".rs")
            && let Ok(content) = std::fs::read_to_string(&path)
        {
            if name == "lib.rs" && content.contains("no_std") {
                *is_no_std = true;
            }
            let mut saw_cfg = false;
            let mut saw_cfg_test = false;
            for line in content.lines() {
                let t = line.trim();
                if t.is_empty() || t.starts_with("//") {
                    continue;
                }
                if t.starts_with("#[cfg(") {
                    saw_cfg = true;
                    if t == "#[cfg(test)]" {
                        saw_cfg_test = true;
                    }
                    continue;
                }
                if t.starts_with("#[") {
                    continue;
                }
                if saw_cfg && let Some(rest) = t.strip_suffix(';') {
                    if let Some(mod_name) = extract_mod_decl_name(rest) {
                        cfg_gated_mods.insert(mod_name.to_string());
                    } else if saw_cfg_test && let Some(leaf) = extract_pub_use_leaf_name(rest) {
                        cfg_test_reexports.insert(leaf.to_string());
                    }
                }
                saw_cfg = false;
                saw_cfg_test = false;
            }
        }
    }
}

fn extract_pub_use_leaf_name(decl: &str) -> Option<&str> {
    let rest = decl.strip_prefix("pub use ")?.trim();
    let leaf = rest.rsplit_once("::").map_or(rest, |(_, l)| l).trim();
    if !leaf.is_empty() && leaf.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        Some(leaf)
    } else {
        None
    }
}

fn extract_mod_decl_name(decl: &str) -> Option<&str> {
    let after_mod = if let Some(r) = decl.strip_prefix("mod ") {
        r
    } else if let Some((_, r)) = decl.split_once(" mod ") {
        r
    } else {
        return None;
    };
    let name = after_mod.trim();
    if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        Some(name)
    } else {
        None
    }
}

fn hint_uses_cfg_gated_item(
    ty: &str,
    optional_deps: &HashSet<String>,
    cfg_gated_mods: &HashSet<String>,
    cfg_test_reexports: &HashSet<String>,
) -> bool {
    for dep in optional_deps {
        let needle = format!("{}::", dep);
        if let Some(pos) = ty.find(&needle)
            && (pos == 0
                || !ty[..pos]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_'))
        {
            return true;
        }
    }
    for m in cfg_gated_mods {
        if ty.contains(&format!("::{}::", m)) {
            return true;
        }
    }
    for reexp in cfg_test_reexports {
        let needle = format!("::{}", reexp);
        if let Some(pos) = ty.find(&needle)
            && ty[pos + needle.len()..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_')
        {
            return true;
        }
    }
    false
}

fn hint_uses_unqualified_alloc_type(ty: &str) -> bool {
    for ident in ["String", "Vec", "Box"] {
        let mut search = ty;
        while let Some(pos) = search.find(ident) {
            let before = &search[..pos];
            let after = &search[pos + ident.len()..];
            let left_ok = before.is_empty()
                || (!before.ends_with("::")
                    && !before
                        .chars()
                        .next_back()
                        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_'));
            let right_ok = after
                .chars()
                .next()
                .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_');
            if left_ok && right_ok {
                return true;
            }
            search = &search[pos + ident.len()..];
        }
    }
    false
}
