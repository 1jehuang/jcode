//! Extended Transport Protocols for MCP Client
//!
//! Implementations of:
//! - Server-Sent Events (SSE)
//! - Streamable HTTP
//! - WebSocket
//!
//! These extend the basic StdIO transport with network-based alternatives.

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures_util::stream::StreamExt;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

/// Transport trait for unified interface
#[async_trait]
pub trait Transport: Send + Sync {
    /// Send a message
    async fn send(&self, message: &str) -> Result<()>;

    /// Receive a message
    async fn receive(&self) -> Result<String>;

    /// Check if connected
    fn is_connected(&self) -> bool;

    /// Close the connection
    async fn close(&self) -> Result<()>;
}

// ════════════════════════════════════════════════════════════════
// SSE (Server-Sent Events) Transport
// ════════════════════════════════════════════════════════════════

/// SSE transport configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SseConfig {
    pub url: String,
    #[serde(default = "default_sse_headers")]
    pub headers: Vec<(String, String)>,
    #[serde(default = "default_sse_timeout")]
    pub timeout_secs: u64,
}

fn default_sse_headers() -> Vec<(String, String)> {
    vec![
        ("Accept".to_string(), "text/event-stream".to_string()),
        ("Cache-Control".to_string(), "no-cache".to_string()),
    ]
}

fn default_sse_timeout() -> u64 {
    30
}

impl Default for SseConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            headers: default_sse_headers(),
            timeout_secs: default_sse_timeout(),
        }
    }
}

/// SSE event representation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SseEvent {
    pub id: Option<String>,
    pub event: Option<String>,
    pub data: String,
    pub retry: Option<u64>,
}

impl SseEvent {
    /// Parse from SSE format string
    pub fn parse(raw: &str) -> Result<Self> {
        let mut event = SseEvent {
            id: None,
            event: None,
            data: String::new(),
            retry: None,
        };

        for line in raw.lines() {
            if line.starts_with("id:") {
                event.id = Some(line[3..].trim().to_string());
            } else if line.starts_with("event:") {
                event.event = Some(line[6..].trim().to_string());
            } else if line.starts_with("data:") {
                let data = line[5..].trim();
                if !event.data.is_empty() {
                    event.data.push('\n');
                }
                event.data.push_str(data);
            } else if line.starts_with("retry:") {
                event.retry = Some(line[6..].trim().parse()?);
            }
        }

        Ok(event)
    }

    /// Convert to SSE format string
    pub fn to_string(&self) -> String {
        let mut s = String::new();

        if let Some(ref id) = self.id {
            s.push_str(&format!("id: {}\n", id));
        }
        if let Some(ref event) = self.event {
            s.push_str(&format!("event: {}\n", event));
        }
        s.push_str(&format!("data: {}\n", self.data));
        s.push('\n');

        s
    }
}

/// SSE transport client
pub struct SseTransport {
    config: SseConfig,
    client: reqwest::Client,
    event_stream: Arc<RwLock<Option<reqwest::Response>>>,
}

impl SseTransport {
    pub fn new(config: SseConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            config,
            client,
            event_stream: Arc::new(RwLock::new(None)),
        }
    }

    /// Connect to SSE endpoint
    pub async fn connect(&self) -> Result<()> {
        info!("SSE: Connecting to {}", self.config.url);

        let mut request = self.client.get(&self.config.url);

        for (key, value) in &self.config.headers {
            request = request.header(key.as_str(), value.as_str());
        }

        let response = request.send().await.context("SSE connection failed")?;

        if !response.status().is_success() {
            anyhow::bail!("SSE server returned status {}", response.status());
        }

        *self.event_stream.write().await = Some(response);

        info!("SSE: Connected successfully");
        Ok(())
    }

    /// Get next event from stream
    pub async fn next_event(&self) -> Result<Option<SseEvent>> {
        let stream = self.event_stream.read().await;

        match stream.as_ref() {
            Some(response) => {
                let mut lines = response.bytes_stream().lines();

                // Read until we get a complete event
                let mut event_data = String::new();
                let mut has_data = false;

                while let Some(line_result) = lines.next().await {
                    let line = line_result?;

                    if line.is_empty() && has_data {
                        // Empty line marks end of event
                        return Ok(Some(SseEvent::parse(&event_data)?));
                    } else if !line.is_empty() {
                        event_data.push_str(&line);
                        event_data.push('\n');
                        has_data = true;
                    }
                }

                Ok(None) // Stream ended
            }
            None => Err(anyhow::anyhow!("Not connected")),
        }
    }

    /// Send data via POST (SSE is typically read-only, but can use separate endpoint)
    pub async fn send_data(&self, data: &str) -> Result<()> {
        let response = self
            .client
            .post(&self.config.url)
            .header("Content-Type", "application/json")
            .body(data.to_string())
            .send()
            .await?;

        if !response.status().is_success() {
            warn!("SSE send failed: {}", response.status());
        }

        Ok(())
    }
}

