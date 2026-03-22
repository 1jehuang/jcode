#![allow(dead_code)]

use crate::id::{extract_session_name, new_id, new_memorable_session_id};
use crate::message::{ContentBlock, Message, Role, ToolCall};
use crate::storage;
use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::path::PathBuf;

/// Session exit status - why the session ended
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum SessionStatus {
    /// Session is currently active/running
    #[default]
    Active,
    /// User closed the session normally (Ctrl+C, /quit, etc.)
    Closed,
    /// Session crashed (panic, error)
    Crashed { message: Option<String> },
    /// Session was reloaded (hot reload)
    Reloaded,
    /// Session was compacted (context too large)
    Compacted,
    /// Session ended due to rate limiting
    RateLimited,
    /// Session ended due to an error
    Error { message: String },
}

impl SessionStatus {
    /// Get a short display string for the status
    pub fn display(&self) -> &'static str {
        match self {
            SessionStatus::Active => "active",
            SessionStatus::Closed => "closed",
            SessionStatus::Crashed { .. } => "crashed",
            SessionStatus::Reloaded => "reloaded",
            SessionStatus::Compacted => "compacted",
            SessionStatus::RateLimited => "rate limited",
            SessionStatus::Error { .. } => "error",
        }
    }

    /// Get an icon for the status
    pub fn icon(&self) -> &'static str {
        match self {
            SessionStatus::Active => "▶",
            SessionStatus::Closed => "✓",
            SessionStatus::Crashed { .. } => "💥",
            SessionStatus::Reloaded => "🔄",
            SessionStatus::Compacted => "📦",
            SessionStatus::RateLimited => "⏳",
            SessionStatus::Error { .. } => "❌",
        }
    }

    /// Get additional detail message if available
    pub fn detail(&self) -> Option<&str> {
        match self {
            SessionStatus::Crashed { message } => message.as_deref(),
            SessionStatus::Error { message } => Some(message.as_str()),
            _ => None,
        }
    }
}

/// A memory injection event, stored for replay visualization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMemoryInjection {
    /// Human-readable summary (e.g., "🧠 auto-recalled 3 memories")
    pub summary: String,
    /// The recalled memory content that was injected
    pub content: String,
    /// Number of memories recalled
    pub count: u32,
    /// Age of memories in milliseconds
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub age_ms: Option<u64>,
    /// Message index this injection occurred before (for replay timing)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_message: Option<usize>,
    /// Timestamp when injection occurred
    pub timestamp: DateTime<Utc>,
}

/// Extra non-conversation UI/state events persisted for replay fidelity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredReplayEvent {
    pub timestamp: DateTime<Utc>,
    #[serde(flatten)]
    pub kind: StoredReplayEventKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "event")]
