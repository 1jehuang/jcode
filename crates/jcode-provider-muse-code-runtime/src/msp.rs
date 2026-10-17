//! Muse Session Protocol (MSP) transport.
//!
//! MSP is JSON-RPC 2.0 over the `muse serve` process's stdin/stdout. Frames are
//! **newline-delimited JSON objects**, one per line. Three observations here are
//! not in the published schema and were established by probing a live host:
//!
//! 1. Framing is newline-delimited, *not* `Content-Length`-prefixed. Sending an
//!    LSP-style header yields `-32700 parseError`.
//! 2. `initialize.params.clientInfo.name` must match `^[a-z0-9_]+$`. Anything
//!    else is `-32602 invalidParams` before any other work happens.
//! 3. The handshake is only complete after the client sends an `initialized`
//!    notification (no params). Every method before that returns
//!    `-32600 notInitialized`, *including* methods that would otherwise succeed.
//!
//! Mutation methods additionally require a client-minted UUIDv7 `commandId`
//! idempotency handle; the server never mints one. See [`new_command_id`].

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, broadcast, oneshot};

/// Default request timeout. MSP turns are driven by notifications, so a request
/// that never answers is a bug or a dead host rather than a slow one.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// How many inbound messages to buffer per subscriber.
const EVENT_CHANNEL_CAPACITY: usize = 256;

/// A message the host pushed to us outside a request/response cycle.
#[derive(Debug, Clone)]
pub enum MspInbound {
    /// `{"method": ..., "params": ...}` with no `id`.
    Notification { method: String, params: Value },
    /// Server-initiated request, e.g. `approval/request`. The host expects a
    /// response carrying the same `id`.
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    /// The host closed its stdout (process exit or crash).
    Closed,
}

impl MspInbound {
    pub fn method(&self) -> Option<&str> {
        match self {
            Self::Notification { method, .. } | Self::Request { method, .. } => Some(method),
            Self::Closed => None,
        }
    }
}

/// A structured MSP error, mirroring the wire shape `{code, message, data}`.
#[derive(Debug, Clone)]
pub struct MspError {
    pub code: i64,
    pub message: String,
    /// `data.kind`, when the host supplied one (e.g. `commandRejected`).
    pub kind: Option<String>,
}

impl MspError {
    fn from_value(value: &Value) -> Self {
        Self {
            code: value.get("code").and_then(Value::as_i64).unwrap_or(0),
            message: value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown MSP error")
                .to_string(),
            kind: value
                .get("data")
                .and_then(|d| d.get("kind"))
                .and_then(Value::as_str)
                .map(str::to_string),
        }
    }

    /// JSON-RPC `methodNotFound`. The v1.4.10 host returns this for methods that
    /// the exported *stable* schema still advertises, notably `view/subscribe`
    /// and `session/read`, so callers must not treat schema presence as proof
    /// that a method is served.
    pub fn is_method_not_found(&self) -> bool {
        self.code == -32601
    }

    pub fn is_not_initialized(&self) -> bool {
        self.code == -32600 && self.kind.as_deref() == Some("notInitialized")
    }
}

impl std::fmt::Display for MspError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            Some(kind) => write!(f, "MSP error {} ({}): {}", self.code, kind, self.message),
            None => write!(f, "MSP error {}: {}", self.code, self.message),
        }
    }
}

impl std::error::Error for MspError {}

/// How to launch the host.
#[derive(Debug, Clone)]
pub struct MuseHostConfig {
    /// Executable name or path. Resolved on `PATH` when not absolute.
    pub binary: String,
    /// Extra arguments appended to `serve`.
    ///
    /// Two are worth calling out:
    /// - `--provider echo` runs a stub provider that makes **no model call**,
    ///   which is what the integration tests use.
    /// - `--no-session-log` keeps sessions memory-only.
    pub args: Vec<String>,
    /// Working directory for the host. The host composes its workspace from
    /// here.
    pub cwd: Option<std::path::PathBuf>,
}

impl Default for MuseHostConfig {
    fn default() -> Self {
        Self {
            binary: "muse".to_string(),
            args: vec!["--no-session-log".to_string()],
            cwd: None,
        }
    }
}

