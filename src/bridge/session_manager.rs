//! # Session Manager — 会话生命周期管理
//!
//! 对应 Claude Code 的 `createSession.ts` (~2,000行)
//!
//! 核心职责：
//! - 会话创建 (新对话 / 恢复旧会话)
//! - 会话状态机 (Created → Active → Paused → Closed)
//! - 断线重连 & 状态恢复
//! - 会话持久化 (checkpoint)
//! - 多 IDE 客户端会话同步
//! - 超时 & 清理

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info, warn};
use uuid::Uuid;
use chrono::{DateTime, Utc};

// ════════════════════════════════════════════════════════════════
// 会话状态类型
// ════════════════════════════════════════════════════════════════

/// 会话状态枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    Created,
    Initializing,
    Active,
    Idle,
    Paused,
    Disconnecting,
    Reconnecting,
    Closing,
    Closed,
    Error,
}

impl SessionState {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Closed | Self::Error)
    }
    
    pub fn can_transition_to(&self, target: &SessionState) -> bool {
        match (self, target) {
            (Self::Created, Self::Initializing) => true,
            (Self::Initializing, Self::Active) => true,
            (Self::Initializing, Self::Error) => true,
            (Self::Active, Self::Idle) => true,
            (Self::Active, Self::Paused) => true,
            (Self::Active, Self::Disconnecting) => true,
            (Self::Idle, Self::Active) => true,
            (Self::Idle, Self::Disconnecting) => true,
            (Self::Paused, Self::Active) => true,
            (Self::Paused, Self::Closing) => true,
            (Self::Disconnecting, Self::Reconnecting) => true,
            (Self::Disconnecting, Self::Closed) => true,
            (Self::Reconnecting, Self::Active) => true,
            (Self::Reconnecting, Self::Error) => true,
            (Self::Closing, Self::Closed) => true,
            (_, Self::Error) => true,
            _ => false,
        }
    }
    
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Initializing => "initializing",
            Self::Active => "active",
            Self::Idle => "idle",
            Self::Paused => "paused",
            Self::Disconnecting => "disconnecting",
            Self::Reconnecting => "reconnecting",
            Self::Closing => "closing",
            Self::Closed => "closed",
            Self::Error => "error",
        }
    }
}

/// 会话错误类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionError {
    pub code: i32,
    pub message: String,
    pub recoverable: bool,
}

impl SessionError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            recoverable: false,
        }
    }
    
    pub fn recoverable(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            recoverable: true,
        }
    }
    
    pub fn not_found(id: &str) -> Self {
        Self::new(404, format!("Session {} not found", id))
    }
    
    pub fn invalid_state(from: SessionState, to: SessionState) -> Self {
        Self::new(409, format!("Cannot transition from {:?} to {:?}", from, to))
    }
    
    pub fn timeout() -> Self {
        Self::recoverable(408, "Session operation timed out")
    }
    
    pub fn disconnected() -> Self {
        Self::recoverable(503, "Client disconnected")
    }
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for SessionError {}

// ════════════════════════════════════════════════════════════════
// 会话配置
// ════════════════════════════════════════════════════════════════

/// 创建会话选项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCreateOptions {
    /// 工作区路径
    pub workspace_path: Option<PathBuf>,
    
    /// 初始提示
    pub initial_prompt: Option<String>,
    
    /// IDE 连接 ID
    pub connection_id: String,
    
    /// IDE 类型
    pub ide_type: String,
    
    /// 用户提供的上下文
    pub context_files: Vec<String>,
    
    /// 是否启用自动保存
    pub auto_save: bool,
    
    /// 最大空闲时间 (秒)
    pub max_idle_secs: u64,
    
    /// 自定义元数据
    pub metadata: HashMap<String, String>,
    
    /// 父会话 ID (用于 fork)
    pub parent_session_id: Option<String>,
}

impl Default for SessionCreateOptions {
    fn default() -> Self {
        Self {
            workspace_path: None,
            initial_prompt: None,
            connection_id: String::new(),
            ide_type: "unknown".to_string(),
            context_files: Vec::new(),
            auto_save: true,
            max_idle_secs: 300,
            metadata: HashMap::new(),
            parent_session_id: None,
        }
    }
}

