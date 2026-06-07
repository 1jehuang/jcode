use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{
    EnvSnapshot, SessionImproveMode, SessionStatus, StoredCompactionState, StoredMemoryInjection,
    StoredMessage, StoredReplayEvent,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct SessionJournalMeta {
    pub(super) parent_id: Option<String>,
    pub(super) title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) custom_title: Option<String>,
    pub(super) updated_at: DateTime<Utc>,
    pub(super) compaction: Option<StoredCompactionState>,
    pub(super) provider_session_id: Option<String>,
    pub(super) provider_key: Option<String>,
    pub(super) model: Option<String>,
    #[serde(default)]
    pub(super) reasoning_effort: Option<String>,
    pub(super) subagent_model: Option<String>,
    pub(super) improve_mode: Option<SessionImproveMode>,
    pub(super) autoreview_enabled: Option<bool>,
    pub(super) autojudge_enabled: Option<bool>,
    pub(super) is_canary: bool,
    pub(super) testing_build: Option<String>,
    pub(super) working_dir: Option<String>,
    pub(super) short_name: Option<String>,
    pub(super) status: SessionStatus,
    pub(super) last_pid: Option<u32>,
    pub(super) last_active_at: Option<DateTime<Utc>>,
    pub(super) is_debug: bool,
    pub(super) saved: bool,
    pub(super) save_label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct SessionJournalEntry {
    pub(super) meta: SessionJournalMeta,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) append_messages: Vec<StoredMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) append_env_snapshots: Vec<EnvSnapshot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) append_memory_injections: Vec<StoredMemoryInjection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) append_replay_events: Vec<StoredReplayEvent>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum PersistVectorMode {
    #[default]
    Clean,
    Append,
    Full,
}

#[derive(Debug, Clone, Default)]
pub(super) struct SessionPersistState {
    pub(super) snapshot_exists: bool,
    pub(super) messages_len: usize,
    pub(super) env_snapshots_len: usize,
    pub(super) memory_injections_len: usize,
    pub(super) replay_events_len: usize,
    pub(super) messages_mode: PersistVectorMode,
    pub(super) env_snapshots_mode: PersistVectorMode,
    pub(super) memory_injections_mode: PersistVectorMode,
    pub(super) replay_events_mode: PersistVectorMode,
    pub(super) last_meta: Option<SessionJournalMeta>,
}

pub(super) fn metadata_requires_snapshot(
    prev: &SessionJournalMeta,
    current: &SessionJournalMeta,
) -> bool {
    prev.parent_id != current.parent_id
        || prev.title != current.title
        || prev.custom_title != current.custom_title
        || prev.provider_key != current.provider_key
        || prev.reasoning_effort != current.reasoning_effort
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
