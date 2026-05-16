//! # 错误类型系统
//!
//! 源自 Claude Code 的 `errors.ts`，提供结构化的错误层次。
//!
//! ## 错误类型
//! - `ToolError` — 工具执行错误
//! - `ConfigError` — 配置解析错误
//! - `ShellError` — Shell 执行错误
//! - `AbortError` — 操作中止
//! - `PermissionError` — 权限拒绝
//! - `NetworkError` — 网络错误
//! - `BudgetError` — 上下文预算超限
//!
//! ## 工具函数
//! - `to_error()` — 将任意错误转换为标准错误
//! - `is_abort_error()` — 检测中止错误
//! - `short_error_stack()` — 获取短错误栈

use thiserror::Error;

/// 工具错误 — 执行工具时发生的错误
#[derive(Error, Debug, Clone)]
pub enum ToolError {
    #[error("Unknown tool: {0}")]
    UnknownTool(String),

    #[error("Tool '{tool}' rejected: {reason}")]
    Rejected { tool: String, reason: String },

    #[error("Tool '{0}' execution failed: {1}")]
    ExecutionFailed(String, String),

    #[error("Tool '{0}' timed out after {1}s")]
    Timeout(String, u64),

    #[error("Tool '{0}' returned invalid output: {1}")]
    InvalidOutput(String, String),

    #[error("Permission denied for tool '{0}': {1}")]
    PermissionDenied(String, String),

    #[error("Tool '{0}' disabled in current context")]
    Disabled(String),
}

/// 配置错误
#[derive(Error, Debug, Clone)]
pub enum ConfigError {
    #[error("Config file not found: {0}")]
    NotFound(String),

    #[error("Failed to parse config: {0}")]
    ParseFailed(String),

    #[error("Invalid config value for '{key}': {message}")]
    InvalidValue { key: String, message: String },

    #[error("Missing required config: {0}")]
    MissingRequired(String),
}

/// Shell 执行错误
#[derive(Error, Debug, Clone)]
pub enum ShellError {
    #[error("Shell command '{0}' failed with exit code {1}")]
    ExitCode(String, i32),

    #[error("Shell command '{0}' timed out after {1}s")]
    Timeout(String, u64),

    #[error("Shell command '{0}' killed by signal")]
    Killed(String),

    #[error("Shell not available: {0}")]
    NotAvailable(String),

    #[error("Shell output exceeded limit ({0} chars)")]
    OutputExceeded(usize),
}

/// 操作中止错误
#[derive(Error, Debug, Clone)]
pub enum AbortError {
    #[error("Operation cancelled by user")]
    UserCancelled,

    #[error("Operation timed out")]
    Timeout,

    #[error("Operation superseded by new request")]
    Superseded,

    #[error("Shutdown in progress")]
    Shutdown,

    #[error("Interrupted: {0}")]
    Interrupted(String),
}

/// 权限错误
#[derive(Error, Debug, Clone)]
pub enum PermissionError {
    #[error("Permission denied: {0}")]
    Denied(String),

    #[error("Tool '{0}' requires user interaction")]
    RequiresInteraction(String),

    #[error("Rate limit exceeded for tool '{0}'")]
    RateLimited(String),

    #[error("Context does not allow this operation: {0}")]
    ContextBlocked(String),
}

/// 网络错误
#[derive(Error, Debug, Clone)]
pub enum NetworkError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Request timed out after {0}ms")]
    Timeout(u64),

    #[error("Server returned {status}: {message}")]
    HttpError { status: u16, message: String },

    #[error("DNS resolution failed for {0}")]
    DnsFailed(String),

    #[error("TLS error: {0}")]
    TlsError(String),
}

/// 上下文预算错误
#[derive(Error, Debug, Clone)]
pub enum BudgetError {
    #[error("Context window full ({0}%/{1}k tokens)")]
    ContextFull(f64, usize),

    #[error("Tool result too large ({0} chars, limit {1})")]
    ResultTooLarge(usize, usize),

    #[error("Token budget exceeded for this turn")]
    TurnBudgetExceeded,
}

