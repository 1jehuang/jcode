//! SSO 会话管理模块

use super::{SsoSession, SsoUserInfo};
use chrono::{DateTime, Duration, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// SSO 会话状态
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    /// 活跃
    Active,
    /// 即将过期 (需要刷新)
    ExpiringSoon,
    /// 已过期
    Expired,
    /// 已撤销
    Revoked,
}

/// SSO 会话信息扩展
#[derive(Debug, Clone)]
pub struct SsoSessionExt {
    /// 基础会话信息
    pub base: SsoSession,
    /// 会话状态
    pub state: SessionState,
    /// 最后活动时间
    pub last_activity: DateTime<Utc>,
    /// IP 地址
    pub ip_address: Option<String>,
    /// 用户代理
    pub user_agent: Option<String>,
    /// 刷新计数
    pub refresh_count: u32,
    /// 元数据
    pub metadata: HashMap<String, String>,
}

impl SsoSessionExt {
    pub fn from_session(session: SsoSession) -> Self {
        Self {
            base: session,
            state: SessionState::Active,
            last_activity: Utc::now(),
            ip_address: None,
            user_agent: None,
            refresh_count: 0,
            metadata: HashMap::new(),
        }
    }

    /// 检查会话状态
    pub fn check_state(&mut self) -> SessionState {
        let now = Utc::now();
        
        // 检查是否已过期
        if now > self.base.expires_at {
            self.state = SessionState::Expired;
            return SessionState::Expired;
        }
        
        // 检查是否即将过期 (5分钟内)
        let expiry_threshold = self.base.expires_at - Duration::minutes(5);
        if now > expiry_threshold {
            self.state = SessionState::ExpiringSoon;
            return SessionState::ExpiringSoon;
        }
        
        self.state = SessionState::Active;
        SessionState::Active
    }

    /// 更新活动时间
    pub fn touch(&mut self) {
        self.last_activity = Utc::now();
    }

    /// 增加刷新计数
    pub fn increment_refresh(&mut self) {
        self.refresh_count += 1;
    }
}

/// SSO 会话存储
pub struct SsoSessionStore {
    sessions: Arc<RwLock<HashMap<String, SsoSessionExt>>>,
    user_sessions: Arc<RwLock<HashMap<String, Vec<String>>>>, // user_id -> session_ids
}

impl SsoSessionStore {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            user_sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 创建会话
    pub async fn create_session(
        &self,
        session: SsoSession,
        ip_address: Option<String>,
        user_agent: Option<String>,
    ) -> String {
        let session_id = session.session_id.clone();
        let user_id = session.user_id.clone();
        
        let ext = SsoSessionExt {
            base: session,
            state: SessionState::Active,
            last_activity: Utc::now(),
            ip_address,
            user_agent,
            refresh_count: 0,
            metadata: HashMap::new(),
        };
        
        // 存储会话
        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(session_id.clone(), ext);
        }
        
        // 更新用户会话索引
        {
            let mut user_sessions = self.user_sessions.write().await;
            user_sessions
                .entry(user_id)
                .or_insert_with(Vec::new)
                .push(session_id.clone());
        }
        
        session_id
    }

    /// 获取会话
    pub async fn get_session(&self, session_id: &str) -> Option<SsoSessionExt> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).cloned()
    }

    /// 删除会话
    pub async fn remove_session(&self, session_id: &str) -> Option<SsoSession> {
        let session = {
            let mut sessions = self.sessions.write().await;
            sessions.remove(session_id).map(|ext| ext.base)
        };
        
        // 从用户会话索引中移除
        if let Some(session) = &session {
            let user_id = session.user_id.clone();
            let mut user_sessions = self.user_sessions.write().await;
            if let Some(sessions) = user_sessions.get_mut(&user_id) {
                sessions.retain(|id| id != session_id);
            }
        }
        
        session
    }

    /// 获取用户的所有会话
    pub async fn get_user_sessions(&self, user_id: &str) -> Vec<SsoSessionExt> {
        let user_session_ids: Vec<String> = {
            let user_sessions = self.user_sessions.read().await;
            user_sessions.get(user_id).cloned().unwrap_or_default()
        };
        
        let sessions = self.sessions.read().await;
        user_session_ids
            .into_iter()
            .filter_map(|id| sessions.get(&id).cloned())
            .collect()
    }

    /// 撤销用户的所有会话
    pub async fn revoke_user_sessions(&self, user_id: &str) -> usize {
        let session_ids: Vec<String> = {
            let mut user_sessions = self.user_sessions.write().await;
            user_sessions.remove(user_id).unwrap_or_default()
        };
        
        let mut sessions = self.sessions.write().await;
        let mut count = 0;
        for id in session_ids {
            if let Some(mut ext) = sessions.remove(&id) {
                ext.state = SessionState::Revoked;
                count += 1;
            }
        }
        count
    }

    /// 清理过期会话
    pub async fn cleanup_expired(&self) -> usize {
        let mut sessions = self.sessions.write().await;
        let mut user_sessions = self.user_sessions.write().await;
        let now = Utc::now();
        let mut removed = 0;
        
        let expired_session_ids: Vec<String> = sessions
            .iter()
            .filter(|(_, ext)| ext.base.expires_at < now || ext.state == SessionState::Revoked)
            .map(|(id, _)| id.clone())
            .collect();
        
        for session_id in expired_session_ids {
            if let Some(ext) = sessions.remove(&session_id) {
                let user_id = ext.base.user_id;
                if let Some(user_vec) = user_sessions.get_mut(&user_id) {
                    user_vec.retain(|id| id != &session_id);
                }
                removed += 1;
            }
        }
        
        removed
    }

    /// 检查会话是否有效
    pub async fn is_session_valid(&self, session_id: &str) -> bool {
        let sessions = self.sessions.read().await;
        if let Some(ext) = sessions.get(session_id) {
            matches!(ext.state, SessionState::Active | SessionState::ExpiringSoon)
                && ext.base.expires_at > Utc::now()
        } else {
            false
        }
    }

    /// 更新会话状态
    pub async fn update_session_state(&self, session_id: &str) -> Option<SessionState> {
        let mut sessions = self.sessions.write().await;
        if let Some(ext) = sessions.get_mut(session_id) {
            Some(ext.check_state())
        } else {
            None
        }
    }
}

impl Default for SsoSessionStore {
    fn default() -> Self {
        Self::new()
    }
}

/// 会话统计信息
#[derive(Debug, Clone)]
pub struct SessionStats {
    pub total_sessions: usize,
    pub active_sessions: usize,
    pub expiring_soon_sessions: usize,
    pub expired_sessions: usize,
    pub revoked_sessions: usize,
}

impl SsoSessionStore {
    /// 获取会话统计
    pub async fn get_stats(&self) -> SessionStats {
        let sessions = self.sessions.read().await;
        let mut stats = SessionStats {
            total_sessions: sessions.len(),
            active_sessions: 0,
            expiring_soon_sessions: 0,
            expired_sessions: 0,
            revoked_sessions: 0,
        };
        
        for ext in sessions.values() {
            match ext.state {
                SessionState::Active => stats.active_sessions += 1,
                SessionState::ExpiringSoon => stats.expiring_soon_sessions += 1,
                SessionState::Expired => stats.expired_sessions += 1,
                SessionState::Revoked => stats.revoked_sessions += 1,
            }
        }
        
        stats
    }
}
