//! Server-to-client events: replies and streaming.

use serde::{Deserialize, Serialize};

/// Curated event surface. Internally-tagged on `"ev"`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "ev", rename_all = "snake_case")]
pub enum ApiEvent {
    /// Handshake accepted. Sent in reply to `Hello`.
    HelloOk {
        version: u32,
        /// Server name and version, e.g. "jcode/0.55.1".
        server: String,
        /// Optional capability strings for additive feature discovery.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        capabilities: Vec<String>,
    },

    /// Generic success acknowledgment for requests without a richer reply.
    Ok,

    /// Request failed.
    Error { code: ErrorCode, message: String },

    /// Reply to `ListSessions`.
    Sessions { sessions: Vec<SessionInfo> },

    /// Reply to `CreateSession` / `AttachSession`.
    Attached { session: SessionInfo },

    /// Reply to `ForkSession`.
    SessionForked { session: SessionInfo },

    /// Reply to `GetHistory`.
    History {
        session_id: String,
        messages: Vec<HistoryMessage>,
        /// Images anchored to user prompts or tool calls in this transcript.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<RenderedImage>,
    },

    /// Reply to `Ping`.
    Pong,

    // --- Streaming events (carry session_id, not tied to a request id) ---
    /// Assistant text delta.
    TextDelta { session_id: String, text: String },

    /// Model reasoning delta (render dim/italic; safe to ignore).
    ReasoningDelta { session_id: String, text: String },

    /// Reasoning finished for the current step.
    ReasoningDone {
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_secs: Option<f64>,
    },

