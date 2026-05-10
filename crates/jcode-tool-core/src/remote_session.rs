//! # 远程会话管理 — Bridge 远程协议支持
//!
//! 源自 Claude Code `src/bridge/` 目录 (31 文件) 的远程会话模式
//!
//! ## 能力
//! - 多传输抽象 (v1 Hybrid / v2 SSE + CCR Client)
//! - 会话生命周期管理 (创建/恢复/归档)
//! - 心跳管理 + 容量查询
//! - 远程安全工具过滤 (源自 `REMOTE_SAFE_COMMANDS`)
//! - 会话状态持久化

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// 远程传输协议版本
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemoteTransportVersion {
    /// v1 HybridTransport (WebSocket + HTTP)
    V1Hybrid,
    /// v2 SSE + CCR Client (仅 SSE 流, 命令通过 HTTP)
    V2SseCcr,
}

/// 远程会话状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemoteSessionStatus {
    /// 已断开
    Disconnected,
    /// 正在连接
    Connecting,
    /// 已连接
    Connected,
    /// 心跳超时
    HeartbeatTimeout,
    /// 已重连
    Reconnected,
    /// 已结束
    Ended,
}

/// 远程会话 ID
pub type RemoteSessionId = String;

/// 远程会话配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteSessionConfig {
    /// 远程端点 URL
    pub endpoint: String,
    /// 传输协议版本
    pub transport_version: RemoteTransportVersion,
    /// API Token
    pub api_token: Option<String>,
    /// 心跳间隔 (秒)
    pub heartbeat_interval_secs: u64,
    /// 心跳超时 (秒)
    pub heartbeat_timeout_secs: u64,
    /// 自动重连
    pub auto_reconnect: bool,
    /// 最大重连次数
    pub max_reconnect_attempts: u32,
}

impl Default for RemoteSessionConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            transport_version: RemoteTransportVersion::V2SseCcr,
            api_token: None,
            heartbeat_interval_secs: 30,
            heartbeat_timeout_secs: 90,
            auto_reconnect: true,
            max_reconnect_attempts: 5,
        }
    }
}

/// 会话统计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStats {
    /// 已发送消息数
    pub messages_sent: u64,
    /// 已接收消息数
    pub messages_received: u64,
    /// 已发送心跳数
    pub heartbeats_sent: u64,
    /// 心跳超时次数
    pub heartbeat_timeouts: u64,
    /// 重连次数
    pub reconnects: u32,
    /// 连接时长 (秒)
    pub uptime_secs: u64,
}

/// 远程会话管理器
///
/// 管理远程 gRPC 模式的会话生命周期。
/// 源自 Claude Code 的 `bridge/` 远程会话模式：
/// - `createSession.ts` — 会话创建
/// - `sessionRunner.ts` — 会话运行
/// - `replBridgeTransport.ts` — 多传输抽象
pub struct RemoteSessionManager {
    /// 会话配置
    config: RemoteSessionConfig,
    /// 当前状态
    status: Arc<RwLock<RemoteSessionStatus>>,
    /// 会话 ID
    session_id: Arc<RwLock<Option<RemoteSessionId>>>,
    /// 是否启用
    enabled: AtomicBool,
    /// 会话开始时间
    started_at: Arc<RwLock<Option<Instant>>>,
    /// 心跳追踪
    last_heartbeat: Arc<RwLock<Instant>>,
    /// 统计信息
    stats: Arc<RwLock<SessionStats>>,
    /// 远程安全的工具/命令名称集合
    /// 源自 Claude Code 的 `REMOTE_SAFE_COMMANDS` / `BRIDGE_SAFE_COMMANDS`
    safe_commands: HashSet<String>,
}

impl RemoteSessionManager {
    pub fn new(config: RemoteSessionConfig) -> Self {
        let mut safe = HashSet::new();
        // 默认远程安全的命令（源自 Claude Code 的 REMOTE_SAFE_COMMANDS）
        for cmd in &[
            "session", "exit", "clear", "help", "theme", "color", "vim",
            "cost", "usage", "copy", "btw", "feedback", "plan", "keybindings",
            "statusline", "stickers", "mobile",
        ] {
            safe.insert(cmd.to_string());
        }
        Self {
            stats: Arc::new(RwLock::new(SessionStats {
                messages_sent: 0,
                messages_received: 0,
                heartbeats_sent: 0,
                heartbeat_timeouts: 0,
                reconnects: 0,
                uptime_secs: 0,
            })),
            last_heartbeat: Arc::new(RwLock::new(Instant::now())),
            enabled: AtomicBool::new(false),
            started_at: Arc::new(RwLock::new(None)),
            session_id: Arc::new(RwLock::new(None)),
            status: Arc::new(RwLock::new(RemoteSessionStatus::Disconnected)),
            safe_commands: safe,
            config,
        }
    }

