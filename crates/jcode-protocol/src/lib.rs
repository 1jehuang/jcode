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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Role: "agent", "coordinator", "worktree_manager"
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cycle_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved_dependency_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next_ready_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub newly_ready_ids: Vec<String>,
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
            cycle_ids: Vec::new(),
            unresolved_dependency_ids: Vec::new(),
            next_ready_ids: Vec::new(),
            newly_ready_ids: Vec::new(),
        }
    }

    pub fn from_versioned_plan(
        swarm_id: impl Into<String>,
        plan: &VersionedPlan,
        next_ready_limit: Option<usize>,
        newly_ready_ids: Vec<String>,
    ) -> Self {
        let graph = summarize_plan_graph(&plan.items);
        Self {
            swarm_id: Some(swarm_id.into()),
            version: plan.version,
            item_count: plan.items.len(),
            ready_ids: graph.ready_ids,
            blocked_ids: graph.blocked_ids,
            active_ids: graph.active_ids,
            completed_ids: graph.completed_ids,
            cycle_ids: graph.cycle_ids,
            unresolved_dependency_ids: graph.unresolved_dependency_ids,
            next_ready_ids: next_runnable_item_ids(&plan.items, next_ready_limit),
            newly_ready_ids,
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
    /// Role: "agent", "coordinator", "worktree_manager"
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
            Request::Subscribe { id, .. } => *id,
            Request::GetHistory { id } => *id,
            Request::GetModelCatalog { id } => *id,
            Request::GetCompactedHistory { id, .. } => *id,
            Request::Reload { id, .. } => *id,
            Request::ResumeSession { id, .. } => *id,
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
            Request::CommSpawn { id, .. } => *id,
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
                | Request::CommSpawn { .. }
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

/// Decode a request from a JSON string
pub fn decode_request(line: &str) -> Result<Request, serde_json::Error> {
    serde_json::from_str(line)
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;
