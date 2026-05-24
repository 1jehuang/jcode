use std::path::PathBuf;
use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use carpai_internal::*;
use tracing::{info, debug};

pub struct LocalFileSessionStore {
    base_path: PathBuf,
}

impl LocalFileSessionStore {
    pub fn new(base_path: PathBuf) -> Self {
        Self { base_path }
    }

    fn session_file(&self, id: &SessionId) -> PathBuf {
        self.base_path.join(format!("{}.jsonl", id))
    }

    async fn ensure_dir(&self) -> Result<(), SessionError> {
        tokio::fs::create_dir_all(&self.base_path)
            .await
            .map_err(|e| SessionError::Storage(e.to_string()))
    }
}

#[async_trait]
impl SessionStore for LocalFileSessionStore {
    async fn create_session(
        &self,
        meta: SessionMeta,
    ) -> Result<SessionId, SessionError> {
        self.ensure_dir().await?;

        let id = meta.id.clone();
        let path = self.session_file(&id);

        let initial_meta = serde_json::to_string_pretty(&meta)
            .map_err(|e| SessionError::Serialization(e.to_string()))?;

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&path)
            .await
            .map_err(|e| SessionError::Storage(e.to_string()))?;

        file.write_all(format!("# META\n{}\n", initial_meta).as_bytes())
            .await
            .map_err(|e| SessionError::Storage(e.to_string()))?;