pub enum StoredReplayEventKind {
    /// A non-provider display message shown in the UI (e.g. swarm/system notice).
    #[serde(rename = "display_message")]
    DisplayMessage {
        role: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        content: String,
    },
    /// Historical swarm member status snapshot.
    #[serde(rename = "swarm_status")]
    SwarmStatus {
        members: Vec<crate::protocol::SwarmMemberStatus>,
    },
    /// Historical swarm plan snapshot.
    #[serde(rename = "swarm_plan")]
    SwarmPlan {
        swarm_id: String,
        version: u64,
        items: Vec<crate::plan::PlanItem>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        participants: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub id: String,
    pub role: Role,
    pub content: Vec<ContentBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_role: Option<StoredDisplayRole>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<StoredTokenUsage>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StoredDisplayRole {
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredTokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredCompactionState {
    pub summary_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai_encrypted_content: Option<String>,
    pub covers_up_to_turn: usize,
    pub original_turn_count: usize,
    pub compacted_count: usize,
}

impl StoredMessage {
    pub fn to_message(&self) -> Message {
        Message {
            role: self.role.clone(),
            content: self.content.clone(),
            timestamp: self.timestamp,
            tool_duration_ms: self.tool_duration_ms,
        }
    }

    /// Get a text preview of the message content
    pub fn content_preview(&self) -> String {
        for block in &self.content {
            match block {
                ContentBlock::Text { text, .. } => {
                    // Return first non-empty text block
                    let text = text.trim();
                    if !text.is_empty() {
                        return text.replace('\n', " ");
                    }
                }
                ContentBlock::ToolUse { name, .. } => {
                    return format!("[tool: {}]", name);
                }
                ContentBlock::ToolResult { content, .. } => {
                    let preview = content.trim().replace('\n', " ");
                    if !preview.is_empty() {
                        return format!("[result: {}]", preview);
                    }
                }
                _ => {}
            }
        }
        "(empty)".to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub parent_id: Option<String>,
    pub title: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub messages: Vec<StoredMessage>,
    /// Persisted compacted-view state so reload/resume can continue using the
    /// active summary + recent tail instead of re-sending the full transcript.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<StoredCompactionState>,
    /// Provider-specific session ID (e.g., Claude Code CLI session for resume)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_session_id: Option<String>,
    /// Model identifier for this session (e.g., "gpt-5.2-codex")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Whether this session is a canary session (testing new builds)
    #[serde(default)]
    pub is_canary: bool,
    /// Build hash this session is testing (if canary)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub testing_build: Option<String>,
    /// Working directory (for self-dev detection)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    /// Memorable short name (e.g., "fox", "oak")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short_name: Option<String>,
    /// Session exit status - why it ended (if not active)
    #[serde(default)]
    pub status: SessionStatus,
    /// PID of the process that last owned this session (for crash detection)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_pid: Option<u32>,
    /// Last time the session was marked active
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_active_at: Option<DateTime<Utc>>,
    /// Whether this is a debug/test session (created via debug socket)
    #[serde(default)]
    pub is_debug: bool,
    /// Whether this session has been saved/bookmarked by the user
    #[serde(default)]
    pub saved: bool,
    /// Optional user-provided label for saved sessions
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save_label: Option<String>,
    /// Environment snapshots for post-mortem debugging
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env_snapshots: Vec<EnvSnapshot>,
    /// Memory injection events (for replay visualization)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub memory_injections: Vec<StoredMemoryInjection>,
    /// Non-conversation UI/state events persisted for higher-fidelity replay.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replay_events: Vec<StoredReplayEvent>,
    #[serde(skip)]
    persist_state: SessionPersistState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct SessionJournalMeta {
    parent_id: Option<String>,
    title: Option<String>,
    updated_at: DateTime<Utc>,
    compaction: Option<StoredCompactionState>,
    provider_session_id: Option<String>,
    model: Option<String>,
    is_canary: bool,
    testing_build: Option<String>,
    working_dir: Option<String>,
    short_name: Option<String>,
    status: SessionStatus,
    last_pid: Option<u32>,
    last_active_at: Option<DateTime<Utc>>,
    is_debug: bool,
    saved: bool,
    save_label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionJournalEntry {
    meta: SessionJournalMeta,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    append_messages: Vec<StoredMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    append_env_snapshots: Vec<EnvSnapshot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    append_memory_injections: Vec<StoredMemoryInjection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    append_replay_events: Vec<StoredReplayEvent>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum PersistVectorMode {
    #[default]
    Clean,
    Append,
    Full,
}

#[derive(Debug, Clone, Default)]
struct SessionPersistState {
    snapshot_exists: bool,
    messages_len: usize,
    env_snapshots_len: usize,
    memory_injections_len: usize,
    replay_events_len: usize,
    messages_mode: PersistVectorMode,
    env_snapshots_mode: PersistVectorMode,
    memory_injections_mode: PersistVectorMode,
    replay_events_mode: PersistVectorMode,
    last_meta: Option<SessionJournalMeta>,
}

const MAX_SESSION_JOURNAL_BYTES: u64 = 512 * 1024;

/// Max number of environment snapshots to retain per session
const MAX_ENV_SNAPSHOTS: usize = 8;

fn env_flag_enabled(name: &str) -> bool {
    std::env::var(name)
        .map(|v| {
            let trimmed = v.trim();
            !trimmed.is_empty() && trimmed != "0" && !trimmed.eq_ignore_ascii_case("false")
        })
        .unwrap_or(false)
}

fn default_is_test_session() -> bool {
    env_flag_enabled("JCODE_TEST_SESSION")
}

/// Minimal git state for reproducibility
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitState {
    pub root: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub dirty: Option<bool>,
}

/// Environment snapshot captured for a session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvSnapshot {
    pub captured_at: DateTime<Utc>,
    pub reason: String,
    pub session_id: String,
    pub working_dir: Option<String>,
    pub provider: String,
    pub model: String,
    pub jcode_version: String,
    pub jcode_git_hash: Option<String>,
    pub jcode_git_dirty: Option<bool>,
    pub os: String,
    pub arch: String,
    pub pid: u32,
    pub is_selfdev: bool,
    pub is_debug: bool,
    pub is_canary: bool,
    pub testing_build: Option<String>,
    pub working_git: Option<GitState>,
}

impl Session {
    fn journal_meta(&self) -> SessionJournalMeta {
        SessionJournalMeta {
            parent_id: self.parent_id.clone(),
            title: self.title.clone(),
            updated_at: self.updated_at,
            compaction: self.compaction.clone(),
            provider_session_id: self.provider_session_id.clone(),
            model: self.model.clone(),
            is_canary: self.is_canary,
            testing_build: self.testing_build.clone(),
            working_dir: self.working_dir.clone(),
            short_name: self.short_name.clone(),
            status: self.status.clone(),
            last_pid: self.last_pid,
            last_active_at: self.last_active_at,
            is_debug: self.is_debug,
            saved: self.saved,
            save_label: self.save_label.clone(),
        }
    }

    fn reset_persist_state(&mut self, snapshot_exists: bool) {
        self.persist_state = SessionPersistState {
            snapshot_exists,
            messages_len: self.messages.len(),
            env_snapshots_len: self.env_snapshots.len(),
            memory_injections_len: self.memory_injections.len(),
            replay_events_len: self.replay_events.len(),
            messages_mode: PersistVectorMode::Clean,
            env_snapshots_mode: PersistVectorMode::Clean,
            memory_injections_mode: PersistVectorMode::Clean,
            replay_events_mode: PersistVectorMode::Clean,
            last_meta: Some(self.journal_meta()),
        };
    }

    fn mark_messages_append_dirty(&mut self) {
        if self.persist_state.messages_mode != PersistVectorMode::Full {
            self.persist_state.messages_mode = PersistVectorMode::Append;
        }
    }

    fn mark_messages_full_dirty(&mut self) {
        self.persist_state.messages_mode = PersistVectorMode::Full;
    }

    fn mark_env_snapshots_append_dirty(&mut self) {
        if self.persist_state.env_snapshots_mode != PersistVectorMode::Full {
            self.persist_state.env_snapshots_mode = PersistVectorMode::Append;
        }
    }

    fn mark_env_snapshots_full_dirty(&mut self) {
        self.persist_state.env_snapshots_mode = PersistVectorMode::Full;
    }

    fn mark_memory_injections_append_dirty(&mut self) {
        if self.persist_state.memory_injections_mode != PersistVectorMode::Full {
            self.persist_state.memory_injections_mode = PersistVectorMode::Append;
        }
    }

    fn mark_memory_injections_full_dirty(&mut self) {
        self.persist_state.memory_injections_mode = PersistVectorMode::Full;
    }

    fn mark_replay_events_append_dirty(&mut self) {
        if self.persist_state.replay_events_mode != PersistVectorMode::Full {
            self.persist_state.replay_events_mode = PersistVectorMode::Append;
        }
    }

    fn mark_replay_events_full_dirty(&mut self) {
        self.persist_state.replay_events_mode = PersistVectorMode::Full;
    }

    fn metadata_requires_snapshot(prev: &SessionJournalMeta, current: &SessionJournalMeta) -> bool {
        prev.parent_id != current.parent_id
            || prev.title != current.title
            || prev.is_canary != current.is_canary
            || prev.testing_build != current.testing_build
            || prev.working_dir != current.working_dir
            || prev.short_name != current.short_name
            || prev.status != current.status
            || prev.is_debug != current.is_debug
            || prev.saved != current.saved
            || prev.save_label != current.save_label
    }

    fn apply_journal_meta(&mut self, meta: SessionJournalMeta) {
        self.parent_id = meta.parent_id;
        self.title = meta.title;
        self.updated_at = meta.updated_at;
        self.compaction = meta.compaction;
        self.provider_session_id = meta.provider_session_id;
        self.model = meta.model;
        self.is_canary = meta.is_canary;
        self.testing_build = meta.testing_build;
        self.working_dir = meta.working_dir;
        self.short_name = meta.short_name;
        self.status = meta.status;
        self.last_pid = meta.last_pid;
        self.last_active_at = meta.last_active_at;
        self.is_debug = meta.is_debug;
        self.saved = meta.saved;
        self.save_label = meta.save_label;
    }

    fn apply_journal_entry(&mut self, entry: SessionJournalEntry) {
        self.apply_journal_meta(entry.meta);
        self.messages.extend(entry.append_messages);
        self.env_snapshots.extend(entry.append_env_snapshots);
        self.memory_injections
            .extend(entry.append_memory_injections);
        self.replay_events.extend(entry.append_replay_events);
    }

    fn checkpoint_snapshot(&mut self, snapshot_path: &Path, journal_path: &Path) -> Result<()> {
        storage::write_json_fast(snapshot_path, self)?;
        if journal_path.exists() {
            let _ = std::fs::remove_file(journal_path);
        }
        self.reset_persist_state(true);
        Ok(())
    }

    pub fn load_from_path(path: &Path) -> Result<Self> {
        let mut session: Session = storage::read_json(path)?;
        let journal_path = session_journal_path_from_snapshot(path);
        if journal_path.exists() {
            let file = std::fs::File::open(&journal_path)?;
            let reader = BufReader::new(file);
            for (line_idx, line) in reader.lines().enumerate() {
                let line = line?;
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<SessionJournalEntry>(trimmed) {
                    Ok(entry) => session.apply_journal_entry(entry),
                    Err(err) => {
                        crate::logging::warn(&format!(
                            "Session journal parse failed at {} line {}: {}",
                            journal_path.display(),
                            line_idx + 1,
                            err
                        ));
                        break;
                    }
                }
            }
        }
        session.reset_persist_state(path.exists());
        Ok(session)
    }

    pub fn create_with_id(
        session_id: String,
        parent_id: Option<String>,
        title: Option<String>,
    ) -> Self {
        let now = Utc::now();
        let is_debug = default_is_test_session();
        // Try to extract short name from ID if it's a memorable ID
        let short_name = extract_session_name(&session_id).map(|s| s.to_string());
        let mut session = Self {
            id: session_id,
            parent_id,
            title,
            created_at: now,
            updated_at: now,
            messages: Vec::new(),
            compaction: None,
            provider_session_id: None,
            model: None,
            is_canary: false,
            testing_build: None,
            working_dir: std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().to_string()),
            short_name,
            status: SessionStatus::Active,
            last_pid: Some(std::process::id()),
            last_active_at: Some(now),
            is_debug,
            saved: false,
            save_label: None,
            env_snapshots: Vec::new(),
            memory_injections: Vec::new(),
            replay_events: Vec::new(),
            persist_state: SessionPersistState::default(),
        };
        session.reset_persist_state(false);
        session
    }

    pub fn create(parent_id: Option<String>, title: Option<String>) -> Self {
        let now = Utc::now();
        let (id, short_name) = new_memorable_session_id();
        let is_debug = default_is_test_session();
        let mut session = Self {
            id,
            parent_id,
            title,
            created_at: now,
            updated_at: now,
            messages: Vec::new(),
            compaction: None,
            provider_session_id: None,
            model: None,
            is_canary: false,
            testing_build: None,
            working_dir: std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().to_string()),
            short_name: Some(short_name),
            status: SessionStatus::Active,
            last_pid: Some(std::process::id()),
            last_active_at: Some(now),
            is_debug,
            saved: false,
            save_label: None,
            env_snapshots: Vec::new(),
            memory_injections: Vec::new(),
            replay_events: Vec::new(),
            persist_state: SessionPersistState::default(),
        };
        session.reset_persist_state(false);
        session
    }

    /// Mark this session as a debug/test session
    pub fn set_debug(&mut self, is_debug: bool) {
        self.is_debug = is_debug;
    }

    /// Save/bookmark this session with an optional label
    pub fn mark_saved(&mut self, label: Option<String>) {
        self.saved = true;
        if label.is_some() {
            self.save_label = label;
        }
    }

    /// Remove the saved/bookmark status
    pub fn unmark_saved(&mut self) {
        self.saved = false;
        self.save_label = None;
    }

    /// Record an environment snapshot for post-mortem debugging
    pub fn record_env_snapshot(&mut self, snapshot: EnvSnapshot) {
        self.env_snapshots.push(snapshot);
        if self.env_snapshots.len() > MAX_ENV_SNAPSHOTS {
            let excess = self.env_snapshots.len() - MAX_ENV_SNAPSHOTS;
            self.env_snapshots.drain(0..excess);
            self.mark_env_snapshots_full_dirty();
        } else {
            self.mark_env_snapshots_append_dirty();
        }
    }

    /// Get the display name for this session (short memorable name if available)
    pub fn display_name(&self) -> &str {
        self.short_name
            .as_deref()
            .or_else(|| extract_session_name(&self.id))
            .unwrap_or(&self.id)
    }

    /// Mark this session as a canary tester
    pub fn set_canary(&mut self, build_hash: &str) {
        self.is_canary = true;
        self.testing_build = Some(build_hash.to_string());
    }

    /// Clear canary status
    pub fn clear_canary(&mut self) {
        self.is_canary = false;
        self.testing_build = None;
    }

    /// Set the session status
    pub fn set_status(&mut self, status: SessionStatus) {
        self.status = status;
    }

    /// Mark session as closed normally
    pub fn mark_closed(&mut self) {
        self.status = SessionStatus::Closed;
        unregister_active_pid(&self.id);
    }

    /// Mark session as crashed
    pub fn mark_crashed(&mut self, message: Option<String>) {
        self.status = SessionStatus::Crashed { message };
        unregister_active_pid(&self.id);
    }

    /// Mark session as having an error
    pub fn mark_error(&mut self, message: String) {
        self.status = SessionStatus::Error { message };
    }

    /// Mark session as active (e.g., when resuming)
    pub fn mark_active(&mut self) {
        self.status = SessionStatus::Active;
        let pid = std::process::id();
        self.last_pid = Some(pid);
        self.last_active_at = Some(Utc::now());
        register_active_pid(&self.id, pid);
    }

    /// Mark session as active for a specific PID
    pub fn mark_active_with_pid(&mut self, pid: u32) {
        self.status = SessionStatus::Active;
        self.last_pid = Some(pid);
        self.last_active_at = Some(Utc::now());
        register_active_pid(&self.id, pid);
    }

    /// Detect if an active session likely crashed (process no longer running)
    /// Returns true if status was updated.
    pub fn detect_crash(&mut self) -> bool {
        if self.status != SessionStatus::Active {
            return false;
        }

        if let Some(pid) = self.last_pid {
            if !is_pid_running(pid) {
                self.mark_crashed(Some(format!(
                    "Process {} exited unexpectedly (no shutdown signal captured)",
                    pid
                )));
                return true;
            }
        } else {
            // No PID info (older sessions): fall back to age heuristic
            let age = Utc::now().signed_duration_since(self.updated_at);
            if age.num_seconds() > 120 {
                self.mark_crashed(Some(
                    "Stale active session (possible abrupt termination)".to_string(),
                ));
                return true;
            }
        }

        false
    }

    /// Check if this session is working on the jcode repository
    pub fn is_self_dev(&self) -> bool {
        if let Some(ref dir) = self.working_dir {
            // Check if working dir contains jcode source
            let path = std::path::Path::new(dir);
            path.join("Cargo.toml").exists()
                && path.join("src/main.rs").exists()
                && std::fs::read_to_string(path.join("Cargo.toml"))
                    .map(|s| s.contains("name = \"jcode\""))
                    .unwrap_or(false)
        } else {
            false
        }
    }

    pub fn load(session_id: &str) -> Result<Self> {
        let path = session_path(session_id)?;
        Self::load_from_path(&path)
    }

    pub fn save(&mut self) -> Result<()> {
        self.updated_at = Utc::now();
        let path = session_path(&self.id)?;
        let journal_path = session_journal_path_from_snapshot(&path);
        let start = std::time::Instant::now();
        let current_meta = self.journal_meta();
        let metadata_needs_snapshot = self
            .persist_state
            .last_meta
            .as_ref()
            .is_some_and(|prev| Self::metadata_requires_snapshot(prev, &current_meta));
        let vectors_need_snapshot = !self.persist_state.snapshot_exists
            || self.persist_state.messages_mode == PersistVectorMode::Full
            || self.persist_state.env_snapshots_mode == PersistVectorMode::Full
            || self.persist_state.memory_injections_mode == PersistVectorMode::Full
            || self.persist_state.replay_events_mode == PersistVectorMode::Full
            || self.messages.len() < self.persist_state.messages_len
            || self.env_snapshots.len() < self.persist_state.env_snapshots_len
            || self.memory_injections.len() < self.persist_state.memory_injections_len
            || self.replay_events.len() < self.persist_state.replay_events_len;

        let result = if metadata_needs_snapshot || vectors_need_snapshot {
            self.checkpoint_snapshot(&path, &journal_path)
        } else {
            let entry = SessionJournalEntry {
                meta: current_meta.clone(),
                append_messages: self.messages[self.persist_state.messages_len..].to_vec(),
                append_env_snapshots: self.env_snapshots[self.persist_state.env_snapshots_len..]
                    .to_vec(),
                append_memory_injections: self.memory_injections
                    [self.persist_state.memory_injections_len..]
                    .to_vec(),
                append_replay_events: self.replay_events[self.persist_state.replay_events_len..]
                    .to_vec(),
            };
            let append_result = storage::append_json_line_fast(&journal_path, &entry);
            match append_result {
                Ok(()) => {
                    self.reset_persist_state(true);
                    if std::fs::metadata(&journal_path)
                        .map(|meta| meta.len() > MAX_SESSION_JOURNAL_BYTES)
                        .unwrap_or(false)
                    {
                        self.checkpoint_snapshot(&path, &journal_path)
                    } else {
                        Ok(())
                    }
                }
                Err(err) => {
                    crate::logging::warn(&format!(
                        "Session journal append failed for {} ({}); checkpointing full snapshot",
                        self.id, err
                    ));
                    self.checkpoint_snapshot(&path, &journal_path)
                }
            }
        };
        let elapsed = start.elapsed();
        if elapsed.as_millis() > 50 {
            crate::logging::info(&format!(
                "Session save slow: {:.0}ms ({} messages)",
                elapsed.as_secs_f64() * 1000.0,
                self.messages.len()
            ));
        }
        result
    }

    pub fn redacted_for_export(&self) -> Self {
        let mut redacted = self.clone();
        if let Some(compaction) = redacted.compaction.as_mut() {
            compaction.summary_text = crate::message::redact_secrets(&compaction.summary_text);
        }
        for msg in &mut redacted.messages {
            for block in &mut msg.content {
                match block {
                    ContentBlock::Text { text, .. } | ContentBlock::Reasoning { text } => {
                        *text = crate::message::redact_secrets(text);
                    }
                    ContentBlock::ToolResult { content, .. } => {
                        *content = crate::message::redact_secrets(content);
                    }
                    ContentBlock::ToolUse { input, .. } => redact_json_value(input),
                    ContentBlock::Image { .. } => {}
                    ContentBlock::OpenAICompaction { .. } => {}
                }
            }
        }
        for event in &mut redacted.replay_events {
            match &mut event.kind {
                StoredReplayEventKind::DisplayMessage { title, content, .. } => {
                    if let Some(title) = title.as_mut() {
                        *title = crate::message::redact_secrets(title);
                    }
                    *content = crate::message::redact_secrets(content);
                }
                StoredReplayEventKind::SwarmStatus { members } => {
                    for member in members {
                        if let Some(detail) = member.detail.as_mut() {
                            *detail = crate::message::redact_secrets(detail);
                        }
                    }
                }
                StoredReplayEventKind::SwarmPlan { items, reason, .. } => {
                    if let Some(reason) = reason.as_mut() {
                        *reason = crate::message::redact_secrets(reason);
                    }
                    for item in items {
                        item.content = crate::message::redact_secrets(&item.content);
                    }
                }
            }
        }
        redacted
    }

    pub fn add_message(&mut self, role: Role, content: Vec<ContentBlock>) -> String {
        self.add_message_ext_with_display_role(role, content, None, None, None)
    }

    pub fn add_message_with_duration(
        &mut self,
        role: Role,
        content: Vec<ContentBlock>,
        tool_duration_ms: Option<u64>,
    ) -> String {
        self.add_message_ext_with_display_role(role, content, tool_duration_ms, None, None)
    }

    pub fn add_message_with_display_role(
        &mut self,
        role: Role,
        content: Vec<ContentBlock>,
        display_role: Option<StoredDisplayRole>,
    ) -> String {
        self.add_message_ext_with_display_role(role, content, None, None, display_role)
    }

    pub fn add_message_ext(
        &mut self,
        role: Role,
        content: Vec<ContentBlock>,
        tool_duration_ms: Option<u64>,
        token_usage: Option<StoredTokenUsage>,
    ) -> String {
        self.add_message_ext_with_display_role(role, content, tool_duration_ms, token_usage, None)
    }

    pub fn add_message_ext_with_display_role(
        &mut self,
        role: Role,
        content: Vec<ContentBlock>,
        tool_duration_ms: Option<u64>,
        token_usage: Option<StoredTokenUsage>,
        display_role: Option<StoredDisplayRole>,
    ) -> String {
        let id = new_id("message");
        self.append_stored_message(StoredMessage {
            id: id.clone(),
            role,
            content,
            display_role,
            timestamp: Some(Utc::now()),
            tool_duration_ms,
            token_usage,
        });
        id
    }

    pub fn append_stored_message(&mut self, message: StoredMessage) {
        self.messages.push(message);
        self.mark_messages_append_dirty();
    }

    pub fn insert_message(&mut self, index: usize, message: StoredMessage) {
        self.messages.insert(index, message);
        self.mark_messages_full_dirty();
    }

    pub fn replace_messages(&mut self, messages: Vec<StoredMessage>) {
        self.messages = messages;
        self.mark_messages_full_dirty();
    }

    pub fn truncate_messages(&mut self, len: usize) {
        if len < self.messages.len() {
            self.messages.truncate(len);
            self.mark_messages_full_dirty();
        }
    }

    /// Record a memory injection event for replay visualization
    pub fn record_memory_injection(
        &mut self,
        summary: String,
        content: String,
        count: u32,
        age_ms: u64,
    ) {
        self.memory_injections.push(StoredMemoryInjection {
            summary,
            content,
            count,
            age_ms: Some(age_ms),
            before_message: Some(self.messages.len()),
            timestamp: Utc::now(),
        });
        self.mark_memory_injections_append_dirty();
    }

    pub fn record_replay_display_message(
        &mut self,
        role: impl Into<String>,
        title: Option<String>,
        content: impl Into<String>,
    ) {
        self.replay_events.push(StoredReplayEvent {
            timestamp: Utc::now(),
            kind: StoredReplayEventKind::DisplayMessage {
                role: role.into(),
                title,
                content: content.into(),
            },
        });
        self.mark_replay_events_append_dirty();
    }

    pub fn record_swarm_status_event(&mut self, members: Vec<crate::protocol::SwarmMemberStatus>) {
        let kind = StoredReplayEventKind::SwarmStatus { members };
        if self
            .replay_events
            .last()
            .is_some_and(|last| last.kind == kind)
        {
            return;
        }
        self.replay_events.push(StoredReplayEvent {
            timestamp: Utc::now(),
            kind,
        });
        self.mark_replay_events_append_dirty();
    }

    pub fn record_swarm_plan_event(
        &mut self,
        swarm_id: String,
        version: u64,
        items: Vec<crate::plan::PlanItem>,
        participants: Vec<String>,
        reason: Option<String>,
    ) {
        let kind = StoredReplayEventKind::SwarmPlan {
            swarm_id,
            version,
            items,
            participants,
            reason,
        };
        if self
            .replay_events
            .last()
            .is_some_and(|last| last.kind == kind)
        {
            return;
        }
        self.replay_events.push(StoredReplayEvent {
            timestamp: Utc::now(),
            kind,
        });
        self.mark_replay_events_append_dirty();
    }

    pub fn messages_for_provider(&self) -> Vec<Message> {
        self.messages.iter().map(|msg| msg.to_message()).collect()
    }

    /// Remove all ToolUse content blocks from a specific message.
    /// Used when tool calls are discarded (e.g. due to truncated output / max_tokens).
    pub fn remove_tool_use_blocks(&mut self, message_id: &str) {
        for msg in &mut self.messages {
            if msg.id == *message_id {
                msg.content
                    .retain(|block| !matches!(block, ContentBlock::ToolUse { .. }));
                self.mark_messages_full_dirty();
                break;
            }
        }
    }
}

fn redact_json_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(s) => {
            *s = crate::message::redact_secrets(s);
        }
        serde_json::Value::Array(values) => {
            for entry in values {
                redact_json_value(entry);
            }
        }
        serde_json::Value::Object(map) => {
            for entry in map.values_mut() {
                redact_json_value(entry);
            }
        }
        _ => {}
    }
}

#[derive(Debug, Clone)]
pub struct RenderedMessage {
    pub role: String,
    pub content: String,
    pub tool_calls: Vec<String>,
    pub tool_data: Option<ToolCall>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RenderedImageSource {
    UserInput,
    ToolResult { tool_name: String },
    Other { role: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenderedImage {
    pub media_type: String,
    pub data: String,
    pub label: Option<String>,
    pub source: RenderedImageSource,
}

fn image_source_for_message(role: Role, tool: Option<&ToolCall>) -> RenderedImageSource {
    if let Some(tool) = tool {
        return RenderedImageSource::ToolResult {
            tool_name: tool.name.clone(),
        };
    }

    match role {
        Role::User => RenderedImageSource::UserInput,
        Role::Assistant => RenderedImageSource::Other {
            role: "assistant".to_string(),
        },
    }
}

fn fallback_image_label_for_tool(tool: &ToolCall) -> Option<String> {
    tool.input
        .get("file_path")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn parse_attached_image_label(text: &str) -> Option<String> {
    let prefix = "[Attached image associated with the preceding tool result: ";
    let suffix = "]";
    text.trim()
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_suffix(suffix))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub fn render_images(session: &Session) -> Vec<RenderedImage> {
    let mut images = Vec::new();
    let mut tool_map: HashMap<String, ToolCall> = HashMap::new();

    for msg in &session.messages {
        let message_role = msg.role.clone();
        let mut current_tool: Option<ToolCall> = None;
        let mut last_image_idx: Option<usize> = None;

        for block in &msg.content {
            match block {
                ContentBlock::ToolUse { id, name, input } => {
                    tool_map.insert(
                        id.clone(),
                        ToolCall {
                            id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                            intent: None,
                        },
                    );
                }
                ContentBlock::ToolResult { tool_use_id, .. } => {
                    current_tool = tool_map.get(tool_use_id).cloned().or_else(|| {
                        Some(ToolCall {
                            id: tool_use_id.clone(),
                            name: "tool".to_string(),
                            input: serde_json::Value::Null,
                            intent: None,
                        })
                    });
                }
                ContentBlock::Image { media_type, data } => {
                    images.push(RenderedImage {
                        media_type: media_type.clone(),
                        data: data.clone(),
                        label: current_tool.as_ref().and_then(fallback_image_label_for_tool),
                        source: image_source_for_message(
                            message_role.clone(),
                            current_tool.as_ref(),
                        ),
                    });
                    last_image_idx = Some(images.len().saturating_sub(1));
                }
                ContentBlock::Text { text, .. } => {
                    let Some(label) = parse_attached_image_label(text) else {
                        continue;
                    };
                    if let Some(last_idx) = last_image_idx {
                        if let Some(image) = images.get_mut(last_idx) {
                            image.label = Some(label);
                        }
                    }
                }
                ContentBlock::Reasoning { .. } => {}
                ContentBlock::OpenAICompaction { .. } => {}
            }
        }
    }

    images
}

pub fn has_rendered_images(session: &Session) -> bool {
    session.messages.iter().any(|msg| {
        msg.content
            .iter()
            .any(|block| matches!(block, ContentBlock::Image { .. }))
    })
}

pub fn summarize_tool_calls(
    session: &Session,
    limit: usize,
) -> Vec<crate::protocol::ToolCallSummary> {
    let mut calls: Vec<crate::protocol::ToolCallSummary> = Vec::new();

    for msg in session.messages.iter().rev() {
        if calls.len() >= limit {
            break;
        }

        let text_summary = msg
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                ContentBlock::OpenAICompaction { .. } => Some("[OpenAI native compaction]"),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        for block in &msg.content {
            if calls.len() >= limit {
                break;
            }

            if let ContentBlock::ToolUse { name, input, .. } = block {
                let fallback = input.to_string();
                let brief = if text_summary.trim().is_empty() {
                    crate::util::truncate_str(&fallback, 200).to_string()
                } else {
                    crate::util::truncate_str(&text_summary, 200).to_string()
                };
                calls.push(crate::protocol::ToolCallSummary {
                    tool_name: name.clone(),
                    brief_output: brief,
                    timestamp_secs: msg.timestamp.map(|ts| ts.timestamp().max(0) as u64),
                });
            }
        }
    }

    calls.reverse();
    calls
}

/// Convert stored session messages into renderable messages (including tool output).
pub fn render_messages(session: &Session) -> Vec<RenderedMessage> {
    let mut rendered: Vec<RenderedMessage> = Vec::new();
    let mut tool_map: HashMap<String, ToolCall> = HashMap::new();

    for msg in &session.messages {
        let role = match msg.display_role {
            Some(StoredDisplayRole::System) => "system",
            None => match msg.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            },
        };
        let mut text = String::new();
        let mut tool_calls: Vec<String> = Vec::new();

        for block in &msg.content {
            match block {
                ContentBlock::Text { text: t, .. } => {
                    text.push_str(t);
                }
                ContentBlock::ToolUse { id, name, input } => {
                    tool_map.insert(
                        id.clone(),
                        ToolCall {
                            id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                            intent: None,
                        },
                    );
                    tool_calls.push(name.clone());
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => {
                    if !text.is_empty() {
                        rendered.push(RenderedMessage {
                            role: role.to_string(),
                            content: std::mem::take(&mut text),
                            tool_calls: tool_calls.clone(),
                            tool_data: None,
                        });
                    }

                    let tool_data = tool_map.get(tool_use_id).cloned().or_else(|| {
                        Some(ToolCall {
                            id: tool_use_id.clone(),
                            name: "tool".to_string(),
                            input: serde_json::Value::Null,
                            intent: None,
                        })
                    });

                    rendered.push(RenderedMessage {
                        role: "tool".to_string(),
                        content: content.clone(),
                        tool_calls: Vec::new(),
                        tool_data,
                    });
                }
                ContentBlock::Reasoning { .. } => {}
                ContentBlock::Image { .. } => {}
                ContentBlock::OpenAICompaction { .. } => {}
            }
        }

        if !text.is_empty() {
            rendered.push(RenderedMessage {
                role: role.to_string(),
                content: text,
                tool_calls,
                tool_data: None,
            });
        }
    }

    rendered
}

fn session_path_in_dir(base: &std::path::Path, session_id: &str) -> PathBuf {
    base.join("sessions").join(format!("{}.json", session_id))
}

pub(crate) fn session_journal_path_from_snapshot(path: &Path) -> PathBuf {
    let mut name = path
        .file_stem()
        .map(|stem| stem.to_os_string())
        .unwrap_or_default();
    name.push(".journal.jsonl");
    path.with_file_name(name)
}

pub fn session_path(session_id: &str) -> Result<PathBuf> {
    let base = storage::jcode_dir()?;
    Ok(session_path_in_dir(&base, session_id))
}

pub fn session_journal_path(session_id: &str) -> Result<PathBuf> {
    Ok(session_journal_path_from_snapshot(&session_path(
        session_id,
    )?))
}

pub fn session_exists(session_id: &str) -> bool {
    session_path(session_id)
        .map(|path| path.exists())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::{Mutex, OnceLock};

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        let mutex = ENV_LOCK.get_or_init(|| Mutex::new(()));
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    struct EnvVarGuard {
        key: &'static str,
        prev: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let prev = std::env::var_os(key);
            crate::env::set_var(key, value);
            Self { key, prev }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(prev) = &self.prev {
                crate::env::set_var(self.key, prev);
            } else {
                crate::env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn test_session_exists_roundtrip() {
        let tmp_dir = std::env::temp_dir().join(format!(
            "jcode-session-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(tmp_dir.join("sessions")).unwrap();

        assert!(!session_path_in_dir(&tmp_dir, "missing-session").exists());

        let session_path = session_path_in_dir(&tmp_dir, "exists-session");
        std::fs::write(&session_path, "{}").unwrap();
        assert!(session_path.exists());

        let random_id = format!(
            "missing-session-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        assert!(!session_exists(&random_id));
    }

    #[test]
    fn test_create_marks_debug_when_test_session_env_enabled() {
        let _env_lock = lock_env();
        let _test_flag = EnvVarGuard::set("JCODE_TEST_SESSION", "1");

        let s1 = Session::create(None, None);
        assert!(s1.is_debug);

        let s2 = Session::create_with_id("session_test_1".to_string(), None, None);
        assert!(s2.is_debug);
    }

    #[test]
    fn test_create_not_debug_when_test_session_env_disabled() {
        let _env_lock = lock_env();
        let _test_flag = EnvVarGuard::set("JCODE_TEST_SESSION", "0");

        let s = Session::create(None, None);
        assert!(!s.is_debug);
    }

    #[test]
    fn test_recover_crashed_sessions_preserves_debug_flag() {
        let _env_lock = lock_env();
        let temp_home = tempfile::Builder::new()
            .prefix("jcode-recover-debug-test-")
            .tempdir()
            .expect("create temp JCODE_HOME");
        let _home = EnvVarGuard::set("JCODE_HOME", temp_home.path().as_os_str());
        let _test_flag = EnvVarGuard::set("JCODE_TEST_SESSION", "0");

        let mut crashed = Session::create_with_id(
            "session_recover_debug_source".to_string(),
            None,
            Some("debug source".to_string()),
        );
        crashed.is_debug = true;
        crashed.mark_crashed(Some("test crash".to_string()));
        crashed.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "hello".to_string(),
                cache_control: None,
            }],
        );
        crashed.save().expect("save crashed session");

        let recovered_ids = recover_crashed_sessions().expect("recover crashed sessions");
        assert_eq!(recovered_ids.len(), 1);

        let recovered = Session::load(&recovered_ids[0]).expect("load recovered session");
        assert!(recovered.is_debug);
    }

    #[test]
    fn test_save_persists_full_session_content() {
        let _env_lock = lock_env();
        let temp_home = tempfile::Builder::new()
            .prefix("jcode-session-save-test-")
            .tempdir()
            .expect("create temp JCODE_HOME");
        let _home = EnvVarGuard::set("JCODE_HOME", temp_home.path().as_os_str());

        let mut session = Session::create_with_id(
            "session_save_persist_test".to_string(),
            None,
            Some("save fidelity test".to_string()),
        );

        session.add_message(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "tool_1".to_string(),
                content: "OPENROUTER_API_KEY=sk-or-v1-abcdefghijklmnopqrstuvwxyz0123456789"
                    .to_string(),
                is_error: None,
            }],
        );

        session.add_message(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "tool_2".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({
                    "command": "echo ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123"
                }),
            }],
        );

        session.save().expect("save session");

        let loaded = Session::load("session_save_persist_test").expect("load saved session");

        match &loaded.messages[0].content[0] {
            ContentBlock::ToolResult { content, .. } => {
                assert!(content.contains("sk-or-v1-abcdefghijklmnopqrstuvwxyz0123456789"));
                assert!(!content.contains("[REDACTED_SECRET]"));
            }
            _ => panic!("expected tool result block"),
        }

        match &loaded.messages[1].content[0] {
            ContentBlock::ToolUse { input, .. } => {
                let input_str = input.to_string();
                assert!(input_str.contains("ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123"));
                assert!(!input_str.contains("[REDACTED_SECRET]"));
            }
            _ => panic!("expected tool use block"),
        }
    }

    #[test]
    fn test_save_persists_compaction_state() {
        let _env_lock = lock_env();
        let temp_home = tempfile::Builder::new()
            .prefix("jcode-session-compaction-save-test-")
            .tempdir()
            .expect("create temp JCODE_HOME");
        let _home = EnvVarGuard::set("JCODE_HOME", temp_home.path().as_os_str());

        let mut session = Session::create_with_id(
            "session_compaction_persist_test".to_string(),
            None,
            Some("compaction persistence test".to_string()),
        );
        session.compaction = Some(StoredCompactionState {
            summary_text: "saved summary".to_string(),
            openai_encrypted_content: None,
            covers_up_to_turn: 8,
            original_turn_count: 8,
            compacted_count: 8,
        });

        session.save().expect("save session with compaction state");

        let loaded = Session::load("session_compaction_persist_test").expect("load saved session");
        assert_eq!(loaded.compaction, session.compaction);
    }

    #[test]
    fn test_save_appends_journal_and_load_replays_it() {
        let _env_lock = lock_env();
        let temp_home = tempfile::Builder::new()
            .prefix("jcode-session-journal-test-")
            .tempdir()
            .expect("create temp JCODE_HOME");
        let _home = EnvVarGuard::set("JCODE_HOME", temp_home.path().as_os_str());

        let mut session = Session::create_with_id(
            "session_journal_append_test".to_string(),
            None,
            Some("journal append test".to_string()),
        );
        session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "first".to_string(),
                cache_control: None,
            }],
        );
        session.save().expect("save initial snapshot");

        let snapshot_path = session_path("session_journal_append_test").expect("snapshot path");
        let journal_path =
            session_journal_path("session_journal_append_test").expect("journal path");
        assert!(snapshot_path.exists());
        assert!(!journal_path.exists());

        session.add_message(
            Role::Assistant,
            vec![ContentBlock::Text {
                text: "second".to_string(),
                cache_control: None,
            }],
        );
        session.save().expect("append journal delta");

        assert!(journal_path.exists());
        let journal = std::fs::read_to_string(&journal_path).expect("read journal");
        assert!(journal.contains("second"));

        let loaded = Session::load("session_journal_append_test").expect("load with journal");
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.messages[1].content_preview(), "second");
    }

