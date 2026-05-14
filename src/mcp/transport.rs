//! # MCP传输层 - StreamableHTTP & SSE
//!
//! 实现MCP协议的多种传输方式：
//! - **StreamableHTTP** - HTTP长轮询 + 流式响应
//! - **SSE (Server-Sent Events)** - 单向服务器推送
//! - **Stdio** - 标准输入输出（已存在）
//!
//! ## 协议兼容性
//!
//! 符合 MCP 2024-11-05 规范：
//! - Content-Length 帧格式
//! - JSON-RPC 2.0 消息协议
//! - Session管理
//! - 错误处理

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use url::Url;

/// 传输层 trait 定义
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// 发送消息
    async fn send(&self, message: JsonRpcMessage) -> Result<(), TransportError>;
    
    /// 接收消息（阻塞直到有消息）
    async fn receive(&mut self) -> Result<JsonRpcMessage, TransportError>;
    
    /// 关闭连接
    async fn close(&mut self) -> Result<(), TransportError>;
    
    /// 检查是否已连接
    fn is_connected(&self) -> bool;
}

/// JSON-RPC消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcMessage {
    #[serde(rename = "jsonrpc")]
    pub version: String,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    
    #[serde(flatten)]
    pub content: MessageContent,
}

/// 消息内容（Request或Response）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Request { method: String, params: Option<serde_json::Value> },
    Response { result: Option<serde_json::Value>, error: Option<JsonRpcError> },
    Notification { method: String, params: Option<serde_json::Value> },
}

/// JSON-RPC错误
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// 传输错误类型
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("Connection error: {0}")]
    Connection(String),
    
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    
    #[error("Serialization error: {0}")]
    Serialization(String),
    
    #[error("Deserialization error: {0}")]
    Deserialization(String),
    
    #[error("Timeout error")]
    Timeout,
    
    #[error("Not connected")]
    NotConnected,
    
    #[error("HTTP error: {status} - {message}")]
    Http { status: u16, message: String },
}

// ════════════════════════════
// StreamableHTTP 传输实现
// ════════════════════════════

/// StreamableHTTP传输配置
#[derive(Debug, Clone)]
pub struct StreamableHttpConfig {
    /// 服务端点URL
    pub endpoint: Url,
    
    /// 会话ID（可选，首次连接时由服务端分配）
    pub session_id: Option<String>,
    
    /// HTTP客户端
    pub client: Client,
    
    /// 请求超时
    pub request_timeout: std::time::Duration,
    
    /// 轮询超时（长轮询等待时间）
    pub poll_timeout: std::time::Duration,
    
    /// 最大重试次数
    pub max_retries: u32,
    
    /// 自定义headers
    pub headers: Vec<(String, String)>,
}

impl Default for StreamableHttpConfig {
    fn default() -> Self {
        Self {
            endpoint: Url::parse("http://localhost:3000/mcp").unwrap(),
            session_id: None,
            client: Client::new(),
            request_timeout: std::time::Duration::from_secs(30),
            poll_timeout: std::time::Duration::from_secs(60),
            max_retries: 3,
            headers: vec![
                ("Content-Type".to_string(), "application/json".to_string()),
                ("Accept".to_string(), "application/json".to_string()),
                ("Mcp-Version".to_string(), "2024-11-05".to_string()),
            ],
        }
    }
}

/// StreamableHTTP传输实现
pub struct StreamableHttpTransport {
    config: StreamableHttpConfig,
    session_id: Arc<Mutex<Option<String>>>,
    request_counter: Arc<Mutex<u64>>,
    pending_requests: Arc<Mutex<Vec<JsonRpcMessage>>>,
    is_connected_flag: bool,
}

impl StreamableHttpTransport {
    /// 创建新的StreamableHTTP传输实例
    pub fn new(config: StreamableHttpConfig) -> Self {
        Self {
            config,
            session_id: Arc::new(Mutex::new(config.session_id)),
            request_counter: Arc::new(Mutex::new(0)),
            pending_requests: Arc::new(Mutex::new(Vec::new())),
            is_connected_flag: false,
        }
    }

    /// 使用默认配置创建
    pub fn with_endpoint(endpoint: &str) -> Result<Self, url::ParseError> {
        let url = Url::parse(endpoint)?;
        Ok(Self::new(StreamableHttpConfig {
            endpoint: url,
            ..Default::default()
        }))
    }

