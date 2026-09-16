//! A minimal, synchronous Language Server Protocol client.
//!
//! The four `lsp_*` tools exist to answer one question the rest of the tool
//! surface cannot: *did the AI just break something?* Without a language
//! server an edit is a blind write, and the only way to learn that a
//! signature change orphaned three call sites is to run a build — slow,
//! coarse, and unavailable in a project that does not build cleanly to begin
//! with. A server knows the answer in milliseconds.
//!
//! ## Why this is small on purpose
//!
//! LSP is a large protocol; a coding agent needs almost none of it. This
//! client speaks exactly four requests (`initialize`, `definition`,
//! `references`, `documentSymbol`/`workspace/symbol`) plus the
//! `publishDiagnostics` notification, and it is **best-effort by
//! construction**: a project with no installed server, a server that crashes,
//! or a request that never answers all degrade to "no answer available"
//! rather than a failed tool call. Diagnostics are a bonus signal, never a
//! dependency.
//!
//! ## Threading
//!
//! One background thread per client owns reading the server's stdout and
//! pushes every decoded message into an `mpsc` channel. Requests send and then
//! drain that channel until their id comes back, stashing notifications as
//! they pass. This is the whole concurrency model: a request cannot be
//! answered by a message the reader thread has not seen, because it is the
//! only reader. Commands run on Tauri's blocking pool, so blocking here costs
//! nothing the UI can feel.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// How long `initialize` may take. A cold Rust project can legitimately spend
/// several seconds indexing before it answers, and aborting it produces a
/// spurious "unavailable".
const INIT_TIMEOUT: Duration = Duration::from_secs(20);

/// How long a definition/reference/symbol request may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// How long to wait for diagnostics after `didOpen`. Short: the server either
/// has them cached or is re-analyzing, and a diagnostic is worth waiting a
/// moment for, not a full analysis pass.
const DIAGNOSTIC_WAIT: Duration = Duration::from_secs(8);

/// A server that could not be started or did not answer.
#[derive(Debug)]
pub enum LspError {
    /// No server is installed for this project, or none was detected.
    Unavailable(String),
    Protocol(String),
}

impl std::fmt::Display for LspError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LspError::Unavailable(e) => write!(f, "no language server available: {e}"),
            LspError::Protocol(e) => write!(f, "language server protocol error: {e}"),
        }
    }
}

/// Which server to run for a project, and with what arguments.
///
/// Detected from the same marker files every tool in this space uses. A
/// project can match more than one (a Tauri app is Rust *and* TypeScript);
/// the order is most-specific first, and only one server is started per
/// project, so a mixed project gets code intelligence for its primary
/// language and none for the other. That is a deliberate limit: running two
/// servers and merging their diagnostics would double the failure modes for a
/// capability that is already best-effort.
fn pick_server(root: &Path) -> Option<(&'static str, &'static [&'static str])> {
    let has = |name: &str| root.join(name).exists();
    if has("Cargo.toml") {
        Some(("rust-analyzer", &[]))
    } else if has("tsconfig.json") || has("package.json") {
        Some(("typescript-language-server", &["--stdio"]))
    } else if has("pyproject.toml") || has("setup.py") || has("requirements.txt") {
        Some(("pyright-langserver", &["--stdio"]))
    } else if has("go.mod") {
        Some(("gopls", &[]))
    } else {
        None
    }
}

/// The LSP `languageId` for a path, by extension.
fn language_id(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "rs" => "rust",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "py" => "python",
        "go" => "go",
        _ => "plaintext",
    }
}

/// A position in a document, 1-based — the convention the tools and
/// `read_file` share. LSP itself is 0-based; the conversion lives at the
/// boundary, in exactly two places.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

/// A location a server returned, converted back to 1-based.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub path: String,
    pub line: u32,
    pub character: u32,
}

/// One diagnostic, already reduced to the fields a reader acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub path: String,
    pub line: u32,
    pub character: u32,
    /// `error` | `warning` | `information` | `hint`.
    pub severity: String,
    pub message: String,
    pub source: String,
}