    #[test]
    fn test_save_checkpoints_after_full_mutation_and_clears_journal() {
        let _env_lock = lock_env();
        let temp_home = tempfile::Builder::new()
            .prefix("jcode-session-checkpoint-test-")
            .tempdir()
            .expect("create temp JCODE_HOME");
        let _home = EnvVarGuard::set("JCODE_HOME", temp_home.path().as_os_str());

        let mut session = Session::create_with_id(
            "session_journal_checkpoint_test".to_string(),
            None,
            Some("checkpoint test".to_string()),
        );
        session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "one".to_string(),
                cache_control: None,
            }],
        );
        session.save().expect("save initial snapshot");

        session.add_message(
            Role::Assistant,
            vec![ContentBlock::Text {
                text: "two".to_string(),
                cache_control: None,
            }],
        );
        session.save().expect("save journal append");

        let journal_path =
            session_journal_path("session_journal_checkpoint_test").expect("journal path");
        assert!(journal_path.exists());

        session.truncate_messages(1);
        session.title = Some("checkpointed title".to_string());
        session.save().expect("checkpoint snapshot");

        assert!(!journal_path.exists());

        let loaded =
            Session::load("session_journal_checkpoint_test").expect("load checkpointed session");
        assert_eq!(loaded.title.as_deref(), Some("checkpointed title"));
        assert_eq!(loaded.messages.len(), 1);
        assert_eq!(loaded.messages[0].content_preview(), "one");
    }

    #[test]
    fn test_redacted_for_export_redacts_tool_result_and_tool_input() {
        let mut session = Session::create_with_id(
            "session_redact_persist_test".to_string(),
            None,
            Some("redaction test".to_string()),
        );

        session.add_message(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "tool_1".to_string(),
                content: "OPENROUTER_API_KEY=sk-or-v1-abcdefghijklmnopqrstuvwxyz0123456789"
                    .to_string(),
                is_error: None,
            }],
        );

        session.add_message(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "tool_2".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({
                    "command": "echo ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123"
                }),
            }],
        );

        let persisted = session.redacted_for_export();

        let first_content = &persisted.messages[0].content[0];
        match first_content {
            ContentBlock::ToolResult { content, .. } => {
                assert!(content.contains("OPENROUTER_API_KEY=[REDACTED_SECRET]"));
                assert!(!content.contains("sk-or-v1-abcdefghijklmnopqrstuvwxyz0123456789"));
            }
            _ => panic!("expected tool result block"),
        }

        let second_content = &persisted.messages[1].content[0];
        match second_content {
            ContentBlock::ToolUse { input, .. } => {
                let input_str = input.to_string();
                assert!(input_str.contains("[REDACTED_SECRET]"));
                assert!(!input_str.contains("ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123"));
            }
            _ => panic!("expected tool use block"),
        }
    }

    #[test]
    fn test_redacted_for_export_redacts_replay_events() {
        let mut session = Session::create_with_id(
            "session_redacted_replay_events_test".to_string(),
            None,
            Some("redacted replay events".to_string()),
        );

        session.record_replay_display_message(
            "swarm",
            Some("DM from fox".to_string()),
            "OPENROUTER_API_KEY=sk-or-v1-secret-value",
        );
        session.record_swarm_status_event(vec![crate::protocol::SwarmMemberStatus {
            session_id: "session_fox".to_string(),
            friendly_name: Some("fox".to_string()),
            status: "running".to_string(),
            detail: Some("ANTHROPIC_API_KEY=sk-ant-secret-value".to_string()),
            role: Some("agent".to_string()),
        }]);
        session.record_swarm_plan_event(
            "swarm_test".to_string(),
            1,
            vec![crate::plan::PlanItem {
                content: "OPENROUTER_API_KEY=sk-or-v1-abcdefghijklmnopqrstuvwxyz0123456789"
                    .to_string(),
                status: "pending".to_string(),
                priority: "high".to_string(),
                id: "task-1".to_string(),
                blocked_by: vec![],
                assigned_to: None,
            }],
            vec![],
            Some("ANTHROPIC_API_KEY=sk-ant-secret-value".to_string()),
        );

        let redacted = session.redacted_for_export();
        assert_eq!(redacted.replay_events.len(), 3);

        match &redacted.replay_events[0].kind {
            StoredReplayEventKind::DisplayMessage { content, .. } => {
                assert!(content.contains("OPENROUTER_API_KEY=[REDACTED_SECRET]"));
                assert!(!content.contains("sk-or-v1-secret-value"));
            }
            other => panic!("expected display message replay event, got {other:?}"),
        }

        match &redacted.replay_events[1].kind {
            StoredReplayEventKind::SwarmStatus { members } => {
                let detail = members[0].detail.as_deref().unwrap_or_default();
                assert!(detail.contains("ANTHROPIC_API_KEY=[REDACTED_SECRET]"));
                assert!(!detail.contains("sk-ant-secret-value"));
            }
            other => panic!("expected swarm status replay event, got {other:?}"),
        }

        match &redacted.replay_events[2].kind {
            StoredReplayEventKind::SwarmPlan { items, reason, .. } => {
                assert!(
                    items[0]
                        .content
                        .contains("OPENROUTER_API_KEY=[REDACTED_SECRET]")
                );
                assert!(
                    !items[0]
                        .content
                        .contains("sk-or-v1-abcdefghijklmnopqrstuvwxyz0123456789")
                );
                let reason = reason.as_deref().unwrap_or_default();
                assert!(reason.contains("ANTHROPIC_API_KEY=[REDACTED_SECRET]"));
                assert!(!reason.contains("sk-ant-secret-value"));
            }
            other => panic!("expected swarm plan replay event, got {other:?}"),
        }
    }

    #[test]
    fn test_summarize_tool_calls_includes_tool_only_assistant_messages() {
        let mut session = Session::create_with_id(
            "session_tool_summary_test".to_string(),
            None,
            Some("tool summary test".to_string()),
        );

        session.add_message(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "tool_1".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({
                    "command": "pwd"
                }),
            }],
        );

        let summaries = summarize_tool_calls(&session, 10);
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].tool_name, "bash");
        assert!(summaries[0].brief_output.contains("pwd"));
    }

    #[test]
    fn test_render_messages_honors_system_display_role_override() {
        let mut session = Session::create_with_id(
            "session_display_role_test".to_string(),
            None,
            Some("display role test".to_string()),
        );

        session.add_message_with_display_role(
            Role::User,
            vec![ContentBlock::Text {
                text: "[Background Task Completed]\nTask: abc123 (bash)".to_string(),
                cache_control: None,
            }],
            Some(StoredDisplayRole::System),
        );

        let rendered = render_messages(&session);
        assert_eq!(rendered.len(), 1);
        assert_eq!(rendered[0].role, "system");
        assert!(rendered[0].content.contains("Background Task Completed"));
    }
}

