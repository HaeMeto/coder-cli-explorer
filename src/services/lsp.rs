//! Minimal LSP client: one owner task per language server.
//!
//! Mirrors `services/pty.rs` — an async task owns the child process, a reader
//! forwards server output, and an mpsc carries client intents in. The task
//! speaks JSON-RPC over Content-Length framing and translates server messages
//! into `Msg`s for the main loop. It never touches the `Model`.
//!
//! Concurrency: `start()` creates the intent channel and spawns `run_server`.
//! `run_server` spawns the child, sends `initialize`, then `select!`s over
//! incoming client intents and a reader task's parsed frames. Requests are
//! queued until the server is initialized, then flushed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::app::msg::Msg;
use crate::services::extensions::ServerSpec;

/// The `(tab index, buffer version)` a request was issued for, so a late
/// response can be discarded if the buffer moved on (same guard as `active_hl`).
pub type Token = (usize, u64);

/// A running server, held in the Model like `PtySession`.
pub struct LspHandle {
    pub to_server: UnboundedSender<LspClientMsg>,
}

/// A client -> server intent. The owner task turns each into a JSON-RPC message.
pub enum LspClientMsg {
    DidOpen {
        uri: String,
        language_id: String,
        version: i32,
        text: String,
    },
    DidChange {
        uri: String,
        version: i32,
        text: String,
    },
    DidSave {
        uri: String,
    },
    DidClose {
        uri: String,
    },
    Completion {
        uri: String,
        line: u32,
        character: u32,
        token: Token,
    },
    Formatting {
        uri: String,
        token: Token,
    },
}

/// Diagnostic severity (LSP 1..=4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

impl Severity {
    fn from_lsp(n: u64) -> Severity {
        match n {
            1 => Severity::Error,
            2 => Severity::Warning,
            3 => Severity::Info,
            _ => Severity::Hint,
        }
    }
}

/// A diagnostic in raw LSP coordinates (UTF-16). `update` converts to char cols.
#[derive(Debug, Clone)]
pub struct RawDiagnostic {
    pub start_line: usize,
    pub start_char: u32,
    pub end_line: usize,
    pub end_char: u32,
    pub severity: Severity,
    pub message: String,
}

/// A completion item, normalized from either shape the server may return.
#[derive(Debug, Clone)]
pub struct CompletionItem {
    pub label: String,
    pub insert_text: String,
    pub detail: Option<String>,
    /// Text the client matches the typed prefix against (defaults to `label`).
    /// Servers like rust-analyzer return every candidate and expect the client
    /// to do the filtering.
    pub filter_text: String,
    /// Server-provided sort key; `update` ranks by it within a match tier.
    pub sort_text: String,
}

/// A text edit in raw LSP coordinates (UTF-16). `update` converts to char cols.
#[derive(Debug, Clone)]
pub struct RawTextEdit {
    pub start_line: usize,
    pub start_char: u32,
    pub end_line: usize,
    pub end_char: u32,
    pub new_text: String,
}

/// What an in-flight request id is waiting for.
enum PendingKind {
    Initialize,
    Completion(Token),
    Formatting(Token),
}

/// Creates the intent channel, spawns the owner task, and returns the handle.
pub fn start(language: String, spec: ServerSpec, root: PathBuf, tx: UnboundedSender<Msg>) -> LspHandle {
    let (to_server, from_client) = unbounded_channel::<LspClientMsg>();
    tokio::spawn(run_server(language, spec, root, from_client, tx));
    LspHandle { to_server }
}

