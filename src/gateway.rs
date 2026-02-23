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
use futures::stream::StreamExt;
use futures::SinkExt;
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

use crate::logging;
use crate::storage;

/// Default gateway port ("jc" on phone keypad = 52, but we use 7643)
pub const DEFAULT_PORT: u16 = 7643;

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
// Device registry (persisted to ~/.jcode/devices.json)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PairedDevice {
    pub id: String,
    pub name: String,
    pub token_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apns_token: Option<String>,
    pub paired_at: String,
    pub last_seen: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DeviceRegistry {
    pub devices: Vec<PairedDevice>,
    #[serde(default)]
    pub pending_codes: Vec<PairingCode>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PairingCode {
    pub code: String,
    pub created_at: String,
    pub expires_at: String,
}

impl DeviceRegistry {
    /// Load from ~/.jcode/devices.json
    pub fn load() -> Self {
        let path = match storage::jcode_dir() {
            Ok(d) => d.join("devices.json"),
            Err(_) => return Self::default(),
        };
        if !path.exists() {
            return Self::default();
        }
        match std::fs::read_to_string(&path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Save to ~/.jcode/devices.json
    pub fn save(&self) -> Result<()> {
        let path = storage::jcode_dir()?.join("devices.json");
        let contents = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, contents)?;
        Ok(())
    }

    /// Generate a 6-digit pairing code, valid for 5 minutes
    pub fn generate_pairing_code(&mut self) -> String {
        use rand::Rng;
        let code: String = format!("{:06}", rand::rng().random_range(0..1_000_000u32));
        let now = chrono::Utc::now();
        let expires = now + chrono::Duration::minutes(5);

        // Clean up expired codes
        let now_str = now.to_rfc3339();
        self.pending_codes.retain(|c| c.expires_at > now_str);

        self.pending_codes.push(PairingCode {
            code: code.clone(),
            created_at: now.to_rfc3339(),
            expires_at: expires.to_rfc3339(),
        });

        let _ = self.save();
        code
    }

    /// Validate a pairing code and consume it. Returns true if valid.
    pub fn validate_code(&mut self, code: &str) -> bool {
        let now = chrono::Utc::now().to_rfc3339();
        if let Some(idx) = self
            .pending_codes
            .iter()
            .position(|c| c.code == code && c.expires_at > now)
        {
            self.pending_codes.remove(idx);
            let _ = self.save();
            true
        } else {
            false
        }
    }

    /// Register a new paired device. Returns the auth token.
    pub fn pair_device(
        &mut self,
        device_id: String,
        device_name: String,
        apns_token: Option<String>,
    ) -> String {
        use rand::Rng;
        // Generate a random auth token
        let token_bytes: [u8; 32] = rand::rng().random();
        let token = hex::encode(token_bytes);

        // Store hash of token, not the token itself
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        let token_hash = format!("sha256:{}", hex::encode(hasher.finalize()));

        let now = chrono::Utc::now().to_rfc3339();

        // Remove existing device with same ID (re-pairing)
        self.devices.retain(|d| d.id != device_id);

        self.devices.push(PairedDevice {
            id: device_id,
            name: device_name,
            apns_token,
            token_hash,
            paired_at: now.clone(),
            last_seen: now,
        });

        let _ = self.save();
        token
    }

    /// Validate an auth token. Returns the device if valid.
    pub fn validate_token(&self, token: &str) -> Option<&PairedDevice> {
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        let token_hash = format!("sha256:{}", hex::encode(hasher.finalize()));

        self.devices.iter().find(|d| d.token_hash == token_hash)
    }

    /// Update last_seen for a device
    pub fn touch_device(&mut self, token: &str) {
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        let token_hash = format!("sha256:{}", hex::encode(hasher.finalize()));
        let now = chrono::Utc::now().to_rfc3339();

        if let Some(device) = self.devices.iter_mut().find(|d| d.token_hash == token_hash) {
            device.last_seen = now;
            let _ = self.save();
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
    mut tcp_stream: tokio::net::TcpStream,
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
async fn handle_ws_connection(
    tcp_stream: tokio::net::TcpStream,
    peer_addr: SocketAddr,
    registry: Arc<tokio::sync::RwLock<DeviceRegistry>>,
    client_tx: tokio::sync::mpsc::UnboundedSender<GatewayClient>,
) -> Result<()> {
    // Perform WebSocket handshake with a callback to inspect headers
    let auth_token = Arc::new(tokio::sync::Mutex::new(None::<String>));
    let auth_token_cb = Arc::clone(&auth_token);

    let ws_stream = tokio_tungstenite::accept_hdr_async(
        tcp_stream,
        |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
         response: tokio_tungstenite::tungstenite::handshake::server::Response| {
            // Extract Bearer token from Authorization header or query param
            if let Some(auth) = request.headers().get("authorization") {
                if let Ok(auth_str) = auth.to_str() {
                    if let Some(token) = auth_str.strip_prefix("Bearer ") {
                        *auth_token_cb.blocking_lock() = Some(token.to_string());
                    }
                }
            }
            // Also check ?token= query parameter
            if auth_token_cb.blocking_lock().is_none() {
                let uri = request.uri().to_string();
                if let Some(query) = uri.split('?').nth(1) {
                    for param in query.split('&') {
                        if let Some(token) = param.strip_prefix("token=") {
                            *auth_token_cb.blocking_lock() = Some(token.to_string());
                        }
                    }
                }
            }
            Ok(response)
        },
    )
    .await?;

    // Validate auth token
    let token = auth_token
        .lock()
        .await
        .take()
        .ok_or_else(|| anyhow::anyhow!("No auth token provided"))?;

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
                        if sink.send(Message::Text(trimmed.into())).await.is_err() {
                            break;
                        }
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Wait for either direction to finish
    tokio::select! {
        _ = ws_to_unix => {}
        _ = unix_to_ws => {}
    }

    logging::info(&format!("Gateway: {} disconnected", device_name));
    Ok(())
}

// ---------------------------------------------------------------------------
// HTTP handler for POST /pair and GET /health
// ---------------------------------------------------------------------------

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
                "version": env!("JCODE_VERSION"),
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
            format!(
                "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type, Authorization\r\nAccess-Control-Max-Age: 86400\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            ).into_bytes()
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
        "server_version": env!("JCODE_VERSION"),
    });
    http_response(200, "OK", &body.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_registry_pairing() {
        let mut registry = DeviceRegistry::default();

        // Generate pairing code
        let code = registry.generate_pairing_code();
        assert_eq!(code.len(), 6);
        assert_eq!(registry.pending_codes.len(), 1);

        // Validate correct code
        assert!(registry.validate_code(&code));
        assert_eq!(registry.pending_codes.len(), 0); // consumed

        // Validate again should fail (consumed)
        assert!(!registry.validate_code(&code));
    }

    #[test]
    fn test_device_registry_token_auth() {
        let mut registry = DeviceRegistry::default();

        // Pair a device
        let token =
            registry.pair_device("test-device-1".to_string(), "Test iPhone".to_string(), None);

        // Validate correct token
        assert!(registry.validate_token(&token).is_some());
        let device = registry.validate_token(&token).unwrap();
        assert_eq!(device.name, "Test iPhone");
        assert_eq!(device.id, "test-device-1");

        // Validate wrong token
        assert!(registry.validate_token("wrong-token").is_none());

        // Token hash should be stored, not raw token
        assert!(registry.devices[0].token_hash.starts_with("sha256:"));
    }

    #[test]
    fn test_device_re_pairing() {
        let mut registry = DeviceRegistry::default();

        // Pair same device twice
        let token1 = registry.pair_device("device-1".to_string(), "iPhone v1".to_string(), None);
        let token2 = registry.pair_device("device-1".to_string(), "iPhone v2".to_string(), None);

        // Only one device entry (old one replaced)
        assert_eq!(registry.devices.len(), 1);
        assert_eq!(registry.devices[0].name, "iPhone v2");

        // Old token should be invalid
        assert!(registry.validate_token(&token1).is_none());
        // New token should be valid
        assert!(registry.validate_token(&token2).is_some());
    }
}