/// Recover crashed sessions from the most recent crash window (text-only).
/// Returns new recovery session IDs (most recent first).
pub fn recover_crashed_sessions() -> Result<Vec<String>> {
    let sessions_dir = storage::jcode_dir()?.join("sessions");
    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }

    let mut sessions: Vec<Session> = Vec::new();
    for entry in std::fs::read_dir(&sessions_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "json").unwrap_or(false) {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if let Ok(mut session) = Session::load(stem) {
                    if session.detect_crash() {
                        let _ = session.save();
                    }
                    sessions.push(session);
                }
            }
        }
    }

    // Track existing recovery sessions to avoid duplicates
    let mut recovered_parents: HashSet<String> = HashSet::new();
    for s in &sessions {
        if s.id.starts_with("session_recovery_") {
            if let Some(parent) = s.parent_id.as_ref() {
                recovered_parents.insert(parent.clone());
            }
        }
    }

    let mut crashed: Vec<Session> = sessions
        .into_iter()
        .filter(|s| matches!(s.status, SessionStatus::Crashed { .. }))
        .collect();
    if crashed.is_empty() {
        return Ok(Vec::new());
    }

    let crash_window = Duration::seconds(60);
    let most_recent = crashed
        .iter()
        .map(|s| s.last_active_at.unwrap_or(s.updated_at))
        .max()
        .unwrap_or_else(Utc::now);
    crashed.retain(|s| {
        let ts = s.last_active_at.unwrap_or(s.updated_at);
        let delta = most_recent.signed_duration_since(ts);
        delta >= Duration::zero() && delta <= crash_window
    });
    crashed.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

    let mut new_ids = Vec::new();
    for mut old in crashed {
        if recovered_parents.contains(&old.id) {
            continue;
        }

        let new_id = format!("session_recovery_{}", crate::id::new_id("rec"));
        let mut new_session =
            Session::create_with_id(new_id.clone(), Some(old.id.clone()), old.title.clone());
        new_session.working_dir = old.working_dir.clone();
        new_session.model = old.model.clone();
        new_session.is_canary = old.is_canary;
        new_session.is_debug = old.is_debug;
        new_session.testing_build = old.testing_build.clone();
        new_session.saved = old.saved;
        new_session.save_label = old.save_label.clone();
        new_session.provider_session_id = None;
        new_session.status = SessionStatus::Closed;

        // Add a recovery header
        new_session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: format!(
                    "Recovered from crashed session `{}` ({})",
                    old.id,
                    old.display_name()
                ),
                cache_control: None,
            }],
        );

        for msg in old.messages.drain(..) {
            let kept_blocks: Vec<ContentBlock> = msg
                .content
                .into_iter()
                .filter(|block| matches!(block, ContentBlock::Text { .. }))
                .collect();
            if kept_blocks.is_empty() {
                continue;
            }
            new_session.add_message(msg.role, kept_blocks);
        }

        new_session.save()?;
        new_ids.push(new_id);
    }

    Ok(new_ids)
}

