// jcode-mcp-advanced
// ════════════════════════════════════════════════════════════════
// 高级 MCP 客户端 - 移植自 Claude Code src/services/mcp/
//
// 核心能力:
//   1. 多传输协议支持 — stdio / SSE / HTTP / WebSocket
//   2. 连接管理 — 自动重连 (指数退避, 5次最大)
//   3. 工具发现与缓存 — 带失效的动态工具注册
//   4. 流式工具调用 — 边执行边返回进度
//   5. Sampling — MCP Server 通过 Client 请求 LLM 采样
//   6. 权限协商 — 工具调用前的权限审批流程
//   7. 进程生命周期管理 — SIGINT -> SIGTERM -> SIGKILL 升级
//
// 对应 Claude Code 源码:
//   - src/services/mcp/client.ts (~2000行) — 核心客户端
//   - src/services/mcp/types.ts (259行) — 类型定义
//   - src/services/mcp/useManageMCPConnections.ts (1142行) — 重连逻辑
//   - src/services/mcp/channelNotification.ts (200+行) — 权限中继
// ════════════════════════════════════════════════════════════════

mod types;
mod transport;
mod client;
mod connection_manager;
mod tool_registry;
mod sampling;
mod permissions;
pub mod auth;

pub use types::*;
pub use transport::*;
pub use client::MCPClient;
pub use connection_manager::{
    ConnectionManager,
    ReconnectPolicy,
};
pub use types::ConnectionState;
pub use tool_registry::{MCPToolRegistry, ToolCacheEntry};
pub use sampling::SamplingHandler;
pub use permissions::{
    McpConnectionPermissionConfig, PermissionLevel, PermissionCheckResult, ToolPermissionRule,
};

/// 默认连接超时 (ms)
pub const DEFAULT_CONNECTION_TIMEOUT_MS: u64 = 30_000;  // 30s

/// 默认重连策略: 最大5次, 初始1s, 最大30s
pub const DEFAULT_MAX_RECONNECT_ATTEMPTS: u32 = 5;
pub const DEFAULT_INITIAL_BACKOFF_MS: u64 = 1_000;
pub const DEFAULT_MAX_BACKOFF_MS: u64 = 30_000;

/// 进程优雅退出序列时间 (ms)
pub const PROCESS_GRACEFUL_SHUTDOWN_MS: u64 = 100;   // SIGINT 后等待
pub const PROCESS_FORCE_SHUTDOWN_MS: u64 = 400;      // SIGTERM 后等待
pub const PROCESS_KILL_TIMEOUT_MS: u64 = 1000;       // 最终 SIGKILL

/// 最大错误次数后强制重连
pub const MAX_ERRORS_BEFORE_RECONNECT: usize = 3;

/// 工具缓存 TTL (秒) — 超时后重新 fetchTools
pub const TOOL_CACHE_TTL_SECS: u64 = 300;  // 5 min

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transport_type_parsing() {
        let t = TransportType::from_str("stdio");
        assert_eq!(t, TransportType::Stdio);
        
        let t = TransportType::from_str("sse");
        assert_eq!(t, TransportType::Sse);
        
        let t = TransportType::from_str("http");
        assert_eq!(t, TransportType::Http);
    }

    #[test]
    fn test_reconnect_policy_backoff() {
        let policy = ReconnectPolicy::default();
        
        // 第1次: ~1000ms
        let d1 = policy.backoff_delay(1);
        assert!(d1 >= Duration::from_millis(800));
        assert!(d1 <= Duration::from_millis(1200));
        
        // 第4次: 应该更长
        let d4 = policy.backoff_delay(4);
        assert!(d4 > d1);
        
        // 超过最大尝试次数返回 None
        assert!(policy.backoff_delay(6).is_none());
    }

    #[test]
    fn test_connection_state_transitions() {
        // Pending -> Connected
        let s1 = ConnectionState::Pending;
        assert!(!s1.is_connected());
        assert!(!s1.is_terminal());
        
        let s2 = ConnectionState::Connected {
            capabilities: Default::default(),
            server_info: None,
        };
        assert!(s2.is_connected());
        
        let s3 = ConnectionState::Failed { 
            error: "test".to_string(), 
            retryable: true 
        };
        assert!(s3.can_reconnect());
    }
}
