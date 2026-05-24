use std::collections::HashMap;
use std::sync::Arc;
use async_trait::async_trait;
use tokio::sync::RwLock;
use carpai_internal::*;

pub struct MockSessionStore {
    sessions: Arc<RwLock<HashMap<SessionId, Vec<StoredMessage>>>>,
    metas: Arc<RwLock<HashMap<SessionId, SessionMeta>>>,
}

impl Default for MockSessionStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MockSessionStore {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            metas: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl SessionStore for MockSessionStore {
    async fn create_session(&self, meta: SessionMeta) -> Result<SessionId, SessionError> {
        let id = SessionId(uuid::Uuid::new_v4().to_string());
        self.sessions.write().await.insert(id.clone(), vec![]);
        self.metas.write().await.insert(id.clone(), meta);
        Ok(id)
    }

    async fn load_session(&self, id: &SessionId) -> Result<Option<LoadedSession>, SessionError> {
        let sessions = self.sessions.read().await;
        let metas = self.metas.read().await;
        if let Some(messages) = sessions.get(id) {
            let meta = metas.get(id).cloned().unwrap_or(SessionMeta {
                id: id.clone(),
                title: Some("Mock Session".into()),
                owner_id: None,
                state: SessionState::Active,
                model: None,
                working_dir: None,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                last_active_at: Some(chrono::Utc::now()),
                tags: HashMap::new(),
                message_count: messages.len(),
                parent_id: None,
            });
            Ok(Some(LoadedSession {
                meta,
                messages: messages.clone(),
                compaction: None,
            }))
        } else {
            Ok(None)
        }
    }

    async fn update_meta(
        &self,
        _id: &SessionId,
        _updates: SessionMetaUpdate,
    ) -> Result<(), SessionError> {
        Ok(())
    }

    async fn delete_session(&self, id: &SessionId, _hard: bool) -> Result<(), SessionError> {
        self.sessions.write().await.remove(id);
        self.metas.write().await.remove(id);
        Ok(())
    }

    async fn append_messages(
        &self,
        session_id: &SessionId,
        messages: Vec<StoredMessage>,
    ) -> Result<Vec<String>, SessionError> {
        let mut sessions = self.sessions.write().await;
        if let Some(existing) = sessions.get_mut(session_id) {
            let ids: Vec<String> = messages.iter().map(|m| m.id.clone()).collect();
            existing.extend(messages);
            Ok(ids)
        } else {
            Err(SessionError::NotFound(session_id.to_string()))
        }
    }

    async fn get_messages(
        &self,
        session_id: &SessionId,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<StoredMessage>, SessionError> {
        let sessions = self.sessions.read().await;
        if let Some(messages) = sessions.get(session_id) {
            Ok(messages.iter().skip(offset).take(limit).cloned().collect())
        } else {
            Err(SessionError::NotFound(session_id.to_string()))
        }
    }

    async fn message_count(&self, session_id: &SessionId) -> Result<usize, SessionError> {
        let sessions = self.sessions.read().await;
        if let Some(messages) = sessions.get(session_id) {
            Ok(messages.len())
        } else {
            Err(SessionError::NotFound(session_id.to_string()))
        }
    }

    async fn set_state(
        &self,
        _id: &SessionId,
        _new_state: SessionState,
    ) -> Result<(), SessionError> {
        Ok(())
    }

    async fn save_compaction(
        &self,
        _session_id: &SessionId,
        _snapshot: CompactionSnapshot,
    ) -> Result<(), SessionError> {
        Ok(())
    }

    async fn load_compaction(
        &self,
        _session_id: &SessionId,
    ) -> Result<Option<CompactionSnapshot>, SessionError> {
        Ok(None)
    }

    async fn list_sessions(
        &self,
        _filter: SessionFilter,
    ) -> Result<Vec<SessionMeta>, SessionError> {
        let metas = self.metas.read().await;
        Ok(metas.values().cloned().collect())
    }

    async fn count_sessions(
        &self,
        _filter: &SessionFilter,
    ) -> Result<usize, SessionError> {
        let metas = self.metas.read().await;
        Ok(metas.len())
    }
}