/// Info about crashed sessions pending batch restore
#[derive(Debug, Clone)]
pub struct CrashedSessionsInfo {
    /// Session IDs that crashed
    pub session_ids: Vec<String>,
    /// Display names of crashed sessions
    pub display_names: Vec<String>,
    /// When the most recent crash occurred
    pub most_recent_crash: DateTime<Utc>,
}

/// Detect crashed sessions that can be batch restored.
/// Returns info about crashed sessions within the crash window (60 seconds),
/// excluding any that have already been recovered.
pub fn detect_crashed_sessions() -> Result<Option<CrashedSessionsInfo>> {
    let sessions_dir = storage::jcode_dir()?.join("sessions");
    if !sessions_dir.exists() {
        return Ok(None);
    }

    let mut sessions: Vec<Session> = Vec::new();
    for entry in std::fs::read_dir(&sessions_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "json").unwrap_or(false) {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if let Ok(mut session) = Session::load(stem) {
                    // Detect if this session crashed (updates status if so)
                    if session.detect_crash() {
                        let _ = session.save();
                    }
                    sessions.push(session);
                }
            }
        }
    }

    // Track existing recovery sessions to avoid showing already-recovered crashes
    let mut recovered_parents: HashSet<String> = HashSet::new();
    for s in &sessions {
        if s.id.starts_with("session_recovery_") {
            if let Some(parent) = s.parent_id.as_ref() {
                recovered_parents.insert(parent.clone());
            }
        }
    }

    // Filter to crashed sessions that haven't been recovered
    let mut crashed: Vec<Session> = sessions
        .into_iter()
        .filter(|s| matches!(s.status, SessionStatus::Crashed { .. }))
        .filter(|s| !recovered_parents.contains(&s.id))
        .collect();

    if crashed.is_empty() {
        return Ok(None);
    }

    // Apply 60-second crash window filter
    let crash_window = Duration::seconds(60);
    let most_recent = crashed
        .iter()
        .map(|s| s.last_active_at.unwrap_or(s.updated_at))
        .max()
        .unwrap_or_else(Utc::now);

    crashed.retain(|s| {
        let ts = s.last_active_at.unwrap_or(s.updated_at);
        let delta = most_recent.signed_duration_since(ts);
        delta >= Duration::zero() && delta <= crash_window
    });

    if crashed.is_empty() {
        return Ok(None);
    }

    // Sort by most recent first
    crashed.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

    let session_ids: Vec<String> = crashed.iter().map(|s| s.id.clone()).collect();
    let display_names: Vec<String> = crashed
        .iter()
        .map(|s| s.display_name().to_string())
        .collect();

    Ok(Some(CrashedSessionsInfo {
        session_ids,
        display_names,
        most_recent_crash: most_recent,
    }))
}

