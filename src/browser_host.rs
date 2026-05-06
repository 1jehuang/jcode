//! Firefox Agent Bridge native messaging host.
//!
//! Firefox native messaging manifests point at an executable path only; they do
//! not portably support adding a jcode subcommand argument. On Windows setup
//! therefore copies `jcode.exe` to `firefox-agent-bridge-host.exe`, and this
//! module enters host mode when the process is launched under that filename.

use futures::SinkExt;
use futures::StreamExt;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::sync::{RwLock, mpsc};
use tokio::time::timeout;
use tokio_tungstenite::accept_async_with_config;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

const HOST_EXE_STEM: &str = "firefox-agent-bridge-host";
const CHUNK_SIZE: usize = 750_000;

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);

struct PendingRequest {
    response_tx: mpsc::Sender<Value>,
    started: Instant,
    profile: bool,
}

struct ClientSession {
    id: String,
    active_tab_id: Option<i64>,
    forks: HashMap<String, i64>,
}

type PendingMap = Arc<RwLock<HashMap<String, PendingRequest>>>;
type NativeTx = mpsc::Sender<Value>;

pub fn is_host_invocation() -> bool {
    std::env::args().any(|arg| arg == "--firefox-agent-bridge-host")
        || std::env::current_exe()
            .ok()
            .and_then(|path| path.file_stem().map(|stem| stem.to_os_string()))
            .and_then(|stem| stem.into_string().ok())
            .map(|stem| stem.eq_ignore_ascii_case(HOST_EXE_STEM))
            .unwrap_or(false)
}

fn ws_host() -> String {
    std::env::var("FAB_WS_HOST").unwrap_or_else(|_| "127.0.0.1".to_string())
}

fn ws_port() -> u16 {
    std::env::var("FAB_WS_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(8766)
}

fn request_timeout_ms() -> u64 {
    std::env::var("FAB_REQUEST_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(30000)
}

fn next_id(prefix: &str) -> String {
    let count = REQUEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{prefix}_{now}_{count}")
}

fn log(message: impl AsRef<str>) {
    eprintln!("[firefox-agent-bridge] {}", message.as_ref());
}

pub async fn run() {
    let addr = format!("{}:{}", ws_host(), ws_port());
    let pending: PendingMap = Arc::new(RwLock::new(HashMap::new()));
    let (native_out_tx, mut native_out_rx) = mpsc::channel::<Value>(100);
    let (native_in_tx, native_in_rx) = mpsc::channel::<Value>(100);

    std::thread::spawn(move || {
        read_native_stdin(native_in_tx);
        std::process::exit(0);
    });

    tokio::spawn(async move {
        while let Some(message) = native_out_rx.recv().await {
            write_native_stdout(&message);
        }
    });

    let pending_for_native = Arc::clone(&pending);
    tokio::spawn(async move {
        handle_native_messages(native_in_rx, pending_for_native).await;
    });

    let listener = match TcpListener::bind(&addr).await {
        Ok(listener) => listener,
        Err(err) => {
            log(format!("Failed to bind to {addr}: {err}"));
            std::process::exit(1);
        }
    };

    log(format!("WebSocket server listening on ws://{addr}"));

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let native_tx = native_out_tx.clone();
                let pending = Arc::clone(&pending);
                tokio::spawn(async move {
                    handle_ws_client(stream, native_tx, pending).await;
                });
            }
            Err(err) => log(format!("Failed to accept connection: {err}")),
        }
    }
}