impl MuseHostConfig {
    /// Config that cannot spend subscription usage: the `echo` provider answers
    /// locally without calling a model.
    pub fn echo() -> Self {
        Self {
            args: vec![
                "--no-session-log".to_string(),
                "--provider".to_string(),
                "echo".to_string(),
            ],
            ..Self::default()
        }
    }

    pub fn with_model(mut self, model: &str) -> Self {
        self.args.push("--model".to_string());
        self.args.push(model.to_string());
        self
    }
}

/// In-flight requests awaiting a response, keyed by JSON-RPC id.
///
/// Aliased because the nested `Arc<Mutex<HashMap<u64, oneshot::Sender<...>>>>`
/// shape appears in the struct, the spawn path and the reader task, and clippy's
/// `type_complexity` lint (denied in CI) rejects it written out.
type PendingRequests = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, MspError>>>>>;

/// A live MSP connection to a `muse serve` process.
pub struct MspTransport {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: PendingRequests,
    events: broadcast::Sender<MspInbound>,
    next_id: AtomicU64,
    request_timeout: Duration,
}

impl MspTransport {
    /// Spawn `muse serve` and start the reader task. Does not handshake; call
    /// [`MspTransport::initialize`] next.
    pub fn spawn(config: &MuseHostConfig) -> Result<Self> {
        let mut command = Command::new(&config.binary);
        command
            .arg("serve")
            .args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Piped so a chatty host cannot corrupt the protocol stream on
            // stdout or leak into jcode's own terminal; drained by the task
            // below so filling the pipe buffer cannot block the child.
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = &config.cwd {
            command.current_dir(cwd);
        }

        let mut child = command
            .spawn()
            .with_context(|| format!("failed to spawn `{} serve`", config.binary))?;

        let stdin = child.stdin.take().context("muse serve produced no stdin")?;
        let stdout = child
            .stdout
            .take()
            .context("muse serve produced no stdout")?;

        // Drain stderr. Piping it without reading is a deadlock, not merely a
        // missed diagnostic: once the OS pipe buffer fills, the child blocks
        // writing and stops answering MSP requests altogether, so the turn
        // hangs with no visible cause. Forwarding it to jcode's log also means
        // a Muse crash leaves a trace instead of vanishing.
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if !line.trim().is_empty() {
                        jcode_logging::warn(&format!("muse host stderr: {line}"));
                    }
                }
            });
        }

        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let pending: PendingRequests = Arc::new(Mutex::new(HashMap::new()));

        let reader_pending = Arc::clone(&pending);
        let reader_events = events.clone();
        tokio::spawn(async move {
            read_loop(stdout, reader_pending, reader_events).await;
        });

        Ok(Self {
            child,
            stdin: Arc::new(Mutex::new(stdin)),
            pending,
            events,
            next_id: AtomicU64::new(1),
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        })
    }

    /// Subscribe to host-pushed notifications and server requests.
    pub fn subscribe(&self) -> broadcast::Receiver<MspInbound> {
        self.events.subscribe()
    }

    pub fn set_request_timeout(&mut self, timeout: Duration) {
        self.request_timeout = timeout;
    }

    /// Send a request and await its response.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let frame = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        if let Err(err) = self.write_frame(&frame).await {
            self.pending.lock().await.remove(&id);
            return Err(err);
        }

        match tokio::time::timeout(self.request_timeout, rx).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(msp_error))) => Err(msp_error.into()),
            Ok(Err(_)) => bail!("MSP connection closed while awaiting `{method}`"),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                bail!(
                    "MSP request `{method}` timed out after {:?}",
                    self.request_timeout
                )
            }
        }
    }

    /// Send a notification (no response expected).
    pub async fn notify(&self, method: &str, params: Option<Value>) -> Result<()> {
        let mut frame = json!({"jsonrpc": "2.0", "method": method});
        if let Some(params) = params {
            frame["params"] = params;
        }
        self.write_frame(&frame).await
    }

    /// Answer a server-initiated request.
    pub async fn respond(&self, id: Value, result: Value) -> Result<()> {
        self.write_frame(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }))
        .await
    }

    async fn write_frame(&self, frame: &Value) -> Result<()> {
        let mut line = serde_json::to_string(frame).context("failed to encode MSP frame")?;
        line.push('\n');
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(line.as_bytes())
            .await
            .context("failed to write MSP frame")?;
        stdin.flush().await.context("failed to flush MSP frame")?;
        Ok(())
    }

    /// Perform the MSP handshake.
    ///
    /// `client_name` must match `^[a-z0-9_]+$`; invalid names are rejected by
    /// the host before anything else. Returns the parsed `initialize` result.
    pub async fn initialize(&self, client_name: &str, client_version: &str) -> Result<Value> {
        if !is_valid_client_name(client_name) {
            bail!("invalid MSP client name `{client_name}`: must match ^[a-z0-9_]+$");
        }
        let result = self
            .request(
                "initialize",
                json!({
                    "clientInfo": { "name": client_name, "version": client_version },
                }),
            )
            .await
            .context("MSP initialize failed")?;

        // Without this notification every subsequent method returns
        // `notInitialized`, even methods that would otherwise be valid.
        self.notify("initialized", None).await?;
        Ok(result)
    }

    /// Kill the host process.
    pub fn shutdown(&mut self) {
        let _ = self.child.start_kill();
    }
}

