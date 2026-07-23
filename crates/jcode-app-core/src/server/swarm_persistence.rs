use super::{SwarmMember, SwarmTaskProgress, VersionedPlan};
use crate::protocol::ServerEvent;
use crate::storage;
use jcode_swarm_core::{SwarmLifecycleStatus, SwarmMemberRecord};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex as StdMutex, Weak};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Directory name under the durable state dir (`~/.jcode/state`).
const SWARM_STATE_DIR: &str = "swarm";
/// Pre-0.36 location under the runtime dir (tmpfs on Linux, wiped on reboot).
const LEGACY_SWARM_STATE_DIR: &str = "jcode-swarm-state";
/// Dormant plans are durable for recovery, but not immortal. A plan with no
/// active/assigned work that has not had *any* swarm snapshot activity for a
/// week is stale coordination state and is removed during startup loading.
const DEFAULT_DORMANT_PLAN_RETENTION_SECS: u64 = 7 * 24 * 60 * 60;

/// Serialize each swarm's complete snapshot/read/write operation. Callers must
/// acquire this before reading the independently locked in-memory maps so an
/// older snapshot cannot finish after a newer one.
static SWARM_OPERATION_LOCKS: LazyLock<StdMutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

/// Protect primary/backup comparisons and filesystem updates, including tests
/// and recovery paths that invoke the synchronous persistence helpers directly.
static SWARM_FILE_LOCKS: LazyLock<StdMutex<HashMap<String, Weak<StdMutex<()>>>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SwarmStateFileVersion(Option<Vec<u8>>);

pub(super) fn swarm_operation_lock(swarm_id: &str) -> Arc<tokio::sync::Mutex<()>> {
    let mut locks = SWARM_OPERATION_LOCKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(swarm_id).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    locks.insert(swarm_id.to_string(), Arc::downgrade(&lock));
    lock
}

fn swarm_file_lock(swarm_id: &str) -> Arc<StdMutex<()>> {
    let mut locks = SWARM_FILE_LOCKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(swarm_id).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(StdMutex::new(()));
    locks.insert(swarm_id.to_string(), Arc::downgrade(&lock));
    lock
}

fn dormant_plan_retention() -> Duration {
    Duration::from_secs(
        std::env::var("JCODE_SWARM_DORMANT_PLAN_RETENTION_SECS")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_DORMANT_PLAN_RETENTION_SECS),
    )
}

fn persisted_plan_is_dormant(plan: &PersistedVersionedPlan) -> bool {
    plan.items.is_empty()
        || plan
            .items
            .iter()
            .all(|item| item.assigned_to.is_none() && !jcode_plan::is_active_status(&item.status))
}

fn persisted_plan_is_expired(
    plan: &PersistedVersionedPlan,
    snapshot_updated_at_unix_ms: u64,
    loaded_at_unix_ms: u64,
    retention: Duration,
) -> bool {
    if plan.items.is_empty() {
        return true;
    }
    persisted_plan_is_dormant(plan)
        && loaded_at_unix_ms.saturating_sub(snapshot_updated_at_unix_ms)
            >= retention.as_millis() as u64
}

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
    #[serde(default = "default_plan_mode", skip_serializing_if = "is_light_mode")]
    mode: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    node_meta: HashMap<String, crate::plan::NodeMeta>,
}

fn default_plan_mode() -> String {
    "light".to_string()
}

