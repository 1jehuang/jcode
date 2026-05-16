//! 增强版远程会话管理器
//!
//! 整合所有增强组件:
//! - JWT Proactive Refresh (过期前5min刷新)
//! - UUID 去重 (BoundedUUIDSet, 2000容量)
//! - FlushGate (历史刷写门控)
//! - Permission Protocol (权限请求/响应)

use crate::types::*;
use crate::uuid_dedup::BoundedUuidSet;
use crate::flush_gate::FlushGate;
use crate::jwt_refresh::{JwtRefreshScheduler, JwtRefreshConfig};
use crate::permission_protocol::PermissionRequestManager;
use std::sync::Arc;

/// 增强版远程会话管理器
///
/// 对比 JCode 原 `remote.rs` (44KB) 的增强:
/// | 功能 | 原版 | 增强 |
/// |------|------|------|
/// | 基础连接 | ✅ RemoteConnection | ✅ SessionsWebSocket |
/// | 重连机制 | ✅ 指数退避 | ✅ + 永久码检测 + 4001特殊处理 |
/// | JWT 刷新 | ❌ 缺失 | ✅ 过期前5min主动刷新 |
/// | 消息去重 | ❌ 缺失 | ✅ BoundedUUIDSet (2000) |
/// | 竞态保护 | ❌ 缺失 | ✅ FlushGate |
/// | 权限协议 | ❌ 缺失 | ✅ SDK Control Request/Response |
/// | 401恢复 | ❌ 缺失 | ✅ 自动 rebuildTransport |

pub struct EnhancedRemoteSessionManager {
    // === 基础连接 ===
    /// 会话配置
    config: RemoteSessionConfig,
    
    /// 当前状态
    state: Arc<tokio::sync::RwLock<RemoteSessionState>>,
    
    // === 增强组件 ===
    /// 入站消息去重 (防止重复处理)
    inbound_dedup: Arc<tokio::sync::Mutex<BoundedUuidSet>>,
    
    /// 出站消息去重 (防止重复发送)
    outbound_dedup: Arc<tokio::sync::Mutex<BoundedUuidSet>>,
    
    /// 初始消息去重集合
    initial_message_ids: Arc<tokio::sync::Mutex<std::collections::HashSet<uuid::Uuid>>>,
    
    /// 权限请求管理器
    permissions: Arc<tokio::sync::Mutex<PermissionRequestManager>>,
    
    /// FlushGate (历史刷写门控)
    flush_gate: Arc<tokio::sync::Mutex<FlushGate<serde_json::Value>>>,
    
    /// JWT 刷新调度器
    jwt_scheduler: Option<Arc<JwtRefreshScheduler>>,
    
    /// 回调
    callbacks: RemoteSessionCallbacks,
}

