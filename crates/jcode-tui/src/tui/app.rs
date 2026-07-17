use super::DisplayMessageRoleExt;
use super::keybind::{
    CenteredToggleKeys, ModelSwitchKeys, OptionalBinding, ScrollKeys, WorkspaceNavigationKeys,
};
use super::markdown::IncrementalMarkdownRenderer;
use super::stream_buffer::StreamBuffer;
use crate::bus::{Bus, BusEvent, LoginCompleted, ToolEvent, ToolStatus};
use crate::compaction::CompactionEvent;
use crate::config::config;
use crate::id;
use crate::mcp::McpManager;
use crate::message::{
    ContentBlock, Message, Role, StreamEvent, TOOL_OUTPUT_MISSING_TEXT, ToolCall, ToolDefinition,
};
use crate::provider::Provider;
use crate::runtime_memory_log::RuntimeMemoryLogController;
use crate::session::{Session, StoredMessage};
use crate::skill::SkillRegistry;
use crate::tool::selfdev::ReloadContext;
use crate::tool::{Registry, ToolContext};
use anyhow::Result;
use auth::PendingLogin;
use crossterm::event::{
    Event, EventStream, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use debug::DebugTrace;
use futures::StreamExt;
use helpers::*;
use jcode_tui_messages::DisplayMessage;
use ratatui::DefaultTerminal;
use std::cell::RefCell;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AppRuntimeMode {
    /// Normal product TUI. The client renders state owned by the jcode server.
    RemoteClient,
    /// Deterministic playback of recorded session/server events. Never calls live providers.
    Replay,
    /// Local in-process harness used by unit tests and transitional UI fixtures only.
    TestHarness,
}

mod auth;
mod auth_account_picker_saved_accounts;
mod catchup;
mod commands;
mod commands_improve;
mod commands_overnight;
mod commands_plan;
mod commands_review;
mod conversation_state;
mod copy_selection;
mod debug;
mod dictation;
mod event_wrappers;
mod handterm_native_scroll;
pub(crate) mod helpers;
mod hotkey_feedback;
mod idle_heap_release;
mod inline_interactive;
mod input;
mod input_help;
mod local;
mod misc_ui;
mod model_context;
mod navigation;
mod observe;
pub(crate) mod onboarding_flow;
mod onboarding_flow_control;
mod onboarding_repair;
mod onboarding_sim;
mod productivity;
mod remote;
mod remote_notifications;
mod replay;
pub(crate) mod run_shell;
mod runtime_memory;
mod shortcut_hints;
mod split_view;
mod sponsor_disclosure;
mod state_ui;
mod state_ui_input_helpers;
mod state_ui_maintenance;
mod state_ui_messages;
mod state_ui_runtime;
mod state_ui_storage;
mod support;
mod swarm_hint;
mod todos_view;
mod tui_lifecycle;
mod tui_lifecycle_runtime;
mod tui_state;
mod turn;
mod turn_memory;
mod turn_notify;
mod ui_prefs;

pub(crate) use self::state_ui_storage::compact_display_messages_for_storage;

pub(crate) fn extract_input_shell_command(input: &str) -> Option<&str> {
    self::input::extract_input_shell_command(input)
}

pub(crate) const COMMAND_SUGGESTION_VISIBLE_LIMIT: usize = 8;

fn active_runtime_provider_key() -> Option<String> {
    std::env::var("JCODE_RUNTIME_PROVIDER")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
}

#[derive(Debug, Clone)]
struct PendingRemoteMessage {
    content: String,
    images: Vec<(String, String)>,
    is_system: bool,
    system_reminder: Option<String>,
    auto_retry: bool,
    retry_attempts: u8,
    retry_at: Option<Instant>,
}

#[derive(Debug, Clone)]
struct PendingSplitPrompt {
    content: String,
    images: Vec<(String, String)>,
}

struct PendingLocalTransfer {
    receiver: mpsc::Receiver<anyhow::Result<PreparedTransferSession>>,
}

/// A reasoning trace anchored in the transcript during the current turn
/// (`current` display mode). `wrapped_lines_at_anchor` snapshots the
/// transcript's total wrapped-line count when the trace anchored; once the
/// transcript grows a viewport past that point the trace is provably above
/// the tail-following viewport and can be removed with zero visible motion.
#[derive(Debug, Clone, Copy)]
struct TurnReasoningTrace {
    display_index: usize,
    wrapped_lines_at_anchor: usize,
}

#[derive(Debug, Clone)]
struct LocalRewindUndoSnapshot {
    messages: Vec<StoredMessage>,
    provider_session_id: Option<String>,
    session_provider_session_id: Option<String>,
    visible_message_count: usize,
}

#[derive(Debug, Clone)]
struct PendingRemoteRewindNotice {
    undo: bool,
    message_index: Option<usize>,
    changed_messages: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tui::app) struct KvCacheRequestSignature {
    pub(in crate::tui::app) system_static_hash: u64,
    pub(in crate::tui::app) tools_hash: u64,
    pub(in crate::tui::app) messages_hash: u64,
    pub(in crate::tui::app) message_hashes: Vec<u64>,
    pub(in crate::tui::app) message_count: usize,
    pub(in crate::tui::app) tool_count: usize,
    pub(in crate::tui::app) system_static_chars: usize,
    pub(in crate::tui::app) tools_json_chars: usize,
    pub(in crate::tui::app) messages_json_chars: usize,
    pub(in crate::tui::app) ephemeral_hash: Option<u64>,
    pub(in crate::tui::app) ephemeral_chars: usize,
    pub(in crate::tui::app) ephemeral_message_count: usize,
}

#[derive(Debug, Clone)]
struct KvCacheBaseline {
    /// Session this baseline was captured for. Baselines must only be diffed
    /// against requests from the same session; otherwise a session switch makes
    /// the new (often smaller) history look like a broken prefix and produces a
    /// spurious `harness:_prefix_changed` miss.
    session_id: Option<String>,
    /// Effective prompt size of the last completed request. This includes input,
    /// cache-read, and cache-creation tokens for split-accounting providers like
    /// Anthropic. It is the reusable cached prefix, meaning what gets resent if
    /// the cache goes cold, not the bare `input` field, which for split providers
    /// is only the uncached remainder of that one request.
    input_tokens: u64,
    completed_at: Instant,
    provider: String,
    model: String,
    upstream_provider: Option<String>,
    signature: Option<KvCacheRequestSignature>,
}

#[derive(Debug, Clone)]
struct PendingKvCacheRequest {
    turn_number: usize,
    call_index: u16,
    provider: String,
    model: String,
    upstream_provider: Option<String>,
    signature: Option<KvCacheRequestSignature>,
    baseline_messages_prefix_matches: Option<bool>,
    baseline: Option<KvCacheBaseline>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KvCacheMissReason {
    ProviderSwitch,
    ModelSwitch,
    UpstreamSwitch,
    Expired,
    HarnessSystemChanged,
    HarnessToolsChanged,
    HarnessPrefixChanged,
    ZeroRead,
    LowRead,
    Unknown,
}

impl KvCacheMissReason {
    fn label(self) -> &'static str {
        match self {
            Self::ProviderSwitch => "provider switch",
            Self::ModelSwitch => "model switch",
            Self::UpstreamSwitch => "upstream switch",
            Self::Expired => "expired",
            Self::HarnessSystemChanged => "harness: system changed",
            Self::HarnessToolsChanged => "harness: tools changed",
            Self::HarnessPrefixChanged => "harness: prefix changed",
            Self::ZeroRead => "zero read",
            Self::LowRead => "low read",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone)]
struct KvCacheMissSample {
    turn_number: usize,
    call_index: u16,
    missed_tokens: u64,
    reason: KvCacheMissReason,
}

struct PendingSessionPickerLoad {
    receiver: mpsc::Receiver<
        anyhow::Result<(
            Vec<super::session_picker::ServerGroup>,
            Vec<super::session_picker::SessionInfo>,
        )>,
    >,
}

struct PendingModelPickerLoad {
    request_id: u64,
    signature: ModelPickerCacheSignature,
    picker_started: Instant,
    receiver: mpsc::Receiver<anyhow::Result<ModelPickerRoutesResult>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelPickerCacheSignature {
    is_remote: bool,
    provider_name: String,
    current_model: String,
    config_default_model: Option<String>,
    config_default_provider: Option<String>,
    reasoning_effort: Option<String>,
    available_efforts: Vec<String>,
    simplified_model_picker: bool,
    catalog_revision: u64,
    remote_provider_name: Option<String>,
    remote_available_len: usize,
    remote_available_first: Option<String>,
    remote_available_last: Option<String>,
    remote_routes_len: usize,
    remote_routes_first: Option<String>,
    remote_routes_last: Option<String>,
}

#[derive(Debug, Clone)]
struct ModelPickerCache {
    signature: ModelPickerCacheSignature,
    entries: Vec<super::PickerEntry>,
    route_count: usize,
    model_count: usize,
}

struct ModelPickerRoutesResult {
    routes: Vec<crate::provider::ModelRoute>,
    routes_ms: u128,
}

#[derive(Debug, Clone)]
struct PreparedTransferSession {
    session_id: String,
    session_name: String,
}

#[derive(Debug, Clone)]
struct PendingProviderFailover {
    prompt: crate::provider::ProviderFailoverPrompt,
    deadline: Instant,
}

/// An interactive "switch to the next best model/method and resend" offer shown
/// after a provider turn error (auth failure, broken API key, rate limit, etc.).
///
/// Unlike [`PendingProviderFailover`], this is a manual, keypress-activated
/// affordance and can switch between *auth methods on the same provider* (e.g.
/// fall back from a broken `claude-api` key to a working `claude-oauth` login),
/// which is exactly the case automatic cross-provider failover cannot handle.
#[derive(Debug, Clone)]
struct PendingFallbackOffer {
    /// The route selection to apply when the user accepts the offer.
    selection: crate::provider::RouteSelection,
    /// Short human label for the target (e.g. "claude-sonnet-4 via OAuth").
    target_label: String,
    /// Short label for what just failed (e.g. "Claude via API key").
    from_label: String,
    /// Remote sessions only: the failed turn's payload, captured before error
    /// cleanup clears it, so accepting the offer can resend it on the new
    /// route. Local sessions resend via `pending_turn` instead.
    remote_resend: Option<FallbackResendPayload>,
}

/// The failed remote turn's payload, held by a [`PendingFallbackOffer`] so a
/// one-keypress accept can resend it after the route switch completes.
#[derive(Debug, Clone)]
struct FallbackResendPayload {
    /// Expanded message content that was sent to the server.
    content: String,
    /// Inline image attachments that accompanied the message.
    images: Vec<(String, String)>,
    /// Whether the failed send was a system continuation (poke/reminder).
    is_system: bool,
    /// Whether the failed send was flagged for automatic retries.
    auto_retry: bool,
    /// Hidden system reminder that accompanied the message, if any.
    system_reminder: Option<String>,
    /// The raw prompt the user typed, when known. Used to de-duplicate the
    /// input box (the error path restores the prompt there) on accept.
    raw_input: Option<String>,
}

/// An interactive "let a jcode agent merge the diverged update for you" offer.
///
/// Surfaced when an update fails because the local checkout and upstream have
/// diverged (a fast-forward pull is impossible). Accepting it spawns a fresh
/// jcode session, pre-loaded with a prompt to reconcile the branches, instead of
/// silently giving up and continuing on the old version.
#[derive(Debug, Clone)]
struct PendingMergeOffer {
    /// Repository whose local/upstream branches diverged, if known. Used as the
    /// spawned agent's working directory and named in its prompt.
    repo_dir: Option<std::path::PathBuf>,
    /// The raw update-failure detail, shown to the user and the merge agent.
    detail: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SessionPickerMode {
    #[default]
    Resume,
    CatchUp,
    /// Opt-in active sessions manager: the picker scoped to live (open)
    /// sessions, showing which are still working vs ready for input.
    ActiveSessions,
    /// First-run onboarding action picker.
    Onboarding,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PendingCatchupResume {
    pub target_session_id: String,
    pub source_session_id: Option<String>,
    pub queue_position: Option<(usize, usize)>,
    pub show_brief: bool,
}

#[derive(Clone, Debug)]
pub(super) struct RemoteResumeActivity {
    pub session_id: String,
    pub observed_at: Instant,
    pub current_tool_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PendingReloadReconnectStatus {
    AwaitingHistory { session_id: Option<String> },
}

const MEMORY_INJECTION_SUPPRESSION_SECS: u64 = 90;

/// Current processing status
#[derive(Clone, Default, Debug)]
pub enum ProcessingStatus {
    #[default]
    Idle,
    /// Sending request to API (with optional connection phase detail)
    Sending,
    /// Connection phase update from transport layer
    Connecting(crate::message::ConnectionPhase),
    /// Model is reasoning/thinking (real-time duration tracking)
    Thinking(Instant),
    /// Receiving streaming response
    Streaming,
    /// Waiting for network connectivity before retrying an interrupted request
    WaitingForNetwork { listener: String },
    /// Executing a tool
    RunningTool(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RemoteStartupPhase {
    StartingServer,
    Connecting,
    LoadingSession,
    WaitingForReload,
    Reconnecting { attempt: u32 },
}

impl RemoteStartupPhase {
    pub(crate) fn header_label(&self) -> String {
        match self {
            Self::StartingServer => "starting server…".to_string(),
            Self::Connecting => "connecting to server…".to_string(),
            Self::LoadingSession => "loading session…".to_string(),
            Self::WaitingForReload => "waiting for reload…".to_string(),
            Self::Reconnecting { attempt } => format!("reconnecting ({attempt})…"),
        }
    }

    pub(crate) fn header_label_with_elapsed(&self, elapsed: Duration) -> String {
        let base = self.header_label();
        if elapsed < Duration::from_secs(1) {
            return base;
        }

        let elapsed_str = if elapsed.as_secs() < 60 {
            format!("{}s", elapsed.as_secs())
        } else {
            format!("{}m {}s", elapsed.as_secs() / 60, elapsed.as_secs() % 60)
        };

        format!("{base} {elapsed_str}")
    }
}

pub(super) fn reload_persisted_background_tasks_note(session_id: &str) -> String {
    crate::tool::selfdev::persisted_background_tasks_note(session_id)
}

#[derive(Clone, Default)]
pub struct CopyBadgeUiState {
    pub alt_active: bool,
    pub shift_active: bool,
    pub alt_pulse_until: Option<Instant>,
    pub shift_pulse_until: Option<Instant>,
    pub key_active: Option<(char, Instant)>,
    pub copied_feedback: Option<CopyBadgeFeedback>,
    pub expand_feedback_until: Option<Instant>,
    pub expand_feedback_line: Option<usize>,
}

#[derive(Clone)]
pub struct CopyBadgeFeedback {
    pub key: char,
    pub success: bool,
    pub expires_at: Instant,
}

impl CopyBadgeUiState {
    fn pulse_active(expires_at: Option<Instant>, now: Instant) -> bool {
        expires_at.is_some_and(|expires_at| expires_at > now)
    }

    pub(crate) fn alt_is_active(&self, now: Instant) -> bool {
        self.alt_active || Self::pulse_active(self.alt_pulse_until, now)
    }

    pub(crate) fn shift_is_active(&self, now: Instant) -> bool {
        self.alt_is_active(now)
            && (self.shift_active || Self::pulse_active(self.shift_pulse_until, now))
    }

    pub(crate) fn key_is_active(&self, key: char, now: Instant) -> bool {
        self.shift_is_active(now)
            && self
                .key_active
                .as_ref()
                .map(|(active_key, expires_at)| {
                    active_key.eq_ignore_ascii_case(&key) && *expires_at > now
                })
                .unwrap_or(false)
    }

    pub(crate) fn feedback_for_key(&self, key: char, now: Instant) -> Option<bool> {
        self.copied_feedback.as_ref().and_then(|feedback| {
            if feedback.key.eq_ignore_ascii_case(&key) && feedback.expires_at > now {
                Some(feedback.success)
            } else {
                None
            }
        })
    }

    pub(crate) fn expand_feedback_is_active(&self, now: Instant) -> bool {
        self.expand_feedback_until
            .is_some_and(|expires_at| expires_at > now)
    }
}

/// Result from running the TUI
#[derive(Debug, Default)]
pub struct RunResult {
    /// Session ID to reload (hot-reload, no rebuild)
    pub reload_session: Option<String>,
    /// Session ID to rebuild (full git pull + cargo build + tests)
    pub rebuild_session: Option<String>,
    /// Session ID to update (download from GitHub releases and reload)
    pub update_session: Option<String>,
    /// Session ID to restart (exec into current binary, no build)
    pub restart_session: Option<String>,
    /// Exit code to use (for canary wrapper communication)
    pub exit_code: Option<i32>,
    /// The session ID that was active (for resume hints on exit)
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SendAction {
    Submit,
    Queue,
    Interleave,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ImproveMode {
    ImproveRun,
    ImprovePlan,
    RefactorRun,
    RefactorPlan,
}

impl ImproveMode {
    pub(super) fn status_label(self) -> &'static str {
        match self {
            Self::ImproveRun => "active improvement loop",
            Self::ImprovePlan => "improvement plan-only",
            Self::RefactorRun => "active refactor loop",
            Self::RefactorPlan => "refactor plan-only",
        }
    }

    pub(super) fn is_improve(self) -> bool {
        matches!(self, Self::ImproveRun | Self::ImprovePlan)
    }

    pub(super) fn is_refactor(self) -> bool {
        matches!(self, Self::RefactorRun | Self::RefactorPlan)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MouseScrollTarget {
    Chat,
    SidePane,
    HelpOverlay,
    ChangelogOverlay,
    ModelStatusOverlay,
    /// The right-hand preview pane of the /resume session picker overlay.
    SessionPickerPreview,
}

#[derive(Debug, Clone, Default)]
pub(super) struct CompactedHistoryLazyState {
    pub total_messages: usize,
    pub visible_messages: usize,
    pub remaining_messages: usize,
    /// Number of user prompts hidden before the first visible message. Used to
    /// keep prompt numbers absolute when older history is truncated.
    pub hidden_user_prompts: usize,
    pub pending_request_visible: Option<usize>,
}

/// Pending viewport anchor used to keep the chat stable when older compacted
/// history is loaded in. Older messages are prepended above the current view,
/// which would otherwise teleport the reader to the new absolute top. We instead
/// remember the reader's distance from the bottom (which is invariant under a
/// top-side prepend) and let the next render resolve it into an absolute offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct HistoryScrollAnchor {
    /// Wrapped lines between the top of the viewport and the bottom of the
    /// transcript at the moment the load was requested. Invariant across the
    /// prepend, so `new_total - lines_from_bottom` reproduces the same view.
    pub lines_from_bottom: usize,
    /// Total wrapped line count of the frame this anchor was captured from. Used
    /// to detect when a frame with the newly-loaded content has rendered (its
    /// total differs), so the anchor can be reconciled into `scroll_offset`.
    pub base_total: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OvernightAutoPokeFingerprint {
    pub run_id: String,
    pub status: String,
    pub last_activity_at: String,
    pub events_len: usize,
    pub task_total: usize,
    pub task_completed: usize,
    pub task_active: usize,
    pub task_blocked: usize,
    pub task_validated: usize,
    pub session_message_count: usize,
    pub review_notes_mtime: Option<u64>,
    pub validation_files: usize,
}

#[derive(Debug, Clone)]
pub(super) struct OvernightAutoPokeState {
    pub run_id: String,
    pub last_fingerprint: OvernightAutoPokeFingerprint,
    pub stalled_turns: u8,
    pub error_turns: u8,
    pub total_pokes_sent: u16,
    pub diagnostic_sent: bool,
    pub morning_report_poked: bool,
    pub final_wrap_poked: bool,
}

#[derive(Clone, Debug, Default)]
struct CommandCandidatesCache {
    candidates: Vec<(String, &'static str)>,
}

/// Session-wide token and cache accounting accumulated across all turns.
///
/// Grouped out of [`App`] to keep the cohesive token/cache totals together. The
/// `total_*` fields accumulate over the whole session; the `last_*` fields hold
/// the most recently reported per-turn values used for cache TTL display.
#[derive(Clone, Debug, Default)]
struct TokenAccounting {
    // Total session token usage (accumulated across all turns)
    total_input_tokens: u64,
    total_output_tokens: u64,
    // Total session KV cache usage for turns where the provider reported cache telemetry.
    total_cache_reported_input_tokens: u64,
    total_cache_read_tokens: u64,
    total_cache_creation_tokens: u64,
    total_cache_optimal_input_tokens: u64,
    last_cache_reported_input_tokens: Option<u64>,
    last_cache_read_tokens: Option<u64>,
    last_cache_creation_tokens: Option<u64>,
    last_cache_optimal_input_tokens: Option<u64>,
    cache_next_optimal_input_tokens: Option<u64>,
}

/// KV cache baseline tracking and per-turn cache-miss attribution.
///
/// Grouped out of [`App`]. The baseline and pending-request fields drive cache
/// telemetry recording; the turn/call indices and miss samples feed the cache
/// hit/miss attribution surfaced in the info widget.
#[derive(Clone, Debug, Default)]
struct KvCacheState {
    kv_cache_baseline: Option<KvCacheBaseline>,
    pending_kv_cache_request: Option<PendingKvCacheRequest>,
    current_api_usage_recorded: bool,
    kv_cache_turn_number: Option<usize>,
    kv_cache_turn_call_index: u16,
    kv_cache_miss_samples: Vec<KvCacheMissSample>,
    /// Baseline completion time the cold-cache warning was last pushed for.
    ///
    /// The warning fires at most once per cache write: the idle tick warns as
    /// soon as the TTL expires, and the request-start fallback is suppressed
    /// for the same cold period. A newly completed call refreshes the
    /// baseline's `completed_at`, which re-arms the warning automatically.
    cold_cache_warned_baseline_completed_at: Option<Instant>,
}

/// Where a cold-cache warning is being surfaced from, so the copy can say
/// "will be resent with your next message" while idle vs "may be resent on
/// this request" when the request is already being built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ColdCacheWarningTrigger {
    /// The prompt-cache TTL expired while the session sat idle.
    IdleExpiry,
    /// A request is starting against an already-expired cache (fallback for
    /// when the idle tick never got a chance to warn, e.g. suspended TUI).
    RequestStart,
}

/// Live streaming/turn progress: streamed text, per-turn token counts, and the
/// tokens-per-second tracking state.
///
/// Grouped out of [`App`]. These fields are reset/updated as a unit each turn,
/// so keeping them together clarifies the streaming lifecycle.
#[derive(Clone, Debug, Default)]
struct StreamingProgress {
    streaming_text: String,
    // Live token usage (per turn)
    streaming_input_tokens: u64,
    streaming_output_tokens: u64,
    streaming_cache_read_tokens: Option<u64>,
    streaming_cache_creation_tokens: Option<u64>,
    /// Set at the start of each API call; cleared when the call's first usage
    /// report (with input tokens) arrives. The first report of a call replaces
    /// the cache counters wholesale (even with `None`) instead of merging, so
    /// stale cache-read/creation numbers from a previous call can never leak
    /// into the context-size display for a call that reported no cache usage
    /// (issue #441).
    streaming_usage_call_reset_pending: bool,
    /// True when the last provider-reported usage no longer describes the
    /// active message list (set when a compaction event is applied). While
    /// stale, `current_stream_context_tokens()` returns `None` so the context
    /// display falls back to the local estimate. Cleared by the next usage
    /// report (issue #441).
    streaming_context_stale: bool,
    // Accurate TPS tracking: counts model output generation time, not tool execution.
    /// Set while the provider is generating output tokens (text, reasoning, or tool-call JSON).
    streaming_tps_start: Option<Instant>,
    /// Accumulated model-output generation time across agentic loop iterations.
    streaming_tps_elapsed: Duration,
    /// Whether incoming provider output-token deltas should contribute to TPS.
    ///
    /// This is enabled while an API call has generated model output, and can stay enabled
    /// briefly after generation ends so late final usage snapshots still count.
    streaming_tps_collect_output: bool,
    /// Accumulated output tokens across all API calls in a turn.
    ///
    /// Providers may emit repeated cumulative usage snapshots for a single API call,
    /// so we accumulate per-call deltas to avoid double counting.
    streaming_total_output_tokens: u64,
    /// Latest provider output-token snapshot used for TPS display.
    ///
    /// We update this only when newly generated output tokens are observed. That keeps the
    /// displayed TPS anchored to the latest real token sample instead of decaying on every
    /// redraw while no new usage data has arrived.
    streaming_tps_observed_output_tokens: u64,
    /// Streaming-only elapsed time corresponding to streaming_tps_observed_output_tokens.
    streaming_tps_observed_elapsed: Duration,
}

/// Accumulated session cost and cached per-model pricing.
///
/// Grouped out of [`App`]. `total_cost` accrues across the session; the cached
/// price fields memoize the active model's pricing so they are re-resolved only
/// when `cached_price_model` no longer matches the current model.
#[derive(Clone, Debug, Default)]
struct CostState {
    // Total cost in USD (for API-key providers)
    total_cost: f32,
    // Cached pricing (input $/1M tokens, output $/1M tokens)
    cached_prompt_price: Option<f32>,
    cached_completion_price: Option<f32>,
    // Cached cache-read pricing ($/1M tokens), when known for the active model.
    cached_cache_read_price: Option<f32>,
    // Model the cached_*_price values were resolved for, so we re-resolve on switch.
    cached_price_model: Option<String>,
}

/// State for an in-progress OAuth/API-key login flow triggered by `/login`.
/// TUI Application state
pub struct App {
    provider: Arc<dyn Provider>,
    registry: Registry,
    skills: Arc<SkillRegistry>,
    mcp_manager: Arc<RwLock<McpManager>>,
    messages: Vec<Message>,
    session: Session,
    display_messages: Vec<DisplayMessage>,
    display_messages_version: u64,
    display_user_message_count: usize,
    display_edit_tool_message_count: usize,
    compacted_history_lazy: CompactedHistoryLazyState,
    /// When older compacted history has just been loaded, this anchors the
    /// viewport to the content the reader was looking at so the prepend does not
    /// visibly jump. Resolved into `scroll_offset` by the next render frame.
    pending_history_anchor: Option<HistoryScrollAnchor>,
    input: String,
    command_candidates_cache: RefCell<Option<CommandCandidatesCache>>,
    cursor_pos: usize,
    scroll_offset: usize,
    /// Pauses auto-scroll when user scrolls up during streaming
    auto_scroll_paused: bool,
    active_skill: Option<String>,
    is_processing: bool,
    // Live streaming/turn progress (text, per-turn tokens, TPS tracking).
    streaming: StreamingProgress,
    /// Keeps the machine awake while a turn is processing/streaming.
    power_inhibitor: crate::power_inhibit::PowerInhibitor,
    should_quit: bool,
    // Message queueing
    queued_messages: Vec<String>,
    hidden_queued_system_messages: Vec<String>,
    current_turn_system_reminder: Option<String>,
    // Upstream provider (e.g., which provider OpenRouter routed to)
    upstream_provider: Option<String>,
    // Active stream connection type (websocket/https/etc.)
    connection_type: Option<String>,
    // Provider-supplied human-readable transport detail for the current stream
    status_detail: Option<String>,
    // Session-wide token + cache accounting (accumulated across all turns).
    token_accounting: TokenAccounting,
    // KV cache baseline tracking + per-turn miss attribution.
    kv_cache: KvCacheState,
    // Accumulated session cost + cached per-model pricing.
    cost: CostState,
    // Context limit tracking (for compaction warning)
    context_limit: u64,
    context_warning_shown: bool,
    // Context info (what's loaded in system prompt)
    context_info: crate::prompt::ContextInfo,
    // Monotonic revision for prompt/context-affecting state. Info widgets use this to avoid stale
    // cached context after compaction, prompt rebuilds, tool-definition refreshes, or message edits.
    context_revision: u64,
    // Track last streaming activity for "stale" detection
    last_stream_activity: Option<Instant>,
    // Provider has emitted MessageEnd, but the turn is still finalizing bookkeeping.
    stream_message_ended: bool,
    // Server-reported processing snapshot captured from resume/history before live events arrive.
    remote_resume_activity: Option<RemoteResumeActivity>,
    // Reload reconnect is waiting for server history before deciding whether to continue.
    pending_reload_reconnect_status: Option<PendingReloadReconnectStatus>,
    // Current status
    status: ProcessingStatus,
    // Subagent status (shown during Task tool execution)
    subagent_status: Option<String>,
    // Batch progress (shown during batch tool execution)
    batch_progress: Option<crate::bus::BatchProgress>,
    processing_started: Option<Instant>,
    // User-visible turn timer. Preserved across synthetic auto-poke follow-ups so elapsed time
    // reflects the original user turn rather than only the latest poke resend.
    visible_turn_started: Option<Instant>,
    // When the last API response completed (for cache TTL tracking)
    last_api_completed: Option<Instant>,
    // Provider/model that produced the last completed API response. A warm cache is only
    // meaningful for the same provider and model; switching either should make cache state cold.
    last_api_completed_provider: Option<String>,
    last_api_completed_model: Option<String>,
    // Input tokens from the last completed turn (for cache TTL display)
    last_turn_input_tokens: Option<u64>,
    // Pending turn to process (allows UI to redraw before processing starts)
    pending_turn: bool,
    // When armed by /poke, automatically continue prompting until todos are complete.
    auto_poke_incomplete_todos: bool,
    /// Whether the current auto-poke cycle has already challenged an abrupt
    /// final confidence increase. Low or missing completion confidence keeps
    /// retrying, but a spike gets one dedicated independent-validation turn.
    todo_confidence_spike_challenged: bool,
    /// How many completion-confidence gate nudges the current auto-poke cycle
    /// has sent. Without a budget, a model that stops updating its todos gets
    /// nudged on every turn forever, silently burning an API call per tick.
    todo_completion_gate_attempts: u8,
    // When armed by /overnight, automatically continue guarded follow-up turns until wake/wrap.
    overnight_auto_poke: Option<OvernightAutoPokeState>,
    // Pending cross-provider resend after a failover warning/countdown.
    pending_provider_failover: Option<PendingProviderFailover>,
    // Interactive "switch to next best model/method and resend" offer surfaced
    // after a provider turn error; accepted with a keypress.
    pending_fallback_offer: Option<PendingFallbackOffer>,
    // Remote sessions: the failed turn payload staged by an accepted fallback
    // offer, dispatched once the server confirms the route switch.
    pending_fallback_resend: Option<FallbackResendPayload>,
    // Interactive "spawn a jcode agent to merge the diverged update" offer shown
    // after an update fails because the local checkout and upstream diverged.
    // Accepted with the same key as the fallback offer.
    pending_merge_offer: Option<PendingMergeOffer>,
    // Local session file write to flush once the first "sending" frame is visible.
    session_save_pending: bool,
    // Tool calls detected during streaming (shown in real-time with details)
    streaming_tool_calls: Vec<ToolCall>,
    // Assistant transcript messages committed during the current provider
    // attempt (at ToolStart boundaries). A RetryRollback removes exactly this
    // many trailing assistant messages; reset whenever a new API attempt's
    // output starts cleanly or the turn ends.
    attempt_committed_assistant_messages: usize,
    // Provider-specific session ID for conversation resume
    provider_session_id: Option<String>,
    // One-step undo snapshot captured before the most recent local rewind.
    rewind_undo_snapshot: Option<LocalRewindUndoSnapshot>,
    // Cancel flag for interrupting generation
    cancel_requested: bool,
    // Quit confirmation: tracks when first Ctrl+C was pressed
    quit_pending: Option<Instant>,
    // Debounce redraw storms while the terminal is being resized.
    last_resize_redraw: Option<Instant>,
    // A throttled resize still needs one trailing geometry reset and repaint at
    // the final terminal dimensions.
    resize_redraw_pending: bool,
    // Cached MCP server names and tool counts (updated on connect/disconnect)
    mcp_server_names: Vec<(String, usize)>,
    // When the current connection phase (authenticating/connecting/waiting) began.
    // Reset on every phase change so the "suspiciously long" yellow status is
    // measured per-attempt instead of inheriting the whole-turn elapsed time
    // (which would immediately render yellow on later round-trips of a turn).
    connection_phase_started: Option<Instant>,
    // Semantic stream buffer for chunked output
    stream_buffer: StreamBuffer,
    // Track thinking start time for extended thinking display
    thinking_start: Option<Instant>,
    // Whether we've inserted the current turn's thought line
    thought_line_inserted: bool,
    // Buffer for accumulating thinking content during a thinking session
    thinking_buffer: String,
    // Whether the legacy single-line thought prefix was emitted this session
    thinking_prefix_emitted: bool,
    // Whether we are currently streaming reasoning (dim+italic) text
    reasoning_streaming: bool,
    // Incomplete trailing reasoning line awaiting a newline. Rendered live as the
    // streaming buffer's tail (dim+italic) so reasoning trickles in token-by-token;
    // promoted to a committed line once its newline arrives.
    reasoning_pending_line: String,
    // Byte length of the live partial-reasoning markup currently appended to
    // `streaming_text` (the rendered tail of `reasoning_pending_line`). Truncated
    // and re-appended on each delta so the in-progress line updates in place.
    reasoning_partial_len: usize,
    // Byte offset in `streaming_text` where the current reasoning block began
    // (recorded by `open_reasoning_region`). Used in `current` mode to slice the
    // closed reasoning block back out of the stream in place, keeping any answer
    // text that preceded it in order.
    reasoning_block_start: Option<usize>,
    // Reasoning traces anchored during the current turn (`current`
    // reasoning-display mode). Each entry tracks the display index plus the
    // transcript's wrapped-line total when it anchored, so stale traces can be
    // garbage-collected once they are provably scrolled off-screen (no visible
    // motion). All remaining traces are removed when the next user prompt is
    // submitted, keeping `current` mode ephemeral across turns without ever
    // moving a trace while it is visible.
    turn_reasoning_traces: Vec<TurnReasoningTrace>,
    // Hot-reload: if set, exec into new binary with this session ID (no rebuild)
    reload_requested: Option<String>,
    // Hot-rebuild: if set, do full git pull + cargo build + tests then exec
    rebuild_requested: Option<String>,
    // Update: if set, check for and download update from GitHub releases then exec
    update_requested: Option<String>,
    // Interactive background client maintenance action currently running
    background_client_action: Option<crate::bus::ClientMaintenanceAction>,
    // Reload the updated/rebuilt client once the current turn is idle
    pending_background_client_reload: Option<(String, crate::bus::ClientMaintenanceAction)>,
    // Restart: if set, exec into current binary with this session ID (no build)
    restart_requested: Option<String>,
    // Pasted content storage (displayed as placeholders, expanded on submit)
    pasted_contents: Vec<String>,
    // Pending pasted images (media_type, base64_data) attached to next message
    pending_images: Vec<(String, String)>,
    // One-shot flag: the next submitted prompt is routed to a new headed session.
    route_next_prompt_to_new_session: bool,
    // Restore-time flag: auto-submit restored input after startup.
    submit_input_on_startup: bool,
    /// Debug guard: tracks the last reason the startup auto-submit was deferred
    /// so `process_remote_followups` logs each distinct blocker exactly once
    /// instead of spamming every tick. Used to debug headed-spawn prompts that
    /// appear "seen but never sent".
    startup_submit_deferred_reason: Option<&'static str>,
    /// One-shot/session-local preview of the first-run onboarding empty state.
    onboarding_preview_mode: bool,
    /// Active onboarding simulator: `Some(index)` is the current simulated
    /// screen (driven by `onboarding_sim.rs`); `None` when not simulating. The
    /// simulator seeds synthetic phases so a developer can step through every
    /// first-run screen via Alt+5 reset or Cmd+5 toggle without touching real auth state.
    onboarding_sim: Option<usize>,
    /// Active guided first-run onboarding flow (model select -> continue ->
    /// transcript pick -> suggestions). `None` when not onboarding.
    onboarding_flow: Option<onboarding_flow::OnboardingFlow>,
    /// Shared cancellation guard for delayed post-login model catalog refreshes.
    /// Onboarding completion clears it so a late catalog result cannot override
    /// the model after the user has moved into a normal session.
    onboarding_auto_model_selection_active: Arc<AtomicBool>,
    /// One-shot guard: have we evaluated whether to auto-start the onboarding
    /// flow on startup yet? The fresh-install path logs in at the CLI before the
    /// TUI launches, so no in-TUI login event fires; this lets us still begin the
    /// flow once the TUI is ready and already authenticated.
    onboarding_startup_checked: bool,
    /// `Some(started_at)` between committing the login-import screen (Enter on
    /// the Yes/No list) and the async import resolving via `LoginCompleted`.
    /// While set, the onboarding welcome card shows an "Importing your
    /// logins..." progress state instead of the manual-login recovery copy, so
    /// the user isn't told to "log in again" right after choosing to import. The
    /// timestamp lets the onboarding tick watchdog recover the flow if the async
    /// `LoginCompleted` event never arrives (e.g. a wedged runtime), so the user
    /// can never be permanently stranded on the progress screen. `None` when no
    /// import is in flight.
    onboarding_import_in_progress: Option<Instant>,
    /// Set when a login import attempt failed (or imported nothing), so the
    /// onboarding recovery screen can explain what went wrong and give concrete
    /// next steps instead of the generic first-run "log in to get started" copy.
    /// `None` when there is no failure to report. Cleared when the user leaves
    /// the recovery screen (opens the picker) or onboarding advances.
    onboarding_import_error: Option<String>,
    /// The provider id we were importing/validating when onboarding failed, used
    /// to target the agent repair brief (`jcode auth-test --provider X`). `None`
    /// when unknown.
    onboarding_import_failed_provider: Option<String>,
    /// Pending first-run model-validation request for the new-session screen.
    /// In remote/client mode the live default model is reported by the server
    /// asynchronously, so we record that a validation is wanted and let the
    /// onboarding tick fire it once a concrete model id (not "unknown") is
    /// known. `None` means no validation is pending.
    onboarding_pending_model_validation: Option<onboarding_flow::OnboardingPendingValidation>,
    // Inline UI state for copy badges ([Alt] [⇧] [S])
    copy_badge_ui: CopyBadgeUiState,
    // Modal in-app selection/copy state for the chat viewport.
    copy_selection_mode: bool,
    copy_selection_anchor: Option<crate::tui::CopySelectionPoint>,
    copy_selection_cursor: Option<crate::tui::CopySelectionPoint>,
    copy_selection_pending_anchor: Option<crate::tui::CopySelectionPoint>,
    copy_selection_dragging: bool,
    copy_selection_goal_column: Option<usize>,
    /// While drag-selecting with the mouse held at the top/bottom edge of a pane,
    /// keep auto-scrolling on every tick (browser-style) until the drag leaves the
    /// edge or ends. Stores the pane and whether to scroll upward.
    copy_selection_edge_autoscroll: Option<(crate::tui::CopySelectionPane, bool)>,
    // Debug socket broadcast channel (if enabled)
    debug_tx: Option<tokio::sync::broadcast::Sender<super::backend::DebugEvent>>,
    // Remote provider info (set when running in remote mode)
    remote_client_instance_id: String,
    remote_provider_name: Option<String>,
    remote_provider_model: Option<String>,
    /// Monotonic counter bumped each time the server pushes a fresh remote model
    /// catalog snapshot (`AvailableModelsUpdated`). The onboarding readiness
    /// validation uses this to wait for the post-login catalog refresh to land
    /// before capturing the model label, so it reports the freshly-selected
    /// model (e.g. gpt-5.5 after an OpenAI login) instead of the stale pre-login
    /// default.
    remote_model_catalog_generation: u64,
    /// Server-resolved billing credential reported by a remote server: OAuth
    /// (subscription) vs API key (cost-based), or `None` when the active
    /// provider has no OAuth-vs-API-key distinction. Lets the info widget choose
    /// subscription vs cost-based usage display for remote sessions without
    /// re-deriving it from the provider name.
    remote_resolved_credential: Option<jcode_provider_core::ResolvedCredential>,
    remote_startup_phase: Option<RemoteStartupPhase>,
    remote_startup_phase_started: Option<Instant>,
    remote_reasoning_effort: Option<String>,
    remote_service_tier: Option<String>,
    remote_transport: Option<String>,
    remote_compaction_mode: Option<crate::config::CompactionMode>,
    remote_available_entries: Vec<String>,
    remote_model_options: Vec<crate::provider::ModelRoute>,
    pending_remote_model_refresh_snapshot: Option<(Vec<String>, Vec<crate::provider::ModelRoute>)>,
    // Remote MCP servers and skills (set from server in remote mode)
    remote_mcp_servers: Vec<String>,
    remote_skills: Vec<String>,
    // Total session token usage (from server in remote mode)
    remote_total_tokens: Option<(u64, u64)>,
    // Detailed persisted token/cache usage totals (from server in remote mode)
    remote_token_usage_totals: Option<crate::protocol::TokenUsageTotals>,
    // Whether the remote session is canary/self-dev (from server)
    remote_is_canary: Option<bool>,
    // Remote server version (from server)
    remote_server_version: Option<String>,
    // Whether the remote server has a newer binary available
    remote_server_has_update: Option<bool>,
    // Auto-reload server when stale (set on first connect if server_has_update)
    pending_server_reload: bool,
    // Real session id captured from a History event whose payload we deferred
    // because of a server/runtime version mismatch. The deferral returns before
    // `remote_session_id` is assigned, so without stashing the id here the
    // subsequent client reload handoff has no session to resume and would
    // fabricate a bogus `ses_<ts>_<rand>` id, producing
    // "No session found matching ..." on the next launch (issue #328).
    pending_reload_session_id: Option<String>,
    // Defense-in-depth circuit breaker for issue #277: count how many times this
    // client has auto-reloaded the server. A healthy reload happens at most once
    // (afterwards the server is up to date), so repeated auto-reloads indicate a
    // false-positive "update available" loop. Past a small threshold we stop
    // auto-reloading and surface a message instead of flickering forever.
    server_auto_reload_attempts: u32,
    // Remote server short name (e.g., "running", "blazing")
    remote_server_short_name: Option<String>,
    // Remote server icon (e.g., "🔥", "🌫️")
    remote_server_icon: Option<String>,
    // Current message request ID (for remote mode - to match Done events)
    current_message_id: Option<u64>,
    // Whether running in remote mode
    is_remote: bool,
    runtime_mode: AppRuntimeMode,
    // Remote rewind/undo request waiting for the server's replacement History payload.
    pending_remote_rewind_notice: Option<PendingRemoteRewindNotice>,
    // History-recovery watchdog for the "stuck on loading session…" bug. When a
    // remote (re)connect never receives the bootstrap `History` event, every
    // prompt path is gated behind `has_loaded_history()` and the session is
    // permanently stuck on "loading session…" until the user runs `/restart`.
    // These track when the current connection began waiting for history and how
    // many times we have re-requested it, so the watchdog can re-issue
    // `GetHistory` a few times before giving up.
    remote_history_wait_started: Option<Instant>,
    remote_history_recovery_attempts: u32,
    remote_history_recovery_last_attempt: Option<Instant>,
    // Server was just spawned - allow initial connection retries in run_remote
    server_spawning: bool,
    // Whether running in replay mode (readonly playback of a saved session)
    pub is_replay: bool,
    // Suppress terminal title updates for off-screen/silent replay instances.
    suppress_terminal_title_updates: bool,
    /// Override for elapsed time during headless video replay.
    pub replay_elapsed_override: Option<Duration>,
    /// Sim-time at which processing started (video replay only)
    replay_processing_started_ms: Option<f64>,
    // Remember tool call ids that have appeared in the provider transcript
    tool_call_ids: HashSet<String>,
    // Remember tool call ids that already have outputs
    tool_result_ids: HashSet<String>,
    // Number of provider messages already indexed for missing tool-output repair
    tool_output_scan_index: usize,
    // Current session ID (from server in remote mode)
    remote_session_id: Option<String>,
    // All sessions on the server (remote mode only)
    remote_sessions: Vec<String>,
    remote_side_pane_images: Vec<crate::session::RenderedImage>,
    /// Cached image-set signature. Text-only transcript changes must not
    /// invalidate this because rebuilding it materializes every image payload.
    side_pane_images_signature_cache: std::cell::Cell<Option<(usize, u64)>>,
    // Swarm member status snapshots (remote mode only)
    remote_swarm_members: Vec<crate::protocol::SwarmMemberStatus>,
    // Latest swarm plan snapshot (local or remote server event stream)
    swarm_plan_items: Vec<crate::plan::PlanItem>,
    swarm_plan_version: Option<u64>,
    swarm_plan_swarm_id: Option<String>,
    // Number of connected clients (remote mode only)
    remote_client_count: Option<usize>,
    // Build version tracking for auto-migration
    known_stable_version: Option<String>,
    // Last time we checked for stable version
    last_version_check: Option<Instant>,
    // Pending migration to new stable version
    pending_migration: Option<String>,
    // Session to resume on connect (remote mode)
    resume_session_id: Option<String>,
    // Exit code to use when quitting (for canary wrapper communication)
    requested_exit_code: Option<i32>,
    // Memory feature toggle for this session
    memory_enabled: bool,
    // Automatic end-of-turn review toggle for this session
    autoreview_enabled: bool,
    // Automatic end-of-turn judge toggle for this session
    autojudge_enabled: bool,
    // Last requested `/improve` mode for this session.
    improve_mode: Option<ImproveMode>,
    // Suppress duplicate memory injection messages for near-identical prompts.
    last_injected_memory_signature: Option<(String, Instant)>,
    // Swarm feature toggle for this session
    swarm_enabled: bool,
    // Debug-only: force the inline swarm gallery active (bypasses spawn-mode
    // and members-present gating) so visual tests can drive it deterministically.
    debug_force_inline_gallery: bool,
    // Currently selected agent index in the inline swarm panel (display order).
    swarm_panel_selected: usize,
    // Whether the inline swarm panel has keyboard focus (navigable list + detail).
    swarm_panel_focused: bool,
    // Whether the focused swarm panel owns the main transcript viewport.
    swarm_panel_full_page: bool,
    // Diff display mode (toggle with Alt+G)
    diff_mode: crate::config::DiffDisplayMode,
    // Center all content (from config)
    pub(crate) centered: bool,
    // Diagram display mode (from config)
    diagram_mode: crate::config::DiagramDisplayMode,
    // Whether the pinned diagram pane has focus
    diagram_focus: bool,
    // Selected diagram index in pinned mode (most recent = 0)
    diagram_index: usize,
    // Diagram scroll offsets in cells (only used when focused)
    diagram_scroll_x: i32,
    diagram_scroll_y: i32,
    // Diagram pane width ratio (percentage)
    diagram_pane_ratio: u8,
    // Animation state for smooth pane ratio transitions
    diagram_pane_ratio_from: u8,
    diagram_pane_ratio_target: u8,
    diagram_pane_anim_start: Option<Instant>,
    // Set once the user manually resizes the pane (drag or +/- keys), so the
    // adaptive image-width default stops overriding their explicit choice.
    diagram_pane_ratio_user_adjusted: bool,
    // Whether the pinned diagram pane is visible
    diagram_pane_enabled: bool,
    // Position of pinned diagram pane (side or top)
    diagram_pane_position: crate::config::DiagramPanePosition,
    // Diagram zoom percentage (100 = normal)
    diagram_zoom: u8,
    // Last diagram hash that was actually visible in the pinned pane.
    // Used to detect identity/layout changes that should reset back to fit.
    last_visible_diagram_hash: Option<u64>,
    // Whether the user is dragging the diagram pane border
    diagram_pane_dragging: bool,
    // Scroll offset for pinned diff pane
    diff_pane_scroll: usize,
    diff_pane_scroll_x: i32,
    side_panel_image_zoom_percent: u8,
    diff_pane_focus: bool,
    diff_pane_auto_scroll: bool,
    side_panel: crate::side_panel::SidePanelSnapshot,
    observe_mode_enabled: bool,
    observe_page_markdown: String,
    observe_page_updated_at_ms: u64,
    split_view_enabled: bool,
    split_view_markdown: String,
    split_view_updated_at_ms: u64,
    split_view_rendered_display_version: u64,
    split_view_rendered_streaming_hash: u64,
    todos_view_enabled: bool,
    todos_view_markdown: String,
    todos_view_updated_at_ms: u64,
    todos_view_rendered_hash: u64,
    /// Hash of the todo payload rendered into the inline chat todo card, used
    /// to keep the card live-updating while it stays in the transcript.
    todo_card_rendered_hash: u64,
    last_side_panel_refresh: Option<Instant>,
    // Most recently persisted focus target for dictation routing.
    last_client_focus_recorded_at: Option<Instant>,
    last_client_focus_session_id: Option<String>,
    // Most recently focused side panel page, used to restore visibility when toggled off.
    last_side_panel_focus_id: Option<String>,
    // User explicitly hid the side panel with the side-panel toggle key. While set, incoming snapshots may update
    // pages but must not reopen the panel by restoring focused_page_id.
    side_panel_user_hidden: bool,
    // True when the user explicitly hid the side panel (e.g. Alt+M) rather than
    // it being auto-hidden. This makes the hide "sticky" so transient image
    // repopulation (such as after a server reload/reconnect) does not re-reveal
    // a panel the user deliberately closed.
    side_panel_explicit_hidden: bool,
    // Pin read images to side pane
    pin_images: bool,
    // Inline transcript images render expanded (true) or as collapsed label
    // stubs (false). Toggled with Alt+Shift+I; persisted in UI preferences so
    // it survives restarts and session resumes.
    inline_images_visible: bool,
    // Per-image inline expand level (Fit/Large), keyed by image id. Cycled
    // by clicking the `expand` badge under an image. Absent ids are `Fit`.
    // `expanded_images_version` bumps on every change so the body/full prep
    // caches (which embed anchored images) invalidate exactly like the
    // `inline_images_visible` toggle does.
    expanded_images: std::collections::HashMap<u64, super::ui::inline_image_ui::ImageExpandLevel>,
    expanded_images_version: u64,
    // Auto-hide deadline for the pinned image side pane only.
    pinned_images_auto_hide_deadline: Option<Instant>,
    pinned_images_seen_count: usize,
    // Show a native terminal scrollbar in the chat viewport.
    chat_native_scrollbar: bool,
    // Show a native terminal scrollbar in the side panel.
    side_panel_native_scrollbar: bool,
    // Passive inline UI (informational blocks shown above input).
    inline_view_state: Option<super::InlineViewState>,
    // Interactive model/provider picker
    inline_interactive_state: Option<super::InlineInteractiveState>,
    // Cached model picker entries. Building these can require hydrating large provider catalogs.
    model_picker_cache: Option<ModelPickerCache>,
    model_picker_catalog_revision: u64,
    // Short-lived provider boost after login so newly authenticated models surface in /models.
    recent_authenticated_provider: Option<(String, Instant)>,
    /// A successful login/import has invalidated the catalog, but the refreshed
    /// provider snapshot has not reached this client yet. While set, `/model`
    /// shows a loading state instead of reusing the pre-login catalog.
    auth_catalog_refresh_pending: bool,
    pending_model_picker_load: Option<PendingModelPickerLoad>,
    model_picker_load_request_id: u64,
    // Pending model switch from picker (for remote mode async processing)
    pending_model_switch: Option<String>,
    pending_route_selection: Option<crate::provider::RouteSelection>,
    // Reasoning-effort variant chosen together with a model in the picker
    // (e.g. "gpt-5.5 (high)"), staged for remote mode alongside the model
    // switch. Without forwarding this to the server, it keeps its configured
    // default effort (low by default) and silently runs the newly selected
    // model at the wrong effort (issue #427).
    pending_reasoning_effort: Option<String>,
    // Remote SetModel has been sent but ModelChanged has not arrived yet. User
    // prompts submitted in this window are held so the first request cannot race
    // the model switch and use stale provider/model state.
    remote_model_switch_in_flight: bool,
    pending_prompt_after_model_switch: Option<input::PreparedInput>,
    // A manually submitted prompt that arrived before the remote session's
    // bootstrap History payload was applied. Submitting in that window is racy:
    // the locally-echoed user message is wiped by the `session_changed`
    // `clear_display_messages()` in the History handler (the prompt "vanishes"
    // while the server still streams a reply). Hold it until history loads and
    // let `process_remote_followups` dispatch it, exactly like a staged startup
    // prompt.
    pending_prompt_before_history: Option<input::PreparedInput>,
    // Pending account switch from inline picker (for remote mode async processing)
    pending_account_picker_action: Option<crate::tui::AccountPickerAction>,
    // Keybindings for model switching
    model_switch_keys: ModelSwitchKeys,
    // Keybindings for effort switching
    effort_switch_keys: super::keybind::EffortSwitchKeys,
    // Keybindings for scrolling
    scroll_keys: ScrollKeys,
    // Keybinding for centered-mode toggle
    centered_toggle_keys: CenteredToggleKeys,
    // Configurable pane / mode toggle keybindings
    toggle_keys: super::keybind::ToggleKeys,
    // Keybindings for Niri-style workspace navigation
    workspace_navigation_keys: WorkspaceNavigationKeys,
    // Optional configured keybinding for external dictation
    dictation_key: OptionalBinding,
    // Optional configured keybinding for spawning a fresh session in a new terminal
    new_terminal_key: OptionalBinding,
    // Optional configured keybinding for opening the /resume session picker
    open_resume_key: OptionalBinding,
    // Optional configured keybinding for accepting the post-error fallback offer
    fallback_switch_key: OptionalBinding,
    // Active external dictation session, if one is running
    dictation_session: Option<dictation::ActiveDictation>,
    // Whether an external dictation command is currently running
    dictation_in_flight: bool,
    // Ownership token for the current dictation request.
    dictation_request_id: Option<String>,
    // Session that owned the current dictation request when it was started.
    dictation_target_session_id: Option<String>,
    // Keep the current chat viewport while typing instead of snapping to bottom.
    typing_scroll_lock: bool,
    // Scroll bookmark: stashed scroll position for quick teleport back
    scroll_bookmark: Option<usize>,
    // Stashed input: saved via Ctrl+S for later retrieval
    stashed_input: Option<(String, usize)>,
    // Undo history for in-progress input editing (Ctrl+Z)
    input_undo_stack: Vec<(String, usize)>,
    // Short-lived notice for status feedback (model switch, cycle diff mode, etc.)
    status_notice: Option<(String, Instant)>,
    // Distinct learned-keybinding nudge ("you keep doing X the slow way, press
    // <key>"). Rendered in its own pop-out color, separate from status_notice,
    // and shown at most once per session.
    learn_hint: Option<(String, Instant)>,
    // Whether a learned-keybinding nudge has already been surfaced this session.
    learn_hint_shown_this_session: bool,
    // Whether the swarm-config-is-a-prompt hint has been surfaced this session.
    swarm_hint_shown_this_session: bool,
    // Whether the inline sponsored-discovery policy detail has been attached
    // this session. It appears once, on the first discover_tools result.
    sponsor_disclosure_shown_this_session: bool,
    // Inline hotkey feedback: "you just pressed X → does Y" for rarely-used
    // known chords, or "X isn't bound · nearest: ..." for unknown chords.
    // Rendered in the same pop-out slot as learn_hint.
    hotkey_feedback: Option<(String, Instant)>,
    // Lazily-loaded persisted per-action hotkey usage counters.
    hotkey_usage: Option<hotkey_feedback::HotkeyUsageState>,
    // Per-chord counts of unknown-hotkey notices shown this session.
    unknown_hotkey_seen: std::collections::HashMap<String, u32>,
    // When the last unknown-hotkey notice was shown, for rate limiting.
    last_unknown_hotkey_notice: Option<Instant>,
    // Persistent startup notice card (e.g. launch-hotkeys / welcome tip) shown on
    // the idle screen of a fresh session. Stashed so it can be re-applied after
    // the remote History bootstrap clears the transcript for a brand-new session,
    // which otherwise makes the card flash for a moment and disappear.
    pending_startup_notice: Option<(String, String)>,
    // Experimental feature warnings already shown in this session.
    experimental_feature_warnings_seen: HashSet<String>,
    // Active first-use experimental warning for the currently running tool.
    active_experimental_feature_notice: Option<String>,
    // Message to interleave during processing (set via Ctrl+Enter in queue mode)
    interleave_message: Option<String>,
    // Message sent as soft interrupt but not yet injected (shown in queue preview until injected)
    pending_soft_interrupts: Vec<String>,
    // Soft interrupts written to the socket but not yet acknowledged by the server.
    pending_soft_interrupt_requests: Vec<(u64, String)>,
    // Whether the current remote turn should trigger autoreview after completion.
    autoreview_after_current_turn: bool,
    // Whether the current remote turn should trigger autojudge after completion.
    autojudge_after_current_turn: bool,
    // Startup message to preload into the next spawned split window.
    pending_split_startup_message: Option<String>,
    // Parent/original session that feedback flows should report back to after a split launch.
    pending_split_parent_session_id: Option<String>,
    // Startup user prompt to auto-submit in the next spawned split window.
    pending_split_prompt: Option<PendingSplitPrompt>,
    // Optional model override to apply before opening the next spawned split window.
    pending_split_model_override: Option<String>,
    // Optional provider key override to persist into the next spawned split window.
    pending_split_provider_key_override: Option<String>,
    // Human-friendly label for the next spawned split window flow.
    pending_split_label: Option<String>,
    // Timestamp for showing a temporary client-side running state while a split launch is in flight.
    pending_split_started_at: Option<Instant>,
    // Ask the remote followup loop to issue a split request once idle.
    pending_split_request: bool,
    // Ask the followup loop to issue a transfer request once idle.
    pending_transfer_request: bool,
    // Local transfer preparation currently running in the background.
    pending_local_transfer: Option<PendingLocalTransfer>,
    // Queue mode: if true, Enter during processing queues; if false, Enter queues to send next
    // Toggle with Ctrl+Tab or Ctrl+T
    queue_mode: bool,
    // Automatically reload the remote server when a newer server binary is detected.
    auto_server_reload: bool,
    // After an interrupt, wait one redraw before auto-dispatching queued followups so
    // the queued preview can render in the interrupted state first.
    pending_queued_dispatch: bool,
    // Tab completion state: (base_input, suggestion_index)
    // base_input is the original input before cycling, suggestion_index is current position
    tab_completion_state: Option<(String, usize)>,
    // Selected row in the visible command suggestion list.
    command_suggestion_selected: usize,
    // Time when app started (for startup animations)
    app_started: Instant,
    // Whether the client terminal currently has focus. When the terminal window
    // or tab is backgrounded (FocusLost), decorative animations and periodic
    // idle redraws are paused so a swarm of background sessions does not burn
    // CPU animating screens nobody is looking at. Defaults to true because not
    // every terminal reports focus events.
    client_focused: bool,
    // Optional client runtime memory logger for low-overhead attribution journaling.
    runtime_memory_log: Option<RuntimeMemoryLogController>,
    // Once-per-idle-period retained-heap trim state (see idle_heap_release.rs).
    idle_heap_release: idle_heap_release::IdleHeapRelease,
    // Binary modification time when client started (for smart reload detection)
    client_binary_mtime: Option<std::time::SystemTime>,
    // Rate limit state: when rate limit resets (if rate limited)
    rate_limit_reset: Option<Instant>,
    // Message being sent when rate limit hit (to auto-retry in remote mode)
    rate_limit_pending_message: Option<PendingRemoteMessage>,
    // Consecutive turn errors that classify as credential/auth failures.
    // Reset on turn success or auth change; drives the credential-failure
    // circuit breaker that halts automatic resends (see
    // CREDENTIAL_FAILURE_BREAKER_THRESHOLD).
    consecutive_credential_failures: u32,
    // Last turn-level stream error (used by /fix to choose recovery actions)
    last_stream_error: Option<String>,
    // Raw text of the most recent user prompt that started a turn. Restored to the
    // input box if the turn fails (e.g. "token refresh needed") so the user does not
    // lose what they typed and can resend after recovering.
    last_submitted_input: Option<String>,
    // Store reload info to pass to agent after reconnection (remote mode)
    reload_info: Vec<String>,
    // Debug trace for scripted testing
    debug_trace: DebugTrace,
    // Incremental markdown renderer for streaming text (uses RefCell for interior mutability)
    streaming_md_renderer: RefCell<IncrementalMarkdownRenderer>,
    /// Ambient mode system prompt override (when running as visible ambient cycle)
    ambient_system_prompt: Option<String>,
    /// Pending login flow: if set, next input is intercepted as OAuth code or API key
    pending_login: Option<PendingLogin>,
    /// Pending account picker follow-up input (new label or setting value)
    pending_account_input: Option<auth::PendingAccountInput>,
    /// Pending SSH remote target prompt. Stores the friendly remote name.
    pending_ssh_remote_name: Option<String>,
    /// One-shot flag: force the next paint to clear the terminal first.
    /// Needed after native terminal scrolls mutate the screen outside ratatui's diff model.
    force_full_redraw: bool,
    /// One-shot flag: force the next paint to re-emit every cell by invalidating
    /// ratatui's previous buffer, without an intermediate ED2 clear escape.
    /// Chat scrolling uses this to clear wide-grapheme ghosts (ratatui #2357)
    /// without the clear-then-repaint flicker around kitty image placeholders
    /// (issue #404).
    force_full_repaint: bool,
    /// Last mouse scroll event timestamp (for trackpad velocity detection)
    last_mouse_scroll: Option<Instant>,
    /// Active smooth-scroll target for queued mouse-wheel motion.
    mouse_scroll_target: Option<MouseScrollTarget>,
    /// Remaining queued mouse-wheel lines. Positive = down, negative = up.
    mouse_scroll_queue: i16,
    /// When the user overscrolls past the bottom of the transcript, an extra
    /// status line is revealed below the input. This records the last time an
    /// overscroll tick was received; the line dwells for a fixed window after
    /// the last tick, then rebounds away. `None` means the line is hidden.
    chat_overscroll_last: Option<Instant>,
    /// Scroll offset for changelog overlay (None = not visible)
    changelog_scroll: Option<usize>,
    help_scroll: Option<usize>,
    model_status_scroll: Option<usize>,
    model_status_content: String,
    /// Session picker overlay (None = not visible)
    session_picker_overlay: Option<RefCell<super::session_picker::SessionPicker>>,
    session_picker_mode: SessionPickerMode,
    pending_session_picker_load: Option<PendingSessionPickerLoad>,
    catchup_return_stack: Vec<String>,
    pending_catchup_resume: Option<PendingCatchupResume>,
    in_flight_catchup_resume: Option<PendingCatchupResume>,
    /// Login picker overlay (None = not visible)
    login_picker_overlay: Option<RefCell<super::login_picker::LoginPicker>>,
    /// Account picker overlay (None = not visible)
    account_picker_overlay: Option<RefCell<super::account_picker::AccountPicker>>,
    /// Usage overlay (None = not visible)
    usage_overlay: Option<RefCell<super::usage_overlay::UsageOverlay>>,
    /// Whether a usage refresh request is currently in flight.
    usage_report_refreshing: bool,
    /// Whether a `/productivity` report generation is currently in flight.
    productivity_refreshing: bool,
    /// Last time the passive overnight progress card polled its run files.
    last_overnight_card_refresh: Option<Instant>,
    /// Per-client Niri-style workspace navigation state. Previously a process
    /// global; now owned per App instance.
    workspace_client: super::workspace_client::WorkspaceClientState,
}

/// Inert provider used by runtime modes whose output is supplied by another source.
///
/// Remote clients render server events. Replay renders recorded events. Neither mode may call a
/// live provider from the TUI process.
struct InertRuntimeProvider {
    runtime_mode: AppRuntimeMode,
}

impl InertRuntimeProvider {
    fn new(runtime_mode: AppRuntimeMode) -> Self {
        Self { runtime_mode }
    }

    fn provider_label(&self) -> &'static str {
        match self.runtime_mode {
            AppRuntimeMode::RemoteClient => "remote",
            AppRuntimeMode::Replay => "replay",
            AppRuntimeMode::TestHarness => "test-harness",
        }
    }
}

#[async_trait::async_trait]
impl Provider for InertRuntimeProvider {
    fn name(&self) -> &str {
        self.provider_label()
    }
    fn model(&self) -> String {
        "unknown".to_string()
    }

    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _session_id: Option<&str>,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        Err(anyhow::anyhow!(
            "{} runtime does not allow live provider completion from the TUI",
            self.provider_label()
        ))
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(InertRuntimeProvider::new(self.runtime_mode))
    }
}

impl App {
    const AUTO_RETRY_BASE_DELAY_SECS: u64 = 2;
    const AUTO_RETRY_MAX_ATTEMPTS: u8 = 3;
    /// Budget for completion-confidence gate nudges per auto-poke cycle.
    /// Observed live: a session that stopped updating its todos was re-nudged
    /// with the same hidden continuation every ~5 seconds indefinitely, one
    /// full API call per nudge. The counter resets whenever a nudge actually
    /// changes the stored todos (progress) or auto-poke is re-armed.
    const TODO_COMPLETION_GATE_MAX_ATTEMPTS: u8 = 5;
    /// Circuit breaker for credential failures: once this many consecutive
    /// turn errors classify as credential/auth failures, every automatic
    /// resend path (auto-retry, auto-poke, overnight poke, queued follow-ups)
    /// is stopped until auth changes or a turn succeeds. Telemetry showed
    /// runaway sessions logging thousands of 401s at one failed turn per
    /// retry (18k in one session) because retry loops kept resending against
    /// a dead credential.
    const CREDENTIAL_FAILURE_BREAKER_THRESHOLD: u32 = 3;
    const INPUT_UNDO_LIMIT: usize = 128;
    const CLIENT_FOCUS_RECORD_DEBOUNCE: Duration = Duration::from_secs(2);
    const KV_CACHE_OPTIMAL_OK_PCT: u8 = 85;
    const KV_CACHE_MIN_MISSED_TOKENS: u64 = 1_024;
    const KV_CACHE_MAX_MISS_SAMPLES: usize = 12;

    pub(super) fn begin_kv_cache_request(
        &mut self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system_static: &str,
        system_dynamic: &str,
    ) {
        let turn_number = self
            .display_messages
            .iter()
            .filter(|message| message.role == "user")
            .count()
            .max(1);
        if self.kv_cache.kv_cache_turn_number == Some(turn_number) {
            self.kv_cache.kv_cache_turn_call_index = self
                .kv_cache
                .kv_cache_turn_call_index
                .saturating_add(1)
                .max(1);
        } else {
            self.kv_cache.kv_cache_turn_number = Some(turn_number);
            self.kv_cache.kv_cache_turn_call_index = 1;
        }

        let baseline = self.kv_cache_baseline_for_current_session();
        let signature =
            Self::kv_cache_request_signature(messages, tools, system_static, system_dynamic);
        let baseline_messages_prefix_matches = baseline
            .as_ref()
            .and_then(|baseline| baseline.signature.as_ref())
            .map(|previous| Self::kv_cache_signatures_prefix_match(&signature, previous));

        self.maybe_push_cold_cache_warning(
            turn_number,
            self.kv_cache.kv_cache_turn_call_index,
            baseline.as_ref(),
        );
        self.pause_streaming_tps(false);
        self.kv_cache.current_api_usage_recorded = false;
        self.mark_stream_usage_call_boundary();

        self.kv_cache.pending_kv_cache_request = Some(PendingKvCacheRequest {
            turn_number,
            call_index: self.kv_cache.kv_cache_turn_call_index,
            provider: self.kv_cache_provider_name(),
            model: self.kv_cache_provider_model(),
            upstream_provider: self.upstream_provider.clone(),
            signature: Some(signature),
            baseline_messages_prefix_matches,
            baseline,
        });
    }

    pub(in crate::tui::app) fn begin_remote_kv_cache_request(
        &mut self,
        signature: KvCacheRequestSignature,
    ) {
        let turn_number = self
            .display_messages
            .iter()
            .filter(|message| message.role == "user")
            .count()
            .max(1);
        if self.kv_cache.kv_cache_turn_number == Some(turn_number) {
            self.kv_cache.kv_cache_turn_call_index = self
                .kv_cache
                .kv_cache_turn_call_index
                .saturating_add(1)
                .max(1);
        } else {
            self.kv_cache.kv_cache_turn_number = Some(turn_number);
            self.kv_cache.kv_cache_turn_call_index = 1;
        }

        let baseline = self.kv_cache_baseline_for_current_session();
        let baseline_messages_prefix_matches = baseline
            .as_ref()
            .and_then(|baseline| baseline.signature.as_ref())
            .map(|previous| Self::kv_cache_signatures_prefix_match(&signature, previous));
        self.maybe_push_cold_cache_warning(
            turn_number,
            self.kv_cache.kv_cache_turn_call_index,
            baseline.as_ref(),
        );
        self.pause_streaming_tps(false);
        self.kv_cache.current_api_usage_recorded = false;
        self.mark_stream_usage_call_boundary();
        self.kv_cache.pending_kv_cache_request = Some(PendingKvCacheRequest {
            turn_number,
            call_index: self.kv_cache.kv_cache_turn_call_index,
            provider: self.kv_cache_provider_name(),
            model: self.kv_cache_provider_model(),
            upstream_provider: self.upstream_provider.clone(),
            signature: Some(signature),
            baseline_messages_prefix_matches,
            baseline,
        });
    }

    /// Session id the next KV-cache baseline should be tagged with.
    ///
    /// A single `App` can stream several sessions over its lifetime (remote
    /// session switches, local handoffs). The baseline must only be compared
    /// against requests from the same session, so we capture the active id here.
    fn kv_cache_session_id(&self) -> Option<String> {
        if self.is_remote {
            self.remote_session_id.clone()
        } else {
            Some(self.session.id.clone())
        }
    }

    /// Return the stored baseline only when it belongs to the active session.
    ///
    /// Diffing a request against a baseline captured for a different (often
    /// larger) session makes the new history look like a broken prefix and
    /// emits a spurious `harness:_prefix_changed` miss. Treat a foreign baseline
    /// as absent (warmup) instead.
    fn kv_cache_baseline_for_current_session(&self) -> Option<KvCacheBaseline> {
        let baseline = self.kv_cache.kv_cache_baseline.clone()?;
        let current = self.kv_cache_session_id();
        if baseline.session_id == current {
            Some(baseline)
        } else {
            None
        }
    }

    fn maybe_push_cold_cache_warning(
        &mut self,
        turn_number: usize,
        call_index: u16,
        baseline: Option<&KvCacheBaseline>,
    ) {
        if turn_number <= 1 || call_index != 1 {
            return;
        }
        let Some(baseline) = baseline else {
            return;
        };
        self.push_cold_cache_warning_for_baseline(baseline, ColdCacheWarningTrigger::RequestStart);
    }

    /// Idle-tick counterpart of [`Self::maybe_push_cold_cache_warning`]: warn
    /// in the transcript the moment the prompt-cache TTL expires while the
    /// session sits idle, instead of only after the user submits the next
    /// message and the miss is already unavoidable. Returns true when a
    /// warning was pushed so the tick loop can request a redraw.
    pub(super) fn maybe_push_idle_cold_cache_warning(&mut self) -> bool {
        if self.is_processing {
            return false;
        }
        // Cheap per-tick gates first: this runs on every tick (up to animation
        // cadence), and the baseline clone below can carry a full request
        // signature, so bail before cloning whenever possible.
        {
            let Some(baseline) = self.kv_cache.kv_cache_baseline.as_ref() else {
                return false;
            };
            if self.kv_cache.cold_cache_warned_baseline_completed_at == Some(baseline.completed_at)
            {
                return false;
            }
            let Some(ttl_secs) =
                crate::tui::cache_ttl_for_provider_model(&baseline.provider, Some(&baseline.model))
            else {
                return false;
            };
            if baseline.completed_at.elapsed().as_secs() < ttl_secs {
                return false;
            }
        }
        // Turn-1 sessions have no meaningful warm prefix to lose; mirror the
        // request-start gate.
        let user_turns = self
            .display_messages
            .iter()
            .filter(|message| message.role == "user")
            .count();
        if user_turns < 1 {
            return false;
        }
        let Some(baseline) = self.kv_cache_baseline_for_current_session() else {
            return false;
        };
        self.push_cold_cache_warning_for_baseline(&baseline, ColdCacheWarningTrigger::IdleExpiry)
    }

    /// Push the cold-cache transcript warning if `baseline`'s TTL has expired
    /// and this cold period has not been warned about yet. Returns true when
    /// a warning was pushed.
    fn push_cold_cache_warning_for_baseline(
        &mut self,
        baseline: &KvCacheBaseline,
        trigger: ColdCacheWarningTrigger,
    ) -> bool {
        let Some(ttl_secs) =
            crate::tui::cache_ttl_for_provider_model(&baseline.provider, Some(&baseline.model))
        else {
            return false;
        };
        let age_secs = baseline.completed_at.elapsed().as_secs();
        if age_secs < ttl_secs {
            return false;
        }
        // Warn at most once per cache write (the baseline's completed_at is
        // refreshed by every completed call, which re-arms the warning).
        if self.kv_cache.cold_cache_warned_baseline_completed_at == Some(baseline.completed_at) {
            return false;
        }
        self.kv_cache.cold_cache_warned_baseline_completed_at = Some(baseline.completed_at);

        let expired_ago_secs = age_secs.saturating_sub(ttl_secs);
        let tokens = baseline.input_tokens;
        let token_label = if tokens >= 1_000_000 {
            format!("{:.1}M", tokens as f64 / 1_000_000.0)
        } else if tokens >= 1_000 {
            format!("{}K", tokens / 1_000)
        } else {
            tokens.to_string()
        };
        // Keep this to a single short line. The idle trigger fires the moment
        // the TTL expires, so an "N ago" detail would always read ~0s there;
        // the request-start fallback can fire long after expiry (e.g.
        // suspended TUI), where the age is genuinely informative.
        let message = match trigger {
            ColdCacheWarningTrigger::IdleExpiry => format!(
                "🧊 Prompt cache went cold · next turn may resend ~{} tok · /cache extends",
                token_label
            ),
            ColdCacheWarningTrigger::RequestStart => format!(
                "🧊 Prompt cache went cold {} ago · this request may resend ~{} tok",
                crate::tui::format_compact_age(expired_ago_secs),
                token_label
            ),
        };
        self.push_display_message(DisplayMessage::system(message));
        true
    }

    pub(super) fn record_completed_stream_cache_usage(&mut self) -> bool {
        let has_cache_telemetry = self.streaming.streaming_cache_read_tokens.is_some()
            || self.streaming.streaming_cache_creation_tokens.is_some();
        if self.kv_cache.current_api_usage_recorded {
            return false;
        }
        if self.streaming.streaming_input_tokens == 0 {
            return false;
        }

        let optimal_input_tokens = self.token_accounting.cache_next_optimal_input_tokens;
        // Stash the *effective* prompt size for this request so the next request's
        // cache-read can be compared against everything that just became cacheable.
        // For split-accounting providers (Anthropic) bare `input` is only the
        // uncached remainder, so the reusable prefix is input + read + creation.
        let effective_prompt_tokens = crate::tui::info_widget::effective_prompt_tokens(
            self.streaming.streaming_input_tokens,
            self.streaming.streaming_cache_read_tokens.unwrap_or(0),
            self.streaming.streaming_cache_creation_tokens.unwrap_or(0),
        );
        self.token_accounting.cache_next_optimal_input_tokens = Some(effective_prompt_tokens);

        let request = self
            .kv_cache
            .pending_kv_cache_request
            .take()
            .unwrap_or_else(|| self.fallback_pending_kv_cache_request());
        self.kv_cache.current_api_usage_recorded = true;

        self.record_kv_cache_miss_sample(&request);

        let baseline_session_id = self.kv_cache_session_id();

        if !has_cache_telemetry {
            self.kv_cache.kv_cache_baseline = Some(KvCacheBaseline {
                session_id: baseline_session_id,
                input_tokens: self.streaming.streaming_input_tokens,
                completed_at: Instant::now(),
                provider: request.provider,
                model: request.model,
                upstream_provider: request.upstream_provider,
                signature: request.signature,
            });
            return true;
        }

        self.token_accounting.total_cache_reported_input_tokens = self
            .token_accounting
            .total_cache_reported_input_tokens
            .saturating_add(self.streaming.streaming_input_tokens);
        if let Some(optimal) = optimal_input_tokens {
            self.token_accounting.total_cache_optimal_input_tokens = self
                .token_accounting
                .total_cache_optimal_input_tokens
                .saturating_add(optimal);
        }
        self.token_accounting.total_cache_read_tokens = self
            .token_accounting
            .total_cache_read_tokens
            .saturating_add(self.streaming.streaming_cache_read_tokens.unwrap_or(0));
        self.token_accounting.total_cache_creation_tokens = self
            .token_accounting
            .total_cache_creation_tokens
            .saturating_add(self.streaming.streaming_cache_creation_tokens.unwrap_or(0));
        self.token_accounting.last_cache_reported_input_tokens =
            Some(self.streaming.streaming_input_tokens);
        self.token_accounting.last_cache_read_tokens =
            Some(self.streaming.streaming_cache_read_tokens.unwrap_or(0));
        self.token_accounting.last_cache_creation_tokens =
            Some(self.streaming.streaming_cache_creation_tokens.unwrap_or(0));
        self.token_accounting.last_cache_optimal_input_tokens = optimal_input_tokens;

        self.log_kv_cache_usage_summary(&request, optimal_input_tokens);

        self.kv_cache.kv_cache_baseline = Some(KvCacheBaseline {
            session_id: baseline_session_id,
            input_tokens: effective_prompt_tokens,
            completed_at: Instant::now(),
            provider: request.provider,
            model: request.model,
            upstream_provider: request.upstream_provider,
            signature: request.signature,
        });
        true
    }

    fn log_kv_cache_usage_summary(
        &self,
        request: &PendingKvCacheRequest,
        optimal_input_tokens: Option<u64>,
    ) {
        let input_tokens = self.streaming.streaming_input_tokens;
        let read_tokens = self.streaming.streaming_cache_read_tokens.unwrap_or(0);
        let creation_tokens = self.streaming.streaming_cache_creation_tokens.unwrap_or(0);
        let read_pct = ratio_pct(read_tokens, input_tokens);
        let creation_pct = ratio_pct(creation_tokens, input_tokens);
        let optimal_read_pct = optimal_input_tokens.map(|optimal| ratio_pct(read_tokens, optimal));
        let session_read_pct = ratio_pct(
            self.token_accounting.total_cache_read_tokens,
            self.token_accounting.total_cache_reported_input_tokens,
        );
        let session_optimal_read_pct = if self.token_accounting.total_cache_optimal_input_tokens > 0
        {
            Some(ratio_pct(
                self.token_accounting.total_cache_read_tokens,
                self.token_accounting.total_cache_optimal_input_tokens,
            ))
        } else {
            None
        };
        let miss = self
            .kv_cache
            .kv_cache_miss_samples
            .last()
            .filter(|sample| {
                sample.turn_number == request.turn_number && sample.call_index == request.call_index
            })
            .map(|sample| {
                format!(
                    "{}:{}",
                    sample.missed_tokens,
                    sample.reason.label().replace(' ', "_")
                )
            })
            .unwrap_or_else(|| {
                if request.baseline.is_none() {
                    "warmup:no_baseline".to_string()
                } else {
                    "none".to_string()
                }
            });
        let baseline_age_secs = request
            .baseline
            .as_ref()
            .map(|baseline| baseline.completed_at.elapsed().as_secs());
        let baseline_input_tokens = request
            .baseline
            .as_ref()
            .map(|baseline| baseline.input_tokens);
        let missed_tokens =
            baseline_input_tokens.map(|baseline| baseline.saturating_sub(read_tokens));
        let ttl_secs = request.baseline.as_ref().and_then(|baseline| {
            crate::tui::cache_ttl_for_provider_model(&baseline.provider, Some(&baseline.model))
        });
        let ttl_remaining_secs = ttl_secs
            .zip(baseline_age_secs)
            .map(|(ttl, age)| ttl.saturating_sub(age));
        let current_signature = request.signature.as_ref();
        let baseline_signature = request
            .baseline
            .as_ref()
            .and_then(|baseline| baseline.signature.as_ref());
        let system_static_hash_changed = current_signature
            .zip(baseline_signature)
            .map(|(current, baseline)| current.system_static_hash != baseline.system_static_hash);
        let tools_hash_changed = current_signature
            .zip(baseline_signature)
            .map(|(current, baseline)| current.tools_hash != baseline.tools_hash);
        let message_full_hash_changed = current_signature
            .zip(baseline_signature)
            .map(|(current, baseline)| current.messages_hash != baseline.messages_hash);
        let message_prefix_changed = request
            .baseline_messages_prefix_matches
            .map(|matches| !matches);
        let common_prefix_messages = current_signature
            .zip(baseline_signature)
            .map(|(current, baseline)| Self::kv_cache_common_prefix_messages(current, baseline));
        let first_changed_message_index = common_prefix_messages
            .zip(baseline_signature.map(|signature| signature.message_count))
            .and_then(|(common, baseline_count)| (common < baseline_count).then_some(common));
        let dynamic_hash_changed = current_signature
            .zip(baseline_signature)
            .map(|(current, baseline)| current.ephemeral_hash != baseline.ephemeral_hash);
        let current_message_count = current_signature.map(|signature| signature.message_count);
        let baseline_message_count = baseline_signature.map(|signature| signature.message_count);
        let current_tool_count = current_signature.map(|signature| signature.tool_count);
        let baseline_tool_count = baseline_signature.map(|signature| signature.tool_count);
        let current_system_chars = current_signature.map(|signature| signature.system_static_chars);
        let baseline_system_chars =
            baseline_signature.map(|signature| signature.system_static_chars);
        let current_tools_json_chars =
            current_signature.map(|signature| signature.tools_json_chars);
        let baseline_tools_json_chars =
            baseline_signature.map(|signature| signature.tools_json_chars);
        let current_messages_json_chars =
            current_signature.map(|signature| signature.messages_json_chars);
        let baseline_messages_json_chars =
            baseline_signature.map(|signature| signature.messages_json_chars);
        let current_ephemeral_chars = current_signature.map(|signature| signature.ephemeral_chars);
        let baseline_ephemeral_chars =
            baseline_signature.map(|signature| signature.ephemeral_chars);
        let current_ephemeral_message_count =
            current_signature.map(|signature| signature.ephemeral_message_count);
        let baseline_ephemeral_message_count =
            baseline_signature.map(|signature| signature.ephemeral_message_count);
        let current_hashes_present = current_signature
            .map(|signature| !signature.message_hashes.is_empty())
            .unwrap_or(false);
        let baseline_hashes_present = baseline_signature
            .map(|signature| !signature.message_hashes.is_empty())
            .unwrap_or(false);

        crate::logging::info(&format!(
            "KV_CACHE_USAGE: turn={} call={} provider={} upstream={:?} model={} \
             input={} cache_read={} cache_write={} read_pct={} write_pct={} \
             optimal_input={:?} optimal_read_pct={:?} missed_tokens={:?} miss={} \
             session_input={} session_read={} session_write={} session_read_pct={} \
             session_optimal_input={} session_optimal_read_pct={:?} \
             baseline_input={:?} baseline_age_secs={:?} ttl_secs={:?} ttl_remaining_secs={:?} \
             prefix_matches={:?} common_prefix_messages={:?} first_changed_message_index={:?} \
             system_changed={:?} tools_changed={:?} message_prefix_changed={:?} message_full_hash_changed={:?} dynamic_changed={:?} \
             message_count={:?} baseline_message_count={:?} tool_count={:?} baseline_tool_count={:?} \
             system_chars={:?} baseline_system_chars={:?} tools_json_chars={:?} baseline_tools_json_chars={:?} \
             messages_json_chars={:?} baseline_messages_json_chars={:?} ephemeral_chars={:?} baseline_ephemeral_chars={:?} \
             ephemeral_message_count={:?} baseline_ephemeral_message_count={:?} message_hashes_present={} baseline_message_hashes_present={} \
             connection={:?} status_detail={:?}",
            request.turn_number,
            request.call_index,
            request.provider,
            request.upstream_provider,
            request.model,
            input_tokens,
            read_tokens,
            creation_tokens,
            read_pct,
            creation_pct,
            optimal_input_tokens,
            optimal_read_pct,
            missed_tokens,
            miss,
            self.token_accounting.total_cache_reported_input_tokens,
            self.token_accounting.total_cache_read_tokens,
            self.token_accounting.total_cache_creation_tokens,
            session_read_pct,
            self.token_accounting.total_cache_optimal_input_tokens,
            session_optimal_read_pct,
            baseline_input_tokens,
            baseline_age_secs,
            ttl_secs,
            ttl_remaining_secs,
            request.baseline_messages_prefix_matches,
            common_prefix_messages,
            first_changed_message_index,
            system_static_hash_changed,
            tools_hash_changed,
            message_prefix_changed,
            message_full_hash_changed,
            dynamic_hash_changed,
            current_message_count,
            baseline_message_count,
            current_tool_count,
            baseline_tool_count,
            current_system_chars,
            baseline_system_chars,
            current_tools_json_chars,
            baseline_tools_json_chars,
            current_messages_json_chars,
            baseline_messages_json_chars,
            current_ephemeral_chars,
            baseline_ephemeral_chars,
            current_ephemeral_message_count,
            baseline_ephemeral_message_count,
            current_hashes_present,
            baseline_hashes_present,
            self.connection_type.as_deref(),
            self.status_detail.as_deref(),
        ));
    }

    fn fallback_pending_kv_cache_request(&self) -> PendingKvCacheRequest {
        PendingKvCacheRequest {
            turn_number: self
                .display_messages
                .iter()
                .filter(|message| message.role == "user")
                .count()
                .max(1),
            call_index: 1,
            provider: self.kv_cache_provider_name(),
            model: self.kv_cache_provider_model(),
            upstream_provider: self.upstream_provider.clone(),
            signature: None,
            baseline_messages_prefix_matches: None,
            baseline: self.kv_cache_baseline_for_current_session(),
        }
    }

    fn record_kv_cache_miss_sample(&mut self, request: &PendingKvCacheRequest) {
        let Some(baseline) = request.baseline.as_ref() else {
            return;
        };
        let expected_tokens = baseline.input_tokens;
        if expected_tokens == 0 {
            return;
        }

        let read_tokens = self.streaming.streaming_cache_read_tokens.unwrap_or(0);
        let missed_tokens = expected_tokens.saturating_sub(read_tokens);
        if missed_tokens < Self::KV_CACHE_MIN_MISSED_TOKENS {
            return;
        }

        let optimal_pct = ratio_pct(read_tokens, expected_tokens);
        let reason =
            self.classify_kv_cache_miss_reason(request, baseline, read_tokens, optimal_pct);
        if optimal_pct >= Self::KV_CACHE_OPTIMAL_OK_PCT
            && !matches!(
                reason,
                KvCacheMissReason::ProviderSwitch
                    | KvCacheMissReason::ModelSwitch
                    | KvCacheMissReason::UpstreamSwitch
                    | KvCacheMissReason::Expired
                    | KvCacheMissReason::HarnessSystemChanged
                    | KvCacheMissReason::HarnessToolsChanged
                    | KvCacheMissReason::HarnessPrefixChanged
            )
        {
            return;
        }

        self.kv_cache.kv_cache_miss_samples.push(KvCacheMissSample {
            turn_number: request.turn_number,
            call_index: request.call_index,
            missed_tokens,
            reason,
        });
        if self.kv_cache.kv_cache_miss_samples.len() > Self::KV_CACHE_MAX_MISS_SAMPLES {
            let overflow =
                self.kv_cache.kv_cache_miss_samples.len() - Self::KV_CACHE_MAX_MISS_SAMPLES;
            self.kv_cache.kv_cache_miss_samples.drain(0..overflow);
        }

        self.maybe_push_kv_cache_miss_notice(
            request.turn_number,
            reason,
            missed_tokens,
            baseline.completed_at,
        );
    }

    /// Surface a loud in-chat alarm when a request missed the KV cache for a
    /// harness-caused (avoidable) reason. Provider/model/upstream switches and
    /// TTL expiry are legitimate and intentionally excluded — those are user- or
    /// time-driven, not harness bugs. The harness should essentially never
    /// invalidate the prefix cache on its own, so when it does we want it to be
    /// visible immediately rather than buried in logs.
    ///
    /// Some harness actions do legitimately change the prompt mid-session
    /// (config reloads, skill reloads). Those sites document the invalidation
    /// in `cache_invalidation` when it happens; if a documented cause is found
    /// between the baseline and this request, the notice attributes the miss
    /// to it (informational) instead of raising the unexplained-bust alarm.
    fn maybe_push_kv_cache_miss_notice(
        &mut self,
        turn_number: usize,
        reason: KvCacheMissReason,
        missed_tokens: u64,
        baseline_completed_at: Instant,
    ) {
        if !crate::config::config().features.kv_cache_miss_notices {
            return;
        }
        let detail = match reason {
            KvCacheMissReason::HarnessSystemChanged => "system prompt changed mid-session",
            KvCacheMissReason::HarnessToolsChanged => "tool set changed mid-session",
            KvCacheMissReason::HarnessPrefixChanged => "an earlier message was modified",
            // Not harness-caused: provider/model/upstream switch, TTL expiry,
            // and the soft zero/low-read diagnostics. Skip the alarm for these.
            _ => return,
        };

        let token_label = if missed_tokens >= 1_000_000 {
            format!("{:.1}M", missed_tokens as f64 / 1_000_000.0)
        } else if missed_tokens >= 1_000 {
            format!("{}K", missed_tokens / 1_000)
        } else {
            missed_tokens.to_string()
        };

        // Documented invalidation between the baseline and now: expected
        // resend, attribute instead of alarm.
        if let Some(cause) = crate::cache_invalidation::most_recent_since(baseline_completed_at) {
            self.push_display_message(DisplayMessage::system(format!(
                "ℹ️ KV cache refresh [{}] turn {}: ~{} tokens resent ({}).",
                cause.source, turn_number, token_label, detail,
            )));
            return;
        }

        self.push_display_message(DisplayMessage::system(format!(
            "⚠️ KV cache miss [{}] turn {}: ~{} tokens resent ({}). See KV_CACHE_USAGE in logs.",
            reason.label(),
            turn_number,
            token_label,
            detail,
        )));
    }

    fn classify_kv_cache_miss_reason(
        &self,
        request: &PendingKvCacheRequest,
        baseline: &KvCacheBaseline,
        read_tokens: u64,
        optimal_pct: u8,
    ) -> KvCacheMissReason {
        if baseline.provider != request.provider {
            return KvCacheMissReason::ProviderSwitch;
        }
        if baseline.model != request.model {
            return KvCacheMissReason::ModelSwitch;
        }
        if baseline.upstream_provider.is_some()
            && request.upstream_provider.is_some()
            && baseline.upstream_provider != request.upstream_provider
        {
            return KvCacheMissReason::UpstreamSwitch;
        }

        if let Some(ttl_secs) =
            crate::tui::cache_ttl_for_provider_model(&baseline.provider, Some(&baseline.model))
            && baseline.completed_at.elapsed() >= Duration::from_secs(ttl_secs)
        {
            return KvCacheMissReason::Expired;
        }

        if let (Some(previous), Some(current)) = (&baseline.signature, &request.signature) {
            if previous.system_static_hash != current.system_static_hash {
                return KvCacheMissReason::HarnessSystemChanged;
            }
            if previous.tools_hash != current.tools_hash
                || previous.tool_count != current.tool_count
            {
                return KvCacheMissReason::HarnessToolsChanged;
            }
        }

        if request.baseline_messages_prefix_matches == Some(false) {
            return KvCacheMissReason::HarnessPrefixChanged;
        }

        if self.streaming.streaming_cache_read_tokens.is_none() {
            return KvCacheMissReason::Unknown;
        }
        if read_tokens == 0 {
            return KvCacheMissReason::ZeroRead;
        }
        if optimal_pct < Self::KV_CACHE_OPTIMAL_OK_PCT {
            return KvCacheMissReason::LowRead;
        }
        KvCacheMissReason::Unknown
    }

    fn kv_cache_provider_name(&self) -> String {
        if self.uses_server_or_replay_metadata() {
            self.remote_provider_name
                .clone()
                .unwrap_or_else(|| self.provider.name().to_string())
        } else {
            self.provider.name().to_string()
        }
    }

    fn kv_cache_provider_model(&self) -> String {
        if self.uses_server_or_replay_metadata() {
            self.remote_provider_model
                .clone()
                .unwrap_or_else(|| self.provider.model())
        } else {
            self.provider.model()
        }
    }

    fn kv_cache_request_signature(
        messages: &[Message],
        tools: &[ToolDefinition],
        system_static: &str,
        system_dynamic: &str,
    ) -> KvCacheRequestSignature {
        let dynamic_trimmed = system_dynamic.trim();
        KvCacheRequestSignature {
            system_static_hash: stable_hash_str(system_static),
            tools_hash: stable_hash_json(tools),
            messages_hash: stable_hash_json(&cache_relevant_messages(messages)),
            message_hashes: message_hashes(messages),
            message_count: messages.len(),
            tool_count: tools.len(),
            system_static_chars: system_static.chars().count(),
            tools_json_chars: stable_json_len(tools),
            messages_json_chars: stable_json_len(messages),
            ephemeral_hash: if dynamic_trimmed.is_empty() {
                None
            } else {
                Some(stable_hash_str(dynamic_trimmed))
            },
            ephemeral_chars: dynamic_trimmed.chars().count(),
            ephemeral_message_count: usize::from(!dynamic_trimmed.is_empty()),
        }
    }

    fn kv_cache_signatures_prefix_match(
        current: &KvCacheRequestSignature,
        previous: &KvCacheRequestSignature,
    ) -> bool {
        if previous.message_count > current.message_count {
            return false;
        }
        if !previous.message_hashes.is_empty() && !current.message_hashes.is_empty() {
            return current.message_hashes.len() >= previous.message_hashes.len()
                && current.message_hashes[..previous.message_hashes.len()]
                    == previous.message_hashes;
        }
        if previous.message_count == current.message_count {
            current.messages_hash == previous.messages_hash
        } else {
            false
        }
    }

    fn kv_cache_common_prefix_messages(
        current: &KvCacheRequestSignature,
        previous: &KvCacheRequestSignature,
    ) -> usize {
        current
            .message_hashes
            .iter()
            .zip(previous.message_hashes.iter())
            .take_while(|(current, previous)| current == previous)
            .count()
    }
}

fn stable_hash_str(value: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn stable_hash_json<T: serde::Serialize + ?Sized>(value: &T) -> u64 {
    let encoded = serde_json::to_string(value).unwrap_or_default();
    stable_hash_str(&encoded)
}

fn stable_json_len<T: serde::Serialize + ?Sized>(value: &T) -> usize {
    serde_json::to_string(value)
        .map(|encoded| encoded.len())
        .unwrap_or_default()
}

// The cache-relevant projection lives in `jcode-message-types` (re-exported
// through `crate::message`) so this local path and the server event path in
// `jcode-app-core::agent::kv_cache_request_event` hash messages identically.
// If the two projections drift, remote sessions report false
// `harness:_prefix_changed` KV-cache misses.
use crate::message::{cache_relevant_message_value, cache_relevant_messages};

fn message_hashes(messages: &[Message]) -> Vec<u64> {
    messages
        .iter()
        .map(|message| stable_hash_json(&cache_relevant_message_value(message)))
        .collect()
}

fn ratio_pct(numerator: u64, denominator: u64) -> u8 {
    if denominator == 0 {
        0
    } else {
        ((numerator as f32 / denominator as f32) * 100.0)
            .round()
            .clamp(0.0, 100.0) as u8
    }
}

#[cfg(test)]
mod tests;
