// ════════════════════════════════════════════════════════════════
// MCP 传输层 — 3 种协议实现
//
// 1. StdioTransport: 通过子进程 stdio 通信 (最常用)
// 2. SseTransport: Server-Sent Events 长轮询
// 3. HttpTransport: Streamable HTTP POST
//
// 统一接口: McpTransport trait
// ════════════════════════════════════════════════════════════════

use crate::types::{JsonRpcRequest, JsonRpcResponse, JsonRpcSuccessResponse, JsonRpcErrorResponse, ClientCapabilities};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::RwLock;
use uuid::Uuid;

/// Transport 错误
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Protocol error: {0}")]
    Protocol(String),
    #[error("Connection lost")]
    ConnectionLost,
    #[error("Timeout")]
    Timeout,
}

pub type TransportResult<T> = Result<T, TransportError>;

/// MCP Transport 抽象
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// 发送 JSON-RPC 请求并等待响应
    async fn send(&self, request: JsonRpcRequest) -> TransportResult<JsonRpcResponse>;

    /// 发送通知 (不需要响应)
    async fn notify(&self, request: JsonRpcRequest) -> TransportResult<()>;

    /// 关闭连接
    async fn close(&self) -> TransportResult<()>;

    /// 是否已连接
    fn is_connected(&self) -> bool;
}

// ════════════════════════════════════════════════════════════════
// StdioTransport — 子进程 stdio 通信
// ════════════════════════════════════════════════════════════════

/// Stdio 配置
#[derive(Debug, Clone)]
pub struct StdioConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env_vars: Vec<(String, String)>,
    pub cwd: Option<String>,
}

impl Default for StdioConfig {
    fn default() -> Self {
        Self {
            command: String::new(),
            args: Vec::new(),
            env_vars: Vec::new(),
            cwd: None,
        }
    }
}

#[derive(Debug)]
pub struct StdioTransport {
    config: StdioConfig,
    child: Arc<tokio::sync::Mutex<Option<Child>>>,
    write_tx: Arc<tokio::sync::Mutex<Option<tokio::io::BufWriter<tokio::process::ChildStdin>>>>,
    read_rx: Arc<tokio::sync::Mutex<Option<tokio::io::BufReader<tokio::process::ChildStdout>>>>,
    connected: Arc<std::sync::atomic::AtomicBool>,
}

