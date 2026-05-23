//! API 数据模型定义
//!
//! 包含所有 HTTP API 的请求和响应结构体，用于序列化和反序列化。

use serde::{Deserialize, Serialize};

// ============================================================================
// OpenAI 兼容 API 模型 (/v1/*)
// ============================================================================

/// OpenAI 兼容的 Chat Completion 请求
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatRequest {
    pub model: String,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub stream: Option<bool>,
    pub top_p: Option<f64>,
}

/// Chat 消息结构
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: Option<String>,
}

/// FIM (Fill-In-the-Middle) 补全请求
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FIMRequest {
    pub model: String,
    pub prompt: String,      // 光标前的代码
    pub suffix: String,      // 光标后的代码
    pub max_tokens: Option<u32>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub stop: Option<Vec<String>>,
}

/// OpenAI 兼容的 Chat Completion 响应
#[derive(Debug, Clone, Serialize)]
pub struct ChatResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: UsageInfo,
}

/// Chat 响应中的选择项
#[derive(Debug, Clone, Serialize)]
pub struct Choice {
    pub index: u32,
    pub message: ResponseMessage,
    pub finish_reason: Option<String>,
}

/// Chat 响应中的消息
#[derive(Debug, Clone, Serialize)]
pub struct ResponseMessage {
    pub role: String,
    pub content: Option<String>,
}

/// Token 用量信息
#[derive(Debug, Clone, Serialize)]
pub struct UsageInfo {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// 模型列表响应
#[derive(Debug, Clone, Serialize)]
pub struct ModelListResponse {
    pub object: String,
    pub data: Vec<serde_json::Value>,
}

/// 健康检查响应
#[derive(Debug, Clone, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub models: usize,
    pub nodes: u32,
}

// ============================================================================
// 管理后台 API 模型 (/admin/*)
// ============================================================================

/// 登录请求
#[derive(Debug, Clone, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

/// 登录响应
#[derive(Debug, Clone, Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub token_type: String,
    pub expires_in: u64,
}

/// 注册请求
#[derive(Debug, Clone, Deserialize)]
pub struct RegisterRequest {
    pub org_name: String,
    pub admin_email: String,
    pub admin_password: String,
    pub plan: Option<String>, // "free" | "enterprise"
}

/// 注册响应
#[derive(Debug, Clone, Serialize)]
pub struct RegisterResponse {
    pub org_id: String,
    pub message: String,
}

/// 创建用户请求
#[derive(Debug, Clone, Deserialize)]
pub struct CreateUserRequest {
    pub email: String,
    pub name: String,
    pub password: String,
    pub role: String,
}

/// 用户信息响应
#[derive(Debug, Clone, Serialize)]
pub struct UserInfo {
    pub id: String,
    pub email: String,
    pub name: String,
    pub role: String,
    pub is_active: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// API Key 生成响应
#[derive(Debug, Clone, Serialize)]
pub struct ApiKeyResponse {
    pub api_key: String,
    pub message: String,
}

/// 节点信息响应
#[derive(Debug, Clone, Serialize)]
pub struct NodeInfo {
    pub node_id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub node_type: String,
    pub memory_gb: f64,
    pub available_memory_gb: f64,
    pub cpu_cores: u32,
    pub has_gpu: bool,
    pub status: String,
    pub last_heartbeat: i64,
}

/// 组织信息响应
#[derive(Debug, Clone, Serialize)]
pub struct OrgInfo {
    pub name: String,
    pub plan: String,
}

/// 系统指标响应
#[derive(Debug, Clone, Serialize)]
pub struct MetricsResponse {
    pub nodes: serde_json::Value,
    pub models: usize,
    pub uptime_seconds: u64,
}

/// 通用错误响应
#[derive(Debug, Clone, Serialize)]
pub struct ErrorResponse {
    pub error: ErrorDetail,
}

/// 错误详情
#[derive(Debug, Clone, Serialize)]
pub struct ErrorDetail {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: String,
}

/// 通用成功消息响应
#[derive(Debug, Clone, Serialize)]
pub struct MessageResponse {
    pub message: String,
}
