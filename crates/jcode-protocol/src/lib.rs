//! Client-server protocol for jcode
//!
//! Uses newline-delimited JSON over Unix socket.
//! Server streams events back to clients during message processing.
//!
//! Socket types:
//! - Main socket: TUI/client communication with agent
//! - Agent socket: Inter-agent communication (AI-to-AI)

use serde::{Deserialize, Serialize};

mod comm_format;
mod notifications;

pub use comm_format::*;
pub use notifications::{FeatureToggle, NotificationType};

use jcode_batch_types::BatchProgress;
use jcode_message_types::{InputShellResult, ToolCall};
use jcode_plan::{PlanItem, VersionedPlan, next_runnable_item_ids, summarize_plan_graph};
use jcode_side_panel_types::{SidePanelSnapshot, snapshot_is_empty};
use std::collections::BTreeMap;

#[path = "protocol_memory.rs"]
mod memory_snapshots;

pub use memory_snapshots::{
    MemoryActivitySnapshot, MemoryPipelineSnapshot, MemoryStateSnapshot, MemoryStepResultSnapshot,
    MemoryStepStatusSnapshot,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptMode {
    Insert,
    Append,
    Replace,
    #[default]
    Send,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommDeliveryMode {
    Notify,
    Interrupt,
    Wake,
}

/// A message in conversation history (for sync)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_data: Option<ToolCall>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionActivitySnapshot {
    pub is_processing: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool_name: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenUsageTotals {
    pub messages_with_token_usage: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Input tokens from requests where the provider reported cache telemetry.
    /// This may be lower than `input_tokens` for providers or older sessions that
    /// did not expose cache-read/cache-write fields.
    pub cache_reported_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct AuthProviderId(pub String);

impl AuthProviderId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct RuntimeProviderKey(pub String);

impl RuntimeProviderKey {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct CatalogNamespace(pub String);

impl CatalogNamespace {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthCredentialSource {
    ApiKeyFile,
    ProcessEnv,
    OAuthTokenStore,
    ExternalImport,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    TuiPasteApiKey,
    RemoteTuiPasteApiKey,
    CliLogin,
    EnvFilePreseeded,
    ProcessEnvPreseeded,
    OAuthBrowser,
    DeviceCode,
    ExternalImport,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthChanged {
    pub provider: AuthProviderId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_source: Option<AuthCredentialSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_method: Option<AuthMethod>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_runtime: Option<RuntimeProviderKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_catalog_namespace: Option<CatalogNamespace>,
}

impl AuthChanged {
    pub fn new(provider: impl Into<String>) -> Self {
        Self {
            provider: AuthProviderId::new(provider),
            credential_source: None,
            auth_method: None,
            expected_runtime: None,
            expected_catalog_namespace: None,
        }
    }
}

pub type ReloadRecoverySnapshot = jcode_selfdev_types::ReloadRecoveryDirective;

mod wire;
pub use wire::TaskGraphNodeSpec;
pub use wire::{Request, ServerEvent};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallSummary {
    pub tool_name: String,
    pub brief_output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmChannelInfo {
    pub channel: String,
    pub member_count: usize,
}

/// A shared context entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextEntry {
    pub key: String,
    pub value: String,
    pub from_session: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_name: Option<String>,
}

/// Info about an agent
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentInfo {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub friendly_name: Option<String>,
    /// Files this agent has touched
    pub files_touched: Vec<String>,
    /// Current lifecycle status (ready, running, completed, failed, stopped, etc.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Optional status detail (current task, error, etc.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Stable label of the task/role this member was spawned or assigned for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_label: Option<String>,
    /// Role: "agent" or "coordinator"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Whether this member is a headless spawned session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_headless: Option<bool>,
    /// Session that owns report-back/cleanup responsibility for this member.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_back_to_session_id: Option<String>,
    /// Latest structured completion report submitted by this member, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_completion_report: Option<String>,
    /// Number of currently attached live client connections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_attachments: Option<usize>,
    /// Seconds since the last status change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_age_secs: Option<u64>,
    /// Seconds since the last observed activity (token usage, turn start,
    /// tool events, or swarm task heartbeats). Unlike `status_age_secs`,
    /// which measures the last lifecycle transition, this reflects whether
    /// the agent is actually doing work right now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_age_secs: Option<u64>,
    /// Live activity (whether processing + current tool name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<SessionActivitySnapshot>,
    /// Provider name (e.g. "anthropic").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_name: Option<String>,
    /// Provider model id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_model: Option<String>,
    /// Number of turns the agent has run this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_count: Option<u64>,
    /// Tokens churned (total, including cache) within the recent lookback window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recent_total_tokens: Option<u64>,
    /// Output tokens produced within the recent lookback window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recent_output_tokens: Option<u64>,
    /// Width of the recent-token lookback window, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recent_window_secs: Option<u64>,
    /// Cumulative total tokens observed for the session lifetime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cumulative_total_tokens: Option<u64>,
    /// Number of completed todos for this agent's session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub todos_completed: Option<usize>,
    /// Total number of todos for this agent's session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub todos_total: Option<usize>,
}

/// Lightweight status snapshot for a swarm member.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStatusSnapshot {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub friendly_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swarm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_headless: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_attachments: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_age_secs: Option<u64>,
    /// Seconds since the last observed activity (tokens, turns, tool events,
    /// or swarm task heartbeats), independent of lifecycle transitions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_age_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub joined_age_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files_touched: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<SessionActivitySnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_model: Option<String>,
}

/// Lightweight swarm plan graph summary for planner-friendly reads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanGraphStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swarm_id: Option<String>,
    pub version: u64,
    pub item_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ready_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub completed_ids: Vec<String>,
    /// Terminal without completing: failed, stopped, or crashed items. A plan
    /// whose run "finished" with entries here did not finish cleanly, so
    /// schedulers and reports must surface these instead of reading the
    /// terminal state as success.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed_ids: Vec<String>,
    /// Recorded failure reason per failed item id (from the durable task
    /// progress checkpoint, e.g. "task failed: Anthropic API error (401
    /// Unauthorized)"). Lets `plan_status` and schedulers explain *why* a node
    /// failed (and classify waves of credential failures) instead of only
    /// listing failed ids. Only failed items with a recorded reason appear.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub failed_reasons: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cycle_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved_dependency_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next_ready_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub newly_ready_ids: Vec<String>,
    /// Completed (non-gate) items whose artifact self-reported LOW confidence.
    /// Shaky coverage the coordinator should widen with follow-up nodes; deep
    /// gates are also blocked from passing over these while unaddressed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub low_confidence_ids: Vec<String>,
    /// Engine mode for this plan: "deep" (comprehensive, gated, wide fan-out) or
    /// "light" (cheap fan-out). Lets schedulers like `run_plan` pick a
    /// mode-appropriate concurrency policy. Defaults to "light" for legacy plans.
    #[serde(default = "default_plan_mode")]
    pub mode: String,
    /// Growth accounting: nodes from the initial seed (legacy/unknown origins
    /// count as seeded).
    #[serde(default)]
    pub seeded_count: usize,
    /// Growth accounting: machinery-generated nodes (expand children, gate-
    /// injected gaps, and the gates themselves). `seeded_count + grown_count ==
    /// item_count`. A deep plan with `grown_count == 0` never decomposed or
    /// gated anything, which almost always means under-exploration.
    #[serde(default)]
    pub grown_count: usize,
}

fn default_plan_mode() -> String {
    "light".to_string()
}

impl PlanGraphStatus {
    pub fn empty_for_swarm(swarm_id: impl Into<String>) -> Self {
        Self {
            swarm_id: Some(swarm_id.into()),
            version: 0,
            item_count: 0,
            ready_ids: Vec::new(),
            blocked_ids: Vec::new(),
            active_ids: Vec::new(),
            completed_ids: Vec::new(),
            failed_ids: Vec::new(),
            failed_reasons: BTreeMap::new(),
            cycle_ids: Vec::new(),
            unresolved_dependency_ids: Vec::new(),
            next_ready_ids: Vec::new(),
            newly_ready_ids: Vec::new(),
            low_confidence_ids: Vec::new(),
            mode: default_plan_mode(),
            seeded_count: 0,
            grown_count: 0,
        }
    }

    pub fn from_versioned_plan(
        swarm_id: impl Into<String>,
        plan: &VersionedPlan,
        next_ready_limit: Option<usize>,
        newly_ready_ids: Vec<String>,
    ) -> Self {
        let graph = summarize_plan_graph(&plan.items);
        let growth = jcode_plan::bridge::growth_stats(plan);
        let failed_reasons: BTreeMap<String, String> = graph
            .failed_ids
            .iter()
            .filter_map(|id| {
                plan.task_progress
                    .get(id)
                    .and_then(|progress| progress.checkpoint_summary.clone())
                    .map(|reason| (id.clone(), reason))
            })
            .collect();
        Self {
            swarm_id: Some(swarm_id.into()),
            version: plan.version,
            item_count: plan.items.len(),
            ready_ids: graph.ready_ids,
            blocked_ids: graph.blocked_ids,
            active_ids: graph.active_ids,
            completed_ids: graph.completed_ids,
            failed_ids: graph.failed_ids,
            failed_reasons,
            cycle_ids: graph.cycle_ids,
            unresolved_dependency_ids: graph.unresolved_dependency_ids,
            next_ready_ids: next_runnable_item_ids(&plan.items, next_ready_limit),
            newly_ready_ids,
            low_confidence_ids: jcode_plan::bridge::low_confidence_completed_ids(plan),
            mode: plan.mode.clone(),
            seeded_count: growth.seeded,
            grown_count: growth.grown(),
        }
    }
}

/// Swarm member status for lifecycle updates
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SwarmMemberStatus {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub friendly_name: Option<String>,
    /// Lifecycle status (ready, running, completed, failed, stopped, etc.)
    pub status: String,
    /// Optional detail (task, error, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Stable label of the task/role this member was spawned or assigned for.
    /// Unlike `detail`, it is not overwritten by transient status updates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_label: Option<String>,
    /// Role: "agent" or "coordinator"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Whether this member is a headless spawned session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_headless: Option<bool>,
    /// Number of currently attached live client connections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_attachments: Option<usize>,
    /// Seconds since the last status change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_age_secs: Option<u64>,
    /// Recent streamed output tail for live inline rendering (last few lines of
    /// the agent's in-progress assistant text). Only populated for swarm
    /// members when inline streaming taps are active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tail: Option<String>,
    /// Session id this member reports back to (its spawner/parent in the swarm
    /// tree). Walking this chain reconstructs the spawn tree, which lets a
    /// client scope the inline gallery to the subtree it actually spawned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_back_to_session_id: Option<String>,
    /// Todo/plan progress as (completed, total) for this member, when known.
    /// Surfaced on the inline swarm strip as a compact "C/T" counter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub todo_progress: Option<(u32, u32)>,
    /// Compact snapshot of this member's todo list (content + status), capped
    /// by the producer. Rendered in the focused inline swarm panel so the
    /// coordinator can see what each agent is working through.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub todo_items: Vec<SwarmTodoItem>,
    /// Ephemeral runtime metadata used by the live swarm card.
    #[serde(default, skip_serializing_if = "SwarmMemberRuntime::is_empty")]
    pub runtime: SwarmMemberRuntime,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SwarmMemberRuntime {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Human-facing credential route, such as "OAuth" or "API key".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_secs: Option<u64>,
}

impl SwarmMemberRuntime {
    fn is_empty(&self) -> bool {
        self.model.is_none()
            && self.provider.is_none()
            && self.auth_method.is_none()
            && self.effort.is_none()
            && self.elapsed_secs.is_none()
    }
}

/// One compact todo entry crossing the swarm status boundary. Only the
/// display essentials travel; full todo metadata stays in the owning session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SwarmTodoItem {
    pub content: String,
    /// "pending", "in_progress", or "completed".
    pub status: String,
    /// The three most recent tool calls made while this todo was active.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_intents: Vec<SwarmToolIntent>,
}

/// Display-only tool activity nested beneath an active swarm todo.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SwarmToolIntent {
    /// Internal correlation key used by the server to update a running call.
    /// It is intentionally omitted from the wire payload.
    #[serde(default, skip_serializing)]
    pub tool_call_id: String,
    pub tool_name: String,
    pub intent: String,
    /// "running", "completed", or "error".
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<SwarmToolProgress>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SwarmToolProgress {
    pub current: u64,
    pub total: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

/// Status of a member being awaited by comm_await_members
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwaitedMemberStatus {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub friendly_name: Option<String>,
    pub status: String,
    /// Whether this member reached the target status
    pub done: bool,
    /// Latest structured completion report submitted by this member, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_report: Option<String>,
}

impl Request {
    pub fn id(&self) -> u64 {
        match self {
            Request::Message { id, .. } => *id,
            Request::Cancel { id } => *id,
            Request::BackgroundTool { id } => *id,
            Request::SoftInterrupt { id, .. } => *id,
            Request::CancelSoftInterrupts { id } => *id,
            Request::Clear { id } => *id,
            Request::Rewind { id, .. } => *id,
            Request::RewindUndo { id } => *id,
            Request::Ping { id } => *id,
            Request::GetState { id } => *id,
            Request::DebugCommand { id, .. } => *id,
            Request::ClientDebugCommand { id, .. } => *id,
            Request::ClientDebugResponse { id, .. } => *id,
            Request::Subscribe { id, .. } | Request::PrepareDisconnect { id } => *id,
            Request::GetHistory { id } => *id,
            Request::GetModelCatalog { id } => *id,
            Request::GetCompactedHistory { id, .. } => *id,
            Request::Reload { id, .. } => *id,
            Request::ResumeSession { id, .. } => *id,
            Request::ResumeAllSessions { id } => *id,
            Request::NotifySession { id, .. } => *id,
            Request::Transcript { id, .. } => *id,
            Request::InputShell { id, .. } => *id,
            Request::CycleModel { id, .. } => *id,
            Request::RefreshModels { id } => *id,
            Request::SetModel { id, .. } => *id,
            Request::SetRoute { id, .. } => *id,
            Request::SetSubagentModel { id, .. } => *id,
            Request::RunSubagent { id, .. } => *id,
            Request::SetReasoningEffort { id, .. } => *id,
            Request::SetServiceTier { id, .. } => *id,
            Request::SetTransport { id, .. } => *id,
            Request::SetPremiumMode { id, .. } => *id,
            Request::SetFeature { id, .. } => *id,
            Request::SetCompactionMode { id, .. } => *id,
            Request::RenameSession { id, .. } => *id,
            Request::Split { id } => *id,
            Request::Transfer { id } => *id,
            Request::Compact { id } => *id,
            Request::TriggerMemoryExtraction { id } => *id,
            Request::NotifyAuthChanged { id, .. } => *id,
            Request::SwitchAnthropicAccount { id, .. } => *id,
            Request::SwitchOpenAiAccount { id, .. } => *id,
            Request::StdinResponse { id, .. } => *id,
            Request::AgentRegister { id, .. } => *id,
            Request::AgentTask { id, .. } => *id,
            Request::AgentCapabilities { id } => *id,
            Request::AgentContext { id } => *id,
            Request::CommShare { id, .. } => *id,
            Request::CommRead { id, .. } => *id,
            Request::CommMessage { id, .. } => *id,
            Request::CommList { id, .. } => *id,
            Request::CommListChannels { id, .. } => *id,
            Request::CommChannelMembers { id, .. } => *id,
            Request::CommProposePlan { id, .. } => *id,
            Request::CommApprovePlan { id, .. } => *id,
            Request::CommRejectPlan { id, .. } => *id,
            Request::CommSeedGraph { id, .. } => *id,
            Request::CommExpandNode { id, .. } => *id,
            Request::CommCompleteNode { id, .. } => *id,
            Request::CommInjectGap { id, .. } => *id,
            Request::CommSpawn { id, .. } => *id,
            Request::CommListModels { id, .. } => *id,
            Request::CommStop { id, .. } => *id,
            Request::CommAssignRole { id, .. } => *id,
            Request::CommSummary { id, .. } => *id,
            Request::CommStatus { id, .. } => *id,
            Request::CommReport { id, .. } => *id,
            Request::CommReadContext { id, .. } => *id,
            Request::CommResyncPlan { id, .. } => *id,
            Request::CommPlanStatus { id, .. } => *id,
            Request::CommAssignTask { id, .. } => *id,
            Request::CommAssignNext { id, .. } => *id,
            Request::CommTaskControl { id, .. } => *id,
            Request::CommSubscribeChannel { id, .. } => *id,
            Request::CommUnsubscribeChannel { id, .. } => *id,
            Request::CommAwaitMembers { id, .. } => *id,
        }
    }

    pub fn is_lightweight_control_request(&self) -> bool {
        matches!(
            self,
            Request::Ping { .. }
                | Request::CommShare { .. }
                | Request::CommRead { .. }
                | Request::CommMessage { .. }
                | Request::CommList { .. }
                | Request::CommListChannels { .. }
                | Request::CommChannelMembers { .. }
                | Request::CommProposePlan { .. }
                | Request::CommApprovePlan { .. }
                | Request::CommRejectPlan { .. }
                | Request::CommSeedGraph { .. }
                | Request::CommExpandNode { .. }
                | Request::CommCompleteNode { .. }
                | Request::CommInjectGap { .. }
                | Request::CommSpawn { .. }
                | Request::CommListModels { .. }
                | Request::CommStop { .. }
                | Request::CommAssignRole { .. }
                | Request::CommSummary { .. }
                | Request::CommStatus { .. }
                | Request::CommReport { .. }
                | Request::CommPlanStatus { .. }
                | Request::CommReadContext { .. }
                | Request::CommResyncPlan { .. }
                | Request::CommAssignTask { .. }
                | Request::CommAssignNext { .. }
                | Request::CommTaskControl { .. }
                | Request::CommSubscribeChannel { .. }
                | Request::CommUnsubscribeChannel { .. }
                | Request::CommAwaitMembers { .. }
        )
    }
}

fn default_model_direction() -> i8 {
    1
}

/// Encode an event as a newline-terminated JSON string
pub fn encode_event(event: &ServerEvent) -> String {
    let mut json = serde_json::to_string(event).unwrap_or_else(|_| "{}".to_string());
    json.push('\n');
    json
}

/// Decode a request from a JSON string.
///
/// Handles a legacy/desktop compatibility shape where a model switch was sent as
/// `{"type":"set_route","model":"..."}` (a bare model string under the
/// `set_route` tag). The current protocol reserves the `set_route` tag for the
/// structured [`Request::SetRoute`] variant (which carries a `selection`
/// object), so this older shape is normalized into [`Request::SetModel`] here
/// instead of via a serde `alias`. Using an alias would make `SetModel` also
/// claim the `set_route` tag and, because serde dispatches internally-tagged
/// enums by tag rather than by fields, shadow the structured variant entirely
/// (every real route switch would then fail with `missing field \`model\``).
pub fn decode_request(line: &str) -> Result<Request, serde_json::Error> {
    match serde_json::from_str::<Request>(line) {
        Ok(request) => Ok(request),
        Err(error) => {
            if let Some(request) = decode_legacy_set_route_model(line) {
                Ok(request)
            } else {
                Err(error)
            }
        }
    }
}

/// Recognize the legacy `{"type":"set_route","id":N,"model":"..."}` shape and
/// translate it into [`Request::SetModel`]. Returns `None` for anything else
/// (including the current structured `set_route` payload that carries a
/// `selection` object) so the original decode error is surfaced unchanged.
fn decode_legacy_set_route_model(line: &str) -> Option<Request> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let obj = value.as_object()?;
    if obj.get("type")?.as_str()? != "set_route" {
        return None;
    }
    // The structured route switch carries `selection`; never reinterpret it.
    if obj.contains_key("selection") {
        return None;
    }
    let model = obj.get("model")?.as_str()?.to_string();
    let id = obj.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
    Some(Request::SetModel { id, model })
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;
