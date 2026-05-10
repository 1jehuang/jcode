//! 核心类型定义 - 移植自 Claude Code 远程调试协议

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

// ============================================================================
// 权限协议类型 - 对应 Claude Code SDK Control Types
// ============================================================================

/// 远程控制请求 (CCR → Client)
/// 移植自 Claude Code: `SDKControlRequest`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SdkControlRequest {
    /// 控制请求 (工具使用权限等)
    #[serde(rename = "control_request")]
    Request {
        request_id: Uuid,
        request: SdkControlRequestBody,
    },

    /// 取消挂起的请求
    #[serde(rename = "control_cancel_request")]
    CancelRequest {
        request_id: Uuid,
    },
}

/// 控制请求体
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkControlRequestBody {
    /// 子类型标识
    pub subtype: String,

    /// 额外字段 (tool_name 等)
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl SdkControlRequestBody {
    /// 创建工具使用权限请求
    pub fn tool_permission(tool_name: &str, extra_fields: Option<HashMap<String, serde_json::Value>>) -> Self {
        let mut body = Self {
            subtype: "can_use_tool".to_string(),
            extra: extra_fields.unwrap_or_default(),
        };
        body.extra.insert("tool_name".to_string(), serde_json::json!(tool_name));
        body
    }

    /// 创建文件读取权限请求
    pub fn file_read_permission(file_path: &str) -> Self {
        let mut body = Self {
            subtype: "can_read_file".to_string(),
            extra: HashMap::new(),
        };
        body.extra.insert("file_path".to_string(), serde_json::json!(file_path));
        body
    }
}

/// 远程控制响应 (Client → CCR)
/// 移植自 Claude Code: `SDKControlResponse`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SdkControlResponse {
    /// 控制响应
    #[serde(rename = "control_response")]
    Response { response: SdkControlResponseBody },
}

/// 控制响应体
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkControlResponseBody {
    /// 子类型 ("success" 或其他)
    pub subtype: String,

    /// 关联的原始请求 ID
    pub request_id: Uuid,

    /// 具体的权限行为结果
    pub response: PermissionBehavior,
}

/// 权限行为枚举
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "behavior")]
pub enum PermissionBehavior {
    /// 允许操作 (可选附带更新后的输入)
    Allow {
        #[serde(skip_serializing_if = "Option::is_none")]
        updated_input: Option<HashMap<String, serde_json::Value>>,
    },

    /// 拒绝操作 (附带原因)
    Deny {
        message: String,
    },
}

// ============================================================================
// 远程会话相关类型
// ============================================================================

/// 简化版远程权限响应 (供 UI 使用)
/// 对应 Claude Code `RemotePermissionResponse`
#[derive(Debug, Clone)]
pub enum RemotePermissionResponse {
    /// 允许
    Allow {
        updated_input: HashMap<String, serde_json::Value>,
    },

    /// 拒绝
    Deny {
        message: String,
    },
}

/// 远程会话配置
/// 对应 Claude Code `RemoteSessionConfig`
#[derive(Debug, Clone)]
pub struct RemoteSessionConfig {
    /// 会话 ID
    pub session_id: String,

    /// 获取访问令牌的回调
    pub get_access_token: Box<dyn Fn() -> String + Send + Sync>,

    /// 组织 UUID
    pub org_uuid: String,

    /// 是否有初始提示正在处理
    pub has_initial_prompt: bool,

    /// 是否为纯观察者模式 (Ctrl+C 不发送中断, 60s重连超时禁用)
    pub viewer_only: bool,
}

/// 远程会话回调集合
/// 对应 Claude Code `RemoteSessionCallbacks`
#[derive(Default)]
pub struct RemoteSessionCallbacks {
    /// 收到 SDK 消息时调用
    pub on_message: Option<Box<dyn Fn(serde_json::Value) + Send + Sync>>,

    /// 收到权限请求时调用
    pub on_permission_request: Option<Box<dyn Fn(SdkControlPermissionRequest, Uuid) + Send + Sync>>,

