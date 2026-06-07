//! WebSocket gateway for remote clients (iOS app, web).
//!
//! Accepts WebSocket connections over TCP and bridges them to the
//! existing newline-delimited JSON protocol used by Unix socket clients.
//! This lets iOS/web clients interact with jcode sessions identically
//! to TUI clients.
//!
//! Architecture:
//!   TCP :7643  →  WebSocket upgrade  →  UnixStream::pair()  →  handle_client()
//!
//! Each WebSocket client gets a virtual UnixStream pair. One end is handed
//! to the server's existing handle_client(); the other is bridged to WebSocket
//! frames by a relay task.

use anyhow::Result;
use futures::SinkExt;
use futures::stream::StreamExt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

use crate::logging;
mod auth;
mod registry;
use auth::{WsAuth, WsAuthSource, extract_ws_auth, ws_error_response};
#[cfg(test)]
pub(crate) use auth::{is_valid_hex_token, parse_bearer_token, parse_query_token};
pub use jcode_gateway_types::{PairedDevice, PairingCode};
pub use registry::DeviceRegistry;

/// Default gateway port ("jc" on phone keypad = 52, but we use 7643)
pub const DEFAULT_PORT: u16 = 7643;
const WEBSOCKET_KEEPALIVE_INTERVAL_SECS: u64 = 20;

/// Gateway configuration
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    /// TCP port to listen on
    pub port: u16,
    /// Bind address (default: 0.0.0.0 for Tailscale access)
    pub bind_addr: String,
    /// Whether gateway is enabled
    pub enabled: bool,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            bind_addr: "0.0.0.0".to_string(),
            enabled: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Gateway listener
// ---------------------------------------------------------------------------

/// Run the WebSocket gateway. Called from Server::run() as a spawned task.
///
/// For each incoming WebSocket connection:
/// 1. Extract auth token from the WebSocket upgrade request
/// 2. Validate against device registry
/// 3. Create a UnixStream::pair() - one end for the bridge, one for handle_client
/// 4. Spawn a relay task that converts WebSocket frames <-> newline-delimited JSON
/// 5. Return the server-side UnixStream for handle_client to consume
pub async fn run_gateway(
    config: GatewayConfig,
    client_tx: tokio::sync::mpsc::UnboundedSender<GatewayClient>,
) -> Result<()> {
    let addr = format!("{}:{}", config.bind_addr, config.port);
    let listener = TcpListener::bind(&addr).await?;
    logging::info(&format!("WebSocket gateway listening on {}", addr));

    let registry = Arc::new(tokio::sync::RwLock::new(DeviceRegistry::load()));

    loop {
        let (tcp_stream, peer_addr) = listener.accept().await?;
        let registry = Arc::clone(&registry);
        let client_tx = client_tx.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_connection(tcp_stream, peer_addr, registry, client_tx).await {
                logging::error(&format!(
                    "Gateway connection error from {}: {}",
                    peer_addr, e
                ));
            }
        });
    }
}

/// Route an incoming TCP connection: either plain HTTP (pair/health) or WebSocket.
///
/// We peek at the first chunk to check for the Upgrade: websocket header.
/// Plain HTTP requests get handled inline; WebSocket connections proceed to
/// the existing auth + bridge flow.
async fn handle_connection(
    tcp_stream: tokio::net::TcpStream,
    peer_addr: SocketAddr,
    registry: Arc<tokio::sync::RwLock<DeviceRegistry>>,
    client_tx: tokio::sync::mpsc::UnboundedSender<GatewayClient>,
) -> Result<()> {
    let mut peek_buf = [0u8; 2048];
    let n = tcp_stream.peek(&mut peek_buf).await?;
    let request_head = String::from_utf8_lossy(&peek_buf[..n]);

    let is_websocket = request_head.lines().any(|line| {
        let lower = line.to_lowercase();
        lower.starts_with("upgrade:") && lower.contains("websocket")
    });

    if is_websocket {
        handle_ws_connection(tcp_stream, peer_addr, registry, client_tx).await
    } else {
        handle_http(tcp_stream, peer_addr, registry).await
    }
}

/// A gateway client ready to be plugged into handle_client
pub struct GatewayClient {
    /// The server-side end of the virtual Unix socket pair
    pub stream: crate::transport::Stream,
    /// Device info for this client
    pub device_name: String,
    /// Device ID
    pub device_id: String,
}

