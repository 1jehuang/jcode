//! Runtime-backed commands, shared by all non-terminal clients.
use serde::{Deserialize, Serialize};

/// Commands executed by the attached runtime, never submitted as an agent prompt.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCommand {
    Skills,
    Context,
    Info,
    Cache,
    Memory,
    Swarm,
    Agents,
    SubagentModel,
    Autoreview,
    Autojudge,
    Transport,
    Fast,
    Telemetry,
    Subscription,
    Overnight,
    Productivity,
    Initiatives,
    Premium,
    ProviderTestCoverage,
    RefreshModelList,
    Continue,
    ResumeAll,
    Remote,
    ServerReload,
    Review,
    Judge,
    Subagent,
    Auth,
    Logout,
}

/// Structured runtime data plus a human-readable rendering. Data is command
/// specific and additive. Clients must ignore unknown fields.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeCommandOutput {
    pub session_id: String,
    pub command: RuntimeCommand,
    pub data: serde_json::Value,
    pub message: String,
}
