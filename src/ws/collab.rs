//! # WebSocket 实时协作同步模块
//!
//! 实现基于 WebSocket 的实时协作编辑同步

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{RwLock, Mutex};
use tokio_tungstenite::{WebSocketStream, Message};
use futures_util::{StreamExt, SinkExt};
use serde::{Deserialize, Serialize};
use tracing::{info, warn, error};

use super::super::crdt::{CrdtOperation, LogicalClock, CrdtNodeId};

/// WebSocket 协作服务器
pub struct WebSocketCollabServer {
    sessions: Arc<RwLock<HashMap<String, SessionData>>>,
    pending_messages: Arc<Mutex<HashMap<String, Vec<CollabMessage>>>>,
    config: ServerConfig,
}

/// 会话数据
struct SessionData {
    participants: HashSet<String>,
    document_content: String,
    version: LogicalClock,
}

/// 服务器配置
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub port: u16,
    pub max_participants: usize,
    pub message_buffer_size: usize,
    pub ping_interval_ms: u64,
    pub max_message_size: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 8080,
            max_participants: 100,
            message_buffer_size: 1000,
            ping_interval_ms: 30000,
            max_message_size: 1024 * 1024, // 1MB
        }
    }
}

/// 协作消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CollabMessage {
    /// 加入会话
    JoinSession {
        session_id: String,
        participant_id: String,
        display_name: String,
        color: String,
    },
    /// 离开会话
    LeaveSession {
        session_id: String,
        participant_id: String,
    },
    /// 编辑操作
    Edit {
        session_id: String,
        participant_id: String,
        operation: CrdtOperation,
        version: LogicalClock,
    },
    /// 光标位置更新
    CursorUpdate {
        session_id: String,
        participant_id: String,
        position: CursorPosition,
        selection: Option<SelectionRange>,
    },
    /// 存在状态更新
    PresenceUpdate {
        session_id: String,
        participant_id: String,
        is_online: bool,
        is_typing: bool,
    },
    /// 文档同步请求
    SyncRequest {
        session_id: String,
        participant_id: String,
        client_version: LogicalClock,
    },
    /// 文档同步响应
    SyncResponse {
        session_id: String,
        content: String,
        version: LogicalClock,
        missed_operations: Vec<CrdtOperation>,
    },
    /// 冲突通知
    Conflict {
        session_id: String,
        conflict_id: String,
        description: String,
    },
    /// 心跳
    Ping,
    /// 心跳响应
    Pong,
}

/// 光标位置
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CursorPosition {
    pub line: usize,
    pub column: usize,
    pub absolute_offset: usize,
}

/// 选择范围
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SelectionRange {
    pub start: CursorPosition,
    pub end: CursorPosition,
}

/// 连接上下文
pub struct ConnectionContext {
    session_id: String,
    participant_id: String,
    stream: WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    config: ServerConfig,
}

impl WebSocketCollabServer {
    pub fn new(config: ServerConfig) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            pending_messages: Arc::new(Mutex::new(HashMap::new())),
            config,
        }
    }

    /// 处理新连接
    pub async fn handle_connection(&self, stream: WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>) {
        let mut conn = Connection {
            stream,
            session_id: None,
            participant_id: None,
            server: self.clone(),
        };
        
        conn.run().await;
    }

    /// 创建新会话
    pub async fn create_session(&self, session_id: &str, initial_content: &str) -> Result<(), String> {
        let mut sessions = self.sessions.write().await;
        
        if sessions.contains_key(session_id) {
            return Err("Session already exists".to_string());
        }
        
        sessions.insert(session_id.to_string(), SessionData {
            participants: HashSet::new(),
            document_content: initial_content.to_string(),
            version: LogicalClock::new(),
        });
        
        info!("Created new session: {}", session_id);
        Ok(())
    }

    /// 广播消息到会话中的所有参与者
    pub async fn broadcast_to_session(&self, session_id: &str, message: &CollabMessage, exclude: Option<&str>) {
        let sessions = self.sessions.read().await;
        let session = match sessions.get(session_id) {
            Some(s) => s,
            None => return,
        };
        
        let message_json = match serde_json::to_string(message) {
            Ok(m) => m,
            Err(e) => {
                error!("Failed to serialize message: {}", e);
                return;
            }
        };
        
        // 简化实现：实际应该通过 WebSocket 连接广播
        for participant in &session.participants {
            if exclude.as_ref() != Some(participant) {
                // 在实际实现中，这里会向参与者的 WebSocket 连接发送消息
                info!("Would broadcast to participant: {}", participant);
            }
        }
    }

    /// 获取会话信息
    pub async fn get_session_info(&self, session_id: &str) -> Option<SessionInfo> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).map(|s| SessionInfo {
            participant_count: s.participants.len(),
            document_length: s.document_content.len(),
        })
    }
}

