//! WebSocket 会话管理
//!
//! 管理客户端连接、状态和生命周期：
//! - 会话创建与销毁
//! - 状态持久化
//! - 心跳检测
//! - 权限管理

use parking_lot::RwLock;
use super::protocol::{DocumentState, TerminalSessionInfo, CollaboratorCursor};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;
use uuid::Uuid;
use tracing::{info};

/// 客户端会话状态
#[derive(Debug)]
pub struct ClientSession {
    /// 唯一会话 ID
    pub id: String,
    
    /// 客户端地址
    pub peer_addr: SocketAddr,
    
    /// 连接时间
    pub connected_at: Instant,
    
    /// 最后活动时间（用于心跳检测）
    pub last_activity: Arc<RwLock<Instant>>,
    
    /// 用户信息（可选，认证后填充）
    pub user_id: Option<String>,
    pub username: Option<String>,
    
    /// 当前打开的文档
    pub open_documents: Arc<RwLock<HashMap<String, DocumentState>>>,
    
    /// 活跃的终端会话
    pub terminal_sessions: Arc<Mutex<HashMap<String, TerminalSessionInfo>>>,
    
    /// 协作状态（如果加入协作编辑）
    pub collaboration: Arc<RwLock<Option<CollaborationState>>>,
    
    /// 工作目录
    pub working_directory: Option<String>,
    
    /// 客户端能力（支持的特性）
    pub capabilities: Vec<String>,
}

/// 协作编辑状态
#[derive(Debug, Clone)]
pub struct CollaborationState {
    /// 协作房间 ID
    pub room_id: String,
    
    /// 用户颜色
    pub user_color: String,
    
    /// 当前光标位置
    pub cursor: Option<CollaboratorCursor>,
    
    /// 加入时间
    pub joined_at: u64,
}

/// 会话管理器
pub struct SessionManager {
    /// 所有活跃会话 (session_id -> session)
    sessions: Arc<RwLock<HashMap<String, Arc<ClientSession>>>>,
    
    /// 地址到会话的映射（用于快速查找）
    addr_to_session: Arc<RwLock<HashMap<SocketAddr, String>>>,
    
    /// 心跳超时时间（秒）
    heartbeat_timeout: u64,
}