async fn handle_ws_client(
    stream: tokio::net::TcpStream,
    native_tx: NativeTx,
    pending: PendingMap,
) {
    let mut session = ClientSession {
        id: next_id("sess"),
        active_tab_id: None,
        forks: HashMap::new(),
    };

    let ws_config = WebSocketConfig {
        max_message_size: Some(128 * 1024 * 1024),
        max_frame_size: Some(64 * 1024 * 1024),
        ..Default::default()
    };
    let ws_stream = match accept_async_with_config(stream, Some(ws_config)).await {
        Ok(ws_stream) => ws_stream,
        Err(err) => {
            log(format!("WebSocket handshake error: {err}"));
            return;
        }
    };

    let (mut write, mut read) = ws_stream.split();
    let ready = json!({
        "type": "ready",
        "host": ws_host(),
        "port": ws_port(),
        "sessionId": session.id,
    });
    if write.send(Message::Text(ready.to_string())).await.is_err() {
        return;
    }

    while let Some(message) = read.next().await {
        let text = match message {
            Ok(Message::Text(text)) => text,
            Ok(Message::Close(_)) => break,
            Ok(_) => continue,
            Err(err) => {
                log(format!("WebSocket read error: {err}"));
                break;
            }
        };

        let mut request: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(_) => {
                let _ = write
                    .send(Message::Text(
                        json!({"ok": false, "error": "Invalid JSON"}).to_string(),
                    ))
                    .await;
                continue;
            }
        };

        if request.get("type").and_then(|value| value.as_str()) == Some("session_info") {
            let info = json!({
                "type": "session_info",
                "sessionId": session.id,
                "activeTabId": session.active_tab_id,
                "forks": session.forks.keys().collect::<Vec<_>>(),
            });
            let _ = write.send(Message::Text(info.to_string())).await;
            continue;
        }

        if request.get("action").is_none() {
            let _ = write
                .send(Message::Text(
                    json!({"ok": false, "error": "Missing action"}).to_string(),
                ))
                .await;
            continue;
        }

        let id = request
            .get("id")
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| {
                let id = next_id("req");
                request["id"] = json!(id);
                id
            });
        let action = request
            .get("action")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();

        apply_session_targeting(&mut request, &session, &action);

        if action == "configureAuth" {
            let _ = write
                .send(Message::Text(
                    json!({
                        "id": id,
                        "ok": false,
                        "error": "configureAuth is not supported by this host build."
                    })
                    .to_string(),
                ))
                .await;
            continue;
        }

        let profile = request
            .get("profile")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
            || request
                .get("params")
                .and_then(|params| params.get("profile"))
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
        let started = Instant::now();
        let (response_tx, mut response_rx) = mpsc::channel::<Value>(1);

        pending.write().await.insert(
            id.clone(),
            PendingRequest {
                response_tx,
                started,
                profile,
            },
        );

        if needs_chunking(&request)
            && let Err(err) = send_chunked_file(&mut request, &native_tx).await
        {
            pending.write().await.remove(&id);
            let _ = write
                .send(Message::Text(
                    json!({"id": id, "ok": false, "error": err}).to_string(),
                ))
                .await;
            continue;
        }

        if let Err(err) = native_tx.send(request).await {
            pending.write().await.remove(&id);
            let _ = write
                .send(Message::Text(
                    json!({"id": id, "ok": false, "error": format!("Failed to send to browser: {err}")})
                        .to_string(),
                ))
                .await;
            continue;
        }

        let response = match timeout(
            Duration::from_millis(request_timeout_ms()),
            response_rx.recv(),
        )
        .await
        {
            Ok(Some(response)) => response,
            Ok(None) => {
                pending.write().await.remove(&id);
                json!({"id": id, "ok": false, "error": "Request channel closed"})
            }
            Err(_) => {
                pending.write().await.remove(&id);
                json!({"id": id, "ok": false, "error": "Request timed out"})
            }
        };

        update_session_from_response(&mut session, &action, &response);

        if write.send(Message::Text(response.to_string())).await.is_err() {
            break;
        }
    }
}

fn apply_session_targeting(request: &mut Value, session: &ClientSession, action: &str) {
    if action_needs_tab(action) {
        let has_explicit_tab = request
            .get("params")
            .and_then(|params| params.get("tabId"))
            .and_then(|value| value.as_i64())
            .is_some();
        if !has_explicit_tab
            && let Some(tab_id) = session.active_tab_id
        {
            if request.get("params").is_none() {
                request["params"] = json!({});
            }
            request["params"]["tabId"] = json!(tab_id);
        }
    }

    if let Some(fork_name) = request
        .get("params")
        .and_then(|params| params.get("fork"))
        .and_then(|value| value.as_str())
        .map(str::to_string)
        && let Some(tab_id) = session.forks.get(&fork_name)
    {
        request["params"]["tabId"] = json!(tab_id);
    }
}