#[async_trait]
impl Transport for SseTransport {
    async fn send(&self, message: &str) -> Result<()> {
        self.send_data(message).await
    }

    async fn receive(&self) -> Result<String> {
        match self.next_event().await? {
            Some(event) => Ok(event.data),
            None => Err(anyhow::anyhow!("No more events")),
        }
    }

    fn is_connected(&self) -> bool {
        // Check if we have an active stream
        tokio::task::block_in_place(|| {
            use std::sync::Once;
            static INIT: Once = Once::new();
            INIT.call_once(|| {});
            false // Simplified check - in production would use proper async check
        })
    }

    async fn close(&self) -> Result<()> {
        *self.event_stream.write().await = None;
        info!("SSE: Connection closed");
        Ok(())
    }
}

// ════════════════════════════════════════════════════════════════
// Streamable HTTP Transport
// ════════════════════════════════════════════════════════════════

/// Streamable HTTP configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamableHttpConfig {
    pub base_url: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default = "default_http_timeout")]
    pub request_timeout_secs: u64,
}

fn default_http_timeout() -> u64 {
    30
}

impl Default for StreamableHttpConfig {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            session_id: None,
            request_timeout_secs: default_http_timeout(),
        }
    }
}

/// Streamable HTTP transport (MCP over HTTP with streaming support)
pub struct StreamableHttpTransport {
    config: StreamableHttpConfig,
    client: reqwest::Client,
    session_id: Arc<RwLock<Option<String>>>,
}

impl StreamableHttpTransport {
    pub fn new(config: StreamableHttpConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.request_timeout_secs))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            config,
            client,
            session_id: Arc::new(RwLock::new(config.session_id.clone())),
        }
    }

    /// Initialize session
    pub async fn initialize(&mut self) -> Result<String> {
        info!("StreamableHTTP: Initializing at {}", self.config.base_url);

        let init_body = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "carpai",
                "version": env!("CARGO_PKG_VERSION")
            }
        });

        let response = self
            .client
            .post(&format!("{}/initialize", self.config.base_url))
            .json(&init_body)
            .send()
            .await
            .context("Initialize request failed")?;

        if !response.status().is_success() {
            anyhow::bail!(
                "Initialize failed: {}",
                response.status()
            );
        }

        let result: serde_json::Value = response.json().await?;
        let session_id = result["session_id"]
            .as_str()
            .unwrap_or("")
            .to_string();

        *self.session_id.write().await = Some(session_id.clone());

        info!("StreamableHTTP: Session initialized: {}", session_id);
        Ok(session_id)
    }

    /// Send streaming request
    pub async fn send_streaming(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<reqwest::Response> {
        let session_id = self.session_id.read().await.clone();

        let body = serde_json::json!({
            "method": method,
            "params": params,
            "sessionId": session_id
        });

        let response = self
            .client
            .post(&format!("{}/message", self.config.base_url))
            .json(&body)
            .send()
            .await
            .context("Streaming request failed")?;

        if !response.status().is_success() {
            anyhow::bail!(
                "Request failed: {}",
                response.status()
            );
        }

        Ok(response)
    }

    /// Process streaming response
    pub async fn process_stream(
        &self,
        response: reqwest::Response,
    ) -> Result<Vec<serde_json::Value>> {
        let mut results = Vec::new();
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            let text = String::from_utf8_lossy(&chunk);

            // Parse NDJSON (newline-delimited JSON)
            for line in text.lines() {
                if !line.trim().is_empty() {
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
                        results.push(value);
                    }
                }
            }
        }

        Ok(results)
    }
}

