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
use std::time::Instant;
mod active_pids;
use active_pids::{active_pids_dir, register_active_pid, unregister_active_pid};
pub use active_pids::{active_session_ids, find_active_session_id_by_pid};

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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SessionImproveMode {
    #[serde(rename = "improve_run", alias = "run")]
    ImproveRun,
    #[serde(rename = "improve_plan", alias = "plan")]
    ImprovePlan,
    #[serde(rename = "refactor_run")]
    RefactorRun,
    #[serde(rename = "refactor_plan")]
    RefactorPlan,
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
    /// Stable memory IDs included in this injection, used to avoid re-injecting
    /// the same memories after session resume/reload.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub memory_ids: Vec<String>,
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
    BackgroundTask,
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

const LARGE_MEMORY_BLOB_THRESHOLD_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Default)]
struct ContentBlockMemoryStats {
    block_count: usize,
    text_blocks: usize,
    text_bytes: usize,
    reasoning_blocks: usize,
    reasoning_bytes: usize,
    tool_use_blocks: usize,
    tool_use_input_json_bytes: usize,
    tool_result_blocks: usize,
    tool_result_bytes: usize,
    image_blocks: usize,
    image_data_bytes: usize,
    openai_compaction_blocks: usize,
    openai_compaction_bytes: usize,
    large_block_count: usize,
    large_block_bytes: usize,
    large_tool_result_count: usize,
    large_tool_result_bytes: usize,
    max_block_bytes: usize,
    max_tool_result_bytes: usize,
}

impl ContentBlockMemoryStats {
    fn merge_from(&mut self, other: &Self) {
        self.block_count += other.block_count;
        self.text_blocks += other.text_blocks;
        self.text_bytes += other.text_bytes;
        self.reasoning_blocks += other.reasoning_blocks;
        self.reasoning_bytes += other.reasoning_bytes;
        self.tool_use_blocks += other.tool_use_blocks;
        self.tool_use_input_json_bytes += other.tool_use_input_json_bytes;
        self.tool_result_blocks += other.tool_result_blocks;
        self.tool_result_bytes += other.tool_result_bytes;
        self.image_blocks += other.image_blocks;
        self.image_data_bytes += other.image_data_bytes;
        self.openai_compaction_blocks += other.openai_compaction_blocks;
        self.openai_compaction_bytes += other.openai_compaction_bytes;
        self.large_block_count += other.large_block_count;
        self.large_block_bytes += other.large_block_bytes;
        self.large_tool_result_count += other.large_tool_result_count;
        self.large_tool_result_bytes += other.large_tool_result_bytes;
        self.max_block_bytes = self.max_block_bytes.max(other.max_block_bytes);
        self.max_tool_result_bytes = self.max_tool_result_bytes.max(other.max_tool_result_bytes);
    }

    fn record_bytes(&mut self, bytes: usize) {
        self.max_block_bytes = self.max_block_bytes.max(bytes);
        if bytes >= LARGE_MEMORY_BLOB_THRESHOLD_BYTES {
            self.large_block_count += 1;
            self.large_block_bytes += bytes;
        }
    }

    fn record_block(&mut self, block: &ContentBlock) {
        self.block_count += 1;
        match block {
            ContentBlock::Text { text, .. } => {
                self.text_blocks += 1;
                self.text_bytes += text.len();
                self.record_bytes(text.len());
            }
            ContentBlock::Reasoning { text } => {
                self.reasoning_blocks += 1;
                self.reasoning_bytes += text.len();
                self.record_bytes(text.len());
            }
            ContentBlock::ToolUse { input, .. } => {
                self.tool_use_blocks += 1;
                let input_bytes = estimate_json_bytes(input);
                self.tool_use_input_json_bytes += input_bytes;
                self.record_bytes(input_bytes);
            }
            ContentBlock::ToolResult { content, .. } => {
                self.tool_result_blocks += 1;
                self.tool_result_bytes += content.len();
                self.max_tool_result_bytes = self.max_tool_result_bytes.max(content.len());
                if content.len() >= LARGE_MEMORY_BLOB_THRESHOLD_BYTES {
                    self.large_tool_result_count += 1;
                    self.large_tool_result_bytes += content.len();
                }
                self.record_bytes(content.len());
            }
            ContentBlock::Image { data, .. } => {
                self.image_blocks += 1;
                self.image_data_bytes += data.len();
                self.record_bytes(data.len());
            }
            ContentBlock::OpenAICompaction { encrypted_content } => {
                self.openai_compaction_blocks += 1;
                self.openai_compaction_bytes += encrypted_content.len();
                self.record_bytes(encrypted_content.len());
            }
        }
    }

    fn payload_text_bytes(&self) -> usize {
        self.text_bytes
            + self.reasoning_bytes
            + self.tool_result_bytes
            + self.image_data_bytes
            + self.openai_compaction_bytes
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "content_blocks": self.block_count,
            "text_blocks": self.text_blocks,
            "text_bytes": self.text_bytes,
            "reasoning_blocks": self.reasoning_blocks,
            "reasoning_bytes": self.reasoning_bytes,
            "tool_use_blocks": self.tool_use_blocks,
            "tool_use_input_json_bytes": self.tool_use_input_json_bytes,
            "tool_result_blocks": self.tool_result_blocks,
            "tool_result_bytes": self.tool_result_bytes,
            "image_blocks": self.image_blocks,
            "image_data_bytes": self.image_data_bytes,
            "openai_compaction_blocks": self.openai_compaction_blocks,
            "openai_compaction_bytes": self.openai_compaction_bytes,
            "large_block_count": self.large_block_count,
            "large_block_bytes": self.large_block_bytes,
            "large_tool_result_count": self.large_tool_result_count,
            "large_tool_result_bytes": self.large_tool_result_bytes,
            "max_block_bytes": self.max_block_bytes,
            "max_tool_result_bytes": self.max_tool_result_bytes,
            "payload_text_bytes": self.payload_text_bytes(),
        })
    }
}