/// Lightweight session header for fast scanning (skips messages array).
/// Uses serde's `deny_unknown_fields` = false (default) so the large `messages`
/// field is silently ignored during deserialization.
#[derive(Debug, Clone, Deserialize)]
struct SessionHeader {
    id: String,
    #[serde(default)]
    parent_id: Option<String>,
    #[allow(dead_code)]
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    #[serde(default)]
    short_name: Option<String>,
    #[serde(default)]
    status: SessionStatus,
    #[serde(default)]
    last_active_at: Option<DateTime<Utc>>,
}

impl SessionHeader {
    fn display_name(&self) -> &str {
        if let Some(ref name) = self.short_name {
            name
        } else if let Some(name) = extract_session_name(&self.id) {
            name
        } else {
            &self.id
        }
    }
}

/// Find recent crashed sessions for showing resume hints.
///
/// Uses a fast O(n) scan of `~/.jcode/active_pids/` (typically 0-5 files)
/// instead of scanning the full sessions directory (tens of thousands).
/// Each file in active_pids/ contains a PID; if that PID is dead, the
/// session crashed. We then load only those specific session files.
///
/// Falls back to the legacy directory scan if active_pids/ doesn't exist
/// (first run after upgrade).
pub fn find_recent_crashed_sessions() -> Vec<(String, String)> {
    if let Some(results) = find_crashed_via_pid_files() {
        return results;
    }
    find_crashed_legacy_scan()
}