impl SessionManager {
    /// 创建新的会话管理器
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            addr_to_session: Arc::new(RwLock::new(HashMap::new())),
            heartbeat_timeout: 60,
        }
    }

    /// 创建新的客户端会话
    pub async fn create_session(&self, addr: SocketAddr) -> String {
        let session_id = Uuid::new_v4().to_string();
        
        let session = Arc::new(ClientSession {
            id: session_id.clone(),
            peer_addr: addr,
            connected_at: Instant::now(),
            last_activity: Arc::new(RwLock::new(Instant::now())),
            user_id: None,
            username: None,
            open_documents: Arc::new(RwLock::new(HashMap::new())),
            terminal_sessions: Arc::new(Mutex::new(HashMap::new())),
            collaboration: Arc::new(RwLock::new(None)),
            working_directory: None,
            capabilities: Vec::new(),
        });

        // 注册会话
        self.sessions.write().insert(session_id.clone(), session.clone());
        self.addr_to_session.write().insert(addr, session_id.clone());

        info!(
            session_id = %session_id,
            addr = %addr,
            total_sessions = self.sessions.read().len(),
            "New session created"
        );

        session_id
    }

    /// 获取会话
    pub fn get_session(&self, session_id: &str) -> Option<Arc<ClientSession>> {
        self.sessions.read().get(session_id).cloned()
    }

    /// 根据地址获取会话
    pub fn get_session_by_addr(&self, addr: &SocketAddr) -> Option<Arc<ClientSession>> {
        let binding = self.addr_to_session.read();
        let session_id = binding.get(addr)?;
        self.get_session(session_id)
    }

    /// 移除会话
    pub async fn remove_session(&self, session_id: &str) {
        // 先获取会话信息以便日志
        if let Some(session) = self.sessions.write().remove(session_id) {
            // 清理地址映射
            self.addr_to_session.write().remove(&session.peer_addr);
            
            info!(
                session_id = %session_id,
                addr = %session.peer_addr,
                duration_secs = session.connected_at.elapsed().as_secs(),
                remaining = self.sessions.read().len(),
                "Session removed"
            );
        }
    }

    /// 更新心跳时间戳
    pub async fn update_heartbeat(&self, session_id: &str) {
        if let Some(session) = self.get_session(session_id) {
            *session.last_activity.write() = Instant::now();
        }
    }

    /// 检查会话是否活跃
    pub fn is_session_alive(&self, session_id: &str) -> bool {
        if let Some(session) = self.get_session(session_id) {
            let last_activity = *session.last_activity.read();
            last_activity.elapsed().as_secs() < self.heartbeat_timeout
        } else {
            false
        }
    }

    /// 获取所有活跃会话数量
    pub fn active_count(&self) -> usize {
        self.sessions.read().len()
    }

    /// 设置用户信息（认证后调用）
    pub async fn set_user_info(
        &self,
        session_id: &str,
        user_id: &str,
        username: &str,
    ) -> Result<(), String> {
        let mut sessions = self.sessions.write();
        if let Some(session) = sessions.get_mut(session_id) {
            let session = Arc::<ClientSession>::get_mut(session).ok_or_else(|| "Session has multiple references".to_string())?;
            session.user_id = Some(user_id.to_string());
            session.username = Some(username.to_string());
            Ok(())
        } else {
            Err(format!("Session {} not found", session_id))
        }
    }

    /// 设置工作目录
    pub async fn set_working_directory(&self, session_id: &str, dir: &str) -> Result<(), String> {
        let mut sessions = self.sessions.write();
        if let Some(session) = sessions.get_mut(session_id) {
            let session = Arc::<ClientSession>::get_mut(session).ok_or_else(|| "Session has multiple references".to_string())?;
            session.working_directory = Some(dir.to_string());
            Ok(())
        } else {
            Err(format!("Session {} not found", session_id))
        }
    }

    /// 打开文档
    pub async fn open_document(
        &self,
        session_id: &str,
        doc: DocumentState,
    ) -> Result<(), String> {
        if let Some(session) = self.get_session(session_id) {
            session.open_documents.write().insert(doc.file_path.clone(), doc);
            Ok(())
        } else {
            Err(format!("Session {} not found", session_id))
        }
    }

    /// 关闭文档
    pub async fn close_document(&self, session_id: &str, file_path: &str) -> Result<(), String> {
        if let Some(session) = self.get_session(session_id) {
            session.open_documents.write().remove(file_path);
            Ok(())
        } else {
            Err(format!("Session {} not found", session_id))
        }
    }

    /// 获取打开的文档
    pub fn get_open_document(
        &self,
        session_id: &str,
        file_path: &str,
    ) -> Option<DocumentState> {
        if let Some(session) = self.get_session(session_id) {
            session.open_documents.read().get(file_path).cloned()
        } else {
            None
        }
    }

    /// 创建终端会话
    pub async fn create_terminal(
        &self,
        session_id: &str,
        term_info: TerminalSessionInfo,
    ) -> Result<(), String> {
        if let Some(session) = self.get_session(session_id) {
            let mut terminals = session.terminal_sessions.lock().await;
            terminals.insert(term_info.session_id.clone(), term_info);
            Ok(())
        } else {
            Err(format!("Session {} not found", session_id))
        }
    }

    /// 关闭终端会话
    pub async fn close_terminal(
        &self,
        session_id: &str,
        terminal_id: &str,
    ) -> Result<(), String> {
        if let Some(session) = self.get_session(session_id) {
            let mut terminals = session.terminal_sessions.lock().await;
            terminals.remove(terminal_id);
            Ok(())
        } else {
            Err(format!("Session {} not found", session_id))
        }
    }

    /// 加入协作房间
    pub async fn join_collaboration(
        &self,
        session_id: &str,
        room_id: &str,
        color: &str,
    ) -> Result<(), String> {
        if let Some(session) = self.get_session(session_id) {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;

            *session.collaboration.write() = Some(CollaborationState {
                room_id: room_id.to_string(),
                user_color: color.to_string(),
                cursor: None,
                joined_at: now,
            });
            Ok(())
        } else {
            Err(format!("Session {} not found", session_id))
        }
    }

    /// 离开协作房间
    pub async fn leave_collaboration(&self, session_id: &str) -> Result<(), String> {
        if let Some(session) = self.get_session(session_id) {
            *session.collaboration.write() = None;
            Ok(())
        } else {
            Err(format!("Session {} not found", session_id))
        }
    }

    /// 更新协作光标
    pub async fn update_cursor(
        &self,
        session_id: &str,
        cursor: CollaboratorCursor,
    ) -> Result<(), String> {
        if let Some(session) = self.get_session(session_id) {
            if let Some(ref mut collab) = *session.collaboration.write() {
                collab.cursor = Some(cursor);
            }
            Ok(())
        } else {
            Err(format!("Session {} not found", session_id))
        }
    }

    /// 获取同一房间的所有协作者光标
    pub fn get_collaborators_in_room(&self, room_id: &str) -> Vec<CollaboratorCursor> {
        let mut collaborators = Vec::new();
        
        for session in self.sessions.read().values() {
            if let Some(ref collab) = *session.collaboration.read() {
                if collab.room_id == room_id {
                    if let Some(cursor) = &collab.cursor {
                        collaborators.push(cursor.clone());
                    }
                }
            }
        }
        
        collaborators
    }

    /// 清理不活跃的会话
    pub async fn cleanup_stale_sessions(&self) -> usize {
        let stale_ids: Vec<String> = self
            .sessions
            .read()
            .iter()
            .filter(|(_, session)| {
                let last_activity = *session.last_activity.read();
                last_activity.elapsed().as_secs() >= self.heartbeat_timeout
            })
            .map(|(id, _)| id.clone())
            .collect();

        for session_id in &stale_ids {
            self.remove_session(session_id).await;
        }

        info!(
            cleaned_count = stale_ids.len(),
            remaining = self.sessions.read().len(),
            "Cleaned up stale sessions"
        );

        stale_ids.len()
    }
}
