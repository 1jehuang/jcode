use super::{SwarmMember, SwarmTaskProgress, VersionedPlan};
use crate::protocol::ServerEvent;
use crate::storage;
use jcode_swarm_core::{SwarmLifecycleStatus, SwarmMemberRecord};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use tokio::sync::mpsc;

const SWARM_STATE_DIR: &str = "jcode-swarm-state";

pub(super) struct LoadedSwarmRuntimeState {
    pub plans: HashMap<String, VersionedPlan>,
    pub coordinators: HashMap<String, String>,
    pub members: HashMap<String, SwarmMember>,
    pub swarms_by_id: HashMap<String, HashSet<String>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PersistedSwarmState {
    swarm_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plan: Option<PersistedVersionedPlan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    coordinator_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    members: Vec<PersistedSwarmMember>,
    updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PersistedVersionedPlan {
    items: Vec<crate::plan::PlanItem>,
    version: u64,
    participants: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    task_progress: HashMap<String, SwarmTaskProgress>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PersistedSwarmMember {
    #[serde(flatten)]
    record: SwarmMemberRecord,
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn state_dir() -> PathBuf {
    storage::runtime_dir().join(SWARM_STATE_DIR)
}

fn state_path(swarm_id: &str) -> PathBuf {
    let sanitized: String = swarm_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    state_dir().join(format!("{}.json", sanitized))
}

fn from_persisted_plan(mut plan: PersistedVersionedPlan, updated_at_unix_ms: u64) -> VersionedPlan {
    for item in &mut plan.items {
        if item.status == "running" {
            item.status = "running_stale".to_string();
            plan.task_progress
                .entry(item.id.clone())
                .or_default()
                .stale_since_unix_ms
                .get_or_insert(updated_at_unix_ms);
        }
    }
    VersionedPlan {
        items: plan.items,
        version: plan.version,
        participants: plan.participants.into_iter().collect(),
        task_progress: plan.task_progress,
    }
}

fn to_persisted_plan(plan: &VersionedPlan) -> PersistedVersionedPlan {
    let mut participants: Vec<String> = plan.participants.iter().cloned().collect();
    participants.sort();
    PersistedVersionedPlan {
        items: plan.items.clone(),
        version: plan.version,
        participants,
        task_progress: plan.task_progress.clone(),
    }
}

fn to_persisted_member(member: &SwarmMember) -> PersistedSwarmMember {
    PersistedSwarmMember {
        record: member.durable_record(),
    }
}

fn append_recovery_detail(detail: Option<String>, note: &str) -> Option<String> {
    match detail {
        Some(existing) if !existing.trim().is_empty() => Some(format!("{} ({})", existing, note)),
        _ => Some(note.to_string()),
    }
}

fn recover_member_status(
    status: SwarmLifecycleStatus,
    detail: Option<String>,
    is_headless: bool,
) -> (SwarmLifecycleStatus, Option<String>) {
    if status == SwarmLifecycleStatus::Running {
        return (
            SwarmLifecycleStatus::Crashed,
            append_recovery_detail(detail, "recovered after reload while running"),
        );
    }

    if is_headless
        && !matches!(
            status,
            SwarmLifecycleStatus::Completed
                | SwarmLifecycleStatus::Failed
                | SwarmLifecycleStatus::Stopped
        )
    {
        return (
            SwarmLifecycleStatus::Crashed,
            append_recovery_detail(detail, "headless session did not survive reload"),
        );
    }

    (status, detail)
}

fn recovered_member_event_tx() -> mpsc::UnboundedSender<ServerEvent> {
    let (tx, rx) = mpsc::unbounded_channel();
    drop(rx);
    tx
}

fn from_persisted_member(member: PersistedSwarmMember) -> SwarmMember {
    let record = member.record;
    let (status, detail) = recover_member_status(record.status, record.detail, record.is_headless);
    SwarmMember::from_record(
        SwarmMemberRecord {
            status,
            detail,
            ..record
        },
        recovered_member_event_tx(),
    )
}

pub(super) fn load_runtime_state() -> LoadedSwarmRuntimeState {
    let dir = state_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return LoadedSwarmRuntimeState {
            plans: HashMap::new(),
            coordinators: HashMap::new(),
            members: HashMap::new(),
            swarms_by_id: HashMap::new(),
        };
    };

    let mut plans = HashMap::new();
    let mut coordinators = HashMap::new();
    let mut members = HashMap::new();
    let mut swarms_by_id = HashMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Ok(state) = storage::read_json::<PersistedSwarmState>(&path) else {
            continue;
        };
        let swarm_id = state.swarm_id.clone();
        if let Some(plan) = state.plan {
            plans.insert(
                swarm_id.clone(),
                from_persisted_plan(plan, state.updated_at_unix_ms),
            );
        }
        if let Some(coordinator_session_id) = state.coordinator_session_id {
            coordinators.insert(swarm_id, coordinator_session_id);
        }
        for member in state.members {
            let Some(member_swarm_id) = member.record.swarm_id.clone() else {
                continue;
            };
            swarms_by_id
                .entry(member_swarm_id.clone())
                .or_insert_with(HashSet::new)
                .insert(member.record.session_id.clone());
            members.insert(
                member.record.session_id.clone(),
                from_persisted_member(member),
            );
        }
    }
    LoadedSwarmRuntimeState {
        plans,
        coordinators,
        members,
        swarms_by_id,
    }
}

pub(super) fn persist_swarm_state(
    swarm_id: &str,
    swarm_plan: Option<&VersionedPlan>,
    coordinator_session_id: Option<&str>,
    swarm_members: &[SwarmMember],
) {
    if swarm_plan.is_none() && coordinator_session_id.is_none() && swarm_members.is_empty() {
        let _ = std::fs::remove_file(state_path(swarm_id));
        return;
    }

    let mut members = swarm_members
        .iter()
        .map(to_persisted_member)
        .collect::<Vec<_>>();
    members.sort_by(|left, right| left.record.session_id.cmp(&right.record.session_id));

    let state = PersistedSwarmState {
        swarm_id: swarm_id.to_string(),
        plan: swarm_plan.map(to_persisted_plan),
        coordinator_session_id: coordinator_session_id.map(str::to_string),
        members,
        updated_at_unix_ms: now_unix_ms(),
    };

    if let Err(err) = storage::write_json_fast(&state_path(swarm_id), &state) {
        crate::logging::warn(&format!(
            "Failed to persist swarm state {}: {}",
            swarm_id, err
        ));
    }
}

pub(super) fn remove_swarm_state(swarm_id: &str) {
    let _ = std::fs::remove_file(state_path(swarm_id));
}

#[cfg(test)]
#[path = "swarm_persistence_tests.rs"]
mod swarm_persistence_tests;