/// Handle a single incoming TCP connection: upgrade to WebSocket, auth, bridge.
#[expect(
    clippy::result_large_err,
    reason = "WebSocket handshake callback must return Tungstenite ErrorResponse directly"
)]
async fn handle_ws_connection(
    tcp_stream: tokio::net::TcpStream,
    peer_addr: SocketAddr,
    registry: Arc<tokio::sync::RwLock<DeviceRegistry>>,
    client_tx: tokio::sync::mpsc::UnboundedSender<GatewayClient>,
) -> Result<()> {
    // Perform WebSocket handshake with a callback to inspect headers.
    // Prefer Authorization headers, but continue accepting ?token= for browser clients.
    let auth = Arc::new(std::sync::Mutex::new(None::<WsAuth>));
    let auth_cb = Arc::clone(&auth);

    let ws_stream = tokio_tungstenite::accept_hdr_async(
        tcp_stream,
        |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
         response: tokio_tungstenite::tungstenite::handshake::server::Response| {
            if request.uri().path() != "/ws" {
                return Err(ws_error_response(
                    404,
                    "Not Found",
                    "WebSocket endpoint not found",
                ));
            }

            let ws_auth = extract_ws_auth(request)?;
            let mut guard = auth_cb
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *guard = Some(ws_auth);
            Ok(response)
        },
    )
    .await?;

    // Validate auth token
    let auth = auth
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
        .ok_or_else(|| anyhow::anyhow!("No auth token provided"))?;
    let token = auth.token;

    if auth.source == WsAuthSource::Query {
        logging::info(&format!(
            "Gateway: {} connected with deprecated query token auth",
            peer_addr
        ));
    }

    let (device_name, device_id) = {
        let mut reg = registry.write().await;
        // Reload from disk to pick up newly paired devices
        *reg = DeviceRegistry::load();
        match reg.validate_token(&token) {
            Some(device) => {
                let name = device.name.clone();
                let id = device.id.clone();
                reg.touch_device(&token);
                (name, id)
            }
            None => {
                anyhow::bail!("Invalid auth token from {}", peer_addr);
            }
        }
    };

    logging::info(&format!(
        "Gateway: {} connected (device: {}, addr: {})",
        device_name, device_id, peer_addr
    ));

    // Create a virtual Unix socket pair
    let (server_stream, bridge_stream) = crate::transport::stream_pair()
        .map_err(|e| anyhow::anyhow!("Failed to create socket pair: {}", e))?;

    // Send the server-side stream to the main server loop for handle_client
    client_tx.send(GatewayClient {
        stream: server_stream,
        device_name: device_name.clone(),
        device_id,
    })?;

    // Bridge WebSocket frames <-> newline-delimited JSON on the bridge stream
    let (ws_sink, ws_source) = ws_stream.split();
    let ws_sink = Arc::new(tokio::sync::Mutex::new(ws_sink));

    let (bridge_reader, bridge_writer) = bridge_stream.into_split();
    let mut bridge_reader = BufReader::new(bridge_reader);
    let bridge_writer = Arc::new(tokio::sync::Mutex::new(bridge_writer));

    // Task 1: WebSocket → Unix socket (client requests)
    let writer_for_ws = Arc::clone(&bridge_writer);
    let sink_for_ping = Arc::clone(&ws_sink);
    let sink_for_unix = Arc::clone(&ws_sink);
    let sink_for_keepalive = Arc::clone(&ws_sink);
    let ws_to_unix = tokio::spawn(async move {
        let mut ws_source = ws_source;
        while let Some(msg) = ws_source.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    let mut writer = writer_for_ws.lock().await;
                    if text.ends_with('\n') {
                        if writer.write_all(text.as_bytes()).await.is_err() {
                            break;
                        }
                    } else {
                        if writer.write_all(text.as_bytes()).await.is_err() {
                            break;
                        }
                        if writer.write_all(b"\n").await.is_err() {
                            break;
                        }
                    }
                    if writer.flush().await.is_err() {
                        break;
                    }
                }
                Ok(Message::Close(_)) => break,
                Ok(Message::Ping(data)) => {
                    let mut sink = sink_for_ping.lock().await;
                    let _ = sink.send(Message::Pong(data)).await;
                }
                Err(_) => break,
                _ => {}
            }
        }
    });

    let keepalive_device_name = device_name.clone();
    let keepalive = tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(Duration::from_secs(WEBSOCKET_KEEPALIVE_INTERVAL_SECS));
        loop {
            interval.tick().await;
            let mut sink = sink_for_keepalive.lock().await;
            if sink.send(Message::Ping(Vec::new())).await.is_err() {
                logging::info(&format!(
                    "Gateway: stopping keepalive for {} after ping send failure",
                    keepalive_device_name
                ));
                break;
            }
        }
    });

    // Task 2: Unix socket → WebSocket (server events)
    let unix_to_ws = tokio::spawn(async move {
        let mut line = String::new();
        loop {
            line.clear();
            match bridge_reader.read_line(&mut line).await {
                Ok(0) => break, // EOF
                Ok(_) => {
                    let trimmed = line.trim_end().to_string();
                    if !trimmed.is_empty() {
                        let mut sink = sink_for_unix.lock().await;
                        if sink.send(Message::Text(trimmed)).await.is_err() {
                            break;
                        }
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Wait for either direction to finish
    tokio::pin!(ws_to_unix);
    tokio::pin!(unix_to_ws);
    tokio::pin!(keepalive);

    tokio::select! {
        _ = &mut ws_to_unix => {}
        _ = &mut unix_to_ws => {}
        _ = &mut keepalive => {}
    }

    ws_to_unix.abort();
    unix_to_ws.abort();
    keepalive.abort();

    logging::info(&format!("Gateway: {} disconnected", device_name));
    Ok(())
}

fn http_response(status: u16, status_text: &str, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Headers: Content-Type\r\n\r\n{}",
        status, status_text, body.len(), body
    ).into_bytes()
}

/// Handle a plain HTTP request (not WebSocket).
/// Supports:
///   GET  /health  - server status
///   POST /pair    - exchange pairing code for auth token
///   OPTIONS *     - CORS preflight
async fn handle_http(
    mut tcp_stream: tokio::net::TcpStream,
    peer_addr: SocketAddr,
    registry: Arc<tokio::sync::RwLock<DeviceRegistry>>,
) -> Result<()> {
    let mut buf = vec![0u8; 8192];
    let n = tcp_stream.read(&mut buf).await?;
    let request = String::from_utf8_lossy(&buf[..n]);

    let first_line = request.lines().next().unwrap_or("");
    let (method, path) = {
        let parts: Vec<&str> = first_line.split_whitespace().collect();
        if parts.len() >= 2 {
            (parts[0], parts[1])
        } else {
            ("", "")
        }
    };

    // Strip query params from path for matching
    let path_base = path.split('?').next().unwrap_or(path);

    logging::info(&format!(
        "Gateway HTTP: {} {} from {}",
        method, path_base, peer_addr
    ));

    let response = match (method, path_base) {
        ("GET", "/health") => {
            let body = serde_json::json!({
                "status": "ok",
                "version": jcode_build_meta::VERSION,
                "gateway": true,
            });
            http_response(200, "OK", &body.to_string())
        }

        ("POST", "/pair") => {
            // Extract JSON body (after \r\n\r\n)
            let body_str = request.split("\r\n\r\n").nth(1).unwrap_or("");
            handle_pair_request(body_str, &registry).await
        }

        ("OPTIONS", _) => {
            // CORS preflight
            "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type, Authorization\r\nAccess-Control-Max-Age: 86400\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            .to_string().into_bytes()
        }

        _ => {
            let body = serde_json::json!({"error": "Not found"});
            http_response(404, "Not Found", &body.to_string())
        }
    };

    tcp_stream.write_all(&response).await?;
    tcp_stream.shutdown().await?;
    Ok(())
}

/// Handle POST /pair request.
///
/// Expected JSON body:
/// ```json
/// {
///   "code": "123456",
///   "device_id": "uuid-here",
///   "device_name": "Jeremy's iPhone",
///   "apns_token": "optional-apns-token"
/// }
/// ```
///
/// Returns:
/// ```json
/// {
///   "token": "hex-auth-token",
///   "server_name": "jcode",
///   "server_version": "v0.4.0"
/// }
/// ```
async fn handle_pair_request(
    body: &str,
    registry: &Arc<tokio::sync::RwLock<DeviceRegistry>>,
) -> Vec<u8> {
    #[derive(serde::Deserialize)]
    struct PairRequest {
        code: String,
        device_id: String,
        device_name: String,
        apns_token: Option<String>,
    }

    let req: PairRequest = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => {
            let body = serde_json::json!({"error": format!("Invalid JSON: {}", e)});
            return http_response(400, "Bad Request", &body.to_string());
        }
    };

    let mut reg = registry.write().await;

    // Reload from disk - pairing codes are generated by `jcode pair` CLI
    *reg = DeviceRegistry::load();

    if !reg.validate_code(&req.code) {
        let body = serde_json::json!({"error": "Invalid or expired pairing code"});
        return http_response(401, "Unauthorized", &body.to_string());
    }

    let token = reg.pair_device(
        req.device_id.clone(),
        req.device_name.clone(),
        req.apns_token,
    );

    logging::info(&format!(
        "Gateway: paired device '{}' ({})",
        req.device_name, req.device_id
    ));

    let body = serde_json::json!({
        "token": token,
        "server_name": "jcode",
        "server_version": jcode_build_meta::VERSION,
    });
    http_response(200, "OK", &body.to_string())
}

#[cfg(test)]
#[path = "gateway_tests.rs"]
mod gateway_tests;