fn is_light_mode(mode: &str) -> bool {
    mode == "light"
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PersistedSwarmMember {
    #[serde(flatten)]
    record: SwarmMemberRecord,
    /// Wall-clock time when the member entered its current terminal status.
    /// Legacy snapshots omit this; their snapshot timestamp becomes the
    /// conservative migration fallback so reports are not discarded eagerly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    terminal_since_unix_ms: Option<u64>,
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(not(test))]
fn state_dir() -> PathBuf {
    storage::durable_state_dir().join(SWARM_STATE_DIR)
}

/// Unit tests that exercise high-level swarm mutation helpers do not all set
/// `JCODE_RUNTIME_DIR`. Never let those tests fall through to the real
/// `~/.jcode/state/swarm`: that leaked synthetic `swarm-1` plans/members into
/// live user state during ordinary `cargo test` runs. Tests that need an
/// isolated explicit location still set `JCODE_RUNTIME_DIR` and use the normal
/// resolver; otherwise use a process-local temp directory.
#[cfg(test)]
fn state_dir() -> PathBuf {
    if std::env::var_os("JCODE_RUNTIME_DIR").is_some() {
        storage::durable_state_dir().join(SWARM_STATE_DIR)
    } else {
        std::env::temp_dir()
            .join(format!("jcode-test-state-{}", std::process::id()))
            .join(SWARM_STATE_DIR)
    }
}

fn legacy_state_dir() -> PathBuf {
    storage::runtime_dir().join(LEGACY_SWARM_STATE_DIR)
}

/// One-time migration from the legacy runtime-dir location (tmpfs, wiped on
/// reboot) to the durable state dir. Copies legacy snapshots only when the
/// new dir has none, so an already-migrated dir is never clobbered.
fn migrate_legacy_state() {
    let new_dir = state_dir();
    let has_new_state = std::fs::read_dir(&new_dir)
        .map(|entries| {
            entries
                .flatten()
                .any(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        })
        .unwrap_or(false);
    if has_new_state {
        return;
    }

    let legacy_dir = legacy_state_dir();
    let Ok(entries) = std::fs::read_dir(&legacy_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let Some(file_name) = path.file_name() else {
            continue;
        };
        if let Err(err) = storage::ensure_dir(&new_dir) {
            crate::logging::warn(&format!(
                "Failed to create swarm state dir {}: {}",
                new_dir.display(),
                err
            ));
            return;
        }
        if let Err(err) = std::fs::copy(&path, new_dir.join(file_name)) {
            crate::logging::warn(&format!(
                "Failed to migrate legacy swarm state {}: {}",
                path.display(),
                err
            ));
        }
    }
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

fn read_primary_version(swarm_id: &str) -> SwarmStateFileVersion {
    SwarmStateFileVersion(std::fs::read(state_path(swarm_id)).ok())
}

pub(super) fn capture_swarm_state_version(swarm_id: &str) -> SwarmStateFileVersion {
    let file_lock = swarm_file_lock(swarm_id);
    let _guard = file_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    read_primary_version(swarm_id)
}

fn remove_snapshot_files(swarm_id: &str) -> bool {
    let path = state_path(swarm_id);
    // First atomically replace the primary with an empty tombstone. The write
    // may rotate the old primary to `.bak`, but load_runtime_state ignores that
    // backup while the tombstone exists. Thus every crash point is safe: before
    // rename the deletion did not happen, and after rename the old state is
    // already logically invalid even if physical cleanup is interrupted.
    let tombstone = PersistedSwarmState {
        swarm_id: swarm_id.to_string(),
        plan: None,
        coordinator_session_id: None,
        members: Vec::new(),
        updated_at_unix_ms: now_unix_ms(),
    };
    if let Err(err) = storage::write_json_fast(&path, &tombstone) {
        crate::logging::warn(&format!(
            "Failed to tombstone swarm state {}: {}",
            path.display(),
            err
        ));
        return false;
    }

    let mut removed = true;
    for candidate in [path.with_extension("bak"), path] {
        if let Err(err) = std::fs::remove_file(&candidate)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            removed = false;
            crate::logging::warn(&format!(
                "Failed to remove swarm state {}: {}",
                candidate.display(),
                err
            ));
        }
    }
    removed
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
    let mut plan = VersionedPlan {
        items: plan.items,
        version: plan.version,
        participants: plan.participants.into_iter().collect(),
        task_progress: plan.task_progress,
        mode: plan.mode,
        node_meta: plan.node_meta,
    };
    plan.prune_side_maps();
    plan
}

fn to_persisted_plan(plan: &VersionedPlan) -> PersistedVersionedPlan {
    let mut participants: Vec<String> = plan.participants.iter().cloned().collect();
    participants.sort();
    PersistedVersionedPlan {
        items: plan.items.clone(),
        version: plan.version,
        participants,
        task_progress: plan.task_progress.clone(),
        mode: plan.mode.clone(),
        node_meta: plan.node_meta.clone(),
    }
}

fn to_persisted_member(member: &SwarmMember, snapshot_unix_ms: u64) -> PersistedSwarmMember {
    let terminal_since_unix_ms =
        super::swarm::member_status_is_terminal(&member.status).then(|| {
            snapshot_unix_ms.saturating_sub(member.last_status_change.elapsed().as_millis() as u64)
        });
    PersistedSwarmMember {
        record: member.durable_record(),
        terminal_since_unix_ms,
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

    // No client or headless process survives a server restart. A connected TUI
    // will explicitly mark its member ready again during subscribe; until that
    // happens, restoring a persisted `ready` member as live creates a ghost
    // that can never enter terminal-member GC. This previously resurrected
    // hundreds of detached historical clients as ready on every reload.
    if status == SwarmLifecycleStatus::Ready {
        return (
            SwarmLifecycleStatus::Stopped,
            append_recovery_detail(
                detail,
                if is_headless {
                    "idle worker not restored after server restart"
                } else {
                    "client not attached after server restart"
                },
            ),
        );
    }

    // Done headless members finished their work before the reload. Nothing
    // in-flight was lost and their completion report remains available.
    if is_headless
        && !matches!(
            status,
            SwarmLifecycleStatus::Completed
                | SwarmLifecycleStatus::Done
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

fn from_persisted_member(
    member: PersistedSwarmMember,
    snapshot_updated_at_unix_ms: u64,
    loaded_at_unix_ms: u64,
    terminal_retention: Duration,
) -> Option<SwarmMember> {
    let record = member.record;
    let original_status = record.status.as_str();
    let was_terminal_before_recovery =
        super::swarm::member_status_is_terminal(original_status.as_ref());
    let (status, detail) = recover_member_status(record.status, record.detail, record.is_headless);
    let status_text = status.as_str();
    let terminal_since_unix_ms = super::swarm::member_status_is_terminal(status_text.as_ref())
        .then(|| {
            member
                .terminal_since_unix_ms
                .unwrap_or(if was_terminal_before_recovery {
                    snapshot_updated_at_unix_ms
                } else {
                    loaded_at_unix_ms
                })
        });
    if terminal_since_unix_ms.is_some_and(|terminal_since| {
        loaded_at_unix_ms.saturating_sub(terminal_since) >= terminal_retention.as_millis() as u64
    }) {
        return None;
    }

    let mut recovered = SwarmMember::from_record(
        SwarmMemberRecord {
            status,
            detail,
            ..record
        },
        recovered_member_event_tx(),
    );
    if let Some(terminal_since) = terminal_since_unix_ms {
        let terminal_age = Duration::from_millis(loaded_at_unix_ms.saturating_sub(terminal_since));
        recovered.last_status_change = Instant::now()
            .checked_sub(terminal_age)
            .unwrap_or_else(Instant::now);
    }
    Some(recovered)
}

pub(super) fn load_runtime_state() -> LoadedSwarmRuntimeState {
    migrate_legacy_state();
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
    let loaded_at_unix_ms = now_unix_ms();
    let terminal_retention = super::swarm::swarm_terminal_member_retention();
    let plan_retention = dormant_plan_retention();
    let mut pruned_terminal_members = 0usize;
    let mut pruned_dormant_plans = 0usize;
    let mut pruned_members_by_swarm: HashMap<String, HashSet<String>> = HashMap::new();
    let mut pruned_plan_swarms: HashSet<String> = HashSet::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        // `.bak` files are corruption-recovery fallbacks, not co-equal
        // snapshots. When the primary `.json` still exists, reading the
        // `.bak` alongside it can resurrect state the primary deliberately
        // dropped (e.g. a cleared plan: the rotate-on-write keeps the old
        // plan-bearing snapshot as `.bak`, and a union-load would re-insert
        // that plan forever). `read_json` already falls back to the `.bak`
        // internally when the primary is corrupt, so skipping it here loses
        // nothing.
        if path.extension().and_then(|ext| ext.to_str()) == Some("bak")
            && path.with_extension("json").is_file()
        {
            continue;
        }
        let Ok(state) = storage::read_json::<PersistedSwarmState>(&path) else {
            continue;
        };
        let swarm_id = state.swarm_id.clone();
        if let Some(plan) = state.plan {
            if persisted_plan_is_expired(
                &plan,
                state.updated_at_unix_ms,
                loaded_at_unix_ms,
                plan_retention,
            ) {
                pruned_dormant_plans += 1;
                pruned_plan_swarms.insert(swarm_id.clone());
            } else {
                plans.insert(
                    swarm_id.clone(),
                    from_persisted_plan(plan, state.updated_at_unix_ms),
                );
            }
        }
        if let Some(coordinator_session_id) = state.coordinator_session_id {
            coordinators.insert(swarm_id, coordinator_session_id);
        }
        for member in state.members {
            let Some(member_swarm_id) = member.record.swarm_id.clone() else {
                continue;
            };
            let member_session_id = member.record.session_id.clone();
            let Some(member) = from_persisted_member(
                member,
                state.updated_at_unix_ms,
                loaded_at_unix_ms,
                terminal_retention,
            ) else {
                pruned_terminal_members += 1;
                pruned_members_by_swarm
                    .entry(member_swarm_id)
                    .or_default()
                    .insert(member_session_id);
                continue;
            };
            swarms_by_id
                .entry(member_swarm_id.clone())
                .or_insert_with(HashSet::new)
                .insert(member_session_id.clone());
            members.insert(member_session_id, member);
        }
    }
    coordinators.retain(|swarm_id, session_id| {
        !pruned_members_by_swarm
            .get(swarm_id)
            .is_some_and(|pruned| pruned.contains(session_id))
    });
    for (swarm_id, pruned_session_ids) in &pruned_members_by_swarm {
        if let Some(plan) = plans.get_mut(swarm_id) {
            plan.participants
                .retain(|session_id| !pruned_session_ids.contains(session_id));
        }
    }
    // Rewrite every affected snapshot once so startup collection shrinks the
    // durable state too. Without this, the same expired records would be parsed
    // and discarded on every restart forever.
    let rewritten_swarms: HashSet<String> = pruned_members_by_swarm
        .keys()
        .chain(pruned_plan_swarms.iter())
        .cloned()
        .collect();
    for swarm_id in &rewritten_swarms {
        let retained_members = swarms_by_id
            .get(swarm_id)
            .into_iter()
            .flat_map(|session_ids| session_ids.iter())
            .filter_map(|session_id| members.get(session_id).cloned())
            .collect::<Vec<_>>();
        persist_swarm_state(
            swarm_id,
            plans.get(swarm_id),
            coordinators.get(swarm_id).map(String::as_str),
            &retained_members,
        );
    }
    if pruned_terminal_members > 0 {
        crate::logging::info(&format!(
            "Pruned {pruned_terminal_members} expired terminal swarm member(s) while loading durable state"
        ));
    }
    if pruned_dormant_plans > 0 {
        crate::logging::info(&format!(
            "Pruned {pruned_dormant_plans} expired dormant swarm plan(s) while loading durable state"
        ));
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
    let file_lock = swarm_file_lock(swarm_id);
    let _guard = file_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    if swarm_plan.is_none() && coordinator_session_id.is_none() && swarm_members.is_empty() {
        let _ = remove_snapshot_files(swarm_id);
        return;
    }

    // A snapshot can be captured before another task advances the plan and
    // reach disk afterwards. Never let that stale completion regress the
    // durable plan. Full member/coordinator ordering is provided by the
    // per-swarm operation lock around load_runtime + this write.
    if let Some(candidate_plan) = swarm_plan
        && let Ok(current) = storage::read_json::<PersistedSwarmState>(&state_path(swarm_id))
        && current
            .plan
            .as_ref()
            .is_some_and(|plan| plan.version > candidate_plan.version)
    {
        return;
    }

    let snapshot_unix_ms = now_unix_ms();
    let mut members = swarm_members
        .iter()
        .map(|member| to_persisted_member(member, snapshot_unix_ms))
        .collect::<Vec<_>>();
    members.sort_by(|left, right| left.record.session_id.cmp(&right.record.session_id));

    let state = PersistedSwarmState {
        swarm_id: swarm_id.to_string(),
        plan: swarm_plan.map(to_persisted_plan),
        coordinator_session_id: coordinator_session_id.map(str::to_string),
        members,
        updated_at_unix_ms: snapshot_unix_ms,
    };

    if let Err(err) = storage::write_json_fast(&state_path(swarm_id), &state) {
        crate::logging::warn(&format!(
            "Failed to persist swarm state {}: {}",
            swarm_id, err
        ));
    }
}

#[cfg(test)]
pub(super) fn remove_swarm_state(swarm_id: &str) {
    let file_lock = swarm_file_lock(swarm_id);
    let _guard = file_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _ = remove_snapshot_files(swarm_id);
}

pub(super) fn remove_swarm_state_if_version(
    swarm_id: &str,
    expected: &SwarmStateFileVersion,
) -> bool {
    let file_lock = swarm_file_lock(swarm_id);
    let _guard = file_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if &read_primary_version(swarm_id) != expected {
        return false;
    }
    remove_snapshot_files(swarm_id)
}

#[cfg(test)]
#[path = "swarm_persistence_tests.rs"]
mod swarm_persistence_tests;