    /// 初始化会话（发送initialize请求）
    pub async fn initialize(&mut self) -> Result<InitializeResponse, TransportError> {
        let init_request = JsonRpcMessage {
            version: "2.0".to_string(),
            id: Some(serde_json::json!(1)),
            content: MessageContent::Request {
                method: "initialize".to_string(),
                params: Some(serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {
                        "name": "carpai",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                })),
            },
        };

        // 发送初始化请求
        self.send(init_request).await?;

        // 等待响应
        let response = self.receive().await?;

        match response.content {
            MessageContent::Response { result, error } => {
                if let Some(err) = error {
                    Err(TransportError::Serialization(format!(
                        "Initialization failed: {} ({})",
                        err.message, err.code
                    )))
                } else {
                    let init_result: InitializeResponse = serde_json::from_value(result.unwrap_or_default())
                        .map_err(|e| TransportError::Deserialization(e.to_string()))?;
                    
                    // 更新session ID
                    if let Some(session_id) = &init_result.session_id {
                        *self.session_id.lock().await = Some(session_id.clone());
                        self.config.session_id = Some(session_id.clone());
                    }

                    self.is_connected_flag = true;
                    Ok(init_result)
                }
            }
            _ => Err(TransportError::Deserialization(
                "Expected response, got other type".to_string()
            )),
        }
    }

    /// 发送带重试的HTTP请求
    async fn send_http_request(
        &self,
        path: &str,
        body: &JsonRpcMessage,
    ) -> Result<reqwest::Response, TransportError> {
        let mut retries = 0;
        
        loop {
            let mut request = self.config.client
                .post(self.config.endpoint.join(path))
                .timeout(self.config.request_timeout)
                .json(body);

            // 添加自定义headers
            for (key, value) in &self.config.headers {
                request = request.header(key.as_str(), value.as_str());
            }

            // 添加session header
            {
                let session_id = self.session_id.lock().await;
                if let Some(sid) = session_id.as_ref() {
                    request = request.header("Mcp-Session-Id", sid);
                }
            }

            match request.send().await {
                Ok(response) => return Ok(response),
                Err(e) if e.is_timeout() && retries < self.config.max_retries => {
                    retries += 1;
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                }
                Err(e) => return Err(TransportError::Connection(e.to_string())),
            }
        }
    }

    /// 长轮询等待消息
    async fn poll_for_messages(&self) -> Result<Vec<JsonRpcMessage>, TransportError> {
        let mut retries = 0u32;

        loop {
            let mut request = self.config.client
                .get(self.config.endpoint.join("messages"))
                .timeout(self.config.poll_timeout)
                .query(&[("sessionId", self.config.session_id.as_deref().unwrap_or(""))]);

            for (key, value) in &self.config.headers {
                request = request.header(key.as_str(), value.as_str());
            }

            match request.send().await {
                Ok(response) => {
                    match response.status() {
                        reqwest::StatusCode::NO_CONTENT => {
                            // 继续等待
                            retries += 1;
                            if retries > 10 {
                                return Ok(vec![]);
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                            continue;
                        }
                        reqwest::StatusCode::OK => {
                            let messages: Vec<JsonRpcMessage> = response.json().await
                                .map_err(|e| TransportError::Deserialization(e.to_string()))?;
                            return Ok(messages);
                        }
                        status => {
                            let text = response.text().await.unwrap_or_default();
                            return Err(TransportError::Http {
                                status: status.as_u16(),
                                message: text,
                            });
                        }
                    }
                }
                Err(e) if e.is_timeout() => {
                    // 超时是正常的，继续轮询
                    continue;
                }
                Err(e) => {
                    return Err(TransportError::Connection(e.to_string()));
                }
            }
        }
    }
}

#[async_trait]
impl McpTransport for StreamableHttpTransport {
    async fn send(&self, message: JsonRpcMessage) -> Result<(), TransportError> {
        if !self.is_connected() {
            return Err(TransportError::NotConnected);
        }

        // 分发到正确的端点
        let path = match &message.content {
            MessageContent::Notification { .. } => "notification",
            _ => "message",
        };

        let response = self.send_http_request(path, &message).await?;

        if !response.status().is_success() {
            let status = response.status();
            let msg = response.text().await.unwrap_or_default();
            
            if status == reqwest::StatusCode::NOT_FOUND {
                // 尝试使用通用端点
                drop(response);
                let fallback_response = self.send_http_request("", &message).await?;
                
                if !fallback_response.status().is_success() {
                    return Err(TransportError::Http {
                        status: fallback_response.status().as_u16(),
                        message: fallback_response.text().await.unwrap_or_default(),
                    });
                }
            } else {
                return Err(TransportError::Http {
                    status: status.as_u16(),
                    message: msg,
                });
            }
        }

        Ok(())
    }

