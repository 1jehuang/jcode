// ════════════════════════════════════════════════════════════════
// MCP 协议核心类型定义
// ════════════════════════════════════════════════════════════════

use serde::{Deserialize, Serialize};

/// MCP 协议版本
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

/// JSON-RPC 请求 ID (支持数字和字符串)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcId {
    Num(i64),
    Str(String),
}

impl std::fmt::Display for JsonRpcId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Num(n) => write!(f, "{}", n),
            Self::Str(s) => write!(f, "{}", s),
        }
    }
}

/// JSON-RPC 请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<JsonRpcId>,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// JSON-RPC 响应 (成功)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcSuccessResponse {
    pub jsonrpc: String,
    pub id: Option<JsonRpcId>,
    pub result: serde_json::Value,
}

/// JSON-RPC 响应 (错误)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcErrorResponse {
    pub jsonrpc: String,
    pub id: Option<JsonRpcId>,
    pub error: JsonRpcError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// 统一响应类型
#[derive(Debug, Clone)]
pub enum JsonRpcResponse {
    Success(JsonRpcSuccessResponse),
    Error(JsonRpcErrorResponse),
}

// --- MCP 核心类型 ------------------------------------

/// MCP Server 信息
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerInfo {
    pub name: String,
    pub version: String,
}

/// Server capabilities
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ServerCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourceCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompts: Option<PromptCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logging: Option<LoggingCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolsCapability {
    pub list_changed: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceCapability {
    pub subscribe: Option<bool>,
    pub list_changed: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptCapability {
    pub list_changed: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggingCapability {}

/// Client capabilities
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClientCapabilities {
    #[serde(rename = "experimental", skip_serializing_if = "Option::is_none")]
    pub experimental: Option<serde_json::Value>,
}

/// 初始化结果 (server -> client)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeResult {
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    pub server_info: McpServerInfo,
}

// --- Tool 类型 -------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: serde_json::Value, // JSON Schema
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListToolsResult {
    pub tools: Vec<McpTool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallToolResult {
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    pub is_error: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentBlock {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: Option<String>,
    pub data: Option<serde_json::Value>,
}

// --- Resource 类型 ----------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "mimeType")]
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadResourceResult {
    pub contents: Vec<ResourceContent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceContent {
    pub uri: String,
    #[serde(rename = "mimeType")]
    pub mime_type: Option<String>,
    pub blob: Option<String>, // base64 encoded
    pub text: Option<String>,
}

// --- Sampling 类型 ----------------------------------

/// Include context type for sampling requests
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IncludeContext {
    NoContext,
    ClientOnly,
    ServerOnly,
    All,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SamplingMessage {
    pub role: String,
    pub content: ContentBlock,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateMessageRequest {
    /// Messages so far (role alternates user/assistant)
    pub messages: Vec<SamplingMessage>,
    /// Model preferences
    pub model_preferences: Option<ModelPreferences>,
    /// Include context
    pub include_context: Option<IncludeContext>,
    /// Max tokens
    pub max_tokens: u32,
    /// Stop sequences
    pub stop_sequences: Option<Vec<String>>,
    /// System prompt
    pub system_prompt: Option<String>,
    /// Temperature
    pub temperature: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPreferences {
    pub hints: Option<Vec<String>>,
    pub cost_priority: Option<String>,
    pub speed_priority: Option<String>,
    pub intelligence_priority: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateMessageResult {
    pub role: String,
    pub content: ContentBlock,
    pub model: Option<String>,
    pub stop_reason: Option<String>,
}

// --- Transport 类型 --------------------------------

/// 传输协议类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TransportType {
    Stdio,
    Sse,
    Http,
    WebSocket,
}

impl std::str::FromStr for TransportType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "stdio" => Ok(Self::Stdio),
            "sse" | "streamable-http" => Ok(Self::Sse),
            "http" => Ok(Self::Http),
            "ws" | "websocket" => Ok(Self::WebSocket),
            _ => Err(format!("Unknown transport type: {}", s)),
        }
    }
}

/// 连接状态
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionState {
    Pending,
    Connected { capabilities: ServerCapabilities, server_info: Option<McpServerInfo> },
    Disconnected { reason: String },
    Failed { error: String, retryable: bool },
    Connecting,
    Reconnecting { attempt: u32 },
}

impl ConnectionState {
    pub fn is_connected(&self) -> bool {
        matches!(self, Self::Connected { .. })
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Disconnected { .. })
    }

    pub fn can_reconnect(&self) -> bool {
        matches!(self, Self::Failed { retryable: true, .. })
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Connected { .. } => "connected",
            Self::Disconnected { .. } => "disconnected",
            Self::Failed { .. } => "failed",
            Self::Connecting => "connecting",
            Self::Reconnecting { .. } => "reconnecting",
        }
    }
}