/// One symbol, from a document outline or a workspace query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    /// The LSP `SymbolKind` as a word (`function`, `struct`, …).
    pub kind: String,
    pub path: String,
    pub line: u32,
    pub container: Option<String>,
}

/// A running language server, owned by one workspace root.
pub struct Client {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<serde_json::Value>,
    next_id: i64,
    root: PathBuf,
    /// The detected server's program name, for the tool's structured output.
    pub server: String,
    /// Latest diagnostics per document URI, updated as notifications arrive.
    diagnostics: HashMap<String, Vec<Diagnostic>>,
}

impl Drop for Client {
    fn drop(&mut self) {
        // Ask politely, then insist: a server that ignores `shutdown` must not
        // outlive the app. Best-effort — the process may already be gone.
        let _ = self.notify("exit", serde_json::json!(null));
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Client {
    /// Detect, spawn, and initialize a server for `root`.
    pub fn start(root: &Path) -> Result<Client, LspError> {
        let (program, args) = pick_server(root).ok_or_else(|| {
            LspError::Unavailable(
                "no Cargo.toml, tsconfig.json, pyproject.toml, or go.mod to identify a language"
                    .into(),
            )
        })?;
        let mut child = Command::new(program)
            .args(args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| LspError::Unavailable(format!("could not start {program}: {e}")))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| LspError::Protocol("server has no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| LspError::Protocol("server has no stdout".into()))?;

        // The reader thread is the only reader, so a response can never be
        // consumed by a concurrent request.
        let (tx, rx) = mpsc::channel::<serde_json::Value>();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Some(msg) = read_message(&mut reader) {
                if tx.send(msg).is_err() {
                    break;
                }
            }
        });

        let mut client = Client {
            child,
            stdin,
            rx,
            next_id: 1,
            root: root.to_path_buf(),
            server: program.to_string(),
            diagnostics: HashMap::new(),
        };
        client.initialize()?;
        Ok(client)
    }

    fn send(&mut self, msg: &serde_json::Value) -> Result<(), LspError> {
        let body = serde_json::to_string(msg).map_err(|e| LspError::Protocol(e.to_string()))?;
        write!(self.stdin, "Content-Length: {}\r\n\r\n", body.len())
            .and_then(|_| self.stdin.write_all(body.as_bytes()))
            .and_then(|_| self.stdin.flush())
            .map_err(|e| LspError::Protocol(format!("could not write to server: {e}")))
    }

    fn notify(&mut self, method: &str, params: serde_json::Value) -> Result<(), LspError> {
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
    }

    /// Send a request and block until its response arrives, stashing any
    /// notification that arrives first.
    fn request(
        &mut self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, LspError> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))?;
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(LspError::Protocol(format!("{method} timed out")));
            }
            match self.rx.recv_timeout(remaining) {
                Ok(msg) => {
                    if msg.get("id").and_then(|v| v.as_i64()) == Some(id) {
                        if let Some(err) = msg.get("error") {
                            return Err(LspError::Protocol(err.to_string()));
                        }
                        return Ok(msg.get("result").cloned().unwrap_or(serde_json::Value::Null));
                    }
                    self.absorb(&msg);
                }
                Err(RecvTimeoutError::Timeout) => {
                    return Err(LspError::Protocol(format!("{method} timed out")))
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(LspError::Protocol("server exited".into()))
                }
            }
        }
    }

    /// Pull diagnostics and any other notification out of a message.
    fn absorb(&mut self, msg: &serde_json::Value) {
        if msg.get("method").and_then(|m| m.as_str()) != Some("textDocument/publishDiagnostics") {
            return;
        }
        let Some(params) = msg.get("params") else {
            return;
        };
        let Some(uri) = params.get("uri").and_then(|u| u.as_str()) else {
            return;
        };
        let path = uri_to_path(uri);
        let items = params
            .get("diagnostics")
            .and_then(|d| d.as_array())
            .map(|items| {
                items.iter().filter_map(|d| diagnostic_from(d, &path)).collect()
            })
            .unwrap_or_default();
        self.diagnostics.insert(uri.to_string(), items);
    }

    fn initialize(&mut self) -> Result<(), LspError> {
        let root_uri = path_to_uri(&self.root);
        let params = serde_json::json!({
            "processId": serde_json::Value::Null,
            "rootUri": root_uri,
            "workspaceFolders": [{ "uri": root_uri, "name": "workspace" }],
            "capabilities": {
                "textDocument": {
                    "publishDiagnostics": {},
                    "definition": {},
                    "references": {},
                    "documentSymbol": {},
                    "hover": {}
                },
                "workspace": { "symbol": {} }
            }
        });
        self.request("initialize", params, INIT_TIMEOUT)?;
        self.notify("initialized", serde_json::json!({}))?;
        Ok(())
    }

    /// Tell the server a file is open, so it will analyze it.
    fn did_open(&mut self, path: &Path) -> Result<(), LspError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| LspError::Protocol(format!("{}: {e}", path.display())))?;
        self.notify(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {
                    "uri": path_to_uri(path),
                    "languageId": language_id(path),
                    "version": 1,
                    "text": text,
                }
            }),
        )
    }

    /// Wait for (or find already-stored) diagnostics for one document.
    fn diagnostics_for(&mut self, path: &Path, wait: Duration) -> Vec<Diagnostic> {
        let uri = path_to_uri(path);
        // The server may have pushed diagnostics for this file already (it
        // does not have to wait for a didOpen to do so), so check first.
        if let Some(d) = self.diagnostics.get(&uri) {
            if !d.is_empty() {
                return d.clone();
            }
        }
        let deadline = Instant::now() + wait;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match self.rx.recv_timeout(remaining) {
                Ok(msg) => {
                    let is_ours = msg
                        .get("params")
                        .and_then(|p| p.get("uri"))
                        .and_then(|u| u.as_str())
                        == Some(uri.as_str());
                    self.absorb(&msg);
                    if is_ours {
                        // One more short grace period for a follow-up batch,
                        // then answer with what is known.
                        let grace = Duration::from_millis(300);
                        while let Ok(next) = self.rx.recv_timeout(grace) {
                            self.absorb(&next);
                        }
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        self.diagnostics.get(&uri).cloned().unwrap_or_default()
    }

    /// Diagnostics for one file, or for every file the server has announced.
    pub fn diagnostics(&mut self, path: Option<&Path>) -> Result<Vec<Diagnostic>, LspError> {
        match path {
            Some(p) => {
                self.did_open(p)?;
                Ok(self.diagnostics_for(p, DIAGNOSTIC_WAIT))
            }
            None => {
                // Nothing to open: return what the server has pushed so far.
                let grace = Duration::from_millis(500);
                let deadline = Instant::now() + grace;
                while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
                    match self.rx.recv_timeout(remaining) {
                        Ok(msg) => self.absorb(&msg),
                        Err(_) => break,
                    }
                }
                let mut all: Vec<Diagnostic> = self.diagnostics.values().flatten().cloned().collect();
                all.sort_by(|a, b| {
                    (a.path.as_str(), a.line, a.character)
                        .cmp(&(b.path.as_str(), b.line, b.character))
                });
                Ok(all)
            }
        }
    }

    fn position_params(&self, path: &Path, pos: Position) -> serde_json::Value {
        serde_json::json!({
            "textDocument": { "uri": path_to_uri(path) },
            "position": { "line": pos.line.saturating_sub(1), "character": pos.character.saturating_sub(1) }
        })
    }

    pub fn definition(&mut self, path: &Path, pos: Position) -> Result<Vec<Location>, LspError> {
        self.did_open(path)?;
        let result = self.request(
            "textDocument/definition",
            self.position_params(path, pos),
            REQUEST_TIMEOUT,
        )?;
        Ok(locations_from(&result, &self.root))
    }

    pub fn references(
        &mut self,
        path: &Path,
        pos: Position,
        include_declaration: bool,
    ) -> Result<Vec<Location>, LspError> {
        self.did_open(path)?;
        let mut params = self.position_params(path, pos);
        params["context"] = serde_json::json!({ "includeDeclaration": include_declaration });
        let result = self.request("textDocument/references", params, REQUEST_TIMEOUT)?;
        Ok(locations_from(&result, &self.root))
    }

    /// Symbols in one document, or (with `query`) across the workspace.
    pub fn symbols(
        &mut self,
        path: Option<&Path>,
        query: Option<&str>,
    ) -> Result<Vec<Symbol>, LspError> {
        match (path, query) {
            (Some(p), None) => {
                self.did_open(p)?;
                let result = self.request(
                    "textDocument/documentSymbol",
                    serde_json::json!({ "textDocument": { "uri": path_to_uri(p) } }),
                    REQUEST_TIMEOUT,
                )?;
                Ok(symbols_from_document(&result, p, &self.root))
            }
            (_, Some(q)) => {
                let result = self.request(
                    "workspace/symbol",
                    serde_json::json!({ "query": q }),
                    REQUEST_TIMEOUT,
                )?;
                Ok(symbols_from_workspace(&result, &self.root))
            }
            (None, None) => Err(LspError::Protocol(
                "lsp_symbols needs a path or a query".into(),
            )),
        }
    }
}

// --- framing -----------------------------------------------------------------

/// Read one `Content-Length`-framed JSON-RPC message. `None` at EOF or on a
/// malformed frame, which ends the reader thread and lets requests time out
/// rather than block forever.
fn read_message(reader: &mut BufReader<impl Read>) -> Option<serde_json::Value> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(v) = trimmed.strip_prefix("Content-Length:") {
            content_length = v.trim().parse().ok();
        }
    }
    let len = content_length?;
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

