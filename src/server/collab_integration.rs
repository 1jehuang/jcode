use super::collab::*;
use crate::config::Config;
use anyhow::{Context, Result};
use std::sync::Arc;
use uuid::Uuid;

pub use super::collab::*;

pub struct CollaborationManager {
    server: Arc<CollaborationServer>,
    config: CollabConfig,
}

impl CollaborationManager {
    pub fn new(_config: &Config) -> Self {
        let collab_config = CollabConfig::default();
        
        let (tx, _) = broadcast::channel(1024);
        
        let server = CollaborationServer {
            sessions: RwLock::new(HashMap::new()),
            participants: RwLock::new(HashMap::new()),
            document_store: Arc::new(DocumentStore::new()),
            change_broadcast: tx,
            presence: PresenceManager::new(),
            conflict_resolver: ConflictResolver::new(MergeStrategy::OperationalTransform),
            config: collab_config.clone(),
        };
        
        CollaborationManager {
            server: Arc::new(server),
            config: collab_config,
        }
    }

    pub async fn create_collab_session(
        &self,
        document_content: &str,
        owner_name: &str,
    ) -> Result<CollabSessionId> {
        let session_id = CollabSessionId::new();
        let owner_id = ParticipantId::new();
        
        let document = CollaborativeDocument {
            doc_id: Uuid::new_v4(),
            content: crate::utils::rope::Rope::from_str(document_content),
            version: VectorClock::default(),
            history: OperationLog::new(1000),
        };
        
        let session = CollabSession {
            id: session_id.clone(),
            document,
            participants: std::collections::HashSet::from([owner_id.clone()]),
            owner_id,
            created_at: chrono::Utc::now(),
            settings: CollabSettings::default(),
            history: OperationLog::new(1000),
        };
        
        self.server.sessions.write().await.insert(session_id.clone(), session);
        
        let user_id = UserId::Anonymous;
        let participant = Participant {
            id: owner_id,
            user_id,
            display_name: owner_name.to_string(),
            avatar: None,
            role: ParticipantRole::Owner,
            permissions: PermissionSet::owner(),
            connection: ConnectionHandle::new(),
            joined_at: chrono::Utc::now(),
            last_activity: chrono::Utc::now(),
        };
        
        self.server.participants.write().await.insert(owner_id, participant);
        
        Ok(session_id)
    }