impl StdioTransport {
    pub fn new(config: StdioConfig) -> Self {
        Self {
            config,
            child: Arc::new(tokio::sync::Mutex::new(None)),
            write_tx: Arc::new(tokio::sync::Mutex::new(None)),
            read_rx: Arc::new(tokio::sync::Mutex::new(None)),
            connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// 启动子进程并建立 stdio 连接
    pub async fn connect(&self) -> TransportResult<()> {
        let mut cmd = Command::new(&self.config.command);
        
        cmd.args(&self.config.args)
           .kill_on_drop(true);

        if let Some(cwd) = &self.config.cwd {
            cmd.current_dir(cwd);
        }

        for (key, val) in &self.config.env_vars {
            cmd.env(key, val);
        }

        // stdin/stdout 使用 pipe
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = cmd.spawn()
            .map_err(|e| TransportError::Protocol(format!("Failed to spawn process '{}': {}", self.config.command, e)))?;

        let stdin = child.stdin.take()
            .ok_or_else(|| TransportError::Protocol("Failed to capture stdin".into()))?;
        let stdout = child.stdout.take()
            .ok_or_else(|| TransportError::Protocol("Failed to capture stdout".into()))?;

        // Store the I/O handles
        {
            let mut write_guard = self.write_tx.lock().await;
            *write_guard = Some(tokio::io::BufWriter::new(stdin));
        }
        {
            let mut read_guard = self.read_rx.lock().await;
            *read_guard = Some(tokio::io::BufReader::new(stdout));
        }
        {
            let mut child_guard = self.child.lock().await;
            *child_guard = Some(child);
        }

        self.connected.store(true, std::sync::atomic::Ordering::SeqCst);

        tracing::info!(
            command = %self.config.command,
            "Stdio transport connected"
        );

        Ok(())
    }

    /// 从 stdout 读取一行 JSON (以 \n 分隔的 JSON-RPC 消息)
    async fn read_response(&self) -> TransportResult<serde_json::Value> {
        let mut reader_guard = self.read_rx.lock().await;
        let reader = reader_guard
            .as_mut()
            .ok_or(TransportError::ConnectionLost)?;

        // Read Content-Length header
        let mut header_line = String::new();
        loop {
            let mut byte = [0u8; 1];
            reader.read_exact(&mut byte).await?;
            let ch = byte[0] as char;
            header_line.push(ch);
            if header_line.ends_with("\r\n\r\n") {
                break;
            }
            if header_line.len() > 4096 {
                return Err(TransportError::Protocol("Header too long".into()));
            }
        }

        // Parse Content-Length
        let content_length: usize = header_line
            .lines()
            .find_map(|line| {
                line.strip_prefix("Content-Length: ")
                    .and_then(|v| v.trim().parse().ok())
            })
            .ok_or_else(|| TransportError::Protocol("Missing Content-Length header".into()))?;

        // Read JSON body
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body).await?;

        let value: serde_json::Value = serde_json::from_slice(&body)?;
        Ok(value)
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn send(&self, request: JsonRpcRequest) -> TransportResult<JsonRpcResponse> {
        if !self.is_connected() {
            return Err(TransportError::ConnectionLost);
        }

        // 序列化请求为 JSON
        let json_str = serde_json::to_string(&request)?;
        let header = format!("Content-Length: {}\r\n\r\n", json_str.len());

        // 写入 stdin
        {
            let mut writer_opt = self.write_tx.lock().await;
            if let Some(writer) = writer_opt.as_mut() {
                writer.write_all(header.as_bytes()).await?;
                writer.write_all(json_str.as_bytes()).await?;
                writer.flush().await?;
            }
        }

        // 读取响应
        let response_value = self.read_response().await?;

        // 解析响应
        if response_value.get("result").is_some() {
            let resp: JsonRpcSuccessResponse = serde_json::from_value(response_value)?;
            Ok(JsonRpcResponse::Success(resp))
        } else if response_value.get("error").is_some() {
            let resp: JsonRpcErrorResponse = serde_json::from_value(response_value)?;
            Ok(JsonRpcResponse::Error(resp))
        } else {
            Err(TransportError::Protocol("Invalid JSON-RPC response".into()))
        }
    }

    async fn notify(&self, request: JsonRpcRequest) -> TransportResult<()> {
        let json_str = serde_json::to_string(&request)?;
        let header = format!("Content-Length: {}\r\n\r\n", json_str.len());

        let mut writer_opt = self.write_tx.lock().await;
        if let Some(writer) = writer_opt.as_mut() {
            writer.write_all(header.as_bytes()).await?;
            writer.write_all(json_str.as_bytes()).await?;
            writer.flush().await?;
        }

        Ok(())
    }

    async fn close(&self) -> TransportResult<()> {
        self.connected.store(false, std::sync::atomic::Ordering::SeqCst);

        let mut child_guard = self.child.lock().await;
        if let Some(mut child) = child_guard.take() {
            // Graceful shutdown sequence: SIGINT → SIGTERM → SIGKILL
            use std::time::Duration;

            // Step 1: Try graceful shutdown
            match child.try_wait()? {
                Some(_) => {} // Already exited
                None => {
                    // Send SIGINT equivalent on Windows / Unix
                    #[cfg(unix)]
                    {
                        use nix::sys::signal::{kill, Signal};
                        use nix::unistd::Pid;
                        let _ = kill(Pid::from_raw(child.id() as i32), Signal::SIGINT);
                    }
                    #[cfg(windows)]
                    {
                        // Windows doesn't have SIGINT; just kill
                    }

                    tokio::time::sleep(Duration::from_millis(crate::PROCESS_GRACEFUL_SHUTDOWN_MS)).await;

                    // Step 2: Force kill if still running
                    if child.try_wait()?.is_none() {
                        child.kill().await?;
                        tokio::time::sleep(Duration::from_millis(crate::PROCESS_FORCE_SHUTDOWN_MS)).await;

                        // Step 3: Final SIGKILL
                        if child.try_wait()?.is_none() {
                            child.kill().await?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Transport 枚举包装器 (用于动态分发)
#[derive(Debug, Clone)]
pub enum TransportEnum {
    Stdio(Arc<StdioTransport>),
    Sse(SseTransport),
    Http(HttpTransport),
}

impl TransportEnum {
    pub async fn connect(&self) -> TransportResult<()> {
        match self {
            Self::Stdio(t) => t.connect().await,
            Self::Sse(t) => { t.connect().await?; Ok(()) }
            Self::Http(t) => { t.initialize(crate::types::ClientCapabilities::default()).await?; Ok(()) }
        }
    }

    pub async fn send(&self, request: JsonRpcRequest) -> TransportResult<JsonRpcResponse> {
        match self {
            Self::Stdio(t) => McpTransport::send(t.as_ref(), request).await,
            Self::Sse(t) => McpTransport::send(t, request).await,
            Self::Http(t) => McpTransport::send(t, request).await,
        }
    }

    pub async fn notify(&self, request: JsonRpcRequest) -> TransportResult<()> {
        match self {
            Self::Stdio(t) => McpTransport::notify(t.as_ref(), request).await,
            Self::Sse(t) => McpTransport::notify(t, request).await,
            Self::Http(t) => McpTransport::notify(t, request).await,
        }
    }

    pub async fn close(&self) -> TransportResult<()> {
        match self {
            Self::Stdio(t) => McpTransport::close(t.as_ref()).await,
            Self::Sse(t) => McpTransport::close(t).await,
            Self::Http(t) => McpTransport::close(t).await,
        }
    }

    fn is_connected(&self) -> bool {
        match self {
            Self::Stdio(t) => McpTransport::is_connected(t.as_ref()),
            Self::Sse(t) => McpTransport::is_connected(t),
            Self::Http(t) => McpTransport::is_connected(t),
        }
    }
}

// ════════════════════════════════════════════════════════════════
// SseTransport — Server-Sent Events
// ════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct SseTransport {
    base_url: String,
    client: reqwest::Client,
    session_id: Arc<tokio::sync::RwLock<Option<String>>>,
    connected: Arc<std::sync::atomic::AtomicBool>,
}

impl SseTransport {
    pub fn new(base_url: String) -> Self {
        Self {
            base_url,
            client: reqwest::Client::new(),
            session_id: Arc::new(RwLock::new(None)),
            connected: Arc::new(false.into()),
        }
    }

    /// 建立 SSE 连接 (GET /sse 并获取 session_id)
    pub async fn connect(&self) -> TransportResult<String> {
        let url = format!("{}/sse", self.base_url.trim_end_matches('/'));

        let response = self.client.get(&url).send().await?;

        if !response.status().is_success() {
            return Err(TransportError::Protocol(format!(
                "SSE connection failed: HTTP {}", response.status()
            )));
        }

        // 从 SSE stream 读取 endpoint URL
        // Format: event: endpoint\ndata: <session_url>\n\n
        let body = response.text().await?;

        // Extract session ID from the body
        // TODO: Parse proper SSE event stream

        let session_id = Uuid::new_v4().to_string(); // fallback

        *self.session_id.write().await = Some(session_id.clone());
        self.connected.store(true, std::sync::atomic::Ordering::SeqCst);

        tracing::info!(url = %url, "SSE transport connected");

        Ok(session_id)
    }

    async fn post_message(&self, method: &str, params: serde_json::Value) -> TransportResult<serde_json::Value> {
        let session_id = self.session_id.read().await;
        let sid = session_id.as_ref()
            .ok_or_else(|| TransportError::Protocol("Not connected (no session ID)".into()))?;

        let url = format!("{}/message?sessionId={}", 
            self.base_url.trim_end_matches('/'), sid);

        let request_body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });

        let response = self.client.post(&url).json(&request_body).send().await?;

        if !response.status().is_success() {
            return Err(TransportError::Protocol(format!(
                "POST /message failed: HTTP {}", response.status()
            )));
        }

        Ok(response.json().await?)
    }
}

#[async_trait]
impl McpTransport for SseTransport {
    async fn send(&self, request: JsonRpcRequest) -> TransportResult<JsonRpcResponse> {
        let value = self.post_message(&request.method, request.params).await?;

        if value.get("result").is_some() {
            Ok(JsonRpcResponse::Success(serde_json::from_value(value)?))
        } else if value.get("error").is_some() {
            Ok(JsonRpcResponse::Error(serde_json::from_value(value)?))
        } else {
            Err(TransportError::Protocol("Invalid response from SSE/HTTP".into()))
        }
    }

    async fn notify(&self, request: JsonRpcRequest) -> TransportResult<()> {
        self.post_message(&request.method, request.params).await?;
        Ok(())
    }

    async fn close(&self) -> TransportResult<()> {
        self.connected.store(false, std::sync::atomic::Ordering::SeqCst);
        *self.session_id.write().await = None;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(std::sync::atomic::Ordering::SeqCst)
    }
}

// ════════════════════════════════════════════════════════════════
// HttpTransport — Streamable HTTP (最新标准)
// ════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct HttpTransport {
    base_url: String,
    client: reqwest::Client,
    session_id: Arc<tokio::sync::RwLock<Option<String>>>,
    connected: Arc<std::sync::atomic::AtomicBool>,
}

impl HttpTransport {
    pub fn new(base_url: String) -> Self {
        Self {
            base_url,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            session_id: Arc::new(RwLock::new(None)),
            connected: Arc::new(false.into()),
        }
    }

    /// 初始化连接 (POST /initialize)
    pub async fn initialize(&self, client_caps: crate::types::ClientCapabilities) -> TransportResult<crate::types::InitializeResult> {
        let url = format!("{}/initialize", self.base_url.trim_end_matches('/'));

        let request = serde_json::json!({
            "protocolVersion": crate::MCP_PROTOCOL_VERSION,
            "capabilities": client_caps,
            "clientInfo": { "name": "jcode-mcp", "version": "0.1.0" },
        });

        let response = self.client.post(&url).json(&request).send().await?;

        if !response.status().is_success() {
            return Err(TransportError::Protocol(format!(
                "Initialize failed: HTTP {}", response.status()
            )));
        }

        let result: crate::types::InitializeResult = response.json().await?;
        self.connected.store(true, std::sync::atomic::Ordering::SeqCst);

        tracing::info!(
            server_name = %result.server_info.name,
            server_version = %result.server_info.version,
            "MCP HTTP transport initialized"
        );

        Ok(result)
    }
}

#[async_trait]
impl McpTransport for HttpTransport {
    async fn send(&self, request: JsonRpcRequest) -> TransportResult<JsonRpcResponse> {
        let url = format!("{}/{}", self.base_url.trim_end_matches('/'), request.method);

        let body = serde_json::to_string(&request)?;

        let response = self.client.post(&url)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await?;

        let value: serde_json::Value = response.json().await?;

        if value.get("result").is_some() {
            Ok(JsonRpcResponse::Success(serde_json::from_value(value)?))
        } else if value.get("error").is_some() {
            Ok(JsonRpcResponse::Error(serde_json::from_value(value)?))
        } else {
            Err(TransportError::Protocol("Invalid response".into()))
        }
    }

    async fn notify(&self, request: JsonRpcRequest) -> TransportResult<()> {
        let url = format!("{}/{}", self.base_url.trim_end_matches('/'), request.method);
        let body = serde_json::to_string(&request)?;

        self.client.post(&url)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await?;

        Ok(())
    }

    async fn close(&self) -> TransportResult<()> {
        self.connected.store(false, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(std::sync::atomic::Ordering::SeqCst)
    }
}