/// The owner task: owns the child process and drives the JSON-RPC session.
async fn run_server(
    language: String,
    spec: ServerSpec,
    root: PathBuf,
    mut from_client: UnboundedReceiver<LspClientMsg>,
    tx: UnboundedSender<Msg>,
) {
    let mut command = Command::new(&spec.command);
    command.args(&spec.args);
    for (k, v) in &spec.env {
        command.env(k, v);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(Msg::LspError {
                language,
                message: format!("could not start '{}': {e}", spec.command),
            });
            return;
        }
    };

    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");

    // Reader task: forwards every parsed frame to the owner (pty.rs pattern).
    let (frame_tx, mut frame_rx) = unbounded_channel::<Value>();
    tokio::spawn(reader_task(stdout, frame_tx));

    let mut next_id: i64 = 1;
    let mut pending: HashMap<i64, PendingKind> = HashMap::new();
    let mut initialized = false;
    let mut queue: Vec<LspClientMsg> = Vec::new();

    // Send `initialize` immediately; the rest waits for its response.
    let init_id = next_id;
    next_id += 1;
    pending.insert(init_id, PendingKind::Initialize);
    if write_frame(&mut stdin, &initialize_request(init_id, &root))
        .await
        .is_err()
    {
        let _ = tx.send(Msg::LspExited { language });
        return;
    }

    loop {
        tokio::select! {
            maybe_intent = from_client.recv() => {
                let Some(intent) = maybe_intent else { break }; // handle dropped -> stop
                if !initialized {
                    queue.push(intent);
                    continue;
                }
                if send_intent(&mut stdin, intent, &mut next_id, &mut pending).await.is_err() {
                    break;
                }
            }
            maybe_frame = frame_rx.recv() => {
                let Some(frame) = maybe_frame else { break }; // server exited (EOF)
                if handle_frame(&frame, &mut stdin, &mut pending, &tx, &language, &mut initialized).await {
                    // Just became initialized: flush queued intents.
                    for intent in std::mem::take(&mut queue) {
                        let _ = send_intent(&mut stdin, intent, &mut next_id, &mut pending).await;
                    }
                }
            }
        }
    }

    let _ = child.start_kill();
    let _ = tx.send(Msg::LspExited { language });
}

/// Reads Content-Length frames off the server's stdout and forwards each parsed
/// JSON value. Returns on EOF or a read/frame error (which ends the session).
async fn reader_task(stdout: tokio::process::ChildStdout, frame_tx: UnboundedSender<Value>) {
    let mut reader = BufReader::new(stdout);
    loop {
        // Read headers up to the blank line, capturing Content-Length.
        let mut content_len: Option<usize> = None;
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line).await {
                Ok(0) => return, // EOF
                Ok(_) => {}
                Err(_) => return,
            }
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                break; // end of headers
            }
            if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
                content_len = rest.trim().parse().ok();
            }
        }
        let Some(len) = content_len else { continue };
        let mut buf = vec![0u8; len];
        if reader.read_exact(&mut buf).await.is_err() {
            return;
        }
        if let Ok(value) = serde_json::from_slice::<Value>(&buf)
            && frame_tx.send(value).is_err()
        {
            return;
        }
    }
}

/// Classifies one server frame. Returns `true` iff the server just finished
/// initializing (so the caller flushes the queued intents).
async fn handle_frame(
    frame: &Value,
    stdin: &mut ChildStdin,
    pending: &mut HashMap<i64, PendingKind>,
    tx: &UnboundedSender<Msg>,
    language: &str,
    initialized: &mut bool,
) -> bool {
    let id = frame.get("id").and_then(|v| v.as_i64());
    let method = frame.get("method").and_then(|m| m.as_str());

    match (id, method) {
        // Server -> client request: reply so the server doesn't block.
        (Some(id), Some(method)) => {
            let result = server_request_reply(method, frame);
            let _ = write_frame(stdin, &json!({"jsonrpc": "2.0", "id": id, "result": result})).await;
            false
        }
        // Notification.
        (None, Some("textDocument/publishDiagnostics")) => {
            if let Some((path, diagnostics)) = parse_publish_diagnostics(frame) {
                let _ = tx.send(Msg::LspDiagnostics { path, diagnostics });
            }
            false
        }
        (None, Some(_)) => false, // other notifications ignored
        // Response to one of our requests.
        (Some(id), None) => {
            let Some(kind) = pending.remove(&id) else {
                return false;
            };
            match kind {
                PendingKind::Initialize => {
                    let _ = write_frame(stdin, &initialized_notification()).await;
                    *initialized = true;
                    let _ = tx.send(Msg::LspInitialized {
                        language: language.to_string(),
                    });
                    return true;
                }
                PendingKind::Completion(token) => {
                    let _ = tx.send(Msg::LspCompletions {
                        token,
                        items: parse_completions(frame),
                    });
                }
                PendingKind::Formatting(token) => {
                    let _ = tx.send(Msg::LspFormatEdits {
                        token,
                        edits: parse_text_edits(frame),
                    });
                }
            }
            false
        }
        (None, None) => false,
    }
}

