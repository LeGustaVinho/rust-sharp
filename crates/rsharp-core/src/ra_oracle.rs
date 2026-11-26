use std::collections::HashMap;
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
    let raw = abs.to_string_lossy();
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    let forward = stripped.replace('\\', "/");
    let mut encoded = String::with_capacity(forward.len() + 16);
    if forward.starts_with('/') {
        encoded.push_str("file://");
    } else {
        encoded.push_str("file:///");
    }
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

/// Minimal stdio JSON-RPC client driving `rust-analyzer` once per Cargo workspace to collect
/// full Cargo-aware type hints (`textDocument/inlayHint`) across all workspace `.rs` files.
pub struct RustAnalyzerOracle {
    reader: Box<dyn BufRead>,
    writer: Box<dyn Write>,
    child: Option<Child>,
    next_id: i64,
}

impl RustAnalyzerOracle {
    /// Spawns `rust-analyzer` (or `RUST_ANALYZER` env override) rooted at `workspace_root`
    /// and waits for initial workspace indexing (`experimental/serverStatus` quiescent).
    pub fn open(workspace_root: &Path) -> Result<Self> {
        let bin = std::env::var("RUST_ANALYZER").unwrap_or_else(|_| "rust-analyzer".to_string());
        Self::open_with_bin(&bin, workspace_root)
    }

    /// Spawns a specific `rust-analyzer` executable path for `workspace_root`.
    pub fn open_with_bin(bin: &str, workspace_root: &Path) -> Result<Self> {
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
        };
        oracle.initialize(workspace_root)?;
        Ok(oracle)
    }

    /// Creates an oracle over arbitrary stdio streams (used for deterministic unit testing).
    pub fn from_io(
        reader: impl BufRead + 'static,
        writer: impl Write + 'static,
        workspace_root: &Path,
    ) -> Result<Self> {
        let mut oracle = Self {
            reader: Box::new(reader),
            writer: Box::new(writer),
            child: None,
            next_id: 1,
        };
        oracle.initialize(workspace_root)?;
        Ok(oracle)
    }

    fn initialize(&mut self, workspace_root: &Path) -> Result<()> {
        let root_uri = path_to_file_uri(workspace_root);
        let ws_name = workspace_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("workspace");
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
                        "dynamicRegistration": false,
                        "resolveSupport": {
                            "properties": ["textEdits"]
                        }
                    }
                }
            },
            "initializationOptions": {
                "cargo": {
                    "allTargets": true,
                    "buildScripts": { "enable": true }
                },
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
        let req_id = self.send_request(
            "textDocument/inlayHint",
            json!({
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": end_line, "character": end_char }
                }
            }),
        )?;
        let mut resp = self.read_response(req_id)?;
        if let Some(items) = resp.as_array_mut() {
            for item in items.iter_mut() {
                if item.get("kind").and_then(Value::as_u64) != Some(1) {
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
                let res_id = self.send_request("inlayHint/resolve", item.clone())?;
                if let Ok(mut resolved) = self.read_response(res_id) {
                    mark_no_edit_if_empty(&mut resolved);
                    *item = resolved;
                }
            }
        }

        self.send_notification(
            "textDocument/didClose",
            json!({
                "textDocument": { "uri": uri }
            }),
        )?;

        Ok(parse_inlay_hint_response(&resp))
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

    fn read_response(&mut self, expected_id: i64) -> Result<Value> {
        loop {
            let msg = read_lsp_message(&mut *self.reader)?;
            if self.try_reply_server_request(&msg)? {
                continue;
            }
            if msg.get("id").and_then(Value::as_i64) == Some(expected_id) {
                if let Some(err) = msg.get("error") {
                    return Err(RSharpError::from_rust(format!(
                        "rust-analyzer LSP error: {}",
                        err
                    )));
                }
                return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
            }
        }
    }
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

fn mark_no_edit_if_empty(item: &mut Value) {
    if let Some(obj) = item.as_object_mut() {
        let has_edits = obj
            .get("textEdits")
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty());
        if !has_edits {
            obj.insert("_rsharp_no_edit".to_string(), Value::Bool(true));
        }
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
