//! Session Manager — 会话生命周期管理
//!
//! ## 核心能力
//! - 会话创建/恢复/销毁
//! - 会话状态跟踪
//! - 自动保存机制

use crate::types::SessionId;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

/// 会话状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Active,
    Suspended,
    Completed,
    Error,
}

/// 会话信息
pub struct SessionInfo {
    pub id: SessionId,
    pub state: SessionState,
    pub created_at: std::time::Instant,
    pub updated_at: std::time::Instant,
    pub message_count: usize,
}

/// 会话管理器
pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<SessionId, SessionInfo>>>,
    event_tx: broadcast::Sender<SessionEvent>,
}

/// 会话事件
#[derive(Debug, Clone)]
pub enum SessionEvent {
    Created(SessionId),
    Updated(SessionId),
    Suspended(SessionId),
    Resumed(SessionId),
    Completed(SessionId),
    Error(SessionId, String),
}

impl SessionManager {
    /// 创建新的会话管理器
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(100);
        
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            event_tx,
        }
    }

    /// 创建新会话
    pub fn create_session(&self) -> SessionId {
        let id = SessionId(format!("session_{}", uuid::Uuid::new_v4()));
        
        let session = SessionInfo {
            id: id.clone(),
            state: SessionState::Active,
            created_at: std::time::Instant::now(),
            updated_at: std::time::Instant::now(),
            message_count: 0,
        };

        self.sessions.write().unwrap_or_else(|e| e.into_inner()).insert(id.clone(), session);
        let _ = self.event_tx.send(SessionEvent::Created(id.clone()));
        
        info!("Session created: {}", id);
        id
    }

    /// 获取会话信息
    pub fn get_session(&self, id: &SessionId) -> Option<SessionInfo> {
        self.sessions.read().unwrap_or_else(|e| e.into_inner()).get(id).cloned()
    }

    /// 暂停会话
    pub fn suspend_session(&self, id: &SessionId) -> anyhow::Result<()> {
        let mut sessions = self.sessions.write().unwrap_or_else(|e| e.into_inner());
        
        if let Some(session) = sessions.get_mut(id) {
            session.state = SessionState::Suspended;
            session.updated_at = std::time::Instant::now();
            
            drop(sessions);
            let _ = self.event_tx.send(SessionEvent::Suspended(id.clone()));
            
            info!("Session suspended: {}", id);
            Ok(())
        } else {
            Err(anyhow::anyhow!("Session not found: {}", id))
        }
    }

    /// 恢复会话
    pub fn resume_session(&self, id: &SessionId) -> anyhow::Result<()> {
        let mut sessions = self.sessions.write().unwrap_or_else(|e| e.into_inner());
        
        if let Some(session) = sessions.get_mut(id) {
            session.state = SessionState::Active;
            session.updated_at = std::time::Instant::now();
            
            drop(sessions);
            let _ = self.event_tx.send(SessionEvent::Resumed(id.clone()));
            
            info!("Session resumed: {}", id);
            Ok(())
        } else {
            Err(anyhow::anyhow!("Session not found: {}", id))
        }
    }

    /// 完成会话
    pub fn complete_session(&self, id: &SessionId) -> anyhow::Result<()> {
        let mut sessions = self.sessions.write().unwrap_or_else(|e| e.into_inner());
        
        if let Some(session) = sessions.get_mut(id) {
            session.state = SessionState::Completed;
            session.updated_at = std::time::Instant::now();
            
            drop(sessions);
            let _ = self.event_tx.send(SessionEvent::Completed(id.clone()));
            
            info!("Session completed: {}", id);
            Ok(())
        } else {
            Err(anyhow::anyhow!("Session not found: {}", id))
        }
    }

    /// 获取所有活跃会话
    pub fn active_sessions(&self) -> Vec<SessionInfo> {
        self.sessions.read().unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|s| s.state == SessionState::Active)
            .cloned()
            .collect()
    }

    /// 订阅事件
    pub fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.event_tx.subscribe()
    }
}