// --- conversions -------------------------------------------------------------

fn path_to_uri(path: &Path) -> String {
    // Percent-encode the bytes RFC 3986 says a URI path may not contain.
    // Spaces and `#` are the ones that actually appear in real paths.
    let mut encoded = String::new();
    for &b in path.to_string_lossy().as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':'
            | b'\\' => encoded.push(b as char),
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    format!("file://{encoded}")
}

fn uri_to_path(uri: &str) -> String {
    let rest = uri.strip_prefix("file://").unwrap_or(uri);
    let mut out = Vec::with_capacity(rest.len());
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A location from a server, relative to the workspace when possible.
fn location_from(v: &serde_json::Value, root: &Path) -> Option<Location> {
    let uri = v.get("uri")?.as_str()?;
    // `targetUri`/`targetRange` is the LocationLink shape; `uri`/`range` the
    // Location shape. Accept both — servers differ on which they return.
    let range = v.get("range").or_else(|| v.get("targetRange"))?;
    let start = range.get("start")?;
    let path = uri_to_path(uri);
    let shown = Path::new(&path)
        .strip_prefix(root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or(path);
    Some(Location {
        path: shown,
        line: start.get("line").and_then(|l| l.as_u64()).unwrap_or(0) as u32 + 1,
        character: start.get("character").and_then(|c| c.as_u64()).unwrap_or(0) as u32 + 1,
    })
}

fn locations_from(result: &serde_json::Value, root: &Path) -> Vec<Location> {
    match result {
        serde_json::Value::Array(items) => {
            items.iter().filter_map(|v| location_from(v, root)).collect()
        }
        serde_json::Value::Object(_) => location_from(result, root).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// The LSP `SymbolKind` number as a word. Unlisted values are `symbol`.
fn symbol_kind(kind: Option<u64>) -> &'static str {
    match kind {
        Some(1) => "file",
        Some(2) => "module",
        Some(3) => "namespace",
        Some(4) => "package",
        Some(5) => "class",
        Some(6) => "method",
        Some(7) => "property",
        Some(8) => "field",
        Some(9) => "constructor",
        Some(10) => "enum",
        Some(11) => "interface",
        Some(12) => "function",
        Some(13) => "variable",
        Some(14) => "constant",
        Some(15) => "string",
        Some(22) => "struct",
        Some(23) => "event",
        Some(26) => "type_parameter",
        _ => "symbol",
    }
}

fn diagnostic_from(d: &serde_json::Value, path: &str) -> Option<Diagnostic> {
    let range = d.get("range")?;
    let start = range.get("start")?;
    Some(Diagnostic {
        path: path.to_string(),
        line: start.get("line").and_then(|l| l.as_u64()).unwrap_or(0) as u32 + 1,
        character: start.get("character").and_then(|c| c.as_u64()).unwrap_or(0) as u32 + 1,
        severity: match d.get("severity").and_then(|s| s.as_u64()) {
            Some(1) => "error",
            Some(2) => "warning",
            Some(3) => "information",
            Some(4) => "hint",
            _ => "warning",
        }
        .to_string(),
        message: d
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string(),
        source: d
            .get("source")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

/// Document symbols: either a flat `SymbolInformation[]` (each with a
/// `location`) or a nested `DocumentSymbol[]` (each with a `selectionRange`
/// and optional `children`). Both are legal for the same request.
fn symbols_from_document(result: &serde_json::Value, path: &Path, root: &Path) -> Vec<Symbol> {
    let shown = path
        .strip_prefix(root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string_lossy().into_owned());
    let mut out = Vec::new();
    let items = result.as_array().cloned().unwrap_or_default();
    for item in &items {
        if let Some(loc) = item.get("location") {
            let line = loc
                .get("range")
                .and_then(|r| r.get("start"))
                .and_then(|s| s.get("line"))
                .and_then(|l| l.as_u64())
                .unwrap_or(0) as u32
                + 1;
            let path = loc
                .get("uri")
                .and_then(|u| u.as_str())
                .map(|u| {
                    let p = uri_to_path(u);
                    Path::new(&p)
                        .strip_prefix(root)
                        .map(|x| x.to_string_lossy().into_owned())
                        .unwrap_or(p)
                })
                .unwrap_or_else(|| shown.clone());
            out.push(Symbol {
                name: item.get("name").and_then(|n| n.as_str()).unwrap_or("").into(),
                kind: symbol_kind(item.get("kind").and_then(|k| k.as_u64())).into(),
                path,
                line,
                container: item
                    .get("containerName")
                    .and_then(|c| c.as_str())
                    .map(str::to_string),
            });
        } else {
            flatten_document_symbol(item, &shown, None, &mut out);
        }
    }
    out
}

fn flatten_document_symbol(
    item: &serde_json::Value,
    path: &str,
    container: Option<&str>,
    out: &mut Vec<Symbol>,
) {
    let line = item
        .get("selectionRange")
        .or_else(|| item.get("range"))
        .and_then(|r| r.get("start"))
        .and_then(|s| s.get("line"))
        .and_then(|l| l.as_u64())
        .unwrap_or(0) as u32
        + 1;
    let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("");
    out.push(Symbol {
        name: name.to_string(),
        kind: symbol_kind(item.get("kind").and_then(|k| k.as_u64())).into(),
        path: path.to_string(),
        line,
        container: container.map(str::to_string),
    });
    if let Some(children) = item.get("children").and_then(|c| c.as_array()) {
        for child in children {
            flatten_document_symbol(child, path, Some(name), out);
        }
    }
}

fn symbols_from_workspace(result: &serde_json::Value, root: &Path) -> Vec<Symbol> {
    let mut out = Vec::new();
    for item in result.as_array().cloned().unwrap_or_default() {
        let Some(loc) = item.get("location") else {
            continue;
        };
        let path = loc
            .get("uri")
            .and_then(|u| u.as_str())
            .map(uri_to_path)
            .map(|p| {
                Path::new(&p)
                    .strip_prefix(root)
                    .map(|x| x.to_string_lossy().into_owned())
                    .unwrap_or(p)
            })
            .unwrap_or_default();
        let line = loc
            .get("range")
            .and_then(|r| r.get("start"))
            .and_then(|s| s.get("line"))
            .and_then(|l| l.as_u64())
            .unwrap_or(0) as u32
            + 1;
        out.push(Symbol {
            name: item.get("name").and_then(|n| n.as_str()).unwrap_or("").into(),
            kind: symbol_kind(item.get("kind").and_then(|k| k.as_u64())).into(),
            path,
            line,
            container: item
                .get("containerName")
                .and_then(|c| c.as_str())
                .map(str::to_string),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_round_trip_including_spaces_and_hashes() {
        for p in [
            "/tmp/plain/file.rs",
            "/tmp/with space/file.rs",
            "/tmp/hash#tag/x.ts",
            "/tmp/percent%20literal.rs",
        ] {
            let path = PathBuf::from(p);
            let uri = path_to_uri(&path);
            assert_eq!(uri_to_path(&uri), p, "round trip for {p}");
        }
    }

    #[test]
    fn language_ids_cover_the_detected_servers() {
        assert_eq!(language_id(Path::new("a/b.rs")), "rust");
        assert_eq!(language_id(Path::new("a/b.tsx")), "typescriptreact");
        assert_eq!(language_id(Path::new("a/b.py")), "python");
        assert_eq!(language_id(Path::new("a/b.go")), "go");
        assert_eq!(language_id(Path::new("a/b.unknown")), "plaintext");
    }

    #[test]
    fn a_location_is_relative_to_the_root_and_one_based() {
        let root = Path::new("/work");
        let v = serde_json::json!({
            "uri": "file:///work/src/lib.rs",
            "range": { "start": { "line": 4, "character": 8 }, "end": { "line": 4, "character": 12 } }
        });
        let loc = location_from(&v, root).unwrap();
        assert_eq!(loc.path, "src/lib.rs");
        assert_eq!(loc.line, 5);
        assert_eq!(loc.character, 9);
    }

    #[test]
    fn both_location_link_and_location_shapes_decode() {
        let root = Path::new("/work");
        let links = serde_json::json!([{
            "targetUri": "file:///work/a.rs",
            "targetRange": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } },
            "targetSelectionRange": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } }
        }]);
        let locs = locations_from(&links, root);
        assert_eq!(locs.len(), 1);
        assert_eq!(locs[0].line, 1);
    }

    #[test]
    fn diagnostics_are_one_based_and_severity_is_named() {
        let d = serde_json::json!({
            "range": { "start": { "line": 9, "character": 2 }, "end": { "line": 9, "character": 5 } },
            "severity": 1,
            "message": "mismatched types",
            "source": "rustc"
        });
        let parsed = diagnostic_from(&d, "src/main.rs").unwrap();
        assert_eq!(parsed.line, 10);
        assert_eq!(parsed.character, 3);
        assert_eq!(parsed.severity, "error");
        assert_eq!(parsed.source, "rustc");
    }

    #[test]
    fn nested_document_symbols_flatten_with_their_container() {
        let root = Path::new("/work");
        let result = serde_json::json!([
            {
                "name": "Widget",
                "kind": 22,
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 5, "character": 0 } },
                "selectionRange": { "start": { "line": 0, "character": 7 }, "end": { "line": 0, "character": 13 } },
                "children": [
                    {
                        "name": "render",
                        "kind": 6,
                        "range": { "start": { "line": 2, "character": 0 }, "end": { "line": 4, "character": 0 } },
                        "selectionRange": { "start": { "line": 2, "character": 8 }, "end": { "line": 2, "character": 14 } }
                    }
                ]
            }
        ]);
        let symbols = symbols_from_document(&result, Path::new("/work/src/a.rs"), root);
        assert_eq!(symbols.len(), 2);
        assert_eq!(symbols[0].name, "Widget");
        assert_eq!(symbols[0].kind, "struct");
        assert_eq!(symbols[0].path, "src/a.rs");
        assert_eq!(symbols[1].name, "render");
        assert_eq!(symbols[1].container.as_deref(), Some("Widget"));
    }
}
