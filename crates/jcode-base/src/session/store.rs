//! Explicit-root session storage. File paths remain an import/export format.
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{Session, SessionStatus, StoredMessage};

mod sqlite;
#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SessionBackend {
    #[default]
    Files,
    Sqlite,
}

impl SessionBackend {
    pub fn configured() -> Result<Self> {
        match std::env::var("JCODE_SESSION_BACKEND") {
            Err(std::env::VarError::NotPresent) => Ok(Self::Files),
            Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
                "" | "files" | "file" | "json" => Ok(Self::Files),
                "sqlite" => Ok(Self::Sqlite),
                _ => bail!("Unknown JCODE_SESSION_BACKEND {value:?}; expected files or sqlite"),
            },
            Err(err) => Err(err.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub parent_id: Option<String>,
    pub title: Option<String>,
    pub custom_title: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_active_at: Option<DateTime<Utc>>,
    pub working_dir: Option<String>,
    pub short_name: Option<String>,
    pub provider_key: Option<String>,
    pub model: Option<String>,
    pub is_canary: bool,
    pub is_debug: bool,
    pub saved: bool,
    pub save_label: Option<String>,
    pub status: SessionStatus,
    pub visible_message_count: usize,
    pub user_message_count: usize,
    pub assistant_message_count: usize,
    pub estimated_tokens: usize,
    pub first_user_prompt: Option<String>,
    pub search_text: String,
}

impl SessionSummary {
    fn metadata(session: &Session) -> Self {
        Self {
            id: session.id.clone(),
            parent_id: session.parent_id.clone(),
            title: session.title.clone(),
            custom_title: session.custom_title.clone(),
            created_at: session.created_at,
            updated_at: session.updated_at,
            last_active_at: session.last_active_at,
            working_dir: session.working_dir.clone(),
            short_name: session.short_name.clone(),
            provider_key: session.provider_key.clone(),
            model: session.model.clone(),
            is_canary: session.is_canary,
            is_debug: session.is_debug,
            saved: session.saved,
            save_label: session.save_label.clone(),
            status: session.status.clone(),
            visible_message_count: 0,
            user_message_count: 0,
            assistant_message_count: 0,
            estimated_tokens: 0,
            first_user_prompt: None,
            search_text: String::new(),
        }
    }

    fn add_messages(&mut self, messages: &[StoredMessage]) {
        for message in messages {
            if !super::is_visible_conversation_message(message) {
                continue;
            }
            self.visible_message_count += 1;
            match message.role {
                crate::message::Role::User => {
                    self.user_message_count += 1;
                    if self.first_user_prompt.is_none() {
                        self.first_user_prompt =
                            message.content.iter().find_map(|block| match block {
                                crate::message::ContentBlock::Text { text, .. } => {
                                    Some(bounded_text(text, 4096))
                                }
                                _ => None,
                            });
                    }
                }
                crate::message::Role::Assistant => self.assistant_message_count += 1,
            }
            if let Some(usage) = &message.token_usage {
                let total = usage
                    .input_tokens
                    .saturating_add(usage.output_tokens)
                    .saturating_add(usage.cache_read_input_tokens.unwrap_or(0))
                    .saturating_add(usage.cache_creation_input_tokens.unwrap_or(0));
                self.estimated_tokens = self.estimated_tokens.saturating_add(total as usize);
            }
            for block in &message.content {
                use crate::message::ContentBlock;
                match block {
                    ContentBlock::Text { text, .. }
                    | ContentBlock::Reasoning { text }
                    | ContentBlock::ReasoningTrace { text } => {
                        append_search_sample(&mut self.search_text, text)
                    }
                    ContentBlock::ToolResult { content, .. } => {
                        append_search_sample(&mut self.search_text, content)
                    }
                    ContentBlock::ToolUse { name, input, .. } => {
                        append_search_sample(&mut self.search_text, name);
                        append_search_sample(&mut self.search_text, &input.to_string());
                    }
                    ContentBlock::AnthropicThinking { thinking, .. } => {
                        append_search_sample(&mut self.search_text, thinking)
                    }
                    ContentBlock::OpenAIReasoning { summary, .. } => {
                        for text in summary {
                            append_search_sample(&mut self.search_text, text);
                        }
                    }
                    ContentBlock::ToolReference { tool_name, .. } => {
                        append_search_sample(&mut self.search_text, tool_name)
                    }
                    ContentBlock::ProviderNative { item, .. } => {
                        append_search_sample(&mut self.search_text, &item.to_string())
                    }
                    ContentBlock::Image { .. } | ContentBlock::OpenAICompaction { .. } => {}
                }
            }
        }
    }
}

fn bounded_text(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

fn append_search_sample(sample: &mut String, text: &str) {
    const HALF: usize = 32 * 1024;
    let excerpt = bounded_text(text, 4096);
    sample.push('\n');
    sample.push_str(&excerpt);
    if sample.len() > HALF * 2 {
        let head = bounded_text(sample, HALF);
        let mut tail_start = sample.len() - HALF;
        while !sample.is_char_boundary(tail_start) {
            tail_start += 1;
        }
        *sample = format!("{head}{}", &sample[tail_start..]);
    }
}

#[derive(Debug, Clone)]
pub struct SessionListOptions {
    pub limit: usize,
    pub updated_since: Option<DateTime<Utc>>,
    pub include_saved_beyond_window: bool,
    pub exclude_id_prefixes: Vec<String>,
}

impl Default for SessionListOptions {
    fn default() -> Self {
        Self {
            limit: usize::MAX,
            updated_since: None,
            include_saved_beyond_window: false,
            exclude_id_prefixes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionSearchCandidate {
    pub id: String,
    pub updated_at_ms: i64,
    pub version: u64,
    pub size_bytes: u64,
}

/// Backends own durable I/O, while SessionStore applies common save semantics.
pub trait SessionStorage: Send + Sync {
    fn load(&self, id: &str) -> Result<Session>;
    fn save(&self, session: &mut Session) -> Result<()>;
    fn import(&self, session: &mut Session) -> Result<()> {
        if self.contains_native(&session.id)? {
            return Ok(());
        }
        self.save(session)
    }
    fn contains_native(&self, id: &str) -> Result<bool>;
    fn list(&self) -> Result<Vec<SessionSummary>>;
    fn list_summaries(&self, options: &SessionListOptions) -> Result<Vec<SessionSummary>> {
        let mut summaries = self.list()?;
        summaries.retain(|summary| {
            !options
                .exclude_id_prefixes
                .iter()
                .any(|prefix| summary.id.starts_with(prefix))
                && (options
                    .updated_since
                    .is_none_or(|since| summary.updated_at >= since)
                    || (options.include_saved_beyond_window && summary.saved))
        });
        summaries.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        summaries.truncate(options.limit);
        Ok(summaries)
    }
    fn list_search_candidates(&self, max: usize) -> Result<Vec<SessionSearchCandidate>>;
    fn search_text(&self, id: &str) -> Result<String> {
        Ok(serde_json::to_string(&self.load(id)?)?)
    }
    fn load_startup_stub(&self, id: &str) -> Result<Session> {
        let mut session = self.load(id)?;
        session.strip_transcript_for_remote_client();
        Ok(session)
    }
}

#[derive(Clone)]
pub struct SessionStore {
    root: PathBuf,
    storage: Arc<dyn SessionStorage>,
}

impl SessionStore {
    /// Root is a jcode data directory, not the sessions subdirectory.
    /// Opening Files performs no writes. Opening SQLite creates only its database.
    pub fn open(root: impl Into<PathBuf>, backend: SessionBackend) -> Result<Self> {
        let root = root.into();
        let storage: Arc<dyn SessionStorage> = match backend {
            SessionBackend::Files => Arc::new(FileSessionStorage { root: root.clone() }),
            SessionBackend::Sqlite => Arc::new(sqlite::SqliteSessionStorage::open(root.clone())?),
        };
        Ok(Self::with_storage(root, storage))
    }

    pub fn with_storage(root: impl Into<PathBuf>, storage: Arc<dyn SessionStorage>) -> Self {
        Self {
            root: root.into(),
            storage,
        }
    }

    pub fn configured() -> Result<Self> {
        let root = crate::storage::jcode_dir()?;
        let backend = SessionBackend::configured()?;
        // Keep the configured SQLite connection open between turns, without
        // sharing explicit-root benchmark or test stores.
        static SQLITE: std::sync::Mutex<Option<(PathBuf, SessionStore)>> =
            std::sync::Mutex::new(None);
        if backend == SessionBackend::Files {
            return Self::open(root, backend);
        }
        let mut cache = SQLITE.lock().unwrap_or_else(|err| err.into_inner());
        if let Some((cached_root, store)) = cache.as_ref() {
            if cached_root == &root {
                return Ok(store.clone());
            }
        }
        let store = Self::open(root.clone(), backend)?;
        *cache = Some((root, store.clone()));
        Ok(store)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn load(&self, id: &str) -> Result<Session> {
        validate_id(id)?;
        self.storage.load(id)
    }
    pub fn load_startup_stub(&self, id: &str) -> Result<Session> {
        validate_id(id)?;
        self.storage.load_startup_stub(id)
    }
    pub fn save(&self, session: &mut Session) -> Result<()> {
        self.save_inner(session, false)
    }
    pub fn save_prepared(&self, session: &mut Session) -> Result<()> {
        self.save_inner(session, true)
    }
    fn save_inner(&self, session: &mut Session, force: bool) -> Result<()> {
        validate_id(&session.id)?;
        let _guard = super::persistence::SAVES_IN_FLIGHT
            .read()
            .unwrap_or_else(|err| err.into_inner());
        if !session.should_persist(force) {
            return Ok(());
        }
        session.updated_at = Utc::now();
        self.storage.save(session)
    }
    pub fn contains_native(&self, id: &str) -> Result<bool> {
        validate_id(id)?;
        self.storage.contains_native(id)
    }
    pub fn exists(&self, id: &str) -> Result<bool> {
        Ok(self.contains_native(id)?
            || super::storage_paths::session_path_in_dir(&self.root, id).is_file())
    }
    pub fn list(&self) -> Result<Vec<SessionSummary>> {
        self.list_summaries(&SessionListOptions::default())
    }
    pub fn list_summaries(&self, options: &SessionListOptions) -> Result<Vec<SessionSummary>> {
        self.storage.list_summaries(options)
    }
    pub fn list_search_candidates(&self, max: usize) -> Result<Vec<SessionSearchCandidate>> {
        self.storage.list_search_candidates(max)
    }
    pub fn search_text(&self, id: &str) -> Result<String> {
        validate_id(id)?;
        self.storage.search_text(id)
    }
    /// Import an explicit legacy snapshot and adjacent journal without modifying
    /// the source or refreshing timestamps. Existing native sessions win.
    pub fn import_from_path(&self, path: &Path) -> Result<()> {
        let mut session = Session::load_file_at(path, false, false)?;
        validate_id(&session.id)?;
        let _guard = super::persistence::SAVES_IN_FLIGHT
            .read()
            .unwrap_or_else(|err| err.into_inner());
        self.storage.import(&mut session)
    }
    /// Export is explicit and never creates shadow snapshots during normal saves.
    pub fn export(&self, id: &str, path: &Path) -> Result<()> {
        crate::storage::write_json_secret(path, &self.load(id)?)
    }
}

pub(super) fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.contains('/')
        || id.contains('\\')
        || id == "."
        || id == ".."
        || id.contains('\0')
    {
        bail!("Invalid session id");
    }
    Ok(())
}

struct FileSessionStorage {
    root: PathBuf,
}

impl FileSessionStorage {
    fn path(&self, id: &str) -> PathBuf {
        super::storage_paths::session_path_in_dir(&self.root, id)
    }
    fn paths(&self) -> Result<Vec<PathBuf>> {
        let dir = self.root.join("sessions");
        if !dir.exists() {
            return Ok(Vec::new());
        }
        Ok(std::fs::read_dir(dir)?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect())
    }
}

impl SessionStorage for FileSessionStorage {
    fn load(&self, id: &str) -> Result<Session> {
        let path = self.path(id);
        let mut session = Session::load_file_at(&path, false, true)?;
        session.persist_state.store_key = Some(path);
        Ok(session)
    }
    fn save(&self, session: &mut Session) -> Result<()> {
        let path = self.path(&session.id);
        if session
            .persist_state
            .store_key
            .as_ref()
            .is_some_and(|previous| previous != &path)
        {
            session.reset_persist_state(false);
            session.persist_state.sqlite_revision = None;
            session.persist_state.sqlite_transcript_revision = None;
            session.persist_state.sqlite_metadata = None;
        }
        session.persist_state.store_key = Some(path.clone());
        session.save_file_at(&path)
    }
    fn contains_native(&self, id: &str) -> Result<bool> {
        Ok(self.path(id).is_file())
    }
    fn list(&self) -> Result<Vec<SessionSummary>> {
        let mut summaries = Vec::new();
        for path in self.paths()? {
            let session = Session::load_file_at(&path, false, false)
                .with_context(|| format!("Loading {}", path.display()))?;
            let mut summary = SessionSummary::metadata(&session);
            summary.add_messages(&session.messages);
            summaries.push(summary);
        }
        Ok(summaries)
    }
    fn list_search_candidates(&self, max: usize) -> Result<Vec<SessionSearchCandidate>> {
        let mut candidates = Vec::new();
        for path in self.paths()? {
            let Some(id) = path.file_stem().and_then(|id| id.to_str()) else {
                continue;
            };
            let metadata = std::fs::metadata(&path)?;
            let journal = std::fs::metadata(
                super::storage_paths::session_journal_path_from_snapshot(&path),
            )
            .ok();
            let modified = metadata.modified()?.max(
                journal
                    .as_ref()
                    .and_then(|meta| meta.modified().ok())
                    .unwrap_or(std::time::UNIX_EPOCH),
            );
            let version = modified.duration_since(std::time::UNIX_EPOCH)?.as_nanos() as u64;
            candidates.push(SessionSearchCandidate {
                id: id.to_owned(),
                updated_at_ms: (version / 1_000_000) as i64,
                version,
                size_bytes: metadata.len() + journal.map(|meta| meta.len()).unwrap_or(0),
            });
        }
        candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.updated_at_ms));
        candidates.truncate(max);
        Ok(candidates)
    }
}