fn action_needs_tab(action: &str) -> bool {
    matches!(
        action,
        "navigate"
            | "click"
            | "type"
            | "fillForm"
            | "getContent"
            | "getInteractables"
            | "preexplore"
            | "screenshot"
            | "scroll"
            | "evaluate"
            | "waitFor"
            | "tryUntil"
            | "uploadFile"
            | "dropFile"
            | "getAuthContext"
            | "requestAuth"
            | "secureAutoFill"
            | "listFrames"
    )
}

fn update_session_from_response(session: &mut ClientSession, action: &str, response: &Value) {
    if !response
        .get("ok")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        return;
    }

    let Some(result) = response.get("result") else {
        return;
    };

    if matches!(action, "navigate" | "newSession" | "setActiveTab")
        && let Some(tab_id) = result.get("tabId").and_then(|value| value.as_i64())
    {
        session.active_tab_id = Some(tab_id);
    }

    if action == "fork"
        && let Some(forks) = result.get("forks").and_then(|value| value.as_array())
    {
        for fork in forks {
            if let (Some(name), Some(tab_id)) = (
                fork.get("name").and_then(|value| value.as_str()),
                fork.get("tabId").and_then(|value| value.as_i64()),
            ) {
                session.forks.insert(name.to_string(), tab_id);
            }
        }
    }

    if action == "killFork"
        && let Some(killed) = result.get("fork").and_then(|value| value.as_str())
    {
        session.forks.remove(killed);
    }
}

fn needs_chunking(message: &Value) -> bool {
    message.to_string().len() > CHUNK_SIZE
}

async fn send_chunked_file(message: &mut Value, native_tx: &NativeTx) -> Result<(), String> {
    let id = message
        .get("id")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown")
        .to_string();
    let action = message
        .get("action")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    let (base64_data, file_name, mime_type, selector) = chunkable_file_payload(message, &action)?;
    let transfer_id = format!("chunk_{id}");
    let total_chunks = base64_data.len().div_ceil(CHUNK_SIZE);

    native_tx
        .send(json!({
            "type": "chunk_start",
            "transferId": transfer_id,
            "fileName": file_name,
            "mimeType": mime_type,
            "totalSize": base64_data.len(),
            "totalChunks": total_chunks,
        }))
        .await
        .map_err(|err| format!("Failed to send chunk_start: {err}"))?;

    for chunk_index in 0..total_chunks {
        let start = chunk_index * CHUNK_SIZE;
        let end = std::cmp::min(start + CHUNK_SIZE, base64_data.len());
        native_tx
            .send(json!({
                "type": "chunk_data",
                "transferId": transfer_id,
                "chunkIndex": chunk_index,
                "data": &base64_data[start..end],
            }))
            .await
            .map_err(|err| format!("Failed to send chunk {chunk_index}: {err}"))?;
    }

    if action == "fillForm" {
        message["params"] = json!({
            "fields": [{
                "selector": selector,
                "file": {
                    "name": file_name,
                    "type": mime_type,
                    "chunkedTransfer": transfer_id,
                }
            }]
        });
    } else if action == "dropFile"
        && let Some(params) = message.get_mut("params").and_then(|value| value.as_object_mut())
    {
        params.remove("data");
        params.insert("chunkedTransfer".to_string(), json!(transfer_id));
    }

    Ok(())
}