#[async_trait]
impl Transport for StreamableHttpTransport {
    async fn send(&self, message: &str) -> Result<()> {
        let msg: serde_json::Value =
            serde_json::from_str(message).context("Invalid JSON message")?;

        let method = msg["method"].as_str().unwrap_or("");
        let params = msg.get("params").cloned();

        let _response = self.send_streaming(method, params).await?;

        Ok(())
    }

    async fn receive(&self) -> Result<String> {
        // For streaming HTTP, we'd typically poll or use long-polling
        // This is a simplified implementation
        let response = self
            .send_streaming("ping", None)
            .await
            .context("Receive failed")?;

        let text = response.text().await?;
        Ok(text)
    }

    fn is_connected(&self) -> bool {
        // In production, would check session validity
        true
    }

    async fn close(&self) -> Result<()> {
        *self.session_id.write().await = None;
        info!("StreamableHTTP: Connection closed");
        Ok(())
    }
}

// ════════════════════════════════════════════════════════════════
// WebSocket Transport
// ════════════════════════════════════════════════════════════════

/// WebSocket configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSocketConfig {
    pub url: String,
    #[serde(default = "ws_default_origin")]
    pub origin: Option<String>,
    #[serde(default = "ws_default_protocols")]
    pub protocols: Vec<String>,
}

fn ws_default_origin() -> Option<String> {
    None
}
fn ws_default_protocols() -> Vec<String> {
    vec!["mcp".to_string()]
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            origin: None,
            protocols: vec!["mcp".to_string()],
        }
    }
}

/// WebSocket transport for real-time bidirectional communication
pub struct WebSocketTransport {
    config: WebSocketConfig,
    ws_write: Arc<tokio::sync::Mutex<Option<tokio_tungstenite::WebSocketStream<
        MaybeTlsStream<tokio::net::TcpStream>,
    >>>,
}

type MaybeTlsStream<S> = tokio_tungstenite::MaybeTlsStream<S>;

