//! Remote Debugging Enhancements Module
//!
//! ## 来源
//! 移植自 Claude Code 的远程调试优秀功能:
//! - `src/remote/RemoteSessionManager.ts` (9KB) — 远程会话管理器
//! - `src/remote/SessionsWebSocket.ts` (12KB) — WebSocket 传输层
//! - `src/bridge/remoteBridgeCore.ts` (38KB) — 远程桥接核心 (v2 无环境)
//! - `src/hooks/useRemoteSession.ts` (22KB) — React Hook
//!
//! ## 新增能力 (JCode 原本缺失)
//! 1. **JWT Proactive Refresh**: 过期前5min主动刷新, 避免请求中断
//! 2. **UUID 去重 (BoundedUUIDSet)**: 环形缓冲区去重, 防止消息重复处理
//! 3. **FlushGate 历史刷写门控**: 解决初始历史与实时写入的竞态问题
//! 4. **401 自动恢复**: JWT 过期时自动 rebuildTransport
//! 5. **权限请求/响应协议**: SDK Control Request/Response 完整实现
//! 6. **指数退避重连**: 永久关闭码检测 + SessionNotFound 特殊处理

mod types;
mod uuid_dedup;
mod flush_gate;
mod jwt_refresh;
mod permission_protocol;
mod session_manager;
mod websocket_transport;

pub use types::*;
pub use uuid_dedup::BoundedUuidSet;
pub use flush_gate::FlushGate;
pub use jwt_refresh::{JwtRefreshScheduler, JwtRefreshConfig};
pub use permission_protocol::{
    SdkControlRequest, SdkControlResponse, SdkControlRequestBody,
    SdkControlResponseBody, PermissionBehavior,
    RemotePermissionResponse, PermissionRequestInfo,
};
pub use session_manager::{
    EnhancedRemoteSessionManager, RemoteSessionConfig, 
    RemoteSessionCallbacks, RemoteSessionState,
};
pub use websocket_transport::{
    SessionsWebSocket, SessionsWebSocketConfig, WebSocketCloseCode,
    WebSocketState,
};