#[derive(Debug, Clone)]
struct Connection {
    stream: WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    session_id: Option<String>,
    participant_id: Option<String>,
    server: WebSocketCollabServer,
}

impl Connection {
    async fn run(&mut self) {
        while let Some(msg) = self.stream.next().await {
            match msg {
                Ok(message) => {
                    if let Message::Text(text) = message {
                        self.handle_message(&text).await;
                    }
                }
                Err(e) => {
                    error!("WebSocket error: {}", e);
                    break;
                }
            }
        }
        
        // 清理连接
        if let (Some(session_id), Some(participant_id)) = (&self.session_id, &self.participant_id) {
            self.leave_session(session_id, participant_id).await;
        }
    }

    async fn handle_message(&mut self, text: &str) {
        match serde_json::from_str::<CollabMessage>(text) {
            Ok(msg) => {
                match msg {
                    CollabMessage::JoinSession { session_id, participant_id, display_name, color } => {
                        self.join_session(&session_id, &participant_id, &display_name, &color).await;
                    }
                    CollabMessage::LeaveSession { session_id, participant_id } => {
                        self.leave_session(&session_id, &participant_id).await;
                    }
                    CollabMessage::Edit { session_id, participant_id, operation, version } => {
                        self.handle_edit(&session_id, &participant_id, operation, version).await;
                    }
                    CollabMessage::CursorUpdate { session_id, participant_id, position, selection } => {
                        self.handle_cursor_update(&session_id, &participant_id, position, selection).await;
                    }
                    CollabMessage::PresenceUpdate { session_id, participant_id, is_online, is_typing } => {
                        self.handle_presence_update(&session_id, &participant_id, is_online, is_typing).await;
                    }
                    CollabMessage::SyncRequest { session_id, participant_id, client_version } => {
                        self.handle_sync_request(&session_id, &participant_id, client_version).await;
                    }
                    CollabMessage::Ping => {
                        self.send_pong().await;
                    }
                    _ => {}
                }
            }
            Err(e) => {
                warn!("Failed to parse message: {}", e);
            }
        }
    }

    async fn join_session(&mut self, session_id: &str, participant_id: &str, _display_name: &str, _color: &str) {
        let mut sessions = self.server.sessions.write().await;
        
        let session = sessions.entry(session_id.to_string()).or_insert_with(|| SessionData {
            participants: HashSet::new(),
            document_content: String::new(),
            version: LogicalClock::new(),
        });
        
        if session.participants.len() >= self.server.config.max_participants {
            warn!("Session {} is full", session_id);
            return;
        }
        
        session.participants.insert(participant_id.to_string());
        
        self.session_id = Some(session_id.to_string());
        self.participant_id = Some(participant_id.to_string());
        
        info!("Participant {} joined session {}", participant_id, session_id);
        
        // 广播加入消息
        let msg = CollabMessage::PresenceUpdate {
            session_id: session_id.to_string(),
            participant_id: participant_id.to_string(),
            is_online: true,
            is_typing: false,
        };
        self.server.broadcast_to_session(session_id, &msg, Some(participant_id)).await;
    }

    async fn leave_session(&mut self, session_id: &str, participant_id: &str) {
        let mut sessions = self.server.sessions.write().await;
        
        if let Some(session) = sessions.get_mut(session_id) {
            session.participants.remove(participant_id);
            
            info!("Participant {} left session {}", participant_id, session_id);
            
            // 广播离开消息
            let msg = CollabMessage::PresenceUpdate {
                session_id: session_id.to_string(),
                participant_id: participant_id.to_string(),
                is_online: false,
                is_typing: false,
            };
            self.server.broadcast_to_session(session_id, &msg, Some(participant_id)).await;
        }
    }