/// Fast path: check active_pids/ directory for dead PIDs.
fn find_crashed_via_pid_files() -> Option<Vec<(String, String)>> {
    let dir = active_pids_dir()?;
    if !dir.exists() {
        return None;
    }

    let entries = std::fs::read_dir(&dir).ok()?;
    let mut crashed: Vec<(String, String, DateTime<Utc>)> = Vec::new();

    for entry in entries.flatten() {
        let session_id = match entry.file_name().to_str() {
            Some(s) => s.to_string(),
            None => continue,
        };

        let pid_str = match std::fs::read_to_string(entry.path()) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let pid: u32 = match pid_str.trim().parse() {
            Ok(p) => p,
            Err(_) => {
                let _ = std::fs::remove_file(entry.path());
                continue;
            }
        };

        if is_pid_running(pid) {
            continue;
        }

        match Session::load(&session_id) {
            Ok(mut session) => {
                session.mark_crashed(Some(format!(
                    "Process {} exited unexpectedly (no shutdown signal captured)",
                    pid
                )));
                let _ = session.save();
                let name = extract_session_name(&session_id)
                    .unwrap_or(&session_id)
                    .to_string();
                let ts = session.last_active_at.unwrap_or(session.updated_at);
                crashed.push((session_id, name, ts));
            }
            Err(_) => {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    crashed.sort_by(|a, b| b.2.cmp(&a.2));
    Some(
        crashed
            .into_iter()
            .map(|(id, name, _)| (id, name))
            .collect(),
    )
}

/// Legacy fallback: scan the full sessions directory.
/// Used only on the first launch after upgrading to the active_pids system.
fn find_crashed_legacy_scan() -> Vec<(String, String)> {
    let sessions_dir = match storage::jcode_dir() {
        Ok(d) => d.join("sessions"),
        Err(_) => return Vec::new(),
    };
    if !sessions_dir.exists() {
        return Vec::new();
    }

    let cutoff = Utc::now() - Duration::hours(24);
    let cutoff_system = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(24 * 3600))
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let filename_cutoff_ms: u64 = (chrono::Utc::now() - Duration::hours(48))
        .timestamp_millis()
        .max(0) as u64;

    let mut recovered_parents: HashSet<String> = HashSet::new();
    let mut candidates: Vec<SessionHeader> = Vec::new();

    let entries = match std::fs::read_dir(&sessions_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    for entry in entries.flatten() {
        if let Some(fname) = entry.file_name().to_str() {
            if let Some(ts) = extract_timestamp_from_filename(fname) {
                if ts < filename_cutoff_ms {
                    continue;
                }
            }
        }

        let path = entry.path();
        if !path.extension().map(|e| e == "json").unwrap_or(false) {
            continue;
        }

        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if let Ok(mtime) = meta.modified() {
            if mtime < cutoff_system {
                continue;
            }
        }
        if meta.len() == 0 {
            continue;
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let has_crashed = content.contains("\"Crashed\"");
        let is_recovery = content.contains("\"session_recovery_\"");

        if !has_crashed && !is_recovery {
            continue;
        }

        if let Ok(header) = serde_json::from_str::<SessionHeader>(&content) {
            if header.id.starts_with("session_recovery_") {
                if let Some(parent) = header.parent_id.as_ref() {
                    recovered_parents.insert(parent.clone());
                }
            }
            if has_crashed {
                candidates.push(header);
            }
        }
    }

    let mut crashed: Vec<SessionHeader> = candidates
        .into_iter()
        .filter(|s| matches!(s.status, SessionStatus::Crashed { .. }))
        .filter(|s| !recovered_parents.contains(&s.id))
        .filter(|s| {
            let ts = s.last_active_at.unwrap_or(s.updated_at);
            ts > cutoff
        })
        .collect();

    crashed.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

    crashed
        .into_iter()
        .map(|s| {
            let name = s.display_name().to_string();
            let id = s.id.clone();
            (id, name)
        })
        .collect()
}

/// Extract the epoch-ms timestamp embedded in a session filename.
/// Handles formats like:
///   "session_fox_1772405007295.json" (memorable id)
///   "session_1772405007295_hash.json" (legacy)
///   "session_recovery_1772405007295.json"
fn extract_timestamp_from_filename(filename: &str) -> Option<u64> {
    let stem = filename.strip_suffix(".json").unwrap_or(filename);
    // Walk the underscore-separated parts and find the first one that
    // looks like a plausible epoch-ms (13+ digits, starts with '1').
    for part in stem.split('_') {
        if part.len() >= 13 && part.starts_with('1') && part.chars().all(|c| c.is_ascii_digit()) {
            return part.parse::<u64>().ok();
        }
    }
    None
}

fn is_pid_running(pid: u32) -> bool {
    crate::platform::is_process_running(pid)
}

// ---------------------------------------------------------------------------
// Active PID tracking
// ---------------------------------------------------------------------------
// Lightweight files in ~/.jcode/active_pids/<session_id> containing the PID.
// Written on mark_active(), removed on mark_closed()/mark_crashed().
// On startup we only need to scan this tiny directory (usually 0-5 files)
// instead of the entire sessions/ directory (tens of thousands of files).

fn active_pids_dir() -> Option<std::path::PathBuf> {
    storage::jcode_dir().ok().map(|d| d.join("active_pids"))
}

fn register_active_pid(session_id: &str, pid: u32) {
    if let Some(dir) = active_pids_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(session_id), pid.to_string());
    }
}

fn unregister_active_pid(session_id: &str) {
    if let Some(dir) = active_pids_dir() {
        let _ = std::fs::remove_file(dir.join(session_id));
    }
}

/// Find the active session ID currently owned by the given process ID.
pub fn find_active_session_id_by_pid(pid: u32) -> Option<String> {
    let dir = active_pids_dir()?;
    for entry in std::fs::read_dir(dir).ok()? {
        let entry = entry.ok()?;
        let session_id = entry.file_name().to_string_lossy().to_string();
        let stored = std::fs::read_to_string(entry.path()).ok()?;
        if stored.trim().parse::<u32>().ok()? == pid {
            return Some(session_id);
        }
    }
    None
}

/// List active session IDs currently tracked in ~/.jcode/active_pids.
pub fn active_session_ids() -> Vec<String> {
    let Some(dir) = active_pids_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect()
}

/// Find a session by ID or memorable name
/// If the input doesn't look like a full session ID (doesn't contain underscore followed by digits),
/// try to find a session whose short name matches.
/// Returns the full session ID if found.
pub fn find_session_by_name_or_id(name_or_id: &str) -> Result<String> {
    // If it looks like a full session ID (contains session_), try loading directly first
    if name_or_id.starts_with("session_") {
        match Session::load(name_or_id) {
            Ok(_) => return Ok(name_or_id.to_string()),
            Err(e) => {
                if session_exists(name_or_id) {
                    anyhow::bail!(
                        "Session '{}' exists but failed to load (possibly corrupt):\n  {}",
                        name_or_id,
                        e
                    );
                }
            }
        }
    }

    // Otherwise, search for a session with matching short name
    let sessions_dir = storage::jcode_dir()?.join("sessions");
    if !sessions_dir.exists() {
        anyhow::bail!("No sessions found");
    }

    let mut matches: Vec<(String, chrono::DateTime<chrono::Utc>)> = Vec::new();

    for entry in std::fs::read_dir(&sessions_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "json").unwrap_or(false) {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                // Check if short name matches
                if let Some(short_name) = extract_session_name(stem) {
                    if short_name == name_or_id {
                        if let Ok(session) = Session::load(stem) {
                            matches.push((stem.to_string(), session.updated_at));
                        }
                    }
                }
            }
        }
    }

    if matches.is_empty() {
        anyhow::bail!("No session found matching '{}'", name_or_id);
    }

    // Sort by updated_at descending and return the most recent match
    matches.sort_by(|a, b| b.1.cmp(&a.1));
    Ok(matches[0].0.clone())
}

#[cfg(test)]
mod batch_crash_tests {
    use super::*;

    #[test]
    fn test_crashed_sessions_info_struct() {
        let info = CrashedSessionsInfo {
            session_ids: vec!["session_test_1".to_string(), "session_test_2".to_string()],
            display_names: vec!["fox".to_string(), "oak".to_string()],
            most_recent_crash: Utc::now(),
        };
        assert_eq!(info.session_ids.len(), 2);
        assert_eq!(info.display_names.len(), 2);
        assert_eq!(info.display_names[0], "fox");
    }
}