    pub async fn join_session(
        &self,
        session_id: &CollabSessionId,
        user_name: &str,
        role: ParticipantRole,
    ) -> Result<ParticipantId> {
        let mut sessions = self.server.sessions.write().await;
        let session = sessions.get_mut(session_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found"))?;
        
        if session.participants.len() >= self.config.max_participants_per_session {
            anyhow::bail!("Session is full");
        }
        
        let participant_id = ParticipantId::new();
        
        let permissions = match role {
            ParticipantRole::Owner => PermissionSet::owner(),
            ParticipantRole::Editor => PermissionSet::editor(),
            ParticipantRole::Viewer => PermissionSet::viewer(),
            ParticipantRole::Commenter => PermissionSet::commenter(),
        };
        
        let participant = Participant {
            id: participant_id.clone(),
            user_id: UserId::Anonymous,
            display_name: user_name.to_string(),
            avatar: None,
            role,
            permissions,
            connection: ConnectionHandle::new(),
            joined_at: chrono::Utc::now(),
            last_activity: chrono::Utc::now(),
        };
        
        session.participants.insert(participant_id.clone());
        self.server.participants.write().await.insert(participant_id.clone(), participant);
        
        let msg = ServerPushMessage::ParticipantJoined {
            info: ParticipantInfo {
                id: participant_id.clone(),
                display_name: user_name.to_string(),
                role,
                joined_at: chrono::Utc::now(),
            },
        };
        let _ = self.server.change_broadcast.send(msg);
        
        Ok(participant_id)
    }

    pub async fn leave_session(&self, session_id: &CollabSessionId, participant_id: &ParticipantId) -> Result<()> {
        let mut sessions = self.server.sessions.write().await;
        
        if let Some(session) = sessions.get_mut(session_id) {
            session.participants.remove(participant_id);
            
            if session.participants.is_empty() {
                sessions.remove(session_id);
            }
        }
        
        self.server.participants.write().await.remove(participant_id);
        
        let msg = ServerPushMessage::ParticipantLeft { participant_id: participant_id.clone() };
        let _ = self.server.change_broadcast.send(msg);
        
        Ok(())
    }

    pub async fn apply_operation(
        &self,
        session_id: &CollabSessionId,
        participant_id: &ParticipantId,
        op_type: OpType,
        position: Position,
        text: &str,
    ) -> Result<()> {
        let mut sessions = self.server.sessions.write().await;
        let session = sessions.get_mut(session_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found"))?;
        
        let participants = self.server.participants.read().await;
        let participant = participants.get(participant_id)
            .ok_or_else(|| anyhow::anyhow!("Participant not found"))?;
        
        if !participant.permissions.can_edit() {
            anyhow::bail!("Permission denied");
        }
        
        let mut version = session.document.version.clone();
        version.increment(participant_id);
        
        let operation = TextOperation {
            op_type,
            position: position.clone(),
            text: text.to_string(),
            participant_id: participant_id.clone(),
            timestamp: chrono::Utc::now(),
            version: version.clone(),
        };
        
        match op_type {
            OpType::Insert => {
                session.document.content.insert(position.line, position.column, text);
            }
            OpType::Delete => {
                session.document.content.remove(position.line, position.column, text.len());
            }
            OpType::Replace => {
                session.document.content.remove(position.line, position.column, text.len());
            }
        }
        
        session.document.version = version;
        
        let msg = match op_type {
            OpType::Insert => ServerPushMessage::TextInserted {
                participant: participant_id.clone(),
                position,
                text: text.to_string(),
                op_id: session.document.version.get(participant_id),
            },
            OpType::Delete => ServerPushMessage::TextDeleted {
                participant: participant_id.clone(),
                range: SelectionRange::new(position, Position::new(position.line, position.column + text.len())),
                op_id: session.document.version.get(participant_id),
            },
            OpType::Replace => ServerPushMessage::TextDeleted {
                participant: participant_id.clone(),
                range: SelectionRange::new(position, Position::new(position.line, position.column + text.len())),
                op_id: session.document.version.get(participant_id),
            },
        };
        let _ = self.server.change_broadcast.send(msg);
        
        Ok(())
    }

    pub async fn get_document_content(&self, session_id: &CollabSessionId) -> Result<String> {
        let sessions = self.server.sessions.read().await;
        let session = sessions.get(session_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found"))?;
        
        Ok(session.document.content.to_string())
    }

    pub async fn get_participants(&self, session_id: &CollabSessionId) -> Result<Vec<ParticipantInfo>> {
        let sessions = self.server.sessions.read().await;
        let participants = self.server.participants.read().await;
        
        let session = sessions.get(session_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found"))?;
        
        let infos: Vec<ParticipantInfo> = session.participants.iter()
            .filter_map(|pid| participants.get(pid))
            .map(|p| ParticipantInfo {
                id: p.id.clone(),
                display_name: p.display_name.clone(),
                role: p.role.clone(),
                joined_at: p.joined_at,
            })
            .collect();
        
        Ok(infos)
    }

    pub fn subscribe_to_changes(&self) -> broadcast::Receiver<ServerPushMessage> {
        self.server.change_broadcast.subscribe()
    }

    pub async fn get_session_info(&self, session_id: &CollabSessionId) -> Result<CollabSessionInfo> {
        let sessions = self.server.sessions.read().await;
        let session = sessions.get(session_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found"))?;
        
        Ok(CollabSessionInfo {
            id: session.id.clone(),
            participant_count: session.participants.len(),
            created_at: session.created_at,
            document_version: session.document.version.clone(),
        })
    }

    pub async fn list_sessions(&self) -> Vec<CollabSessionInfo> {
        let sessions = self.server.sessions.read().await;
        sessions.values()
            .map(|s| CollabSessionInfo {
                id: s.id.clone(),
                participant_count: s.participants.len(),
                created_at: s.created_at,
                document_version: s.document.version.clone(),
            })
            .collect()
    }
}

pub struct DocumentStore {
    documents: RwLock<HashMap<Uuid, CollaborativeDocument>>,
}

impl DocumentStore {
    pub fn new() -> Self {
        DocumentStore {
            documents: RwLock::new(HashMap::new()),
        }
    }

    pub async fn save(&self, doc_id: Uuid, document: &CollaborativeDocument) {
        self.documents.write().await.insert(doc_id, document.clone());
    }

    pub async fn load(&self, doc_id: &Uuid) -> Option<CollaborativeDocument> {
        self.documents.read().await.get(doc_id).cloned()
    }
}

pub struct ConnectionHandle {
    id: u64,
}

impl ConnectionHandle {
    pub fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        ConnectionHandle { id: NEXT_ID.fetch_add(1, Ordering::Relaxed) }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParticipantInfo {
    pub id: ParticipantId,
    pub display_name: String,
    pub role: ParticipantRole,
    pub joined_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone, Debug)]
pub struct CollabSessionInfo {
    pub id: CollabSessionId,
    pub participant_count: usize,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub document_version: VectorClock,
}

pub async fn create_collaborative_session(
    manager: &CollaborationManager,
    initial_content: &str,
    owner_name: &str,
) -> Result<CollabSessionId> {
    manager.create_collab_session(initial_content, owner_name).await
}

pub async fn join_collaborative_session(
    manager: &CollaborationManager,
    session_id: &CollabSessionId,
    user_name: &str,
    role: ParticipantRole,
) -> Result<ParticipantId> {
    manager.join_session(session_id, user_name, role).await
}

pub async fn send_edit(
    manager: &CollaborationManager,
    session_id: &CollabSessionId,
    participant_id: &ParticipantId,
    op_type: OpType,
    position: Position,
    text: &str,
) -> Result<()> {
    manager.apply_operation(session_id, participant_id, op_type, position, text).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[tokio::test]
    async fn test_create_collab_manager() {
        let config = Config::default();
        let manager = CollaborationManager::new(&config);
        assert!(manager.config.max_participants_per_session > 0);
    }

    #[tokio::test]
    async fn test_create_and_join_session() {
        let config = Config::default();
        let manager = CollaborationManager::new(&config);
        
        let session_id = manager.create_collab_session("Hello World", "owner").await.unwrap();
        assert!(manager.get_session_info(&session_id).await.is_ok());
        
        let participant_id = manager.join_session(&session_id, "user", ParticipantRole::Editor).await.unwrap();
        assert!(manager.get_participants(&session_id).await.unwrap().len() == 2);
        
        manager.leave_session(&session_id, &participant_id).await.unwrap();
        assert!(manager.get_participants(&session_id).await.unwrap().len() == 1);
    }

    #[tokio::test]
    async fn test_apply_operation() {
        let config = Config::default();
        let manager = CollaborationManager::new(&config);
        
        let session_id = manager.create_collab_session("Hello", "owner").await.unwrap();
        let participants = manager.get_participants(&session_id).await.unwrap();
        let owner_id = &participants[0].id;
        
        manager.apply_operation(&session_id, owner_id, OpType::Insert, Position::new(0, 5), " World").await.unwrap();
        
        let content = manager.get_document_content(&session_id).await.unwrap();
        assert!(content.contains("Hello World"));
    }

    #[tokio::test]
    async fn test_vector_clock_merge() {
        let mut clock1 = VectorClock::default();
        let mut clock2 = VectorClock::default();
        
        let p1 = ParticipantId::new();
        let p2 = ParticipantId::new();
        
        clock1.increment(&p1);
        clock1.increment(&p1);
        clock2.increment(&p2);
        
        let merged = clock1.merge(&clock2);
        assert_eq!(merged.get(&p1), 2);
        assert_eq!(merged.get(&p2), 1);
    }

    #[tokio::test]
    async fn test_conflict_resolver() {
        let resolver = ConflictResolver::new(MergeStrategy::LastWriteWins);
        
        let local_op = TextOperation {
            op_type: OpType::Insert,
            position: Position::new(0, 0),
            text: "local".to_string(),
            participant_id: ParticipantId::new(),
            timestamp: chrono::Utc::now(),
            version: VectorClock::default(),
        };
        
        let remote_op = TextOperation {
            op_type: OpType::Insert,
            position: Position::new(0, 0),
            text: "remote".to_string(),
            participant_id: ParticipantId::new(),
            timestamp: chrono::Utc::now() + chrono::Duration::seconds(1),
            version: VectorClock::default(),
        };
        
        let resolved = resolver.resolve_concurrent_edits(&local_op, &[RemoteOperation { operation: remote_op, received_at: chrono::Utc::now() }], &VectorClock::default());
        assert!(resolved.had_conflicts);
    }
}