    async fn receive(&mut self) -> Result<JsonRpcMessage, TransportError> {
        // 先检查本地队列
        {
            let mut queue = self.pending_requests.lock().await;
            if let Some(message) = queue.pop() {
                return Ok(message);
            }
        }

        // 长轮询获取新消息
        let messages = self.poll_for_messages().await?;

        if let Some(first) = messages.into_iter().next() {
            Ok(first)
        } else {
            Err(TransportError::Timeout)
        }
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        // 发送关闭通知
        let close_notification = JsonRpcMessage {
            version: "2.0".to_string(),
            id: None,
            content: MessageContent::Notification {
                method: "notifications/closed".to_string(),
                params: None,
            },
        };

        let _ = self.send(close_notification).await; // 忽略错误

        self.is_connected_flag = false;
        *self.session_id.lock().await = None;

        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.is_connected_flag
    }
}

// ════════════════════════════
// SSE (Server-Sent Events) 传输
// ════════════════════════════

/// SSE传输配置
#[derive(Debug, Clone)]
pub struct SseTransportConfig {
    /// SSE端点URL
    pub endpoint: Url,
    
    /// HTTP客户端
    pub client: Client,
    
    /// 重连间隔
    pub reconnect_interval: std::time::Duration,
    
    /// 最后事件ID（用于断线重连）
    pub last_event_id: Arc<Mutex<Option<String>>>,
}

impl Default for SseTransportConfig {
    fn default() -> Self {
        Self {
            endpoint: Url::parse("http://localhost:3000/sse").unwrap(),
            client: Client::new(),
            reconnect_interval: std::time::Duration::from_secs(5),
            last_event_id: Arc::new(Mutex::new(None)),
        }
    }
}

/// SSE传输实现
pub struct SseTransport {
    config: SseTransportConfig,
    event_source: Arc<Mutex<Option<SseEventSource>>>,
    message_queue: Arc<Mutex<mpsc::UnboundedReceiver<SseEvent>>>,
    shutdown_sender: Arc<Mutex<Option<mpsc::UnboundedSender<()>>>>,
    is_connected_flag: bool,
}

struct SseEventSource {
    task: tokio::task::JoinHandle<()>,
}

/// SSE事件
#[derive(Debug, Clone)]
enum SseEvent {
    Message(JsonRpcMessage),
    Comment(String),
    KeepAlive,
    Error(String),
}

impl SseTransport {
    pub fn new(config: SseTransportConfig) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();