/// A minimal, safe reply to a server->client request so it doesn't deadlock.
fn server_request_reply(method: &str, frame: &Value) -> Value {
    match method {
        // One settings object per requested item; null = "use defaults".
        "workspace/configuration" => {
            let n = frame
                .get("params")
                .and_then(|p| p.get("items"))
                .and_then(|i| i.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            Value::Array(vec![Value::Null; n])
        }
        _ => Value::Null,
    }
}

/// Serializes a client intent to JSON-RPC and writes it to the server.
async fn send_intent(
    stdin: &mut ChildStdin,
    intent: LspClientMsg,
    next_id: &mut i64,
    pending: &mut HashMap<i64, PendingKind>,
) -> std::io::Result<()> {
    let value = match intent {
        LspClientMsg::DidOpen {
            uri,
            language_id,
            version,
            text,
        } => notification(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": uri, "languageId": language_id, "version": version, "text": text}}),
        ),
        LspClientMsg::DidChange { uri, version, text } => notification(
            "textDocument/didChange",
            json!({"textDocument": {"uri": uri, "version": version}, "contentChanges": [{"text": text}]}),
        ),
        LspClientMsg::DidSave { uri } => {
            notification("textDocument/didSave", json!({"textDocument": {"uri": uri}}))
        }
        LspClientMsg::DidClose { uri } => {
            notification("textDocument/didClose", json!({"textDocument": {"uri": uri}}))
        }
        LspClientMsg::Completion {
            uri,
            line,
            character,
            token,
        } => {
            let id = *next_id;
            *next_id += 1;
            pending.insert(id, PendingKind::Completion(token));
            request(
                id,
                "textDocument/completion",
                json!({"textDocument": {"uri": uri}, "position": {"line": line, "character": character}}),
            )
        }
        LspClientMsg::Formatting { uri, token } => {
            let id = *next_id;
            *next_id += 1;
            pending.insert(id, PendingKind::Formatting(token));
            request(
                id,
                "textDocument/formatting",
                json!({"textDocument": {"uri": uri}, "options": {"tabSize": 4, "insertSpaces": true}}),
            )
        }
    };
    write_frame(stdin, &value).await
}

fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

fn request(id: i64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn initialize_request(id: i64, root: &Path) -> Value {
    let root_uri = path_to_uri(root);
    request(
        id,
        "initialize",
        json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "workspaceFolders": [{"uri": root_uri, "name": "root"}],
            "capabilities": {
                "textDocument": {
                    "synchronization": {"didSave": true, "dynamicRegistration": false},
                    "completion": {"completionItem": {"snippetSupport": false}},
                    "formatting": {"dynamicRegistration": false},
                    "publishDiagnostics": {"relatedInformation": false}
                }
            }
        }),
    )
}

fn initialized_notification() -> Value {
    notification("initialized", json!({}))
}

/// Writes a value as a Content-Length-framed JSON-RPC message.
async fn write_frame(stdin: &mut ChildStdin, value: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(value)?;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    stdin.write_all(header.as_bytes()).await?;
    stdin.write_all(&body).await?;
    stdin.flush().await
}

// ----- Response parsing -----

fn parse_publish_diagnostics(frame: &Value) -> Option<(PathBuf, Vec<RawDiagnostic>)> {
    let params = frame.get("params")?;
    let path = uri_to_path(params.get("uri")?.as_str()?)?;
    let diagnostics = params
        .get("diagnostics")?
        .as_array()?
        .iter()
        .filter_map(parse_one_diagnostic)
        .collect();
    Some((path, diagnostics))
}

