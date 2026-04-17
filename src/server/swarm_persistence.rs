use super::state::{SwarmLifecycleStatus, SwarmMemberRecord};
use super::{SwarmMember, SwarmTaskProgress, VersionedPlan};
use crate::protocol::ServerEvent;
use crate::storage;
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
mod tests {
    use super::*;
    use std::time::Instant;

    struct EnvGuard {
        runtime: Option<std::ffi::OsString>,
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(value) = self.runtime.take() {
                crate::env::set_var("JCODE_RUNTIME_DIR", value);
            } else {
                crate::env::remove_var("JCODE_RUNTIME_DIR");
            }
        }
    }

    fn test_env(dir: &tempfile::TempDir) -> EnvGuard {
        let _guard = storage::lock_test_env();
        let previous = std::env::var_os("JCODE_RUNTIME_DIR");
        crate::env::set_var("JCODE_RUNTIME_DIR", dir.path());
        EnvGuard { runtime: previous }
    }

    #[test]
    fn persisted_swarm_state_round_trips_and_marks_running_stale() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let _env = test_env(&dir);

        let mut plans = HashMap::new();
        plans.insert(
            "swarm-alpha".to_string(),
            VersionedPlan {
                items: vec![crate::plan::PlanItem {
                    content: "do thing".to_string(),
                    status: "running".to_string(),
                    priority: "high".to_string(),
                    id: "task-1".to_string(),
                    subsystem: None,
                    file_scope: Vec::new(),
                    blocked_by: Vec::new(),
                    assigned_to: Some("session-1".to_string()),
                }],
                version: 3,
                participants: ["session-1".to_string(), "session-2".to_string()]
                    .into_iter()
                    .collect(),
                task_progress: HashMap::from([(
                    "task-1".to_string(),
                    SwarmTaskProgress {
                        assigned_session_id: Some("session-1".to_string()),
                        assignment_summary: Some("do thing".to_string()),
                        assigned_at_unix_ms: Some(10),
                        started_at_unix_ms: Some(20),
                        last_heartbeat_unix_ms: Some(30),
                        last_detail: Some("tool start: read".to_string()),
                        last_checkpoint_unix_ms: Some(40),
                        checkpoint_summary: Some("tool done: read".to_string()),
                        completed_at_unix_ms: None,
                        stale_since_unix_ms: None,
                        heartbeat_count: Some(2),
                        checkpoint_count: Some(1),
                    },
                )]),
            },
        );
        let coordinators = HashMap::from([("swarm-alpha".to_string(), "session-2".to_string())]);
        let (event_tx, _event_rx) = tokio::sync::mpsc::unbounded_channel();
        let members = vec![SwarmMember {
            session_id: "session-1".to_string(),
            event_tx,
            event_txs: HashMap::new(),
            working_dir: Some(PathBuf::from("/tmp/swarm-alpha")),
            swarm_id: Some("swarm-alpha".to_string()),
            swarm_enabled: true,
            status: "running".to_string(),
            detail: Some("writing tests".to_string()),
            friendly_name: Some("fox".to_string()),
            report_back_to_session_id: Some("session-2".to_string()),
            role: "agent".to_string(),
            joined_at: Instant::now(),
            last_status_change: Instant::now(),
            is_headless: true,
        }];

        persist_swarm_state(
            "swarm-alpha",
            plans.get("swarm-alpha"),
            coordinators.get("swarm-alpha").map(String::as_str),
            &members,
        );
        let loaded = load_runtime_state();

        let loaded_plan = loaded.plans.get("swarm-alpha").expect("loaded plan");
        assert_eq!(loaded_plan.version, 3);
        assert_eq!(loaded_plan.items.len(), 1);
        assert_eq!(loaded_plan.items[0].status, "running_stale");
        let progress = loaded_plan
            .task_progress
            .get("task-1")
            .expect("task progress");
        assert_eq!(progress.assigned_session_id.as_deref(), Some("session-1"));
        assert_eq!(
            progress.checkpoint_summary.as_deref(),
            Some("tool done: read")
        );
        assert!(progress.stale_since_unix_ms.is_some());
        assert_eq!(
            loaded.coordinators.get("swarm-alpha"),
            Some(&"session-2".to_string())
        );
        let recovered_member = loaded.members.get("session-1").expect("recovered member");
        assert_eq!(recovered_member.role, "agent");
        assert_eq!(
            recovered_member.report_back_to_session_id.as_deref(),
            Some("session-2")
        );
        assert_eq!(recovered_member.status, "crashed");
        assert_eq!(
            recovered_member.detail.as_deref(),
            Some("writing tests (recovered after reload while running)")
        );
        assert_eq!(
            loaded.swarms_by_id.get("swarm-alpha"),
            Some(&HashSet::from(["session-1".to_string()]))
        );
    }

    #[test]
    fn remove_swarm_state_deletes_persisted_snapshot() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let _env = test_env(&dir);

        let plans = HashMap::from([(
            "swarm-beta".to_string(),
            VersionedPlan {
                items: Vec::new(),
                version: 1,
                participants: Default::default(),
                task_progress: HashMap::new(),
            },
        )]);
        persist_swarm_state("swarm-beta", plans.get("swarm-beta"), None, &[]);
        assert!(state_path("swarm-beta").exists());

        remove_swarm_state("swarm-beta");
        assert!(!state_path("swarm-beta").exists());
    }

    #[test]
    fn persisted_swarm_state_without_plan_still_restores_coordinator_and_members() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let _env = test_env(&dir);

        let (event_tx, _event_rx) = tokio::sync::mpsc::unbounded_channel();
        let members = vec![SwarmMember {
            session_id: "coord-1".to_string(),
            event_tx,
            event_txs: HashMap::new(),
            working_dir: Some(PathBuf::from("/tmp/swarm-gamma")),
            swarm_id: Some("swarm-gamma".to_string()),
            swarm_enabled: true,
            status: "ready".to_string(),
            detail: None,
            friendly_name: Some("owl".to_string()),
            report_back_to_session_id: None,
            role: "coordinator".to_string(),
            joined_at: Instant::now(),
            last_status_change: Instant::now(),
            is_headless: false,
        }];

        persist_swarm_state("swarm-gamma", None, Some("coord-1"), &members);

        let loaded = load_runtime_state();
        assert!(!loaded.plans.contains_key("swarm-gamma"));
        assert_eq!(
            loaded.coordinators.get("swarm-gamma"),
            Some(&"coord-1".to_string())
        );
        assert_eq!(
            loaded
                .members
                .get("coord-1")
                .and_then(|member| member.friendly_name.as_deref()),
            Some("owl")
        );
        assert_eq!(
            loaded.swarms_by_id.get("swarm-gamma"),
            Some(&HashSet::from(["coord-1".to_string()]))
        );
    }
}