        Self {
            config,
            event_source: Arc::new(Mutex::new(None)),
            message_queue: Arc::new(Mutex::new(rx)),
            shutdown_sender: Arc::new(Mutex::new(Some(tx))),
            is_connected_flag: false,
        }
    }

    pub fn with_endpoint(endpoint: &str) -> Result<Self, url::ParseError> {
        let url = Url::parse(endpoint)?;
        Ok(Self::new(SseTransportConfig {
            endpoint: url,
            ..Default::default()
        }))
    }

    /// 启动SSE连接并开始接收事件
    pub async fn connect(&mut self) -> Result<(), TransportError> {
        let endpoint = self.config.endpoint.clone();
        let client = self.config.client.clone();
        let last_event_id = self.config.last_event_id.clone();
        let queue_tx = {
            let sender = self.shutdown_sender.lock().await.take();
            sender.ok_or_else(|| TransportError::Connection("Already connected".to_string()))?
        };

        // 启动后台任务监听SSE事件
        let task = tokio::spawn(async move {
            Self::sse_listener_loop(endpoint, client, last_event_id, queue_tx).await;
        });

        *self.event_source.lock().await = Some(SseEventSource { task });
        self.is_connected_flag = true;

        Ok(())
    }

    /// SSE监听循环
    async fn sse_listener_loop(
        endpoint: Url,
        client: Client,
        last_event_id: Arc<Mutex<Option<String>>>,
        queue_tx: mpsc::UnboundedSender<SseEvent>,
    ) {
        loop {
            let mut request = client.get(&endpoint);
            
            // 设置Last-Event-ID用于断线重连
            {
                let id = last_event_id.lock().await;
                if let Some(event_id) = id.as_ref() {
                    request = request.header("Last-Event-ID", event_id);
                }
            }

            match request.send().await {
                Ok(response) => {
                    if !response.status().is_success() {
                        let _ = queue_tx.send(SseEvent::Error(format!(
                            "SSE connection error: {}",
                            response.status()
                        )));
                        break;
                    }

                    let stream = match response.bytes_stream() {
                        Ok(s) => s,
                        Err(e) => {
                            let _ = queue_tx.send(SseEvent::Error(e.to_string()));
                            break;
                        }
                    };

                    use futures::StreamExt;
                    let mut lines = stream.lines();

                    while let Some(line) = lines.next().await {
                        match line {
                            Ok(line_content) => {
                                if line_content.starts_with("data: ") {
                                    let data = &line_content[6..];
                                    
                                    if let Ok(msg) = serde_json::from_str::<JsonRpcMessage>(data) {
                                        if queue_tx.send(SseEvent::Message(msg)).is_err() {
                                            break; // 接收端已关闭
                                        }
                                    }
                                } else if line_content.starts_with(": ") {
                                    // 注释
                                    let comment = line_content[2..].trim().to_string();
                                    let _ = queue_tx.send(SseEvent::Comment(comment));
                                } else if line_content == ":" {
                                    // Keep-alive
                                    let _ = queue_tx.send(SseEvent::KeepAlive);
                                } else if let Some(id) = line_content.strip_prefix("id: ") {
                                    // 更新last-event-id
                                    *last_event_id.lock().await = Some(id.to_string());
                                }
                            }
                            Err(e) => {
                                let _ = queue_tx.send(SseEvent::Error(e.to_string()));
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    let _ = queue_tx.send(SseEvent::Error(e.to_string()));
                }
            }

            // 断线后延迟重连
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }
}

#[async_trait]
impl McpTransport for SseTransport {
    async fn send(&self, message: JsonRpcMessage) -> Result<(), TransportError> {
        if !self.is_connected() {
            return Err(TransportError::NotConnected);
        }

        // SSE是单向的，需要通过POST端点发送消息
        let post_endpoint = self.config.endpoint.join("message");
        
        let response = self.config.client
            .post(post_endpoint)
            .json(&message)
            .send()
            .await
            .map_err(|e| TransportError::Connection(e.to_string()))?;

        if !response.status().is_success() {
            return Err(TransportError::Http {
                status: response.status().as_u16(),
                message: response.text().await.unwrap_or_default(),
            });
        }

        Ok(())
    }

    async fn receive(&mut self) -> Result<JsonRpcMessage, TransportError> {
        let mut queue = self.message_queue.lock().await;
        
        loop {
            match queue.recv().await {
                Some(SseEvent::Message(msg)) => return Ok(msg),
                Some(SseEvent::Error(e)) => return Err(TransportError::Connection(e)),
                Some(_) => continue, // 忽略其他事件
                None => return Err(TransportError::NotConnected),
            }
        }
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        // 发送关闭信号
        if let Some(sender) = self.shutdown_sender.lock().await.take() {
            let _ = sender.send(()); // 触发关闭
        }

        // 等待任务结束
        if let Some(source) = self.event_source.lock().await.take() {
            source.task.abort();
        }

        self.is_connected_flag = false;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.is_connected_flag
    }
}

// ════════════════════════════
// 辅助类型和函数
// ════════════════════════════

/// 初始化响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeResponse {
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    pub server_info: ServerInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// 服务器能力
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourcesCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompts: Option<PromptsCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logging: Option<LoggingCapability>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsCapability {
    pub list_changed: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourcesCapability {
    pub subscribe: Option<bool>,
    pub list_changed: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptsCapability {
    pub list_changed: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingCapability {}

/// 服务器信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

/// 创建JSON-RPC请求
pub fn create_request(method: &str, params: Option<serde_json::Value>) -> JsonRpcMessage {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

    JsonRpcMessage {
        version: "2.0".to_string(),
        id: Some(serde_json::json!(COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed))),
        content: MessageContent::Request {
            method: method.to_string(),
            params,
        },
    }
}

/// 创建JSON-RPC通知
pub fn create_notification(method: &str, params: Option<serde_json::Value>) -> JsonRpcMessage {
    JsonRpcMessage {
        version: "2.0".to_string(),
        id: None,
        content: MessageContent::Notification {
            method: method.to_string(),
            params,
        },
    }
}

/// 创建成功响应
pub fn create_success_response(id: serde_json::Value, result: serde_json::Value) -> JsonRpcMessage {
    JsonRpcMessage {
        version: "2.0".to_string(),
        id: Some(id),
        content: MessageContent::Response {
            result: Some(result),
            error: None,
        },
    }
}

/// 创建错误响应
pub fn create_error_response(id: serde_json::Value, code: i32, message: &str) -> JsonRpcMessage {
    JsonRpcMessage {
        version: "2.0".to_string(),
        id: Some(id),
        content: MessageContent::Response {
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.to_string(),
                data: None,
            }),
        },
    }
}

// ════════════════════════════
// 单元测试
// ════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_request() {
        let request = create_request("tools/list", None);
        
        assert_eq!(request.version, "2.0");
        assert!(request.id.is_some());
        
        match &request.content {
            MessageContent::Request { method, params } => {
                assert_eq!(method, "tools/list");
                assert!(params.is_none());
            }
            _ => panic!("Expected Request"),
        }
    }

    #[test]
    fn test_create_notification() {
        let notification = create_notification("notifications/progress", 
            Some(serde_json::json!({"progressToken": "123"}))
        );
        
        assert_eq!(notification.id, None); // 通知没有ID
        
        match &notification.content {
            MessageContent::Notification { method, .. } => {
                assert_eq!(method, "notifications/progress");
            }
            _ => panic!("Expected Notification"),
        }
    }

    #[test]
    fn test_create_success_response() {
        let response = create_success_response(
            serde_json::json!(1),
            serde_json::json!({"tools": []})
        );

        match &response.content {
            MessageContent::Response { result, error } => {
                assert!(result.is_some());
                assert!(error.is_none());
            }
            _ => panic!("Expected Response"),
        }
    }

    #[test]
    fn test_create_error_response() {
        let response = create_error_response(
            serde_json::json!(1),
            -32600,
            "Invalid request"
        );

        match &response.content {
            MessageContent::Response { result, error } => {
                assert!(result.is_none());
                assert!(error.is_some());
                let err = error.as_ref().unwrap();
                assert_eq!(err.code, -32600);
                assert_eq!(err.message, "Invalid request");
            }
            _ => panic!("Expected Response"),
        }
    }

    #[test]
    fn test_streamable_http_config_defaults() {
        let config = StreamableHttpConfig::default();
        
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.request_timeout.as_secs(), 30);
        assert_eq!(config.poll_timeout.as_secs(), 60);
    }

    #[test]
    fn test_streamable_http_creation() {
        let transport = StreamableHttpTransport::with_endpoint("http://localhost:8080/mcp");
        
        assert!(transport.is_ok());
        let transport = transport.unwrap();
        assert!(!transport.is_connected());
    }

    #[test]
    fn test_sse_transport_creation() {
        let transport = SseTransport::with_endpoint("http://localhost:8080/sse");
        
        assert!(transport.is_ok());
        let transport = transport.unwrap();
        assert!(!transport.is_connected());
    }

    #[test]
    fn test_message_serialization() {
        let message = create_request("ping", None);
        let json = serde_json::to_string(&message).expect("Should serialize");
        
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"method\":\"ping\""));
    }

    #[test]
    fn test_initialize_response_parsing() {
        let json = r#"{
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {"listChanged": true}},
            "serverInfo": {"name": "test-server", "version": "1.0.0"},
            "sessionId": "abc123"
        }"#;

        let response: InitializeResponse = serde_json::from_str(json).expect("Should parse");
        
        assert_eq!(response.protocol_version, "2024-11-05");
        assert_eq!(response.server_info.name, "test-server");
        assert_eq!(response.session_id, Some("abc123".to_string()));
    }

    #[tokio::test]
    async fn test_streamable_http_not_connected_error() {
        let config = StreamableHttpConfig::default();
        let transport = StreamableHttpTransport::new(config);

        let result = transport.send(create_request("test", None)).await;
        
        assert!(result.is_err());
        assert!(matches!(result.err().unwrap(), TransportError::NotConnected));
    }
}