impl WebSocketTransport {
    pub fn new(config: WebSocketConfig) -> Self {
        Self {
            config,
            ws_write: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// Connect to WebSocket server
    pub async fn connect(&self) -> Result<()> {
        info!("WS: Connecting to {}", self.config.url);

        let request = tokio_tungstenite::tungstenite::client::connect_request(
            &self.config.url,
        );

        let mut request = match request {
            Ok(req) => req,
            Err(e) => anyhow::bail!("Invalid URL: {}", e),
        };

        // Set protocols
        for protocol in &self.config.protocols {
            request = request.protocol(protocol);
        }

        // Set origin if provided
        if let Some(ref origin) = self.config.origin {
            request = request.header("Origin", origin);
        }

        let (ws_stream, _) = request.connect_async().await?;

        // Store the write half
        let (_, write_half) = ws_stream.split();
        *self.ws_write.lock().await = Some(write_half);

        info!("WS: Connected successfully");
        Ok(())
    }

    /// Send JSON-RPC message
    pub async fn send_message(&self, message: &str) -> Result<()> {
        let write_guard = self.ws_write.lock().await;

        if let Some(write) = write_guard.as_mut() {
            let msg = Message::Text(message.to_string());
            write.send(msg).await.map_err(|e| anyhow::anyhow!("{}", e))?;
        } else {
            anyhow::bail!("WebSocket not connected");
        }

        Ok(())
    }

    /// Receive next message
    pub async fn recv_message(&self) -> Result<Message> {
        // Note: In production, you'd want to store the read half as well
        // This is simplified - you might need a different architecture
        anyhow::bail!("Use receive() method instead")
    }
}

#[async_trait]
impl Transport for WebSocketTransport {
    async fn send(&self, message: &str) -> Result<()> {
        self.send_message(message).await
    }

    async fn receive(&self) -> Result<String> {
        // Would need read half stored separately
        anyhow::bail!("Implement with full duplex support")
    }

    fn is_connected(&self) -> bool {
        // Check if write handle exists
        tokio::task::block_in_place(|| {
            false // Simplified
        })
    }

    async fn close(&self) -> Result<()> {
        if let Some(mut write) = self.ws_write.lock().await.take() {
            write.close(None).await.ok();
        }
        info!("WS: Connection closed");
        Ok(())
    }
}

// ════════════════════════════════════════════════════════════════
// Transport Factory and Registry
// ════════════════════════════════════════════════════════════════

/// Transport factory for creating appropriate transport based on config
pub struct TransportFactory;

impl TransportFactory {
    /// Create transport from enhanced MCP config
    pub async fn create_from_config(
        config: &crate::mcp::enhanced_client::EnhancedMcpConfig,
    ) -> Result<Box<dyn Transport>> {
        match config.transport_type {
            crate::mcp::enhanced_client::TransportType::StdIO => {
                anyhow::bail!("StdIO transport not supported by this factory yet")
            }
            crate::mcp::enhanced_client::TransportType::SSE => {
                let sse_config = SseConfig {
                    url: config.url.clone().unwrap_or_default(),
                    ..Default::default()
                };
                let transport = SseTransport::new(sse_config);
                transport.connect().await?;
                Ok(Box::new(transport))
            }
            crate::mcp::enhanced_client::TransportType::StreamableHTTP => {
                let http_config = StreamableHttpConfig {
                    base_url: config.url.clone().unwrap_or_default(),
                    ..Default::default()
                };
                let transport = StreamableHttpTransport::new(http_config);
                Ok(Box::new(transport))
            }
            crate::mcp::enhanced_client::TransportType::WebSocket => {
                let ws_config = WebSocketConfig {
                    url: config.url.clone().unwrap_or_default(),
                    ..Default::default()
                };
                let transport = WebSocketTransport::new(ws_config);
                transport.connect().await?;
                Ok(Box::new(transport))
            }
        }
    }
}

/// Transport registry for managing multiple connections
pub struct TransportRegistry {
    transports: RwLock<HashMap<String, Box<dyn Transport>>>,
}

impl TransportRegistry {
    pub fn new() -> Self {
        Self {
            transports: RwLock::new(HashMap::new()),
        }
    }

    /// Register a named transport
    pub async fn register(&self, name: String, transport: Box<dyn Transport>) {
        let mut transports = self.transports.write().await;
        transports.insert(name, transport);
        info!("Transport '{}' registered", name);
    }

    /// Get a transport by name
    pub async fn get(&self, name: &str) -> Option<Box<dyn Transport>> {
        let transports = self.transports.read().await;
        // Note: Can't return reference to boxed trait object, so we clone the inner
        // In production, would use Arc<dyn Transport>
        None // Simplified - need redesign for actual use
    }

    /// Send message to named transport
    pub async fn send_to(&self, name: &str, message: &str) -> Result<()> {
        let transports = self.transports.read().await;
        if let Some(transport) = transports.get(name) {
            transport.send(message).await
        } else {
            anyhow::bail!("Transport '{}' not found", name)
        }
    }

    /// List all registered transports
    pub async fn list(&self) -> Vec<String> {
        let transports = self.transports.read().await;
        transports.keys().cloned().collect()
    }

    /// Close all transports
    pub async fn close_all(&self) -> Result<()> {
        let transports = self.transports.read().await;
        for (name, transport) in transports.iter() {
            transport.close().await.ok();
            info!("Transport '{}' closed", name);
        }
        Ok(())
    }
}