    /// 挂起的请求被取消时调用
    pub on_permission_cancelled: Option<Box<dyn Fn(Uid, Option<String>) + Send + Sync>>,

    /// 连接建立时调用
    pub on_connected: Option<Box<dyn Fn() + Send + Sync>>,

    /// 连接丢失且无法恢复时调用
    pub on_disconnected: Option<Box<dyn Fn() + Send + Sync>>,

    /// 重连进行中时调用
    pub on_reconnecting: Option<Box<dyn Fn() + Send + Sync>>,

    /// 发生错误时调用
    pub on_error: Option<Box<dyn Fn(anyhow::Error) + Send + Sync>>,

    /// 会话标题更新时调用
    pub on_session_title_update: Option<Box<dyn Fn(String) + Send + Sync>>,
}

/// 权限请求信息 (内部使用)
pub struct SdkControlPermissionRequest {
    pub request: SdkControlRequestBody,
    pub request_id: Uuid,
}

/// 权限请求信息 (对外展示)
pub struct PermissionRequestInfo {
    /// 工具名称
    pub tool_name: Option<String>,

    /// 文件路径
    pub file_path: Option<String>,

    /// 请求 ID
    pub request_id: Uuid,

    /// 发起时间
    pub requested_at: chrono::DateTime<chrono::Utc>,

    /// 是否已超时
    pub timed_out: bool,
}

// ============================================================================
// 远程会话状态机
// ============================================================================

/// 远程会话状态
/// 对应 Claude Code 中连接状态的各种枚举
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteSessionState {
    /// 未连接
    Disconnected,

    /// 正在连接
    Connecting,

    /// 已连接 (活跃)
    Connected,

    /// 正在重连
    Reconnecting {
        attempt: u32,
        max_attempts: u32,
        reason: String,
    },

    /// 断开中 (用户主动断开或错误)
    Disconnecting(String),

    /// 错误状态
    Error(String),
}

impl std::fmt::Display for RemoteSessionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disconnected => write!(f, "未连接"),
            Self::Connecting => write!(f, "正在连接..."),
            Self::Connected => write!(f, "已连接"),
            Self::Reconnecting { attempt, max_attempts, .. } => {
                write!(f, "重连中 ({}/{})", attempt, max_attempts)
            }
            Self::Disconnecting(reason) => write!(f, "断开: {}", reason),
            Self::Error(err) => write!(f, "错误: {}", err),
        }
    }
}

// ============================================================================
// WebSocket 相关常量 - 来自 Claude Code SessionsWebSocket.ts
// ============================================================================

/// WebSocket 关闭码常量
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebSocketCloseCode;

impl WebSocketCloseCode {
    // === IANA 标准关闭码 ===
    pub const NORMAL_CLOSURE: u16 = 1000;
    pub const GOING_AWAY: u16 = 1001;
    pub const ABNORMAL_CLOSURE: u16 = 1006;
    
    // === 自定义关闭码 (Claude Code 定义) ===
    /// Session Not Found (可能在压缩期间暂时性不可用)
    pub const SESSION_NOT_FOUND: u16 = 4001;
    
    /// 永久关闭 (不应重试)
    pub const PERMANENT_CLOSE: u16 = 4003;
    
    /// 认证失败
    pub const AUTH_FAILED: u16 = 4004;
    
    /// 协议错误
    pub const PROTOCOL_ERROR: u16 = 4005;
}

/// 检查是否为永久性关闭码 (不需要重连)
/// 移植自 Claude Code: `PERMANENT_CLOSE_CODES.has(closeCode)`
pub fn is_permanent_close_code(code: u16) -> bool {
    matches!(
        code,
        WebSocketCloseCode::PERMANENT_CLOSE | WebSocketCloseCode::AUTH_FAILED | WebSocketCloseCode::PROTOCOL_ERROR
    )
}

/// WebSocket 连接状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WebSocketState {
    #[default]
    Closed,
    Open,
    Closing,
}