/// Validate an MSP client identifier (`^[a-z0-9_]+$`).
pub fn is_valid_client_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Mint a UUIDv7 `commandId`.
///
/// MSP's mutation methods require a client-supplied idempotency handle and the
/// server never mints one. `turn/start` derives the resulting `turnId` from
/// this value, so it is also how a client correlates its own turn.
pub fn new_command_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// Split a single newline-delimited frame into a classified inbound message.
///
/// Classification is by shape, not by method name:
/// - `id` + (`result` | `error`) -> response
/// - `id` + `method`             -> server request
/// - `method` only               -> notification
///
/// Returns `Ok(None)` for blank lines and lines that are not JSON objects.
pub fn classify_frame(line: &str) -> Result<Option<ClassifiedFrame>> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let value: Value = serde_json::from_str(trimmed)
        .with_context(|| format!("MSP frame was not valid JSON: {}", truncate(trimmed, 120)))?;
    let Some(object) = value.as_object() else {
        return Ok(None);
    };

    let id = object.get("id").filter(|v| !v.is_null()).cloned();
    let method = object.get("method").and_then(Value::as_str);

    match (id, method) {
        (Some(id), Some(method)) => Ok(Some(ClassifiedFrame::Request {
            id,
            method: method.to_string(),
            params: object.get("params").cloned().unwrap_or(Value::Null),
        })),
        (Some(id), None) => {
            if let Some(error) = object.get("error") {
                Ok(Some(ClassifiedFrame::Response {
                    id: id_to_u64(&id),
                    outcome: Err(MspError::from_value(error)),
                }))
            } else {
                Ok(Some(ClassifiedFrame::Response {
                    id: id_to_u64(&id),
                    outcome: Ok(object.get("result").cloned().unwrap_or(Value::Null)),
                }))
            }
        }
        (None, Some(method)) => Ok(Some(ClassifiedFrame::Notification {
            method: method.to_string(),
            params: object.get("params").cloned().unwrap_or(Value::Null),
        })),
        (None, None) => Ok(None),
    }
}

/// A parsed inbound frame.
#[derive(Debug, Clone)]
pub enum ClassifiedFrame {
    Response {
        id: Option<u64>,
        outcome: Result<Value, MspError>,
    },
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
}

fn id_to_u64(id: &Value) -> Option<u64> {
    id.as_u64()
}

fn truncate(value: &str, max: usize) -> String {
    if value.len() <= max {
        value.to_string()
    } else {
        format!("{}…", &value[..max])
    }
}