fn summarize_message_content<'a, I>(messages: I) -> ContentBlockMemoryStats
where
    I: IntoIterator<Item = &'a Vec<ContentBlock>>,
{
    let mut stats = ContentBlockMemoryStats::default();
    for blocks in messages {
        for block in blocks {
            stats.record_block(block);
        }
    }
    stats
}

fn summarize_blocks(blocks: &[ContentBlock]) -> ContentBlockMemoryStats {
    let mut stats = ContentBlockMemoryStats::default();
    for block in blocks {
        stats.record_block(block);
    }
    stats
}

#[derive(Debug, Clone, Default)]
struct SessionMemoryProfileCache {
    messages_count: usize,
    messages_json_bytes: usize,
    message_stats: ContentBlockMemoryStats,
    env_snapshots_count: usize,
    env_snapshots_json_bytes: usize,
    memory_injections_count: usize,
    memory_injections_json_bytes: usize,
    replay_events_count: usize,
    replay_events_json_bytes: usize,
    provider_cache_count: usize,
    provider_cache_json_bytes: usize,
    provider_cache_stats: ContentBlockMemoryStats,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionMemoryProfileSnapshot {
    pub message_count: usize,
    pub provider_cache_message_count: usize,
    pub env_snapshot_count: usize,
    pub memory_injection_count: usize,
    pub replay_event_count: usize,
    pub payload_text_bytes: usize,
    pub total_json_bytes: usize,
    pub provider_cache_json_bytes: usize,
    pub canonical_tool_result_bytes: usize,
    pub provider_cache_tool_result_bytes: usize,
    pub canonical_large_blob_bytes: usize,
    pub provider_cache_large_blob_bytes: usize,
}

fn stored_messages_to_messages(messages: &[StoredMessage]) -> Vec<Message> {
    messages.iter().map(StoredMessage::to_message).collect()
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
    /// Stable provider/profile key for session-source filtering (e.g. "openai",
    /// "opencode", "opencode-go").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_key: Option<String>,
    /// Model identifier for this session (e.g., "gpt-5.2-codex")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Optional fixed model to use for subagents launched from this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_model: Option<String>,
    /// Last requested `/improve` mode for this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub improve_mode: Option<SessionImproveMode>,
    /// Whether automatic end-of-turn review is enabled for this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autoreview_enabled: Option<bool>,
    /// Whether automatic end-of-turn judging is enabled for this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autojudge_enabled: Option<bool>,
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
    #[serde(skip)]
    provider_messages_cache: Vec<Message>,
    #[serde(skip)]
    provider_message_prefix_hashes_cache: Vec<u64>,
    #[serde(skip)]
    provider_messages_cache_len: usize,
    #[serde(skip)]
    provider_messages_cache_mode: PersistVectorMode,
    #[serde(skip)]
    memory_profile_cache: SessionMemoryProfileCache,
    #[serde(skip)]
    memory_profile_dirty: bool,
}

#[derive(Debug, Deserialize)]
struct SessionStartupStub {
    id: String,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    #[serde(default)]
    compaction: Option<StoredCompactionState>,
    #[serde(default)]
    provider_session_id: Option<String>,
    #[serde(default)]
    provider_key: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    subagent_model: Option<String>,
    #[serde(default)]
    improve_mode: Option<SessionImproveMode>,
    #[serde(default)]
    autoreview_enabled: Option<bool>,
    #[serde(default)]
    autojudge_enabled: Option<bool>,
    #[serde(default)]
    is_canary: bool,
    #[serde(default)]
    testing_build: Option<String>,
    #[serde(default)]
    working_dir: Option<String>,
    #[serde(default)]
    short_name: Option<String>,
    #[serde(default)]
    status: SessionStatus,
    #[serde(default)]
    last_pid: Option<u32>,
    #[serde(default)]
    last_active_at: Option<DateTime<Utc>>,
    #[serde(default)]
    is_debug: bool,
    #[serde(default)]
    saved: bool,
    #[serde(default)]
    save_label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct SessionJournalMeta {
    parent_id: Option<String>,
    title: Option<String>,
    updated_at: DateTime<Utc>,
    compaction: Option<StoredCompactionState>,
    provider_session_id: Option<String>,
    provider_key: Option<String>,
    model: Option<String>,
    subagent_model: Option<String>,
    improve_mode: Option<SessionImproveMode>,
    autoreview_enabled: Option<bool>,
    autojudge_enabled: Option<bool>,
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

pub fn derive_session_provider_key(provider_name: &str) -> Option<String> {
    let normalized_name = provider_name.trim().to_ascii_lowercase();
    if normalized_name == "jcode" {
        return Some("jcode".to_string());
    }

    if let Ok(namespace) = std::env::var("JCODE_OPENROUTER_CACHE_NAMESPACE") {
        let namespace = namespace.trim().to_ascii_lowercase();
        if !namespace.is_empty() {
            return Some(namespace);
        }
    }

    if let Ok(active) = std::env::var("JCODE_ACTIVE_PROVIDER") {
        let active = active.trim().to_ascii_lowercase();
        if !active.is_empty() {
            return Some(active);
        }
    }

    let fallback = match normalized_name.as_str() {
        "anthropic" | "claude" | "claude cli" => "claude",
        "openai" => "openai",
        "github copilot" | "copilot" => "copilot",
        "openrouter" => "openrouter",
        "cursor" => "cursor",
        "gemini" => "gemini",
        "antigravity" => "antigravity",
        "" => return None,
        other => other,
    };

    Some(fallback.to_string())
}

impl Session {
    fn session_from_startup_stub(stub: SessionStartupStub) -> Self {
        let mut session = Self::create_with_id(stub.id, stub.parent_id, stub.title);
        session.created_at = stub.created_at;
        session.updated_at = stub.updated_at;
        session.compaction = stub.compaction;
        session.provider_session_id = stub.provider_session_id;
        session.provider_key = stub.provider_key;
        session.model = stub.model;
        session.subagent_model = stub.subagent_model;
        session.improve_mode = stub.improve_mode;
        session.autoreview_enabled = stub.autoreview_enabled;
        session.autojudge_enabled = stub.autojudge_enabled;
        session.is_canary = stub.is_canary;
        session.testing_build = stub.testing_build;
        session.working_dir = stub.working_dir;
        session.short_name = stub.short_name;
        session.status = stub.status;
        session.last_pid = stub.last_pid;
        session.last_active_at = stub.last_active_at;
        session.is_debug = stub.is_debug;
        session.saved = stub.saved;
        session.save_label = stub.save_label;
        session.messages.clear();
        session.env_snapshots.clear();
        session.memory_injections.clear();
        session.replay_events.clear();
        session.rebuild_memory_profile_cache();
        session.reset_persist_state(true);
        session
    }

    fn session_from_remote_startup_snapshot(snapshot: RemoteStartupSessionSnapshot) -> Self {
        let mut session = Self::create_with_id(snapshot.id, snapshot.parent_id, snapshot.title);
        session.created_at = snapshot.created_at;
        session.updated_at = snapshot.updated_at;
        session.messages = snapshot.messages;
        session.compaction = snapshot.compaction;
        session.provider_session_id = snapshot.provider_session_id;
        session.provider_key = snapshot.provider_key;
        session.model = snapshot.model;
        session.subagent_model = snapshot.subagent_model;
        session.improve_mode = snapshot.improve_mode;
        session.autoreview_enabled = snapshot.autoreview_enabled;
        session.autojudge_enabled = snapshot.autojudge_enabled;
        session.is_canary = snapshot.is_canary;
        session.testing_build = snapshot.testing_build;
        session.working_dir = snapshot.working_dir;
        session.short_name = snapshot.short_name;
        session.status = snapshot.status;
        session.last_pid = snapshot.last_pid;
        session.last_active_at = snapshot.last_active_at;
        session.is_debug = snapshot.is_debug;
        session.saved = snapshot.saved;
        session.save_label = snapshot.save_label;
        session.replay_events.clear();
        session.env_snapshots.clear();
        session.memory_injections.clear();
        session.mark_memory_profile_dirty();
        session.reset_persist_state(true);
        session.reset_provider_messages_cache();
        session
    }

    pub fn debug_memory_profile(&self) -> serde_json::Value {
        let message_stats =
            summarize_message_content(self.messages.iter().map(|message| &message.content));

        let session_message_json_bytes: usize = self.messages.iter().map(estimate_json_bytes).sum();
        let provider_cache_stats = summarize_message_content(
            self.provider_messages_cache
                .iter()
                .map(|message| &message.content),
        );
        let provider_messages_cache_json_bytes: usize = self
            .provider_messages_cache
            .iter()
            .map(estimate_json_bytes)
            .sum();
        let env_snapshots_json_bytes: usize =
            self.env_snapshots.iter().map(estimate_json_bytes).sum();
        let memory_injections_json_bytes: usize =
            self.memory_injections.iter().map(estimate_json_bytes).sum();
        let replay_events_json_bytes: usize =
            self.replay_events.iter().map(estimate_json_bytes).sum();
        let compaction_json_bytes = self
            .compaction
            .as_ref()
            .map(estimate_json_bytes)
            .unwrap_or(0);
        let compaction_summary_bytes = self
            .compaction
            .as_ref()
            .map(|c| c.summary_text.len())
            .unwrap_or(0);
        let compaction_encrypted_bytes = self
            .compaction
            .as_ref()
            .and_then(|c| c.openai_encrypted_content.as_ref())
            .map(|text| text.len())
            .unwrap_or(0);

        serde_json::json!({
            "session_id": self.id,
            "messages": {
                "count": self.messages.len(),
                "json_bytes": session_message_json_bytes,
                "memory": message_stats.to_json(),
            },
            "compaction": {
                "present": self.compaction.is_some(),
                "json_bytes": compaction_json_bytes,
                "summary_text_bytes": compaction_summary_bytes,
                "encrypted_content_bytes": compaction_encrypted_bytes,
            },
            "env_snapshots": {
                "count": self.env_snapshots.len(),
                "json_bytes": env_snapshots_json_bytes,
            },
            "memory_injections": {
                "count": self.memory_injections.len(),
                "json_bytes": memory_injections_json_bytes,
            },
            "replay_events": {
                "count": self.replay_events.len(),
                "json_bytes": replay_events_json_bytes,
            },
            "provider_messages_cache": {
                "count": self.provider_messages_cache.len(),
                "source_len": self.provider_messages_cache_len,
                "mode": persist_vector_mode_label(self.provider_messages_cache_mode),
                "json_bytes": provider_messages_cache_json_bytes,
                "memory": provider_cache_stats.to_json(),
            },
            "totals": {
                "payload_text_bytes": message_stats.payload_text_bytes(),
                "json_bytes": session_message_json_bytes
                    + provider_messages_cache_json_bytes
                    + env_snapshots_json_bytes
                    + memory_injections_json_bytes
                    + replay_events_json_bytes
                    + compaction_json_bytes,
                "canonical_transcript_json_bytes": session_message_json_bytes,
                "provider_cache_json_bytes": provider_messages_cache_json_bytes,
                "canonical_tool_result_bytes": message_stats.tool_result_bytes,
                "provider_cache_tool_result_bytes": provider_cache_stats.tool_result_bytes,
                "canonical_large_blob_bytes": message_stats.large_block_bytes,
                "provider_cache_large_blob_bytes": provider_cache_stats.large_block_bytes,
            }
        })
    }

    fn journal_meta(&self) -> SessionJournalMeta {
        SessionJournalMeta {
            parent_id: self.parent_id.clone(),
            title: self.title.clone(),
            updated_at: self.updated_at,
            compaction: self.compaction.clone(),
            provider_session_id: self.provider_session_id.clone(),
            provider_key: self.provider_key.clone(),
            model: self.model.clone(),
            subagent_model: self.subagent_model.clone(),
            improve_mode: self.improve_mode,
            autoreview_enabled: self.autoreview_enabled,
            autojudge_enabled: self.autojudge_enabled,
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

    fn reset_provider_messages_cache(&mut self) {
        self.provider_messages_cache.clear();
        self.provider_message_prefix_hashes_cache.clear();
        self.provider_messages_cache_len = 0;
        self.provider_messages_cache_mode = PersistVectorMode::Full;
        self.memory_profile_cache.provider_cache_count = 0;
        self.memory_profile_cache.provider_cache_json_bytes = 0;
        self.memory_profile_cache.provider_cache_stats = ContentBlockMemoryStats::default();
    }

    fn push_provider_message_cache_entry(&mut self, message: Message) {
        let message_hash = crate::message::stable_message_hash(&message);
        let prefix_hash = self
            .provider_message_prefix_hashes_cache
            .last()
            .copied()
            .map(|prev| crate::message::extend_stable_hash(prev, message_hash))
            .unwrap_or(message_hash);
        self.memory_profile_cache.provider_cache_count += 1;
        self.memory_profile_cache.provider_cache_json_bytes += estimate_json_bytes(&message);
        self.memory_profile_cache
            .provider_cache_stats
            .merge_from(&summarize_blocks(&message.content));
        self.provider_messages_cache.push(message);
        self.provider_message_prefix_hashes_cache.push(prefix_hash);
    }

    fn mark_memory_profile_dirty(&mut self) {
        self.memory_profile_dirty = true;
    }

    fn rebuild_memory_profile_cache(&mut self) {
        let message_stats =
            summarize_message_content(self.messages.iter().map(|message| &message.content));
        let provider_cache_stats = summarize_message_content(
            self.provider_messages_cache
                .iter()
                .map(|message| &message.content),
        );

        self.memory_profile_cache = SessionMemoryProfileCache {
            messages_count: self.messages.len(),
            messages_json_bytes: self.messages.iter().map(estimate_json_bytes).sum(),
            message_stats,
            env_snapshots_count: self.env_snapshots.len(),
            env_snapshots_json_bytes: self.env_snapshots.iter().map(estimate_json_bytes).sum(),
            memory_injections_count: self.memory_injections.len(),
            memory_injections_json_bytes: self
                .memory_injections
                .iter()
                .map(estimate_json_bytes)
                .sum(),
            replay_events_count: self.replay_events.len(),
            replay_events_json_bytes: self.replay_events.iter().map(estimate_json_bytes).sum(),
            provider_cache_count: self.provider_messages_cache.len(),
            provider_cache_json_bytes: self
                .provider_messages_cache
                .iter()
                .map(estimate_json_bytes)
                .sum(),
            provider_cache_stats,
        };
        self.memory_profile_dirty = false;
    }

    fn ensure_memory_profile_cache(&mut self) {
        if self.memory_profile_dirty {
            self.rebuild_memory_profile_cache();
        }
    }

    pub fn memory_profile_snapshot(&mut self) -> SessionMemoryProfileSnapshot {
        self.ensure_memory_profile_cache();
        let compaction_json_bytes = self
            .compaction
            .as_ref()
            .map(estimate_json_bytes)
            .unwrap_or(0);

        SessionMemoryProfileSnapshot {
            message_count: self.memory_profile_cache.messages_count,
            provider_cache_message_count: self.memory_profile_cache.provider_cache_count,
            env_snapshot_count: self.memory_profile_cache.env_snapshots_count,
            memory_injection_count: self.memory_profile_cache.memory_injections_count,
            replay_event_count: self.memory_profile_cache.replay_events_count,
            payload_text_bytes: self.memory_profile_cache.message_stats.payload_text_bytes(),
            total_json_bytes: self.memory_profile_cache.messages_json_bytes
                + self.memory_profile_cache.provider_cache_json_bytes
                + self.memory_profile_cache.env_snapshots_json_bytes
                + self.memory_profile_cache.memory_injections_json_bytes
                + self.memory_profile_cache.replay_events_json_bytes
                + compaction_json_bytes,
            provider_cache_json_bytes: self.memory_profile_cache.provider_cache_json_bytes,
            canonical_tool_result_bytes: self.memory_profile_cache.message_stats.tool_result_bytes,
            provider_cache_tool_result_bytes: self
                .memory_profile_cache
                .provider_cache_stats
                .tool_result_bytes,
            canonical_large_blob_bytes: self.memory_profile_cache.message_stats.large_block_bytes,
            provider_cache_large_blob_bytes: self
                .memory_profile_cache
                .provider_cache_stats
                .large_block_bytes,
        }
    }

    fn mark_messages_append_dirty(&mut self) {
        if self.persist_state.messages_mode != PersistVectorMode::Full {
            self.persist_state.messages_mode = PersistVectorMode::Append;
        }
        if self.provider_messages_cache_mode != PersistVectorMode::Full {
            self.provider_messages_cache_mode = PersistVectorMode::Append;
        }
    }

    fn mark_messages_full_dirty(&mut self) {
        self.persist_state.messages_mode = PersistVectorMode::Full;
        self.provider_messages_cache_mode = PersistVectorMode::Full;
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

    fn mark_replay_events_append_dirty(&mut self) {
        if self.persist_state.replay_events_mode != PersistVectorMode::Full {
            self.persist_state.replay_events_mode = PersistVectorMode::Append;
        }
    }

    fn metadata_requires_snapshot(prev: &SessionJournalMeta, current: &SessionJournalMeta) -> bool {
        prev.parent_id != current.parent_id
            || prev.title != current.title
            || prev.provider_key != current.provider_key
            || prev.subagent_model != current.subagent_model
            || prev.improve_mode != current.improve_mode
            || prev.autoreview_enabled != current.autoreview_enabled
            || prev.autojudge_enabled != current.autojudge_enabled
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
        self.provider_key = meta.provider_key;
        self.model = meta.model;
        self.subagent_model = meta.subagent_model;
        self.improve_mode = meta.improve_mode;
        self.autoreview_enabled = meta.autoreview_enabled;
        self.autojudge_enabled = meta.autojudge_enabled;
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
        self.mark_memory_profile_dirty();
    }

    fn apply_journal_entry(&mut self, entry: SessionJournalEntry) {
        self.apply_journal_meta(entry.meta);
        self.messages.extend(entry.append_messages);
        self.env_snapshots.extend(entry.append_env_snapshots);
        self.memory_injections
            .extend(entry.append_memory_injections);
        self.replay_events.extend(entry.append_replay_events);
        self.mark_memory_profile_dirty();
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
        let load_start = Instant::now();
        let snapshot_bytes = file_len_or_zero(path);
        let snapshot_start = Instant::now();
        let mut session: Session = storage::read_json(path)?;
        let snapshot_ms = snapshot_start.elapsed().as_millis();
        let journal_path = session_journal_path_from_snapshot(path);
        let journal_bytes = file_len_or_zero(&journal_path);
        let journal_start = Instant::now();
        let mut journal_entries = 0usize;
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
                    Ok(entry) => {
                        journal_entries += 1;
                        session.apply_journal_entry(entry)
                    }
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
        let journal_ms = journal_start.elapsed().as_millis();
        let finalize_start = Instant::now();
        session.reset_persist_state(path.exists());
        session.reset_provider_messages_cache();
        session.mark_memory_profile_dirty();
        let finalize_ms = finalize_start.elapsed().as_millis();
        crate::logging::info(&format!(
            "[TIMING] session_load: session={}, snapshot={}ms, journal={}ms, finalize={}ms, snapshot_bytes={}, journal_bytes={}, journal_entries={}, messages={}, env_snapshots={}, replay_events={}, total={}ms",
            session.id,
            snapshot_ms,
            journal_ms,
            finalize_ms,
            snapshot_bytes,
            journal_bytes,
            journal_entries,
            session.messages.len(),
            session.env_snapshots.len(),
            session.replay_events.len(),
            load_start.elapsed().as_millis(),
        ));
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
            provider_key: None,
            model: None,
            subagent_model: None,
            improve_mode: None,
            autoreview_enabled: None,
            autojudge_enabled: None,
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
            provider_messages_cache: Vec::new(),
            provider_message_prefix_hashes_cache: Vec::new(),
            provider_messages_cache_len: 0,
            provider_messages_cache_mode: PersistVectorMode::Full,
            memory_profile_cache: SessionMemoryProfileCache::default(),
            memory_profile_dirty: false,
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
            provider_key: None,
            model: None,
            subagent_model: None,
            improve_mode: None,
            autoreview_enabled: None,
            autojudge_enabled: None,
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
            provider_messages_cache: Vec::new(),
            provider_message_prefix_hashes_cache: Vec::new(),
            provider_messages_cache_len: 0,
            provider_messages_cache_mode: PersistVectorMode::Full,
            memory_profile_cache: SessionMemoryProfileCache::default(),
            memory_profile_dirty: false,
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
        self.memory_profile_cache.env_snapshots_count += 1;
        self.memory_profile_cache.env_snapshots_json_bytes += estimate_json_bytes(&snapshot);
        self.env_snapshots.push(snapshot);
        if self.env_snapshots.len() > MAX_ENV_SNAPSHOTS {
            let excess = self.env_snapshots.len() - MAX_ENV_SNAPSHOTS;
            self.env_snapshots.drain(0..excess);
            self.mark_memory_profile_dirty();
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

    /// Load only the metadata needed for remote-client startup.
    ///
    /// This intentionally skips heavyweight transcript vectors so the remote
    /// client can paint quickly while the server performs the authoritative
    /// session restore + history bootstrap.
    pub fn load_startup_stub(session_id: &str) -> Result<Self> {
        let path = session_path(session_id)?;
        let reader = BufReader::new(std::fs::File::open(&path)?);
        let stub: SessionStartupStub = serde_json::from_reader(reader)?;
        Ok(Self::session_from_startup_stub(stub))
    }

    pub fn load_for_remote_startup(session_id: &str) -> Result<Self> {
        let path = session_path(session_id)?;
        let load_start = Instant::now();
        let snapshot_bytes = file_len_or_zero(&path);
        let snapshot_start = Instant::now();
        let reader = BufReader::new(std::fs::File::open(&path)?);
        let snapshot: RemoteStartupSessionSnapshot = serde_json::from_reader(reader)?;
        let snapshot_ms = snapshot_start.elapsed().as_millis();
        let mut session = Self::session_from_remote_startup_snapshot(snapshot);
        let journal_path = session_journal_path_from_snapshot(&path);
        let journal_bytes = file_len_or_zero(&journal_path);
        let journal_start = Instant::now();
        let mut journal_entries = 0usize;
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
                    Ok(entry) => {
                        journal_entries += 1;
                        session.apply_journal_meta(entry.meta);
                        session.messages.extend(entry.append_messages);
                        session.replay_events.extend(entry.append_replay_events);
                    }
                    Err(err) => {
                        crate::logging::warn(&format!(
                            "Remote startup journal parse failed at {} line {}: {}",
                            journal_path.display(),
                            line_idx + 1,
                            err
                        ));
                        break;
                    }
                }
            }
        }
        let journal_ms = journal_start.elapsed().as_millis();
        let finalize_start = Instant::now();
        session.reset_persist_state(path.exists());
        session.reset_provider_messages_cache();
        session.mark_memory_profile_dirty();
        let finalize_ms = finalize_start.elapsed().as_millis();
        crate::logging::info(&format!(
            "[TIMING] remote_startup_load: session={}, snapshot={}ms, journal={}ms, finalize={}ms, snapshot_bytes={}, journal_bytes={}, journal_entries={}, messages={}, total={}ms",
            session.id,
            snapshot_ms,
            journal_ms,
            finalize_ms,
            snapshot_bytes,
            journal_bytes,
            journal_entries,
            session.messages.len(),
            load_start.elapsed().as_millis(),
        ));
        Ok(session)
    }

    pub fn save(&mut self) -> Result<()> {
        self.updated_at = Utc::now();
        let path = session_path(&self.id)?;
        let journal_path = session_journal_path_from_snapshot(&path);
        let start = std::time::Instant::now();
        let snapshot_bytes_before = file_len_or_zero(&path);
        let journal_bytes_before = file_len_or_zero(&journal_path);
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

        let delta_messages = self
            .messages
            .len()
            .saturating_sub(self.persist_state.messages_len);
        let delta_env_snapshots = self
            .env_snapshots
            .len()
            .saturating_sub(self.persist_state.env_snapshots_len);
        let delta_memory_injections = self
            .memory_injections
            .len()
            .saturating_sub(self.persist_state.memory_injections_len);
        let delta_replay_events = self
            .replay_events
            .len()
            .saturating_sub(self.persist_state.replay_events_len);
        let (
            result,
            save_mode,
            entry_build_ms,
            append_ms,
            journal_stat_ms,
            checkpoint_ms,
            journal_bytes_after,
        ) = if metadata_needs_snapshot || vectors_need_snapshot {
            let checkpoint_start = Instant::now();
            let result = self.checkpoint_snapshot(&path, &journal_path);
            let checkpoint_ms = checkpoint_start.elapsed().as_millis();
            let journal_bytes_after = file_len_or_zero(&journal_path);
            (
                result,
                "snapshot",
                0,
                0,
                0,
                checkpoint_ms,
                journal_bytes_after,
            )
        } else {
            let entry_build_start = Instant::now();
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
            let entry_build_ms = entry_build_start.elapsed().as_millis();
            let append_start = Instant::now();
            let append_result = storage::append_json_line_fast(&journal_path, &entry);
            let append_ms = append_start.elapsed().as_millis();
            match append_result {
                Ok(()) => {
                    self.reset_persist_state(true);
                    let journal_stat_start = Instant::now();
                    let journal_bytes_after = file_len_or_zero(&journal_path);
                    let journal_stat_ms = journal_stat_start.elapsed().as_millis();
                    if journal_bytes_after > MAX_SESSION_JOURNAL_BYTES {
                        let checkpoint_start = Instant::now();
                        let result = self.checkpoint_snapshot(&path, &journal_path);
                        let checkpoint_ms = checkpoint_start.elapsed().as_millis();
                        let journal_bytes_after = file_len_or_zero(&journal_path);
                        (
                            result,
                            "append+checkpoint",
                            entry_build_ms,
                            append_ms,
                            journal_stat_ms,
                            checkpoint_ms,
                            journal_bytes_after,
                        )
                    } else {
                        (
                            Ok(()),
                            "append",
                            entry_build_ms,
                            append_ms,
                            journal_stat_ms,
                            0,
                            journal_bytes_after,
                        )
                    }
                }
                Err(err) => {
                    crate::logging::warn(&format!(
                        "Session journal append failed for {} ({}); checkpointing full snapshot",
                        self.id, err
                    ));
                    let checkpoint_start = Instant::now();
                    let result = self.checkpoint_snapshot(&path, &journal_path);
                    let checkpoint_ms = checkpoint_start.elapsed().as_millis();
                    let journal_bytes_after = file_len_or_zero(&journal_path);
                    (
                        result,
                        "append_failed_fallback_snapshot",
                        entry_build_ms,
                        append_ms,
                        0,
                        checkpoint_ms,
                        journal_bytes_after,
                    )
                }
            }
        };
        let elapsed = start.elapsed();
        if elapsed.as_millis() > 50 {
            crate::logging::info(&format!(
                "Session save slow: total={:.0}ms mode={} metadata_snapshot={} vectors_snapshot={} entry_build={}ms append={}ms journal_stat={}ms checkpoint={}ms messages={} delta_messages={} delta_env_snapshots={} delta_memory_injections={} delta_replay_events={} snapshot_bytes_before={} journal_bytes_before={} journal_bytes_after={}",
                elapsed.as_secs_f64() * 1000.0,
                save_mode,
                metadata_needs_snapshot,
                vectors_need_snapshot,
                entry_build_ms,
                append_ms,
                journal_stat_ms,
                checkpoint_ms,
                self.messages.len(),
                delta_messages,
                delta_env_snapshots,
                delta_memory_injections,
                delta_replay_events,
                snapshot_bytes_before,
                journal_bytes_before,
                journal_bytes_after,
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
        self.memory_profile_cache.messages_count += 1;
        self.memory_profile_cache.messages_json_bytes += estimate_json_bytes(&message);
        self.memory_profile_cache
            .message_stats
            .merge_from(&summarize_blocks(&message.content));
        self.messages.push(message);
        self.mark_messages_append_dirty();
    }

    pub fn insert_message(&mut self, index: usize, message: StoredMessage) {
        self.messages.insert(index, message);
        self.mark_memory_profile_dirty();
        self.mark_messages_full_dirty();
    }

    pub fn replace_messages(&mut self, messages: Vec<StoredMessage>) {
        self.messages = messages;
        self.mark_memory_profile_dirty();
        self.mark_messages_full_dirty();
    }

    pub fn truncate_messages(&mut self, len: usize) {
        if len < self.messages.len() {
            self.messages.truncate(len);
            self.mark_memory_profile_dirty();
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
        memory_ids: Vec<String>,
    ) {
        let injection = StoredMemoryInjection {
            summary,
            content,
            count,
            memory_ids,
            age_ms: Some(age_ms),
            before_message: Some(self.messages.len()),
            timestamp: Utc::now(),
        };
        self.memory_profile_cache.memory_injections_count += 1;
        self.memory_profile_cache.memory_injections_json_bytes += estimate_json_bytes(&injection);
        self.memory_injections.push(injection);
        self.mark_memory_injections_append_dirty();
    }

    pub fn injected_memory_ids(&self) -> Vec<String> {
        let mut ids = HashSet::new();
        for injection in &self.memory_injections {
            ids.extend(injection.memory_ids.iter().cloned());
        }
        ids.into_iter().collect()
    }

    pub fn record_replay_display_message(
        &mut self,
        role: impl Into<String>,
        title: Option<String>,
        content: impl Into<String>,
    ) {
        let event = StoredReplayEvent {
            timestamp: Utc::now(),
            kind: StoredReplayEventKind::DisplayMessage {
                role: role.into(),
                title,
                content: content.into(),
            },
        };
        self.memory_profile_cache.replay_events_count += 1;
        self.memory_profile_cache.replay_events_json_bytes += estimate_json_bytes(&event);
        self.replay_events.push(event);
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
        let event = StoredReplayEvent {
            timestamp: Utc::now(),
            kind,
        };
        self.memory_profile_cache.replay_events_count += 1;
        self.memory_profile_cache.replay_events_json_bytes += estimate_json_bytes(&event);
        self.replay_events.push(event);
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
        let event = StoredReplayEvent {
            timestamp: Utc::now(),
            kind,
        };
        self.memory_profile_cache.replay_events_count += 1;
        self.memory_profile_cache.replay_events_json_bytes += estimate_json_bytes(&event);
        self.replay_events.push(event);
        self.mark_replay_events_append_dirty();
    }

    pub fn provider_messages(&mut self) -> &[Message] {
        let needs_full_rebuild = self.provider_messages_cache_mode == PersistVectorMode::Full
            || self.provider_messages_cache_len > self.messages.len();

        if needs_full_rebuild {
            self.provider_messages_cache.clear();
            self.provider_message_prefix_hashes_cache.clear();
            self.provider_messages_cache.reserve(self.messages.len());
            self.provider_message_prefix_hashes_cache
                .reserve(self.messages.len());
            for index in 0..self.messages.len() {
                let message = self.messages[index].to_message();
                self.push_provider_message_cache_entry(message);
            }
            self.provider_messages_cache_len = self.messages.len();
            self.provider_messages_cache_mode = PersistVectorMode::Clean;
            return &self.provider_messages_cache;
        }

        if self.provider_messages_cache_mode == PersistVectorMode::Append
            && self.provider_messages_cache_len < self.messages.len()
        {
            let appended_len = self.messages.len() - self.provider_messages_cache_len;
            self.provider_messages_cache.reserve(appended_len);
            self.provider_message_prefix_hashes_cache
                .reserve(appended_len);
            for index in self.provider_messages_cache_len..self.messages.len() {
                let message = self.messages[index].to_message();
                self.push_provider_message_cache_entry(message);
            }
            self.provider_messages_cache_len = self.messages.len();
            self.provider_messages_cache_mode = PersistVectorMode::Clean;
        }

        &self.provider_messages_cache
    }

    pub fn provider_message_prefix_hashes(&mut self) -> &[u64] {
        let _ = self.provider_messages();
        &self.provider_message_prefix_hashes_cache
    }

    pub fn messages_for_provider_uncached(&self) -> Vec<Message> {
        stored_messages_to_messages(&self.messages)
    }

    pub fn messages_for_provider(&mut self) -> Vec<Message> {
        self.provider_messages().to_vec()
    }

    /// Drop heavyweight transcript vectors after remote startup has rendered the
    /// optimistic local history. The authoritative transcript comes from the
    /// server once the connection is established, so keeping another owned copy
    /// in the client only inflates memory during idle remote sessions.
    pub fn strip_transcript_for_remote_client(&mut self) {
        self.messages.clear();
        self.compaction = None;
        self.env_snapshots.clear();
        self.memory_injections.clear();
        self.replay_events.clear();
        self.rebuild_memory_profile_cache();
        self.reset_provider_messages_cache();
        self.reset_persist_state(true);
    }

    /// Remove all ToolUse content blocks from a specific message.
    /// Used when tool calls are discarded (e.g. due to truncated output / max_tokens).
    pub fn remove_tool_use_blocks(&mut self, message_id: &str) {
        for msg in &mut self.messages {
            if msg.id == *message_id {
                msg.content
                    .retain(|block| !matches!(block, ContentBlock::ToolUse { .. }));
                self.mark_memory_profile_dirty();
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderedMessage {
    pub role: String,
    pub content: String,
    pub tool_calls: Vec<String>,
    pub tool_data: Option<ToolCall>,
}

#[derive(Debug, Deserialize)]
struct RemoteStartupSessionSnapshot {
    id: String,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    #[serde(default)]
    messages: Vec<StoredMessage>,
    #[serde(default)]
    compaction: Option<StoredCompactionState>,
    #[serde(default)]
    provider_session_id: Option<String>,
    #[serde(default)]
    provider_key: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    subagent_model: Option<String>,
    #[serde(default)]
    improve_mode: Option<SessionImproveMode>,
    #[serde(default)]
    autoreview_enabled: Option<bool>,
    #[serde(default)]
    autojudge_enabled: Option<bool>,
    #[serde(default)]
    is_canary: bool,
    #[serde(default)]
    testing_build: Option<String>,
    #[serde(default)]
    working_dir: Option<String>,
    #[serde(default)]
    short_name: Option<String>,
    #[serde(default)]
    status: SessionStatus,
    #[serde(default)]
    last_pid: Option<u32>,
    #[serde(default)]
    last_active_at: Option<DateTime<Utc>>,
    #[serde(default)]
    is_debug: bool,
    #[serde(default)]
    saved: bool,
    #[serde(default)]
    save_label: Option<String>,
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
    render_messages_and_images(session).1
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
    render_messages_and_images(session).0
}

pub fn render_messages_and_images(session: &Session) -> (Vec<RenderedMessage>, Vec<RenderedImage>) {
    let mut rendered: Vec<RenderedMessage> = Vec::new();
    let mut images: Vec<RenderedImage> = Vec::new();
    let mut tool_map: HashMap<String, ToolCall> = HashMap::new();

    for msg in &session.messages {
        let role = match msg.display_role {
            Some(StoredDisplayRole::System) => "system",
            Some(StoredDisplayRole::BackgroundTask) => "background_task",
            None => match msg.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            },
        };
        let message_role = msg.role.clone();
        let mut text = String::new();
        let mut tool_calls: Vec<String> = Vec::new();
        let mut current_tool: Option<ToolCall> = None;
        let mut last_image_idx: Option<usize> = None;

        for block in &msg.content {
            match block {
                ContentBlock::Text { text: t, .. } => {
                    text.push_str(t);
                    if let Some(label) = parse_attached_image_label(t)
                        && let Some(last_idx) = last_image_idx
                        && let Some(image) = images.get_mut(last_idx)
                    {
                        image.label = Some(label);
                    }
                }
                ContentBlock::ToolUse { id, name, input } => {
                    let tool_call = ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                        intent: None,
                    };
                    tool_map.insert(id.clone(), tool_call);
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
                    current_tool = tool_data.clone();

                    rendered.push(RenderedMessage {
                        role: "tool".to_string(),
                        content: content.clone(),
                        tool_calls: Vec::new(),
                        tool_data,
                    });
                }
                ContentBlock::Reasoning { .. } => {}
                ContentBlock::Image { media_type, data } => {
                    images.push(RenderedImage {
                        media_type: media_type.clone(),
                        data: data.clone(),
                        label: current_tool
                            .as_ref()
                            .and_then(fallback_image_label_for_tool),
                        source: image_source_for_message(
                            message_role.clone(),
                            current_tool.as_ref(),
                        ),
                    });
                    last_image_idx = Some(images.len().saturating_sub(1));
                }
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

    (rendered, images)
}

fn session_path_in_dir(base: &std::path::Path, session_id: &str) -> PathBuf {
    base.join("sessions").join(format!("{}.json", session_id))
}

fn estimate_json_bytes<T: Serialize>(value: &T) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(0)
}

fn file_len_or_zero(path: &Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

fn persist_vector_mode_label(mode: PersistVectorMode) -> &'static str {
    match mode {
        PersistVectorMode::Clean => "clean",
        PersistVectorMode::Append => "append",
        PersistVectorMode::Full => "full",
    }
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
#[path = "session_tests/mod.rs"]
mod tests;

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
        if path.extension().map(|e| e == "json").unwrap_or(false)
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            && let Ok(mut session) = Session::load(stem)
        {
            if session.detect_crash() {
                let _ = session.save();
            }
            sessions.push(session);
        }
    }

    // Track existing recovery sessions to avoid duplicates
    let mut recovered_parents: HashSet<String> = HashSet::new();
    for s in &sessions {
        if s.id.starts_with("session_recovery_")
            && let Some(parent) = s.parent_id.as_ref()
        {
            recovered_parents.insert(parent.clone());
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
        new_session.provider_key = old.provider_key.clone();
        new_session.model = old.model.clone();
        new_session.improve_mode = old.improve_mode;
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
        if path.extension().map(|e| e == "json").unwrap_or(false)
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            && let Ok(mut session) = Session::load(stem)
        {
            if session.detect_crash() {
                let _ = session.save();
            }
            sessions.push(session);
        }
    }

    // Track existing recovery sessions to avoid showing already-recovered crashes
    let mut recovered_parents: HashSet<String> = HashSet::new();
    for s in &sessions {
        if s.id.starts_with("session_recovery_")
            && let Some(parent) = s.parent_id.as_ref()
        {
            recovered_parents.insert(parent.clone());
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
    #[serde(rename = "created_at")]
    _created_at: DateTime<Utc>,
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
    let cutoff = Utc::now() - Duration::hours(24);
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
                let ts = session.last_active_at.unwrap_or(session.updated_at);
                if ts <= cutoff {
                    continue;
                }
                let name = extract_session_name(&session_id)
                    .unwrap_or(&session_id)
                    .to_string();
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
        if let Some(fname) = entry.file_name().to_str()
            && let Some(ts) = extract_timestamp_from_filename(fname)
            && ts < filename_cutoff_ms
        {
            continue;
        }

        let path = entry.path();
        if !path.extension().map(|e| e == "json").unwrap_or(false) {
            continue;
        }

        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if let Ok(mtime) = meta.modified()
            && mtime < cutoff_system
        {
            continue;
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
            if header.id.starts_with("session_recovery_")
                && let Some(parent) = header.parent_id.as_ref()
            {
                recovered_parents.insert(parent.clone());
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

/// Find a session by ID or memorable name
/// If the input doesn't look like a full session ID (doesn't contain underscore followed by digits),
/// try to find a session whose short name matches.
/// Returns the full session ID if found.
pub fn find_session_by_name_or_id(name_or_id: &str) -> Result<String> {
    // Try loading directly first so stable imported IDs like `imported_codex_*`
    // or other explicit session ids can be resumed without going through the
    // short-name matcher.
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

    // Otherwise, search for a session with matching short name
    let sessions_dir = storage::jcode_dir()?.join("sessions");
    if !sessions_dir.exists() {
        anyhow::bail!("No sessions found");
    }

    let mut matches: Vec<(String, chrono::DateTime<chrono::Utc>)> = Vec::new();

    for entry in std::fs::read_dir(&sessions_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "json").unwrap_or(false)
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            && let Some(short_name) = extract_session_name(stem)
            && short_name == name_or_id
            && let Ok(session) = Session::load(stem)
        {
            matches.push((stem.to_string(), session.updated_at));
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

    #[test]
    fn find_session_by_name_or_id_accepts_imported_session_ids() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("temp dir");
        crate::env::set_var("JCODE_HOME", temp.path());

        let imported_id = "imported_codex_test_resume";
        let mut session =
            Session::create_with_id(imported_id.to_string(), None, Some("Imported".to_string()));
        session.status = SessionStatus::Closed;
        session.save().expect("save imported session");

        let resolved = find_session_by_name_or_id(imported_id).expect("resolve imported id");
        assert_eq!(resolved, imported_id);

        crate::env::remove_var("JCODE_HOME");
    }
}