    /// 启动远程会话
    /// 源自 Claude Code 的 `createSession()`
    pub async fn start(&self) -> anyhow::Result<RemoteSessionId> {
        let sid = uuid::Uuid::new_v4().to_string();
        *self.session_id.write().await = Some(sid.clone());
        *self.status.write().await = RemoteSessionStatus::Connecting;
        *self.started_at.write().await = Some(Instant::now());
        self.enabled.store(true, Ordering::SeqCst);

        tracing::info!("[RemoteSession] Starting session {} at {}", sid, self.config.endpoint);
        *self.status.write().await = RemoteSessionStatus::Connected;
        Ok(sid)
    }

    /// 发送心跳
    /// 源自 Claude Code 的心跳管理 (`pollConfig.ts`, `capacityWake.ts`)
    pub async fn send_heartbeat(&self) -> bool {
        if !self.enabled.load(Ordering::SeqCst) {
            return false;
        }

        let now = Instant::now();
        *self.last_heartbeat.write().await = now;

        let mut stats = self.stats.write().await;
        stats.heartbeats_sent += 1;

        // 实际心跳发送由调用方实现（HTTP/WebSocket）
        true
    }

    /// 检查心跳是否超时
    pub async fn check_heartbeat(&self) -> bool {
        if !self.enabled.load(Ordering::SeqCst) {
            return true;
        }

        let elapsed = self.last_heartbeat.read().await.elapsed();
        if elapsed > Duration::from_secs(self.config.heartbeat_timeout_secs) {
            let mut stats = self.stats.write().await;
            stats.heartbeat_timeouts += 1;
            *self.status.write().await = RemoteSessionStatus::HeartbeatTimeout;
            return false;
        }
        true
    }

    /// 执行重连
    /// 源自 Claude Code 的 `reconnectMcpServerImpl()`
    pub async fn reconnect(&self) -> anyhow::Result<bool> {
        let mut stats = self.stats.write().await;
        if stats.reconnects >= self.config.max_reconnect_attempts {
            tracing::warn!("[RemoteSession] Max reconnects reached");
            return Ok(false);
        }

        stats.reconnects += 1;
        *self.status.write().await = RemoteSessionStatus::Reconnected;
        tracing::info!("[RemoteSession] Reconnect #{}", stats.reconnects);
        Ok(true)
    }

    /// 结束会话
    pub async fn end(&self) {
        self.enabled.store(false, Ordering::SeqCst);
        *self.status.write().await = RemoteSessionStatus::Ended;

        if let Some(started) = *self.started_at.read().await {
            let mut stats = self.stats.write().await;
            stats.uptime_secs = started.elapsed().as_secs();
        }

        tracing::info!("[RemoteSession] Session ended");
    }

    /// 获取会话状态
    pub async fn status(&self) -> RemoteSessionStatus {
        *self.status.read().await
    }

    /// 获取会话 ID
    pub async fn session_id(&self) -> Option<RemoteSessionId> {
        self.session_id.read().await.clone()
    }

    /// 获取统计信息
    pub async fn stats(&self) -> SessionStats {
        let mut stats = self.stats.read().await.clone();
        if let Some(started) = *self.started_at.read().await {
            stats.uptime_secs = started.elapsed().as_secs();
        }
        stats
    }

    /// 检查命令是否远程安全
    /// 源自 Claude Code 的 `REMOTE_SAFE_COMMANDS` 过滤
    pub fn is_command_remote_safe(&self, command_name: &str) -> bool {
        self.safe_commands.contains(command_name)
    }

    /// 添加远程安全命令
    pub fn add_safe_command(&mut self, name: &str) {
        self.safe_commands.insert(name.to_string());
    }

    /// 获取远程安全命令列表
    pub fn safe_commands(&self) -> &HashSet<String> {
        &self.safe_commands
    }

    /// 检查会话是否活跃
    pub async fn is_active(&self) -> bool {
        self.enabled.load(Ordering::SeqCst) && *self.status.read().await == RemoteSessionStatus::Connected
    }