    /// Tool call streaming lifecycle.
    ToolStart {
        session_id: String,
        call_id: String,
        name: String,
    },
    ToolInputDelta {
        session_id: String,
        call_id: String,
        delta: String,
    },
    ToolExec {
        session_id: String,
        call_id: String,
        name: String,
    },
    ToolDone {
        session_id: String,
        call_id: String,
        name: String,
        output: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    /// Images the model just received from a tool result or image generator.
    /// Clients should render these at their transcript anchor immediately.
    SidePaneImages {
        session_id: String,
        images: Vec<RenderedImage>,
    },

    /// Token usage update for the attached session.
    TokenUsage {
        session_id: String,
        input: u64,
        output: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_read_input: Option<u64>,
    },

    /// The turn finished; the agent is idle.
    TurnDone { session_id: String },

    /// A background task the agent is waiting on reported progress, or
    /// finished.
    ///
    /// The daemon already tracks percent/counts for backgrounded work (a long
    /// build, a test sweep, a swarm plan) and pushes it to its own UI, which
    /// draws a bar. Forwarding it as a typed event means any API client can
    /// draw the same bar instead of leaving the user with a spinner that says
    /// only "still working".
    BackgroundProgress {
        session_id: String,
        /// The `bg` task id, so a client can key one bar per task.
        task_id: String,
        /// Human label for the work, e.g. `bash` or `Model list refresh`.
        label: String,
        /// Completion fraction 0..=100, when the task reports one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        percent: Option<f32>,
        /// One-line status, e.g. `42% · Running tests`.
        summary: String,
        /// The task ended: clients should retire its bar.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        done: bool,
    },

    /// The agent accepted a user message: it is in the session's queue and
    /// will be processed. Sent once per `SendMessage` that the daemon acks.
    ///
    /// Distinct from the request-level `Ok`: `Ok` only says the bridge parsed
    /// the frame, while this says the agent itself has the message. A client
    /// that shows "sent" versus "acknowledged" needs the second fact, and
    /// without it the only proof a message landed is the reply, which can be
    /// minutes away.
    MessageAccepted { session_id: String },

    /// The harness needs a permission decision from the user.
    PermissionRequest {
        session_id: String,
        request_id: String,
        tool_name: String,
        description: String,
    },

    /// Session-level status change (idle, generating, tool_running, ...).
    SessionStatus { session_id: String, status: String },

    /// Provider request lifecycle. The value uses the daemon's stable display
    /// vocabulary, for example `connecting`, `sending request`, `waiting for
    /// response`, `streaming`, or `retrying (2/4)`.
    ///
    /// This is separate from `SessionStatus`: a session can be `generating`
    /// throughout all of these phases, while clients need the finer progress to
    /// avoid looking stuck before the model emits its first token.
    ConnectionPhase { session_id: String, phase: String },

    /// The provider and model serving the attached session.
    ///
    /// Sent unsolicited after attach, and again whenever the model changes, so
    /// a client can show which model it is talking to without polling.
    ModelInfo {
        session_id: String,
        /// Provider name, e.g. `anthropic`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        /// Model id, e.g. `claude-sonnet-4-20250514`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// Reasoning effort, e.g. `high`, for providers that expose it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
    },

    /// Reply to `ListModels`: the models this session can switch to.
    Models {
        session_id: String,
        /// Model ids, in the daemon's preferred order.
        models: Vec<String>,
        /// The model currently serving the session, if known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        current: Option<String>,
    },

    /// Provider/runtime identity and every route the daemon currently exposes.
    RuntimeInfo {
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// Reasoning effort, e.g. `high`, for providers that expose it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
        routes: Vec<ModelRouteInfo>,
    },

    /// An API-key credential was persisted or removed.
    CredentialUpdated { provider: String, configured: bool },

    /// Reply to `ReadFile`.
    FileContent {
        session_id: String,
        path: String,
        content: String,
        size: u64,
        truncated: bool,
    },

    /// Reply to `FindFiles`.
    Files {
        session_id: String,
        paths: Vec<String>,
    },

    /// Reply to `SearchText`.
    TextMatches {
        session_id: String,
        matches: Vec<TextMatch>,
    },

    /// Reply to `FileStatus`.
    FileStatus {
        session_id: String,
        path: String,
        exists: bool,
        kind: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        modified_ms: Option<u64>,
    },

    /// Reply to `Compact`: compaction was scheduled.
    ///
    /// Compaction is not synchronous. The daemon summarizes at the next safe
    /// point rather than interrupting a turn mid-flight, so this confirms the
    /// request was accepted, not that the transcript has already shrunk. A
    /// client that wants the result should re-read the history afterwards.
    Compacted {
        session_id: String,
        /// Human-readable status, e.g. why compaction was refused.
        message: String,
    },

    /// A session's title changed, whether set by a client or generated.
    SessionRenamed {
        session_id: String,
        /// The explicit title, absent when it was cleared.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// What a client should display, generated when no title is set.
        display_title: String,
    },

    /// Forward-compatibility catch-all: clients must skip this silently.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    UnsupportedVersion,
    UnknownRequest,
    UnknownSession,
    InvalidRequest,
    Internal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionInfo {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    /// The effective persisted display title. A custom rename takes precedence
    /// over the generated or imported title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub status: String,
    /// Size of the session's stored record, in bytes.
    ///
    /// A cheap, monotonic proxy for "how much conversation is in here": a
    /// client can size or sort by it without fetching every transcript, which
    /// is the difference between an instant overview and one that stalls on a
    /// dozen history requests. Approximate by design; `None` when the server
    /// could not determine it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_bytes: Option<u64>,
    /// Whether the user pinned/saved this session. Saved sessions sort before
    /// ordinary sessions in every first-party session picker.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub saved: bool,
    /// Persisted transcript update time, used for newest-first ordering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at_ms: Option<i64>,
    /// Most recent active-process timestamp when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_active_at_ms: Option<i64>,
    /// Archived sessions are hidden from the default list but never deleted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelRouteInfo {
    pub model: String,
    pub provider: String,
    pub api_method: String,
    pub available: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TextMatch {
    pub path: String,
    pub line: u32,
    pub column: u32,
    pub preview: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryMessage {
    /// "user" | "assistant" | "tool".
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RenderedImageSource {
    UserInput,
    ToolResult { tool_name: String },
    Other { role: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RenderedImageAnchor {
    ToolCall { id: String },
    UserPrompt { ordinal: usize },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenderedImage {
    pub media_type: String,
    pub data: String,
    pub label: Option<String>,
    pub source: RenderedImageSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<RenderedImageAnchor>,
}