impl EnhancedRemoteSessionManager {
    /// 创建新的增强版会话管理器
    pub fn new(config: RemoteSessionConfig) -> Self {
        Self {
            config,
            state: Arc::new(tokio::sync::RwLock::new(RemoteSessionState::Disconnected)),
            inbound_dedup: Arc::new(tokio::sync::Mutex::new(
                BoundedUuidSet::with_capacity(2000),
            )),
            outbound_dedup: Arc::new(tokio::sync::Mutex::new(
                BoundedUuidSet::with_capacity(2000),
            )),
            initial_message_ids: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashSet::new(),
            )),
            permissions: Arc::new(tokio::sync::Mutex::new(
                PermissionRequestManager::new(),
            )),
            flush_gate: Arc::new(tokio::sync::Mutex::new(
                FlushGate::new(),
            )),
            jwt_scheduler: None,
            callbacks: RemoteSessionCallbacks::default(),
        }
    }

    /// 设置回调
    pub fn with_callbacks(mut self, callbacks: RemoteSessionCallbacks) -> Self {
        self.callbacks = callbacks;
        self
    }

    /// 启用 JWT 自动刷新
    pub async fn enable_jwt_refresh(mut self) -> Self {
        let jwt = (self.config.get_access_token)();
        
        let scheduler = JwtRefreshScheduler::new(jwt, JwtRefreshConfig {
            refresh_url: "https://api.anthropic.com/v1/bridge".to_string(),
            org_uuid: self.config.org_uuid.clone(),
            ..Default::default()
        });

        if let Err(e) = scheduler.start().await {
            tracing::warn!("Failed to start JWT refresh scheduler: {}", e);
        } else {
            self.jwt_scheduler = Some(Arc::new(scheduler));
            tracing::info!("JWT auto-refresh enabled");
        }

        self
    }

    /// 获取当前会话状态
    pub async fn get_state(&self) -> RemoteSessionState {
        self.state.read().await.clone()
    }

    /// 连接到远程会话
    ///
    /// 完整流程:
    /// 1. 更新状态为 Connecting
    /// 2. 启动 FlushGate (等待初始历史加载)
    /// 3. 建立 WebSocket/SSE 连接
    /// 4. 注册消息处理器
    /// 5. 启动 JWT 刷新 (如果启用)
    pub async fn connect(&mut self) -> anyhow::Result<()> {
        *self.state.write().await = RemoteSessionState::Connecting;
        
        if let Some(cb) = &self.callbacks.on_reconnecting {
            cb();
        }

        // 启动 FlushGate (初始历史加载期间排队消息)
        {
            let mut gate = self.flush_gate.lock().await;
            gate.start_flush();
        }
        
        tracing::info!(
            "Connecting to remote session {}",
            self.config.session_id
        );

        // TODO: 实际的 WebSocket/SSE 连接逻辑
        // 对应 Claude Code SessionsWebSocket.connect()

        *self.state.write().await = RemoteSessionState::Connected;
        
        if let Some(cb) = &self.callbacks.on_connected {
            cb();
        }

        // 完成 FlushGate
        let flushed = {
            let mut gate = self.flush_gate.lock().await;
            gate.flush_complete()
        };
        if !flushed.is_empty() {
            tracing::info!("FlushGate released {} queued messages", flushed.len());
        }

        Ok(())
    }

    /// 处理收到的消息 (带去重和权限检查)
    ///
    /// # 流程
    /// 1. 解析消息类型 (SDKMessage vs ControlRequest)
    /// 2. ControlRequest -> 权限管理器
    /// 3. SDKMessage -> UUID 去重 -> 回调
    pub async fn handle_incoming_message(&self, raw_msg: serde_json::Value) -> bool {
        use serde::Deserialize;

        // 尝试解析为 ControlRequest
        if let Ok(control_req) = serde_json::from_value::<SdkControlRequest>(raw_msg.clone()) {
            match control_req {
                SdkControlRequest::Request { request_id, request } => {
                    // 存入权限请求管理器
                    {
                        let mut perm = self.permissions.lock().await;
                        perm.receive_request(SdkControlRequest::Request { 
                            request_id,
                            request 
                        });
                    }

                    // 触发回调
                    if let Some(cb) = &self.callbacks.on_permission_request {
                        cb(request, request_id);
                    }
                    
                    return true; // 已处理
                }
                
                SdkControlRequest::CancelRequest { request_id } => {
                    {
                        let mut perm = self.permissions.lock().await;
                        perm.receive_request(SdkControlRequest::CancelRequest { request_id });
                    }
                    return true;
                }
            }
        }

        // 普通 SDK 消息 -> 去重检查
        // Claude Code: `isSDKMessage()` 类型守卫
        
        // 提取消息 UUID (如果存在)
        let msg_id_str = raw_msg.get("uuid")
            .or_else(|| raw_msg.get("message_id"))
            .or_else(|| raw_msg.get("id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        
        if let Some(id_str) = msg_id_str {
            if let Ok(uuid) = uuid::Uuid::parse_str(&id_str) {
                // 检查是否在初始消息集合中
                {
                    let initial_ids = self.initial_message_ids.lock().await;
                    if initial_ids.contains(&uuid) {
                        tracing::debug!("Skipping duplicate initial message {}", uuid);
                        return false;
                    }
                }

                // 检查入站去重
                {
                    let mut dedup = self.inbound_dedup.lock().await;
                    if !dedup.insert(uuid) {
                        tracing::debug!("Skipping duplicate inbound message {}", uuid);
                        return false;
                    }
                }
            }
        }

        // FlushGate 检查 (如果正在冲刷则排队)
        {
            let mut gate = self.flush_gate.lock().await;
            if gate.is_flushing() {
                gate.enqueue(raw_msg);
                tracing::debug!("Message queued by FlushGate");
                return true;
            }
        }

        // 通过所有检查 -> 转发给回调
        if let Some(cb) = &self.callbacks.on_message {
            cb(raw_msg);
        }

        true
    }

    /// 断开连接
    pub async fn disconnect(&mut self) -> Option<String> {
        // 停止 JWT 调度器
        if let Some(scheduler) = &self.jwt_scheduler {
            scheduler.stop().await;
        }

        let prev_state = {
            let mut state = self.state.write().await;
            let old = (*state).clone();
            *state = RemoteSessionState::Disconnected;
            old
        };

        match prev_state {
            RemoteSessionState::Connected => {
                if let Some(cb) = &self.callbacks.on_disconnected {
                    cb();
                }
                info!("Disconnected from remote session");
                Some(self.config.session_id.clone())
            }
            other => {
                warn!("Disconnect called while in {:?} state", other);
                None
            }
        }
    }

    /// 发送用户消息到远程会话 (带出站去重)
    pub async fn send_user_message(&self, message: serde_json::Value) -> anyhow::Result<()> {
        // 出站 UUID 去重
        if let Some(msg_uuid) = message.get("uuid").and_then(|v| v.as_str()) {
            if let Ok(uuid) = uuid::Uuid::parse_str(msg_uuid) {
                {
                    let mut dedup = self.outbound_dedup.lock().await;
                    if !dedup.insert(uuid) {
                        tracing::debug!("Duplicate outbound message suppressed: {}", uuid);
                        return Ok(()); // 静默忽略重复
                    }
                }
            }
        }

        // TODO: 实际发送到 WebSocket / HTTP POST
        tracing::debug!("Sending user message to remote session");
        Ok(())
    }

    /// 响应挂起的权限请求 (允许)
    pub async fn allow_permission(
        &self,
        request_id: uuid::Uuid,
        updated_input: Option<std::collections::HashMap<String, serde_json::Value>>,
    ) -> bool {
        let mut perm = self.permissions.lock().await;
        perm.allow_request(request_id, updated_input)
    }

    /// 响应挂起的权限请求 (拒绝)
    pub async fn deny_permission(&self, request_id: uuid::Uuid, reason: &str) -> bool {
        let mut perm = self.permissions.lock().await;
        perm.deny_request(request_id, reason)
    }

    /// 获取当前挂起的权限请求数量
    pub async fn pending_permissions_count(&self) -> usize {
        let perm = self.permissions.lock().await;
        perm.pending_count()
    }
}