fn chunkable_file_payload(
    message: &Value,
    action: &str,
) -> Result<(String, String, String, String), String> {
    if action == "fillForm" {
        let field = message
            .get("params")
            .and_then(|params| params.get("fields"))
            .and_then(|fields| fields.as_array())
            .and_then(|fields| fields.first())
            .ok_or_else(|| "Large fillForm request has no file field to chunk".to_string())?;
        let file = field
            .get("file")
            .ok_or_else(|| "Large fillForm request has no file payload to chunk".to_string())?;
        let data = file
            .get("data")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Large fillForm file payload has no data".to_string())?
            .to_string();
        let name = file
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or("file")
            .to_string();
        let mime = file
            .get("type")
            .and_then(|value| value.as_str())
            .unwrap_or("application/octet-stream")
            .to_string();
        let selector = field
            .get("selector")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        return Ok((data, name, mime, selector));
    }

    if action == "dropFile" {
        let params = message
            .get("params")
            .ok_or_else(|| "Large dropFile request has no params".to_string())?;
        let data = params
            .get("data")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Large dropFile request has no data".to_string())?
            .to_string();
        let name = params
            .get("fileName")
            .and_then(|value| value.as_str())
            .unwrap_or("file")
            .to_string();
        let mime = params
            .get("mimeType")
            .and_then(|value| value.as_str())
            .unwrap_or("application/octet-stream")
            .to_string();
        let selector = params
            .get("selector")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        return Ok((data, name, mime, selector));
    }

    Err(format!("Large request for action '{action}' is not chunkable"))
}

fn read_native_stdin(tx: mpsc::Sender<Value>) {
    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    let mut len_buf = [0u8; 4];

    loop {
        if stdin.read_exact(&mut len_buf).is_err() {
            log("Native messaging stream ended");
            break;
        }

        let len = u32::from_le_bytes(len_buf) as usize;
        if len == 0 || len > 100 * 1024 * 1024 {
            log(format!("Invalid message length: {len}"));
            continue;
        }

        let mut msg_buf = vec![0u8; len];
        if stdin.read_exact(&mut msg_buf).is_err() {
            log("Failed to read message body");
            continue;
        }

        match serde_json::from_slice::<Value>(&msg_buf) {
            Ok(message) => {
                if tx.blocking_send(message).is_err() {
                    break;
                }
            }
            Err(err) => log(format!("Failed to parse native message: {err}")),
        }
    }
}

fn write_native_stdout(message: &Value) {
    let payload = message.to_string();
    let payload_bytes = payload.as_bytes();
    let len_bytes = (payload_bytes.len() as u32).to_le_bytes();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();

    if stdout.write_all(&len_bytes).is_err() {
        log("Failed to write message length");
        return;
    }
    if stdout.write_all(payload_bytes).is_err() {
        log("Failed to write message body");
        return;
    }
    if stdout.flush().is_err() {
        log("Failed to flush stdout");
    }
}

async fn handle_native_messages(mut native_rx: mpsc::Receiver<Value>, pending: PendingMap) {
    while let Some(mut message) = native_rx.recv().await {
        let Some(id) = message
            .get("id")
            .and_then(|value| value.as_str())
            .map(str::to_string)
        else {
            log(format!("Received event from browser: {message}"));
            continue;
        };

        let Some(request) = pending.write().await.remove(&id) else {
            log(format!("Received response for unknown request: {id}"));
            continue;
        };

        if request.profile {
            let host_ms = request.started.elapsed().as_secs_f64() * 1000.0;
            let mut timing = message
                .get("timing")
                .and_then(|value| value.as_object())
                .cloned()
                .unwrap_or_default();
            timing.insert(
                "hostMs".to_string(),
                json!((host_ms * 100.0).round() / 100.0),
            );
            message["timing"] = json!(timing);
        }

        let _ = request.response_tx.send(message).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_tab_requirement_matches_browser_actions() {
        assert!(action_needs_tab("navigate"));
        assert!(action_needs_tab("listFrames"));
        assert!(!action_needs_tab("listTabs"));
        assert!(!action_needs_tab("ping"));
    }

    #[test]
    fn chunkable_fill_form_payload_is_detected() {
        let message = json!({
            "action": "fillForm",
            "params": {
                "fields": [{
                    "selector": "#file",
                    "file": {
                        "name": "a.txt",
                        "type": "text/plain",
                        "data": "abc"
                    }
                }]
            }
        });

        let payload = chunkable_file_payload(&message, "fillForm").unwrap();
        assert_eq!(payload.0, "abc");
        assert_eq!(payload.1, "a.txt");
        assert_eq!(payload.2, "text/plain");
        assert_eq!(payload.3, "#file");
    }
}
