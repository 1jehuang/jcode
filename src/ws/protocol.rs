//! WebSocket 消息协议定义
//!
//! 基于 JSON-RPC 2.0 规范，扩展支持：
//! - 流式响应（用于 AI 输出）
//! - 通知消息（无需响应）
//! - 广播消息（多用户协作）

use serde::{Deserialize, Serialize};

/// 消息类型枚举
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MessageType {
    /// 请求（客户端 -> 服务端）
    Request,
    
    /// 响应（服务端 -> 客户端）
    Response,
    
    /// 通知（服务端 -> 客户端，无需响应）
    Notification,
    
    /// 错误响应
    Error,
    
    /// 流式数据块（AI 输出等）
    StreamChunk,
    
    /// 流结束标记
    StreamEnd,
    
    /// 广播消息（协作编辑）
    Broadcast,
    
    /// 心跳
    Heartbeat,
    
    /// 欢迎消息（连接建立时发送）
    Welcome,
}

/// WebSocket 消息（通用格式）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsMessage {
    /// 会话 ID
    pub session_id: String,
    
    /// 消息 ID（用于请求-响应匹配）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    
    /// 消息类型
    #[serde(rename = "type")]
    pub msg_type: MessageType,
    
    /// 方法名（仅请求消息）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    
    /// 参数/数据
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    
    /// 结果数据（仅响应消息）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    
    /// 错误信息（仅错误消息）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    
    /// 时间戳
    pub timestamp: u64,
}

impl WsMessage {
    /// 创建新的请求消息
    pub fn request(session_id: &str, id: &str, method: &str, params: serde_json::Value) -> Self {
        Self {
            session_id: session_id.to_string(),
            id: Some(id.to_string()),
            msg_type: MessageType::Request,
            method: Some(method.to_string()),
            params: Some(params),
            result: None,
            error: None,
            timestamp: timestamp_now(),
        }
    }

    /// 创建响应消息
    pub fn response(id: &str, result: serde_json::Value) -> Self {
        Self {
            session_id: String::new(),
            id: Some(id.to_string()),
            msg_type: MessageType::Response,
            method: None,
            params: None,
            result: Some(result),
            error: None,
            timestamp: timestamp_now(),
        }
    }

    /// 创建错误响应
    pub fn error(id: &str, error_msg: &str) -> Self {
        Self {
            session_id: String::new(),
            id: Some(id.to_string()),
            msg_type: MessageType::Error,
            method: None,
            params: None,
            result: None,
            error: Some(error_msg.to_string()),
            timestamp: timestamp_now(),
        }
    }

    /// 创建通知消息
    pub fn notification(session_id: &str, method: &str, data: serde_json::Value) -> Self {
        Self {
            session_id: session_id.to_string(),
            id: None,
            msg_type: MessageType::Notification,
            method: Some(method.to_string()),
            params: Some(data),
            result: None,
            error: None,
            timestamp: timestamp_now(),
        }
    }

    /// 创建流式数据块
    pub fn stream_chunk(id: &str, chunk: &str, is_final: bool) -> Self {
        Self {
            session_id: String::new(),
            id: Some(id.to_string()),
            msg_type: if is_final { MessageType::StreamEnd } else { MessageType::StreamChunk },
            method: None,
            params: Some(serde_json::json!({"content": chunk})),
            result: None,
            error: None,
            timestamp: timestamp_now(),
        }
    }

    /// 创建广播消息
    pub fn broadcast(session_id: &str, event: &str, data: serde_json::Value) -> Self {
        Self {
            session_id: session_id.to_string(),
            id: None,
            msg_type: MessageType::Broadcast,
            method: Some(event.to_string()),
            params: Some(data),
            result: None,
            error: None,
            timestamp: timestamp_now(),
        }
    }

    /// 创建心跳消息
    pub fn heartbeat() -> Self {
        Self {
            session_id: String::new(),
            id: None,
            msg_type: MessageType::Heartbeat,
            method: None,
            params: Some(serde_json::json!({"timestamp": chrono::Utc::now().to_rfc3339()})),
            result: None,
            error: None,
            timestamp: timestamp_now(),
        }
    }
}

/// 获取安全的当前时间戳（毫秒）
fn timestamp_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// WebSocket 请求（简化版，兼容 JSON-RPC）
#[derive(Debug, Deserialize)]
pub struct WsRequest {
    /// 请求 ID
    pub id: String,
    