async fn read_loop(
    stdout: tokio::process::ChildStdout,
    pending: PendingRequests,
    events: broadcast::Sender<MspInbound>,
) {
    let mut lines = BufReader::new(stdout).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => match classify_frame(&line) {
                Ok(Some(ClassifiedFrame::Response { id, outcome })) => {
                    if let Some(id) = id
                        && let Some(sender) = pending.lock().await.remove(&id)
                    {
                        let _ = sender.send(outcome);
                    }
                }
                Ok(Some(ClassifiedFrame::Request { id, method, params })) => {
                    let _ = events.send(MspInbound::Request { id, method, params });
                }
                Ok(Some(ClassifiedFrame::Notification { method, params })) => {
                    let _ = events.send(MspInbound::Notification { method, params });
                }
                // Unparseable or shapeless lines are dropped rather than
                // tearing down the connection: the protocol stream is the
                // authoritative channel and a stray line must not kill a turn.
                Ok(None) => {}
                Err(_) => {}
            },
            Ok(None) => break,
            Err(_) => break,
        }
    }
    let _ = events.send(MspInbound::Closed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_name_must_be_machine_identifiable() {
        assert!(is_valid_client_name("jcode_muse_code"));
        assert!(is_valid_client_name("abc123"));
        assert!(!is_valid_client_name(""));
        assert!(!is_valid_client_name("Jcode"));
        assert!(!is_valid_client_name("jcode-muse"));
        assert!(!is_valid_client_name("jcode muse"));
        assert!(!is_valid_client_name("jcode.muse"));
    }

    #[test]
    fn command_ids_are_unique_uuid_v7() {
        let a = new_command_id();
        let b = new_command_id();
        assert_ne!(a, b);
        let parsed = uuid::Uuid::parse_str(&a).expect("commandId must be a UUID");
        assert_eq!(parsed.get_version_num(), 7);
    }

    #[test]
    fn classifies_response_notification_and_request_frames() {
        let response = classify_frame(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#)
            .unwrap()
            .unwrap();
        match response {
            ClassifiedFrame::Response { id, outcome } => {
                assert_eq!(id, Some(1));
                assert_eq!(outcome.unwrap()["ok"], json!(true));
            }
            other => panic!("expected response, got {other:?}"),
        }

        let error =
            classify_frame(r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32601,"message":"method not found","data":{"kind":"methodNotFound"}}}"#)
                .unwrap()
                .unwrap();
        match error {
            ClassifiedFrame::Response { outcome, .. } => {
                let err = outcome.unwrap_err();
                assert!(err.is_method_not_found());
                assert_eq!(err.kind.as_deref(), Some("methodNotFound"));
            }
            other => panic!("expected error response, got {other:?}"),
        }

        let notification =
            classify_frame(r#"{"jsonrpc":"2.0","method":"item/delta","params":{"delta":"hi"}}"#)
                .unwrap()
                .unwrap();
        assert!(matches!(
            notification,
            ClassifiedFrame::Notification { ref method, .. } if method == "item/delta"
        ));

        let request = classify_frame(
            r#"{"jsonrpc":"2.0","id":9,"method":"approval/request","params":{"approvalId":"a"}}"#,
        )
        .unwrap()
        .unwrap();
        assert!(matches!(
            request,
            ClassifiedFrame::Request { ref method, .. } if method == "approval/request"
        ));
    }

    #[test]
    fn blank_and_shapeless_lines_are_ignored() {
        assert!(classify_frame("").unwrap().is_none());
        assert!(classify_frame("   ").unwrap().is_none());
        assert!(classify_frame(r#"{"jsonrpc":"2.0"}"#).unwrap().is_none());
        assert!(classify_frame("[1,2,3]").unwrap().is_none());
    }

    #[test]
    fn malformed_json_is_an_error_but_not_a_panic() {
        assert!(classify_frame("{not json").is_err());
    }

    #[test]
    fn not_initialized_is_recognised() {
        let err = MspError::from_value(&json!({
            "code": -32600,
            "message": "Not initialized",
            "data": {"kind": "notInitialized"}
        }));
        assert!(err.is_not_initialized());
        assert!(!err.is_method_not_found());
    }

    #[test]
    fn echo_config_never_touches_a_model_provider() {
        let config = MuseHostConfig::echo();
        assert!(config.args.contains(&"--provider".to_string()));
        assert!(config.args.contains(&"echo".to_string()));
        assert!(!config.args.iter().any(|a| a == "meta"));
    }
}