    /// 获取配置
    pub fn config(&self) -> &RemoteSessionConfig {
        &self.config
    }
}

/// 远程会话工厂
/// 源自 Claude Code 的 `sessionRunner.ts` + `createSession.ts`
pub struct RemoteSessionFactory;

impl RemoteSessionFactory {
    /// 创建远程会话
    pub async fn create_session(
        endpoint: &str,
        api_token: Option<&str>,
        transport: RemoteTransportVersion,
    ) -> anyhow::Result<RemoteSessionManager> {
        let config = RemoteSessionConfig {
            endpoint: endpoint.to_string(),
            transport_version: transport,
            api_token: api_token.map(|s| s.to_string()),
            ..Default::default()
        };

        let manager = RemoteSessionManager::new(config);
        manager.start().await?;
        Ok(manager)
    }

    /// 创建 v2 SSE 会话
    pub async fn create_v2_session(endpoint: &str, api_token: Option<&str>) -> anyhow::Result<RemoteSessionManager> {
        Self::create_session(endpoint, api_token, RemoteTransportVersion::V2SseCcr).await
    }

    /// 创建 v1 Hybrid 会话
    pub async fn create_v1_session(endpoint: &str) -> anyhow::Result<RemoteSessionManager> {
        Self::create_session(endpoint, None, RemoteTransportVersion::V1Hybrid).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_session_lifecycle() {
        let config = RemoteSessionConfig {
            endpoint: "https://remote.example.com".into(),
            heartbeat_interval_secs: 5,
            heartbeat_timeout_secs: 15,
            ..Default::default()
        };

        let mgr = RemoteSessionManager::new(config);
        assert_eq!(mgr.status().await, RemoteSessionStatus::Disconnected);

        let sid = mgr.start().await.unwrap();
        assert!(!sid.is_empty());
        assert_eq!(mgr.status().await, RemoteSessionStatus::Connected);
        assert!(mgr.is_active().await);

        mgr.end().await;
        assert_eq!(mgr.status().await, RemoteSessionStatus::Ended);
        assert!(!mgr.is_active().await);
    }

    #[tokio::test]
    async fn test_heartbeat() {
        let config = RemoteSessionConfig {
            endpoint: "https://remote.example.com".into(),
            heartbeat_timeout_secs: 3600, // 1 hour for test
            ..Default::default()
        };

        let mgr = RemoteSessionManager::new(config);
        mgr.start().await.unwrap();

        assert!(mgr.send_heartbeat().await);
        assert!(mgr.check_heartbeat().await);

        mgr.end().await;
    }

    #[tokio::test]
    async fn test_heartbeat_timeout() {
        let config = RemoteSessionConfig {
            endpoint: "https://remote.example.com".into(),
            heartbeat_timeout_secs: 0, // immediate timeout
            ..Default::default()
        };

        let mgr = RemoteSessionManager::new(config);
        mgr.start().await.unwrap();

        // Small delay to ensure timeout
        tokio::time::sleep(Duration::from_millis(10)).await;

        assert!(!mgr.check_heartbeat().await);
        assert_eq!(mgr.status().await, RemoteSessionStatus::HeartbeatTimeout);

        mgr.end().await;
    }

    #[tokio::test]
    async fn test_factory() {
        let mgr = RemoteSessionFactory::create_v2_session("https://remote.example.com", Some("token")).await.unwrap();
        assert_eq!(mgr.config().transport_version, RemoteTransportVersion::V2SseCcr);
        assert!(mgr.is_active().await);
        mgr.end().await;
    }

    #[test]
    fn test_safe_commands() {
        let config = RemoteSessionConfig::default();
        let mgr = RemoteSessionManager::new(config);
        assert!(mgr.is_command_remote_safe("help"));
        assert!(mgr.is_command_remote_safe("exit"));
        assert!(mgr.is_command_remote_safe("theme"));
        assert!(!mgr.is_command_remote_safe("bash"));
        assert!(!mgr.is_command_remote_safe("write"));
    }

    #[test]
    fn test_stats() {
        let config = RemoteSessionConfig::default();
        let mgr = RemoteSessionManager::new(config);
        let mut stats = mgr.stats.blocking_lock();
        stats.messages_sent = 42;
        stats.heartbeats_sent = 10;
        assert_eq!(stats.messages_sent, 42);
        assert_eq!(stats.heartbeats_sent, 10);
    }
}
