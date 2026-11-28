use super::*;

#[derive(Debug, Deserialize)]
pub(super) struct SessionStartupStub {
    pub(super) id: String,
    #[serde(default)]
    pub(super) parent_id: Option<String>,
    #[serde(default)]
    pub(super) title: Option<String>,
    #[serde(default)]
    pub(super) custom_title: Option<String>,
    #[serde(default)]
    pub(super) system_prompt: Option<String>,
    pub(super) created_at: DateTime<Utc>,
    pub(super) updated_at: DateTime<Utc>,
    #[serde(default)]
    pub(super) compaction: Option<std::sync::Arc<StoredCompactionState>>,
    #[serde(default)]
    pub(super) provider_session_id: Option<String>,
    #[serde(default)]
    pub(super) provider_key: Option<String>,
    #[serde(default)]
    pub(super) model: Option<String>,
    #[serde(default)]
    pub(super) route_api_method: Option<String>,
    #[serde(default)]
    pub(super) account_pins: BTreeMap<String, jcode_provider_core::AccountPin>,
    #[serde(default)]
    pub(super) reasoning_effort: Option<String>,
    #[serde(default)]
    pub(super) context_cap: Option<usize>,
    #[serde(default)]
    pub(super) subagent_model: Option<String>,
    #[serde(default)]
    pub(super) agent_model_overrides: AgentModelOverrides,
    #[serde(default)]
    pub(super) improve_mode: Option<SessionImproveMode>,
    #[serde(default)]
    pub(super) autoreview_enabled: Option<bool>,
    #[serde(default)]
    pub(super) autojudge_enabled: Option<bool>,
    #[serde(default)]
    pub(super) is_canary: bool,
    #[serde(default)]
    pub(super) testing_build: Option<String>,
    #[serde(default)]
    pub(super) working_dir: Option<String>,
    #[serde(default)]
    pub(super) original_working_dir: Option<String>,
    #[serde(default)]
    pub(super) short_name: Option<String>,
    #[serde(default)]
    pub(super) status: SessionStatus,
    #[serde(default)]
    pub(super) last_pid: Option<u32>,
    #[serde(default)]
    pub(super) last_active_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub(super) is_debug: bool,
    #[serde(default)]
    pub(super) saved: bool,
    #[serde(default)]
    pub(super) save_label: Option<String>,
    #[serde(default)]
    pub(super) migration_epoch: u64,
}

#[derive(Debug, Deserialize)]
pub(super) struct RemoteStartupSessionSnapshot {
    pub(super) id: String,
    #[serde(default)]
    pub(super) parent_id: Option<String>,
    #[serde(default)]
    pub(super) title: Option<String>,
    #[serde(default)]
    pub(super) custom_title: Option<String>,
    #[serde(default)]
    pub(super) system_prompt: Option<String>,
    pub(super) created_at: DateTime<Utc>,
    pub(super) updated_at: DateTime<Utc>,
    #[serde(default, deserialize_with = "parallel_messages::deserialize")]
    pub(super) messages: Vec<StoredMessage>,
    #[serde(default)]
    pub(super) compaction: Option<std::sync::Arc<StoredCompactionState>>,
    #[serde(default)]
    pub(super) provider_session_id: Option<String>,
    #[serde(default)]
    pub(super) provider_key: Option<String>,
    #[serde(default)]
    pub(super) model: Option<String>,
    #[serde(default)]
    pub(super) route_api_method: Option<String>,
    #[serde(default)]
    pub(super) account_pins: BTreeMap<String, jcode_provider_core::AccountPin>,
    #[serde(default)]
    pub(super) reasoning_effort: Option<String>,
    #[serde(default)]
    pub(super) context_cap: Option<usize>,
    #[serde(default)]
    pub(super) subagent_model: Option<String>,
    #[serde(default)]
    pub(super) agent_model_overrides: AgentModelOverrides,
    #[serde(default)]
    pub(super) improve_mode: Option<SessionImproveMode>,
    #[serde(default)]
    pub(super) autoreview_enabled: Option<bool>,
    #[serde(default)]
    pub(super) autojudge_enabled: Option<bool>,
    #[serde(default)]
    pub(super) is_canary: bool,
    #[serde(default)]
    pub(super) testing_build: Option<String>,
    #[serde(default)]
    pub(super) working_dir: Option<String>,
    #[serde(default)]
    pub(super) original_working_dir: Option<String>,
    #[serde(default)]
    pub(super) short_name: Option<String>,
    #[serde(default)]
    pub(super) status: SessionStatus,
    #[serde(default)]
    pub(super) last_pid: Option<u32>,
    #[serde(default)]
    pub(super) last_active_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub(super) is_debug: bool,
    #[serde(default)]
    pub(super) saved: bool,
    #[serde(default)]
    pub(super) save_label: Option<String>,
}