fn parse_one_diagnostic(d: &Value) -> Option<RawDiagnostic> {
    let range = d.get("range")?;
    let (start_line, start_char) = parse_position(range.get("start")?)?;
    let (end_line, end_char) = parse_position(range.get("end")?)?;
    let severity = d
        .get("severity")
        .and_then(|s| s.as_u64())
        .map(Severity::from_lsp)
        .unwrap_or(Severity::Error);
    let message = d
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();
    Some(RawDiagnostic {
        start_line,
        start_char,
        end_line,
        end_char,
        severity,
        message,
    })
}

fn parse_position(p: &Value) -> Option<(usize, u32)> {
    let line = p.get("line")?.as_u64()? as usize;
    let character = p.get("character")?.as_u64()? as u32;
    Some((line, character))
}

fn parse_completions(frame: &Value) -> Vec<CompletionItem> {
    let result = match frame.get("result") {
        Some(r) if !r.is_null() => r,
        _ => return Vec::new(),
    };
    // Either a CompletionList {items:[...]} or a bare CompletionItem[].
    let items = result
        .get("items")
        .and_then(|i| i.as_array())
        .or_else(|| result.as_array());
    let Some(items) = items else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let label = item.get("label")?.as_str()?.to_string();
            let insert_text = item
                .get("insertText")
                .and_then(|t| t.as_str())
                .or_else(|| {
                    item.get("textEdit")
                        .and_then(|e| e.get("newText"))
                        .and_then(|t| t.as_str())
                })
                .unwrap_or(&label)
                .to_string();
            let detail = item
                .get("detail")
                .and_then(|d| d.as_str())
                .map(|s| s.to_string());
            let filter_text = item
                .get("filterText")
                .and_then(|t| t.as_str())
                .unwrap_or(&label)
                .to_string();
            let sort_text = item
                .get("sortText")
                .and_then(|t| t.as_str())
                .unwrap_or(&label)
                .to_string();
            Some(CompletionItem {
                label,
                insert_text,
                detail,
                filter_text,
                sort_text,
            })
        })
        .collect()
}

fn parse_text_edits(frame: &Value) -> Vec<RawTextEdit> {
    let Some(edits) = frame.get("result").and_then(|r| r.as_array()) else {
        return Vec::new();
    };
    edits
        .iter()
        .filter_map(|e| {
            let range = e.get("range")?;
            let (start_line, start_char) = parse_position(range.get("start")?)?;
            let (end_line, end_char) = parse_position(range.get("end")?)?;
            let new_text = e.get("newText")?.as_str()?.to_string();
            Some(RawTextEdit {
                start_line,
                start_char,
                end_line,
                end_char,
                new_text,
            })
        })
        .collect()
}

// ----- file:// URI <-> path (minimal percent-encoding) -----

/// Converts a filesystem path to a `file://` URI, percent-encoding as needed.
pub fn path_to_uri(path: &Path) -> String {
    let mut s = String::from("file://");
    for byte in path.to_string_lossy().as_bytes() {
        let b = *byte;
        // Unreserved per RFC 3986, plus '/' which stays a path separator.
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            s.push(b as char);
        } else {
            s.push('%');
            s.push_str(&format!("{b:02X}"));
        }
    }
    s
}