        info!(session_id = %id, "Session created");
        Ok(id)
    }

    async fn load_session(
        &self,
        id: &SessionId,
    ) -> Result<Option<LoadedSession>, SessionError> {
        let path = self.session_file(id);

        if !path.exists() {
            return Ok(None);
        }

        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| SessionError::Storage(e.to_string()))?;

        let mut messages = Vec::new();
        let mut meta: Option<SessionMeta> = None;
        let compaction: Option<CompactionSnapshot> = None;

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Ok(m) = serde_json::from_str::<SessionMeta>(line) {
                meta = Some(m);
                continue;
            }
            if let Ok(msg) = serde_json::from_str::<StoredMessage>(line) {
                messages.push(msg);
            }
        }

        match meta {
            Some(m) => Ok(Some(LoadedSession {
                meta: m,
                messages,
                compaction,
            })),
            None => Err(SessionError::Internal(anyhow::anyhow!(
                "No metadata found in session file"
            ))),
        }
    }

    async fn update_meta(
        &self,
        id: &SessionId,
        updates: SessionMetaUpdate,
    ) -> Result<(), SessionError> {
        let loaded = self.load_session(id).await?
            .ok_or_else(|| SessionError::NotFound(id.to_string()))?;

        let mut new_meta = loaded.meta;
        if let Some(title) = updates.title {
            new_meta.title = Some(title);
        }
        if let Some(state) = updates.state {
            new_meta.state = state;
        }
        if let Some(model) = updates.model {
            new_meta.model = Some(model);
        }
        if let Some(working_dir) = updates.working_dir {
            new_meta.working_dir = Some(working_dir);
        }
        if let Some(last_active_at) = updates.last_active_at {
            new_meta.last_active_at = Some(last_active_at);
        }
        if let Some(tags) = updates.tags {
            new_meta.tags = tags;
        }

        let path = self.session_file(id);

        let mut out_lines = Vec::new();
        out_lines.push("# META".to_string());
        out_lines.push(serde_json::to_string(&new_meta)
            .map_err(|e| SessionError::Serialization(e.to_string()))?);

        for msg in &loaded.messages {
            out_lines.push(serde_json::to_string(msg)
                .map_err(|e| SessionError::Serialization(e.to_string()))?);
        }

        if let Some(ref snap) = loaded.compaction {
            out_lines.push("# COMPACTION".to_string());
            out_lines.push(serde_json::to_string(snap)
                .map_err(|e| SessionError::Serialization(e.to_string()))?);
        }

        let new_content = out_lines.join("\n") + "\n";
        tokio::fs::write(&path, new_content)
            .await
            .map_err(|e| SessionError::Storage(e.to_string()))?;

        debug!(session_id = %id, "Session metadata updated");
        Ok(())
    }

    async fn delete_session(
        &self,
        id: &SessionId,
        hard: bool,
    ) -> Result<(), SessionError> {
        let path = self.session_file(id);

        if !path.exists() {
            return Err(SessionError::NotFound(id.to_string()));
        }

        if hard {
            tokio::fs::remove_file(&path)
                .await
                .map_err(|e| SessionError::Storage(e.to_string()))?;
        } else {
            self.update_meta(id, SessionMetaUpdate {
                state: Some(SessionState::Deleted),
                ..Default::default()
            })
            .await?;
        }

        info!(session_id = %id, hard, "Session deleted");
        Ok(())
    }

    async fn append_messages(
        &self,
        session_id: &SessionId,
        messages: Vec<StoredMessage>,
    ) -> Result<Vec<String>, SessionError> {
        let path = self.session_file(session_id);

        if !path.exists() {
            return Err(SessionError::NotFound(session_id.to_string()));
        }

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|e| SessionError::Storage(e.to_string()))?;

        let mut ids = Vec::with_capacity(messages.len());
        for msg in &messages {
            ids.push(msg.id.clone());
            let line = serde_json::to_string(msg)
                .map_err(|e| SessionError::Serialization(e.to_string()))?;
            file.write_all(format!("{}\n", line).as_bytes())
                .await
                .map_err(|e| SessionError::Storage(e.to_string()))?;
        }

        drop(file);
        self.update_meta(session_id, SessionMetaUpdate {
            last_active_at: Some(chrono::Utc::now()),
            ..Default::default()
        })
        .await?;

        debug!(session_id = %session_id, count = ids.len(), "Messages appended");
        Ok(ids)
    }

    async fn get_messages(
        &self,
        session_id: &SessionId,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<StoredMessage>, SessionError> {
        let loaded = self.load_session(session_id).await?
            .ok_or_else(|| SessionError::NotFound(session_id.to_string()))?;

        let end = (offset + limit).min(loaded.messages.len());
        if offset >= loaded.messages.len() {
            return Ok(vec![]);
        }

        Ok(loaded.messages[offset..end].to_vec())
    }

    async fn message_count(&self, session_id: &SessionId) -> Result<usize, SessionError> {
        let loaded = self.load_session(session_id).await?
            .ok_or_else(|| SessionError::NotFound(session_id.to_string()))?;

        Ok(loaded.messages.len())
    }

    async fn set_state(
        &self,
        id: &SessionId,
        new_state: SessionState,
    ) -> Result<(), SessionError> {
        self.update_meta(id, SessionMetaUpdate {
            state: Some(new_state),
            ..Default::default()
        })
        .await
    }

    async fn save_compaction(
        &self,
        session_id: &SessionId,
        snapshot: CompactionSnapshot,
    ) -> Result<(), SessionError> {
        let path = self.session_file(session_id);

        if !path.exists() {
            return Err(SessionError::NotFound(session_id.to_string()));
        }

        let compaction_line = format!(
            "# COMPACTION\n{}",
            serde_json::to_string(&snapshot)
                .map_err(|e| SessionError::Serialization(e.to_string()))?
        );

        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .await
            .map_err(|e| SessionError::Storage(e.to_string()))?;

        file.write_all(format!("{}\n", compaction_line).as_bytes())
            .await
            .map_err(|e| SessionError::Storage(e.to_string()))?;

        Ok(())
    }

    async fn load_compaction(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<CompactionSnapshot>, SessionError> {
        let path = self.session_file(session_id);

        if !path.exists() {
            return Ok(None);
        }

        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| SessionError::Storage(e.to_string()))?;

        let lines: Vec<&str> = content.lines().collect();
        let mut i = 0;
        while i < lines.len() {
            let line = lines[i].trim();
            if line == "# COMPACTION" {
                if i + 1 < lines.len() {
                    let next = lines[i + 1].trim();
                    if !next.is_empty() && !next.starts_with('#') {
                        if let Ok(snapshot) = serde_json::from_str::<CompactionSnapshot>(next) {
                            return Ok(Some(snapshot));
                        }
                    }
                }
                return Ok(None);
            }
            i += 1;
        }

        Ok(None)
    }

    async fn list_sessions(
        &self,
        filter: SessionFilter,
    ) -> Result<Vec<SessionMeta>, SessionError> {
        self.ensure_dir().await?;

        let mut entries = tokio::fs::read_dir(&self.base_path)
            .await
            .map_err(|e| SessionError::Storage(e.to_string()))?;

        let mut sessions = Vec::new();

        while let Some(entry) = entries.next_entry().await.map_err(|e| SessionError::Storage(e.to_string()))? {
            let path = entry.path();

            if !path.extension().map(|e| e == "jsonl").unwrap_or(false) {
                continue;
            }

            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if uuid::Uuid::parse_str(stem).is_ok() {
                    match self.load_session(&SessionId(stem.into())).await {
                        Ok(Some(loaded)) => {
                            let meta = loaded.meta;

                            if let Some(ref owner) = filter.owner_id {
                                if meta.owner_id.as_ref() != Some(owner) {
                                    continue;
                                }
                            }
                            if let Some(ref state) = filter.state {
                                if meta.state != *state {
                                    continue;
                                }
                            }
                            if let Some(ref model) = filter.model {
                                if meta.model.as_ref() != Some(model) {
                                    continue;
                                }
                            }

                            sessions.push(meta);
                        }
                        Ok(None) => {}
                        Err(e) => {
                            tracing::warn!(session = stem, error = %e, "Failed to load session");
                        }
                    }
                }
            }
        }

        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

        let offset = filter.offset.unwrap_or(0);
        let limit = filter.limit.unwrap_or(sessions.len());

        Ok(sessions.into_iter().skip(offset).take(limit).collect())
    }

    async fn count_sessions(
        &self,
        filter: &SessionFilter,
    ) -> Result<usize, SessionError> {
        let sessions = self.list_sessions(filter.clone()).await?;
        Ok(sessions.len())
    }
}