    /// 方法名
    pub method: String,
    
    /// 参数
    pub params: serde_json::Value,
}

/// WebSocket 响应（简化版）
#[derive(Debug, Serialize)]
pub struct WsResponse {
    /// 对应的请求 ID
    pub id: String,
    
    /// 结果数据
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    
    /// 错误信息
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl WsResponse {
    /// 创建成功响应
    pub fn new(id: &str, _msg_type: MessageType, result: serde_json::Value) -> Self {
        Self {
            id: id.to_string(),
            result: Some(result),
            error: None,
        }
    }

    /// 创建错误响应
    pub fn error(id: &str, error_msg: &str) -> Self {
        Self {
            id: id.to_string(),
            result: None,
            error: Some(error_msg.to_string()),
        }
    }
}

// ============================================================================
// 特定领域的消息类型定义
// ============================================================================

/// 编辑器文档状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentState {
    /// 文件路径
    pub file_path: String,
    
    /// 文档内容
    pub content: String,
    
    /// 版本号（用于 OT 算法）
    pub version: u64,
    
    /// 光标位置
    pub cursor: Option<CursorPosition>,
    
    /// 选区
    pub selection: Option<TextSelection>,
    
    /// 语言 ID
    pub language_id: String,
}

/// 光标位置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorPosition {
    /// 行号（从 0 开始）
    pub line: u32,
    
    /// 列号（从 0 开始）
    pub character: u32,
}

/// 文本选区
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextSelection {
    /// 起始位置
    pub start: CursorPosition,
    
    /// 结束位置
    pub end: CursorPosition,
}

/// 文本编辑操作（OT 兼容）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextEditOperation {
    /// 操作类型
    pub op_type: EditOperationType,
    
    /// 起始位置
    pub start: CursorPosition,
    
    /// 结束位置（仅删除操作需要）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<CursorPosition>,
    
    /// 插入的文本（仅插入操作需要）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// 编辑操作类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EditOperationType {
    Insert,
    Delete,
    Replace,
}

/// LSP 补全项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionItem {
    /// 显示标签
    pub label: String,
    
    /// 详细描述
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    
    /// 文档说明
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
    
    /// 类型（函数、变量、关键字等）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    
    /// 插入文本
    pub insert_text: String,
    
    /// 排序优先级（数字越小越靠前）
    pub sort_priority: i32,
}

/// LSP 诊断信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticInfo {
    /// 严重级别
    pub severity: DiagnosticSeverity,
    
    /// 消息内容
    pub message: String,
    
    /// 起始位置
    pub start: CursorPosition,
    
    /// 结束位置
    pub end: CursorPosition,
    
    /// 来源（lsp 名称、编译器等）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    
    /// 错误代码
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// 诊断严重级别
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

/// 终端会话信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalSessionInfo {
    /// 终端会话 ID
    pub session_id: String,
    
    /// Shell 类型（bash、powershell、cmd 等）
    pub shell_type: String,
    
    /// 终端尺寸
    pub size: TerminalSize,
    
    /// 工作目录
    pub working_dir: String,
    
    /// PID
    pub pid: u32,
}

/// 终端尺寸
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalSize {
    /// 行数
    pub rows: u16,
    
    /// 列数
    pub cols: u16,
}

/// Git 状态信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStatusInfo {
    /// 当前分支
    pub branch: String,
    
    /// 已暂存的文件
    pub staged: Vec<GitFileStatus>,
    
    /// 未暂存的修改
    pub modified: Vec<GitFileStatus>,
    
    /// 未跟踪的文件
    pub untracked: Vec<String>,
    
    /// 是否有未提交的更改
    pub has_changes: bool,
}

/// 单个文件的 Git 状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitFileStatus {
    /// 文件路径
    pub path: String,
    
    /// 状态类型（A=添加, M=修改, D=删除, R=重命名等）
    pub status: String,
}

/// 协作用户光标
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollaboratorCursor {
    /// 用户 ID
    pub user_id: String,
    
    /// 用户名（显示用）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    
    /// 光标颜色
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    
    /// 文件路径
    pub file_path: String,
    
    /// 光标位置
    pub position: CursorPosition,
    
    /// 更新时间戳
    pub updated_at: u64,
}