/// 统一错误类型（所有 jcode 错误的包装）
#[derive(Error, Debug, Clone)]
pub enum JcodeError {
    #[error(transparent)]
    Tool(#[from] ToolError),

    #[error(transparent)]
    Config(#[from] ConfigError),

    #[error(transparent)]
    Shell(#[from] ShellError),

    #[error(transparent)]
    Abort(#[from] AbortError),

    #[error(transparent)]
    Permission(#[from] PermissionError),

    #[error(transparent)]
    Network(#[from] NetworkError),

    #[error(transparent)]
    Budget(#[from] BudgetError),

    #[error("{0}")]
    Other(String),
}

impl From<String> for JcodeError {
    fn from(s: String) -> Self { JcodeError::Other(s) }
}

impl From<&str> for JcodeError {
    fn from(s: &str) -> Self { JcodeError::Other(s.to_string()) }
}

// -- 工具函数 --

/// 将任意错误转换为友好显示字符串
/// 源自 Claude Code 的 `errorMessage()`
pub fn error_message(err: &dyn std::error::Error) -> String {
    let msg = err.to_string();

    // 截取短栈
    if let Some(pos) = msg.find("\nStack backtrace") {
        msg[..pos].to_string()
    } else if let Some(pos) = msg.find("\n\n") {
        msg[..pos].to_string()
    } else {
        msg
    }
}

/// 检查是否中止错误
/// 源自 Claude Code 的 `isAbortError()`
pub fn is_abort_error(err: &dyn std::error::Error) -> bool {
    let msg = err.to_string().to_lowercase();
    msg.contains("cancelled")
        || msg.contains("canceled")
        || msg.contains("abort")
        || msg.contains("interrupt")
        || msg.contains("shutdown")
}

/// 获取短错误栈（适用于 UI 显示）
/// 源自 Claude Code 的 `shortErrorStack()`
pub fn short_error_stack(err: &dyn std::error::Error) -> String {
    let msg = err.to_string();
    if msg.len() > 200 {
        format!("{}... ({} chars total)", &msg[..197], msg.len())
    } else {
        msg
    }
}

/// 检查文件系统错误
/// 源自 Claude Code 的 `isFsInaccessible()` / `isENOENT()`
pub fn is_fs_not_found(err: &dyn std::error::Error) -> bool {
    let msg = err.to_string();
    msg.contains("No such file") || msg.contains("entity not found") || msg.contains("ENOENT")
}

/// 检测超时错误
pub fn is_timeout_error(err: &dyn std::error::Error) -> bool {
    let msg = err.to_string().to_lowercase();
    msg.contains("timeout") || msg.contains("timed out")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_error_format() {
        let err = ToolError::UnknownTool("nonexistent".into());
        assert_eq!(err.to_string(), "Unknown tool: nonexistent");

        let err = ToolError::Timeout("bash".into(), 30);
        assert_eq!(err.to_string(), "Tool 'bash' timed out after 30s");
    }

    #[test]
    fn test_jcode_error_conversion() {
        let tool_err = ToolError::UnknownTool("x".into());
        let jcode_err: JcodeError = tool_err.into();
        assert!(matches!(jcode_err, JcodeError::Tool(_)));
    }

    #[test]
    fn test_is_abort_error() {
        let err = AbortError::UserCancelled;
        assert!(is_abort_error(&err));

        let err = ToolError::Timeout("bash".into(), 30);
        assert!(!is_abort_error(&err));
    }

    #[test]
    fn test_short_error_stack() {
        let long_msg = "x".repeat(500);
        let err = ToolError::ExecutionFailed("test".into(), long_msg);
        let short = short_error_stack(&err);
        assert!(short.len() <= 210);
        assert!(short.ends_with("... (500 chars total)"));
    }

    #[test]
    fn test_error_message() {
        let err = ToolError::UnknownTool("test".into());
        let msg = error_message(&err);
        assert_eq!(msg, "Unknown tool: test");
    }

    #[test]
    fn test_fs_not_found() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "No such file or directory");
        assert!(is_fs_not_found(&io_err));
    }
}
