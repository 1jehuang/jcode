//! 系统操作处理器
//!
//! 提供系统级功能：
//! - 心跳检测
/// - 服务器信息查询

use crate::ws::protocol::{WsRequest, WsResponse, MessageType};
use crate::ws::session::SessionManager;
use anyhow::Result;
use tracing::{info};

/// 处理心跳请求
pub async fn handle_ping(
    request: &WsRequest,
    session_id: &str,
    _session_manager: &SessionManager,
) -> Result<WsResponse> {
    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "pong": true,
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "server_time_ms": chrono::Utc::now().timestamp_millis(),
        "session_id": session_id
    })))
}

/// 处理系统信息请求
pub async fn handle_info(
    request: &WsRequest,
    session_id: &str,
    session_manager: &SessionManager,
) -> Result<WsResponse> {
    let active_sessions = session_manager.active_count();

    // 获取系统信息
    let mut sysinfo = sysinfo::System::new_all();
    sysinfo.refresh_all();

    let memory_used = sysinfo.used_memory();
    let memory_total = sysinfo.total_memory();
    let cpu_usage = sysinfo.global_cpu_usage();

    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "server": {
            "version": env!("CARGO_PKG_VERSION"),
            "name": "JCode Web IDE Server",
            "platform": std::env::consts::OS,
            "architecture": std::env::consts::ARCH,
            "uptime_secs": 0, // TODO: 跟踪服务器启动时间
        },
        "session": {
            "id": session_id,
            "connected_at": chrono::Utc::now().to_rfc3339(), // 应该从会话中获取实际连接时间
        },
        "system": {
            "active_sessions": active_sessions,
            "memory": {
                "used_mb": memory_used / 1024 / 1024,
                "total_mb": memory_total / 1024 / 1024,
                "usage_percent": if memory_total > 0 {
                    (memory_used as f64 / memory_total as f64 * 100.0) as u32
                } else {
                    0
                },
            },
            "cpu": {
                "usage_percent": cpu_usage as u32,
            },
        },
        "features": [
            "editor",
            "lsp",
            "filesystem",
            "terminal",
            "git",
            "ai",
            "collaboration",
            "project_management",
        ],
        "supported_languages": [
            "rust",
            "typescript",
            "javascript",
            "python",
            "go",
            "java",
            "c",
            "cpp",
            "html",
            "css",
            "json",
            "yaml",
            "markdown",
        ]
    })))
}