/// 恢复会话选项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResumeOptions {
    /// 要恢复的会话 ID
    pub session_id: String,
    
    /// 新的连接 ID
    pub connection_id: String,
    
    /// 是否强制恢复 (忽略状态检查)
    pub force: bool,
    
    /// 恢复点 (checkpoint index)
    pub checkpoint_index: Option<u32>,
}

// ════════════════════════════════════════════════════════════════
// 会话句柄
// ════════════════════════════════════════════════════════════════

/// 会话内部数据
#[derive(Debug)]
struct SessionData {
    handle: SessionHandle,
    state: SessionState,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    last_activity_at: DateTime<Utc>,
    message_count: u64,
    checkpoint_count: u32,
    reconnect_attempts: u32,
    error: Option<SessionError>,
}

/// 公开会话句柄
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionHandle {
    /// 唯一会话 ID
    pub session_id: String,
    
    /// 关联的连接 ID
    pub connection_id: String,
    
    /// IDE 类型
    pub ide_type: String,
    
    /// 工作区路径
    pub workspace_path: Option<String>,
    
    /// 初始提示
    pub initial_prompt: Option<String>,
    
    /// 上下文文件列表
    pub context_files: Vec<String>,
    
    /// 元数据
    pub metadata: HashMap<String, String>,
    
    /// 父会话 ID
    pub parent_session_id: Option<String>,
    
    /// 子会话 IDs
    pub child_session_ids: Vec<String>,
}

impl SessionHandle {
    pub fn id(&self) -> &str {
        &self.session_id
    }
    
    pub fn is_active(&self) -> bool {
        true
    }
}

// ════════════════════════════════════════════════════════════════
// Checkpoint 数据结构
// ════════════════════════════════════════════════════════════════

