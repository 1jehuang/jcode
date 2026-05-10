//! # WebSocket Bridge — IDE 深度集成桥接系统
//!
//! 从 Claude Code 移植的 WebSocket Bridge 架构：
//!
//! ## 模块结构
//! - `bridge_main.rs`: Bridge 主控 (~3000行 TS 对应)
//!   * WebSocket 服务器管理
//!   * IDE 连接生命周期
//!   * 多客户端协调
//!   * 健康检查 & 重连
//!
//! - `bridge_messaging.rs`: 消息编排系统 (~2000行 TS 对应)
//!   * 协议定义 (JSON-RPC 2.0)
//!   * 消息路由 & 分发
//!   * 广播 & 点对点通信
//!   * 消息队列 & 背压控制
//!
//! - `session_manager.rs`: 会话生命周期 (~2000行 TS 对应)
//!   * 会话创建/恢复/销毁
//!   * 状态持久化
//!   * 断线重连处理
//!   * 会话迁移

pub mod bridge_main;
pub mod bridge_messaging;
pub mod session_manager;

// ════════════════════════════════════════════════════════════════
// 公共类型导出
// ════════════════════════════════════════════════════════════════

pub use bridge_main::{
    BridgeConfig, BridgeServer, ConnectionState, IdeClientInfo,
    BridgeEvent, BridgeEventType,
};
pub use bridge_messaging::{
    BridgeMessage, MessageId, MessageType, MessagePriority,
    MessageRouter, BroadcastScope, DeliveryReceipt,
};
pub use session_manager::{
    SessionManager, SessionState, SessionHandle, SessionCreateOptions,
    SessionResumeOptions, SessionError,
};

use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::info;

/// Bridge 全局状态 (单例模式)
#[derive(Debug)]
pub struct BridgeRuntime {
    pub config: BridgeConfig,
    pub server: Arc<BridgeServer>,
    pub session_mgr: Arc<SessionManager>,
    pub message_router: Arc<MessageRouter>,
    event_tx: broadcast::Sender<BridgeEvent>,
}

impl BridgeRuntime {
    /// 创建新的 Bridge 运行时实例
    pub async fn new(config: BridgeConfig) -> Result<Self, anyhow::Error> {
        let server = Arc::new(BridgeServer::new(config.clone()));
        let session_mgr = Arc::new(SessionManager::new());
        let message_router = Arc::new(MessageRouter::new(1024));
        
        let (event_tx, _) = broadcast::channel(256);
        
        Ok(Self {
            config,
            server,
            session_mgr,
            message_router,
            event_tx,
        })
    }

    /// 启动 Bridge 服务
    pub async fn start(&self) -> Result<(), anyhow::Error> {
        info!("[Bridge] Starting WebSocket Bridge runtime...");
        
        self.server.start().await?;
        self.message_router.start().await?;
        
        info!("[Bridge] Bridge runtime started successfully");
        Ok(())
    }

    /// 停止 Bridge 服务
    pub async fn stop(&self) -> Result<(), anyhow::Error> {
        info!("[Bridge] Stopping WebSocket Bridge...");
        
        self.server.stop().await?;
        self.message_router.stop().await?;
        self.session_mgr.shutdown_all().await?;
        
        info!("[Bridge] Bridge stopped");
        Ok(())
    }
}
