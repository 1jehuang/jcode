//! # 编译引擎错误类型

use std::fmt;
use std::io;

/// 编译引擎统一错误枚举
#[derive(Debug)]
pub enum BuildEngineError {
    NotFound(String),
    InvalidState(String),
    CompilationFailed(String),
    ToolchainNotFound(String),
    DependencyError(String),
    Timeout { operation: String, timeout_secs: u64 },
    Cancelled(String),
    CacheError(String),
    ContainerError(String),
    EnvironmentError(String),
    ImagePullError { image: String, detail: String },
    NoAvailableNodes(String),
    SchedulingFailed(String),
    InsufficientResources(String),
    DependencyCycleDetected,
    Io(io::Error),
    Serialization(String),
}

impl fmt::Display for BuildEngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(msg) => write!(f, "Not found: {}", msg),
            Self::InvalidState(msg) => write!(f, "Invalid state: {}", msg),
            Self::CompilationFailed(msg) => write!(f, "Compilation failed: {}", msg),
            Self::ToolchainNotFound(msg) => write!(f, "Toolchain not found: {}", msg),
            Self::DependencyError(msg) => write!(f, "Dependency error: {}", msg),
            Self::Timeout { operation, timeout_secs } => {
                write!(f, "{} timed out after {}s", operation, timeout_secs)
            }
            Self::Cancelled(msg) => write!(f, "Cancelled: {}", msg),
            Self::CacheError(msg) => write!(f, "Cache error: {}", msg),
            Self::ContainerError(msg) => write!(f, "Container error: {}", msg),
            Self::EnvironmentError(msg) => write!(f, "Environment error: {}", msg),
            Self::ImagePullError { image, detail } => {
                write!(f, "Failed to pull '{}': {}", image, detail)
            }
            Self::NoAvailableNodes(msg) => write!(f, "No available nodes: {}", msg),
            Self::SchedulingFailed(msg) => write!(f, "Scheduling failed: {}", msg),
            Self::InsufficientResources(msg) => write!(f, "Insufficient resources: {}", msg),
            Self::DependencyCycleDetected => write!(f, "Dependency cycle detected"),
            Self::Io(e) => write!(f, "IO error: {}", e),
            Self::Serialization(msg) => write!(f, "Serialization error: {}", msg),
        }
    }
}

impl std::error::Error for BuildEngineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for BuildEngineError {
    fn from(err: io::Error) -> Self { Self::Io(err) }
}

impl From<serde_json::Error> for BuildEngineError {
    fn from(err: serde_json::Error) -> Self { Self::Serialization(err.to_string()) }
}

pub type Result<T> = std::result::Result<T, BuildEngineError>;