/// 会话快照/检查点
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCheckpoint {
    /// 序列号
    pub sequence: u32,
    
    /// 时间戳
    pub timestamp: DateTime<Utc>,
    
    /// 会话状态摘要
    pub state_summary: serde_json::Value,
    
    /// 消息历史摘要
    pub messages_summary: Vec<MessageSummary>,
    
    /// 文件变更列表
    pub file_changes: Vec<FileChangeRecord>,
    
    /// 大小估算 (bytes)
    pub estimated_size_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageSummary {
    pub role: String,
    pub content_preview: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChangeRecord {
    pub path: PathBuf,
    pub change_type: FileChangeType,
    pub size_delta: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileChangeType {
    Created,
    Modified,
    Deleted,
    Renamed,
}

// ════════════════════════════════════════════════════════════════
// Session Manager 核心
// ════════════════════════════════════════════════════════════════

/// 会话管理器
#[derive(Debug)]
pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<String, SessionData>>>,
    checkpoints: Arc<RwLock<HashMap<String, Vec<SessionCheckpoint>>>>,
    shutdown_tx: Option<mpsc::Sender<()>>,
    config: SessionManagerConfig,
}

/// 会话管理器配置
#[derive(Debug, Clone)]
struct SessionManagerConfig {
    max_sessions: usize,
    cleanup_interval_secs: u64,
    default_max_idle_secs: u64,
    max_reconnect_attempts: u32,
    checkpoint_interval_secs: u64,
    max_checkpoints_per_session: usize,
}

impl Default for SessionManagerConfig {
    fn default() -> Self {
        Self {
            max_sessions: 1000,
            cleanup_interval_secs: 60,
            default_max_idle_secs: 300,
            max_reconnect_attempts: 5,
            checkpoint_interval_secs: 30,
            max_checkpoints_per_session: 10,
        }
    }
}

impl SessionManager {
    /// 创建新的会话管理器
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            checkpoints: Arc::new(RwLock::new(HashMap::new())),
            shutdown_tx: None,
            config: SessionManagerConfig::default(),
        }
    }

    /// 创建新会话
    pub async fn create(
        &self,
        options: SessionCreateOptions,
    ) -> Result<SessionHandle, SessionError> {
        let session_id = Uuid::new_v4().to_string();
        
        let current_count = {
            let sessions = self.sessions.read().await;
            sessions.len()
        };
        
        if current_count >= self.config.max_sessions {
            return Err(SessionError::new(
                503,
                format!("Maximum sessions ({}) reached", self.config.max_sessions),
            ));
        }
        
        let now = Utc::now();
        let handle = SessionHandle {
            session_id: session_id.clone(),
            connection_id: options.connection_id.clone(),
            ide_type: options.ide_type.clone(),
            workspace_path: options.workspace_path.as_ref().map(|p| p.to_string_lossy().to_string()),
            initial_prompt: options.initial_prompt.clone(),
            context_files: options.context_files.clone(),
            metadata: options.metadata.clone(),
            parent_session_id: options.parent_session_id.clone(),
            child_session_ids: Vec::new(),
        };
        
        let data = SessionData {
            handle: handle.clone(),
            state: SessionState::Created,
            created_at: now,
            updated_at: now,
            last_activity_at: now,
            message_count: 0,
            checkpoint_count: 0,
            reconnect_attempts: 0,
            error: None,
        };
        
        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(session_id.clone(), data);
        }
        
        if let Some(parent_id) = &options.parent_session_id {
            let mut sessions = self.sessions.write().await;
            if let Some(parent) = sessions.get_mut(parent_id) {
                parent.handle.child_session_ids.push(session_id.clone());
            }
        }
        
        info!("[SessionMgr] Created session {} for connection {}", session_id, options.connection_id);
        
        Ok(handle)
    }

    /// 初始化会话 (进入 Active 状态)
    pub async fn initialize(
        &self,
        session_id: &str,
    ) -> Result<(), SessionError> {
        self.transition_state(session_id, SessionState::Initializing).await?;
        self.transition_state(session_id, SessionState::Active).await?;
        
        Ok(())
    }

    /// 恢复已存在的会话
    pub async fn resume(
        &self,
        options: SessionResumeOptions,
    ) -> Result<SessionHandle, SessionError> {
        let mut sessions = self.sessions.write().await;
        
        let data = sessions.get_mut(&options.session_id).ok_or_else(|| {
            SessionError::not_found(&options.session_id)
        })?;
        
        if !options.force && !matches!(data.state, SessionState::Paused | SessionState::Disconnecting) {
            return Err(SessionError::invalid_state(data.state, SessionState::Reconnecting));
        }
        
        data.state = SessionState::Reconnecting;
        data.reconnect_attempts += 1;
        data.handle.connection_id = options.connection_id.clone();
        data.updated_at = Utc::now();
        data.error = None;
        
        let handle = data.handle.clone();
        
        info!("[SessionMgr] Resumed session {} (attempt #{}) for connection {}",
              options.session_id, data.reconnect_attempts, options.connection_id);
        
        Ok(handle)
    }

    /// 暂停会话
    pub async fn pause(&self, session_id: &str) -> Result<(), SessionError> {
        self.transition_state(session_id, SessionState::Paused).await
    }

    /// 关闭会话
    pub async fn close(&self, session_id: &str) -> Result<(), SessionError> {
        self.transition_state(session_id, SessionState::Closing).await?;
        self.transition_state(session_id, SessionState::Closed).await?;
        
        let mut sessions = self.sessions.write().await;
        sessions.remove(session_id);
        
        let mut checkpoints = self.checkpoints.write().await;
        checkpoints.remove(session_id);
        
        info!("[SessionMgr] Closed session {}", session_id);
        
        Ok(())
    }

    /// 标记断线
    pub async fn mark_disconnected(&self, session_id: &str) -> Result<(), SessionError> {
        self.transition_state(session_id, SessionState::Disconnecting).await
    }

    /// 获取会话信息
    pub async fn get_session(&self, session_id: &str) -> Result<SessionHandle, SessionError> {
        let sessions = self.sessions.read().await;
        
        sessions.get(session_id)
            .map(|data| data.handle.clone())
            .ok_or_else(|| SessionError::not_found(session_id))
    }

    /// 获取会话状态
    pub async fn get_state(&self, session_id: &str) -> Result<SessionState, SessionError> {
        let sessions = self.sessions.read().await;
        
        sessions.get(session_id)
            .map(|data| data.state)
            .ok_or_else(|| SessionError::not_found(session_id))
    }

    /// 更新活动时间
    pub async fn touch(&self, session_id: &str) -> Result<(), SessionError> {
        let mut sessions = self.sessions.write().await;
        
        if let Some(data) = sessions.get_mut(session_id) {
            data.last_activity_at = Utc::now();
            data.message_count += 1;
            Ok(())
        } else {
            Err(SessionError::not_found(session_id))
        }
    }

    /// 创建 checkpoint
    pub async fn create_checkpoint(
        &self,
        session_id: &str,
        state_summary: serde_json::Value,
    ) -> Result<u32, SessionError> {
        let sessions = self.sessions.read().await;
        let data = sessions.get(session_id).ok_or_else(|| SessionError::not_found(session_id))?;
        
        let sequence = data.checkpoint_count + 1;
        
        let checkpoint = SessionCheckpoint {
            sequence,
            timestamp: Utc::now(),
            state_summary,
            messages_summary: Vec::new(),
            file_changes: Vec::new(),
            estimated_size_bytes: 0,
        };
        
        drop(sessions);
        
        let mut checkpoints = self.checkpoints.write().await;
        let list = checkpoints.entry(session_id.to_string()).or_insert_with(Vec::new);
        
        if list.len() >= self.config.max_checkpoints_per_session {
            list.remove(0);
        }
        
        list.push(checkpoint);
        
        let mut sessions = self.sessions.write().await;
        if let Some(data) = sessions.get_mut(session_id) {
            data.checkpoint_count = sequence;
        }
        
        debug!("[SessionMgr] Checkpoint #{} for session {}", sequence, session_id);
        
        Ok(sequence)
    }

    /// 获取 checkpoint 列表
    pub async fn get_checkpoints(&self, session_id: &str) -> Result<Vec<SessionCheckpoint>, SessionError> {
        let checkpoints = self.checkpoints.read().await;
        
        checkpoints.get(session_id)
            .cloned()
            .ok_or_else(|| SessionError::not_found(session_id))
    }

    /// 列出所有活跃会话
    pub async fn list_active(&self) -> Vec<SessionHandle> {
        let sessions = self.sessions.read().await;
        
        sessions.values()
            .filter(|d| !d.state.is_terminal())
            .map(|d| d.handle.clone())
            .collect()
    }

    /// 获取统计信息
    pub async fn get_stats(&self) -> SessionStats {
        let sessions = self.sessions.read().await;
        
        let total = sessions.len();
        let active = sessions.values().filter(|d| d.state == SessionState::Active).count();
        let idle = sessions.values().filter(|d| d.state == SessionState::Idle).count();
        let paused = sessions.values().filter(|d| d.state == SessionState::Paused).count();
        let error = sessions.values().filter(|d| d.state == SessionState::Error).count();
        
        SessionStats { total, active, idle, paused, error }
    }

    /// 清理过期会话
    pub async fn cleanup_expired(&self) -> u64 {
        let now = Utc::now();
        let expired: Vec<String> = {
            let sessions = self.sessions.read().await;
            sessions.iter()
                .filter(|(_, d)| {
                    matches!(d.state, SessionState::Idle | SessionState::Paused | SessionState::Disconnecting)
                        && (now - d.last_activity_at).num_seconds() as u64 > self.config.default_max_idle_secs
                })
                .map(|(id, _)| id.clone())
                .collect()
        };
        
        for session_id in expired.iter() {
            if let Err(e) = self.close(session_id).await {
                warn!("[SessionMgr] Cleanup failed for {}: {}", session_id, e);
            }
        }
        
        let count = expired.len() as u64;
        if count > 0 {
            info!("[SessionMgr] Cleaned up {} expired sessions", count);
        }
        
        count
    }

    /// 关闭所有会话
    pub async fn shutdown_all(&self) -> Result<u64, anyhow::Error> {
        let ids: Vec<String> = {
            let sessions = self.sessions.read().await;
            sessions.keys().cloned().collect()
        };
        
        let mut closed = 0u64;
        for id in ids {
            if self.close(&id).await.is_ok() {
                closed += 1;
            }
        }
        
        Ok(closed)
    }

    // ─── 内部方法 ──────────────────────────────

    async fn transition_state(
        &self,
        session_id: &str,
        target: SessionState,
    ) -> Result<(), SessionError> {
        let mut sessions = self.sessions.write().await;
        
        let data = sessions.get_mut(session_id).ok_or_else(|| {
            SessionError::not_found(session_id)
        })?;
        
        if !data.state.can_transition_to(&target) {
            return Err(SessionError::invalid_state(data.state, target));
        }
        
        debug!("[SessionMgr] Session {}: {:?} -> {:?}", session_id, data.state, target);
        
        data.state = target;
        data.updated_at = Utc::now();
        
        Ok(())
    }
}

/// 会话统计信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStats {
    pub total: usize,
    pub active: usize,
    pub idle: usize,
    pub paused: usize,
    pub error: usize,
}
