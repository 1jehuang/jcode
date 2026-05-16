//! 统一的 thiserror 错误类型，逐步替代 anyhow 冒泡
//! 提供更精细的错误分类和诊断信息

use thiserror::Error;

// -- Provider 错误 --

#[derive(Error, Debug)]
pub enum ProviderError {
    #[error("API call failed (provider={provider}, status={status}): {message}")]
    ApiCallFailed { provider: String, status: u16, message: String },
    #[error("Rate limited by {provider}, retry after {retry_after}s")]
    RateLimited { provider: String, retry_after: u64 },
    #[error("Authentication failed for {0}: {1}")]
    AuthFailed(String, String),
    #[error("Model {model} not available on {provider}")]
    ModelNotAvailable { provider: String, model: String },
    #[error("Context limit exceeded: {0}")]
    ContextLimitExceeded(String),
    #[error("Stream error: {0}")]
    StreamError(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

// -- Tool 执行错误 --

#[derive(Error, Debug)]
pub enum ToolExecuteError {
    #[error("Tool '{name}' not found in registry")]
    NotFound { name: String },
    #[error("Tool '{name}' failed (exit={exit_code}): {stderr:.200}")]
    CommandFailed { name: String, exit_code: i32, stderr: String },
    #[error("Tool '{name}' timed out after {secs}s")]
    Timeout { name: String, secs: u64 },
    #[error("Tool '{name}' rejected: {reason}")]
    Rejected { name: String, reason: String },
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

// -- 配置错误 --

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Config not found: {0}")]
    NotFound(String),
    #[error("Parse error at {path}:{line} - {detail}")]
    ParseError { path: String, line: u32, detail: String },
    #[error("Missing required field: {0}")]
    MissingField(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

// -- Session 错误 --

#[derive(Error, Debug)]
pub enum SessionError {
    #[error("Session {0} not found")]
    NotFound(String),
    #[error("Session {0} is not active")]
    NotActive(String),
    #[error("Session corrupt: {0}")]
    Corrupted(String),
}

// -- 文件操作 错误 --

#[derive(Error, Debug)]
pub enum FileError {
    #[error("File not found: {0}")]
    NotFound(String),
    #[error("Permission denied: {0}")]
    PermissionDenied(String),
    #[error("File too large ({size} > {max} bytes): {path}")]
    TooLarge { path: String, size: u64, max: u64 },
    #[error("Binary file: {0}")]
    BinaryFile(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