/// Parses a `file://` URI back into a path, decoding percent-escapes.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            if let Ok(b) = u8::from_str_radix(hex, 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let s = String::from_utf8(out).ok()?;
    Some(PathBuf::from(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_round_trip_with_spaces() {
        let p = PathBuf::from("/home/user/my file.rs");
        let uri = path_to_uri(&p);
        assert_eq!(uri, "file:///home/user/my%20file.rs");
        assert_eq!(uri_to_path(&uri), Some(p));
    }

    #[test]
    fn completions_from_list_and_array() {
        let list = json!({"result": {"items": [{"label": "foo", "detail": "fn"}]}});
        let items = parse_completions(&list);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "foo");
        assert_eq!(items[0].insert_text, "foo");
        assert_eq!(items[0].detail.as_deref(), Some("fn"));

        let arr = json!({"result": [{"label": "bar", "insertText": "bar()"}]});
        let items = parse_completions(&arr);
        assert_eq!(items[0].insert_text, "bar()");

        let null = json!({"result": Value::Null});
        assert!(parse_completions(&null).is_empty());
    }

    /// Drives the real client against a fake Python LSP server: proves the whole
    /// pipeline — spawn, Content-Length framing, initialize handshake, queued
    /// didOpen flush, publishDiagnostics -> Msg — without a heavy real server.
    #[tokio::test]
    async fn fake_server_end_to_end() {
        // Skip gracefully where python3 is unavailable.
        if std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let program = r#"
import sys, json
def read_msg():
    length = 0
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if line == b'':
            break
        if line.lower().startswith(b'content-length:'):
            length = int(line.split(b':')[1].strip())
    return sys.stdin.buffer.read(length)
def write_msg(obj):
    data = json.dumps(obj).encode('utf-8')
    sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(data))
    sys.stdout.buffer.write(data)
    sys.stdout.buffer.flush()
init = json.loads(read_msg())
write_msg({"jsonrpc":"2.0","id":init["id"],"result":{"capabilities":{}}})
while True:
    body = read_msg()
    if body is None:
        break
    m = json.loads(body)
    if m.get("method") == "textDocument/didOpen":
        uri = m["params"]["textDocument"]["uri"]
        write_msg({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics",
                   "params":{"uri":uri,"diagnostics":[
                       {"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":3}},
                        "severity":1,"message":"boom"}]}})
    elif m.get("method") == "textDocument/completion":
        write_msg({"jsonrpc":"2.0","id":m["id"],
                   "result":{"items":[{"label":"println","insertText":"println!"}]}})
"#;
        let spec = ServerSpec {
            command: "python3".to_string(),
            args: vec!["-c".to_string(), program.to_string()],
            env: vec![],
        };
        let (tx, mut rx) = unbounded_channel::<Msg>();
        let handle = start("rust".to_string(), spec, std::env::temp_dir(), tx);
        // update sends didOpen after ensure-started; the client queues it until
        // the handshake completes.
        handle
            .to_server
            .send(LspClientMsg::DidOpen {
                uri: "file:///tmp/x.rs".to_string(),
                language_id: "rust".to_string(),
                version: 0,
                text: "fn".to_string(),
            })
            .unwrap();
        handle
            .to_server
            .send(LspClientMsg::Completion {
                uri: "file:///tmp/x.rs".to_string(),
                line: 0,
                character: 2,
                token: (0, 0),
            })
            .unwrap();

        let mut initialized = false;
        let mut got_diag = false;
        let mut got_completion = false;
        let deadline = std::time::Duration::from_secs(10);
        while !(got_diag && got_completion) {
            let msg = tokio::time::timeout(deadline, rx.recv())
                .await
                .expect("timed out waiting for LSP messages")
                .expect("channel closed");
            match msg {
                Msg::LspInitialized { .. } => initialized = true,
                Msg::LspError { message, .. } => panic!("server error: {message}"),
                Msg::LspDiagnostics { path, diagnostics } => {
                    assert!(initialized, "diagnostics arrived before initialize");
                    assert_eq!(path, PathBuf::from("/tmp/x.rs"));
                    assert_eq!(diagnostics.len(), 1);
                    assert_eq!(diagnostics[0].message, "boom");
                    assert_eq!(diagnostics[0].severity, Severity::Error);
                    got_diag = true;
                }
                Msg::LspCompletions { token, items } => {
                    assert_eq!(token, (0, 0));
                    assert_eq!(items.len(), 1);
                    assert_eq!(items[0].label, "println");
                    assert_eq!(items[0].insert_text, "println!");
                    got_completion = true;
                }
                _ => {}
            }
        }
    }

    #[test]
    fn diagnostics_parse() {
        let frame = json!({
            "params": {
                "uri": "file:///tmp/a.rs",
                "diagnostics": [{
                    "range": {"start": {"line": 2, "character": 4}, "end": {"line": 2, "character": 9}},
                    "severity": 1,
                    "message": "mismatched types"
                }]
            }
        });
        let (path, diags) = parse_publish_diagnostics(&frame).unwrap();
        assert_eq!(path, PathBuf::from("/tmp/a.rs"));
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].start_line, 2);
        assert_eq!(diags[0].end_char, 9);
        assert_eq!(diags[0].severity, Severity::Error);
    }
}