    async fn handle_edit(&mut self, session_id: &str, participant_id: &str, operation: CrdtOperation, version: LogicalClock) {
        let mut sessions = self.server.sessions.write().await;
        
        if let Some(session) = sessions.get_mut(session_id) {
            // 应用操作
            session.version.merge(&version);
            
            // 更新文档内容（简化实现）
            match operation {
                CrdtOperation::Insert { position, content, .. } => {
                    if position <= session.document_content.len() {
                        session.document_content.insert_str(position, &content);
                    }
                }
                CrdtOperation::Delete { position, length, .. } => {
                    let end = (position + length).min(session.document_content.len());
                    session.document_content.replace_range(position..end, "");
                }
                CrdtOperation::Update { .. } => {}
            }
            
            // 广播编辑消息
            let msg = CollabMessage::Edit {
                session_id: session_id.to_string(),
                participant_id: participant_id.to_string(),
                operation,
                version: session.version.clone(),
            };
            self.server.broadcast_to_session(session_id, &msg, Some(participant_id)).await;
        }
    }

    async fn handle_cursor_update(&self, session_id: &str, participant_id: &str, position: CursorPosition, selection: Option<SelectionRange>) {
        let msg = CollabMessage::CursorUpdate {
            session_id: session_id.to_string(),
            participant_id: participant_id.to_string(),
            position,
            selection,
        };
        self.server.broadcast_to_session(session_id, &msg, Some(participant_id)).await;
    }

    async fn handle_presence_update(&self, session_id: &str, participant_id: &str, is_online: bool, is_typing: bool) {
        let msg = CollabMessage::PresenceUpdate {
            session_id: session_id.to_string(),
            participant_id: participant_id.to_string(),
            is_online,
            is_typing,
        };
        self.server.broadcast_to_session(session_id, &msg, Some(participant_id)).await;
    }

    async fn handle_sync_request(&self, session_id: &str, _participant_id: &str, client_version: LogicalClock) {
        let sessions = self.server.sessions.read().await;
        
        if let Some(session) = sessions.get(session_id) {
            let msg = CollabMessage::SyncResponse {
                session_id: session_id.to_string(),
                content: session.document_content.clone(),
                version: session.version.clone(),
                missed_operations: Vec::new(),
            };
            
            // 简化实现：实际应该发送给特定客户端
            info!("Sync response for session {}", session_id);
        }
    }

    async fn send_pong(&mut self) {
        if let Err(e) = self.stream.send(Message::Text(serde_json::to_string(&CollabMessage::Pong).unwrap())).await {
            error!("Failed to send pong: {}", e);
        }
    }
}

/// 会话信息
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub participant_count: usize,
    pub document_length: usize,
}

impl Clone for WebSocketCollabServer {
    fn clone(&self) -> Self {
        Self {
            sessions: Arc::clone(&self.sessions),
            pending_messages: Arc::clone(&self.pending_messages),
            config: self.config.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_config_defaults() {
        let config = ServerConfig::default();
        
        assert_eq!(config.port, 8080);
        assert_eq!(config.max_participants, 100);
        assert_eq!(config.message_buffer_size, 1000);
        assert_eq!(config.ping_interval_ms, 30000);
    }

    #[test]
    fn test_server_create_session() {
        let server = WebSocketCollabServer::new(ServerConfig::default());
        
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let result = server.create_session("test-session", "Hello World").await;
            assert!(result.is_ok());
            
            let info = server.get_session_info("test-session").await;
            assert!(info.is_some());
            assert_eq!(info.unwrap().document_length, 11);
        });
    }

    #[test]
    fn test_collab_message_serialization() {
        let msg = CollabMessage::JoinSession {
            session_id: "test".to_string(),
            participant_id: "user1".to_string(),
            display_name: "Alice".to_string(),
            color: "#FF0000".to_string(),
        };
        
        let json = serde_json::to_string(&msg).unwrap();
        let restored: CollabMessage = serde_json::from_str(&json).unwrap();
        
        match restored {
            CollabMessage::JoinSession { participant_id, .. } => {
                assert_eq!(participant_id, "user1");
            }
            _ => panic!("Expected JoinSession"),
        }
    }

    #[test]
    fn test_cursor_position() {
        let pos = CursorPosition {
            line: 10,
            column: 5,
            absolute_offset: 105,
        };
        
        let json = serde_json::to_string(&pos).unwrap();
        let restored: CursorPosition = serde_json::from_str(&json).unwrap();
        
        assert_eq!(restored.line, 10);
        assert_eq!(restored.column, 5);
    }
}
