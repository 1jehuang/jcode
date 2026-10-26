//! Scoped migration control. Preparing a transfer fences durable ownership before
//! releasing the idle reservation. A failed/abandoned transfer is fail-closed:
//! only an explicit abort restores the source leases.
//!
//! Protocol: inspect returns a bundle whose stable transfer_id and expected
//! epochs must be durably journaled before prepare. Prepare accepts selection,
//! destination_host and expected bundle. Reserve precedes destination file
//! installation. Import requires installed sessions at the expected epochs
//! under group-activation-pending leases and does not activate or resume them.
//! The transport commits its receive journal before explicitly clearing those
//! leases. Never abort the source after destination activation was attempted.
use super::swarm_persistence::{self, PersistedSwarmState};
use super::{SessionAgents, SwarmState};
use crate::storage::{self, SessionLease};
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};
use std::sync::LazyLock;

const INBOUND_HOST: &str = "group-activation-pending";
static TRANSFER_OPERATION: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GroupTransferSelection {
    #[serde(default)]
    pub session_ids: Vec<String>,
    #[serde(default)]
    pub swarm_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GroupTransferSession {
    pub session_id: String,
    pub epoch: u64,
    pub previous_lease: Option<SessionLease>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GroupTransferBundle {
    pub version: u32,
    /// Stable identity from inspect, journaled before prepare. Cancellation
    /// tombstones prevent a delayed prepare RPC from fencing an aborted move.
    pub transfer_id: String,
    pub destination_host: String,
    pub sessions: Vec<GroupTransferSession>,
    pub swarms: Vec<PersistedSwarmState>,
}

#[derive(Deserialize)]
struct PrepareRequest {
    #[serde(flatten)]
    selection: GroupTransferSelection,
    destination_host: String,
    expected: GroupTransferBundle,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
}

/// Expand to a fixed point: selecting any swarm member includes its entire
/// swarm, coordinator, plan participants/assignees and report-back owners.
fn expand_selection(
    selection: &GroupTransferSelection,
    swarms: &[PersistedSwarmState],
) -> Result<(BTreeSet<String>, BTreeSet<String>)> {
    let mut sessions: BTreeSet<_> = selection.session_ids.iter().cloned().collect();
    let mut selected_swarms: BTreeSet<_> = selection.swarm_ids.iter().cloned().collect();
    ensure!(
        !sessions.is_empty() || !selected_swarms.is_empty(),
        "empty transfer selection"
    );
    loop {
        let before = (sessions.len(), selected_swarms.len());
        for swarm in swarms {
            let related = swarm_session_ids(swarm);
            if selected_swarms.contains(&swarm.swarm_id) || !sessions.is_disjoint(&related) {
                selected_swarms.insert(swarm.swarm_id.clone());
                sessions.extend(related);
            }
        }
        if before == (sessions.len(), selected_swarms.len()) {
            break;
        }
    }
    for id in &sessions {
        ensure!(valid_id(id), "invalid session id: {id}");
    }
    for id in &selected_swarms {
        ensure!(
            swarms.iter().any(|s| &s.swarm_id == id),
            "unknown swarm: {id}"
        );
    }
    Ok((sessions, selected_swarms))
}

fn swarm_session_ids(swarm: &PersistedSwarmState) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    ids.extend(swarm.coordinator_session_id.iter().cloned());
    for member in &swarm.members {
        ids.insert(member.record.session_id.clone());
        ids.extend(member.record.report_back_to_session_id.iter().cloned());
    }
    if let Some(plan) = &swarm.plan {
        ids.extend(plan.participants.iter().cloned());
        ids.extend(
            plan.items
                .iter()
                .filter_map(|item| item.assigned_to.clone()),
        );
        ids.extend(
            plan.task_progress
                .values()
                .filter_map(|progress| progress.assigned_session_id.clone()),
        );
    }
    ids
}

async fn snapshots(state: &SwarmState) -> Vec<PersistedSwarmState> {
    let mut ids = BTreeSet::new();
    ids.extend(state.swarms_by_id.read().await.keys().cloned());
    ids.extend(state.plans.read().await.keys().cloned());
    ids.extend(state.coordinators.read().await.keys().cloned());
    ids.extend(
        state
            .members
            .read()
            .await
            .values()
            .filter_map(|m| m.swarm_id.clone()),
    );
    let mut result = Vec::new();
    for id in ids {
        result.push(swarm_persistence::transfer_snapshot(
            &state.load_runtime(&id).await,
        ));
    }
    result
}

fn validate_bundle(bundle: &GroupTransferBundle) -> Result<()> {
    ensure!(bundle.version == 1, "unsupported group transfer version");
    ensure!(
        !bundle.transfer_id.is_empty()
            && bundle
                .transfer_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
        "invalid transfer id"
    );
    ensure!(
        !bundle.destination_host.trim().is_empty(),
        "destination host is required"
    );
    ensure!(!bundle.sessions.is_empty(), "empty group transfer");
    let mut ids = BTreeSet::new();
    for session in &bundle.sessions {
        ensure!(valid_id(&session.session_id), "invalid session id");
        ensure!(
            ids.insert(session.session_id.clone()),
            "duplicate session id"
        );
        ensure!(session.epoch > 0, "invalid migration epoch");
        if let Some(previous) = &session.previous_lease {
            ensure!(
                previous.session_id == session.session_id
                    && previous.epoch < session.epoch
                    && previous.is_here(),
                "invalid previous lease"
            );
        }
    }
    let mut swarms = HashSet::new();
    let mut storage_paths = HashSet::new();
    let mut members = HashSet::new();
    for swarm in &bundle.swarms {
        ensure!(
            storage_paths.insert(swarm_persistence::state_path(&swarm.swarm_id)),
            "colliding swarm storage keys"
        );
        ensure!(
            !swarm.swarm_id.is_empty() && swarms.insert(swarm.swarm_id.clone()),
            "invalid or duplicate swarm id"
        );
        ensure!(
            swarm_session_ids(swarm).is_subset(&ids),
            "incomplete swarm transfer: {}",
            swarm.swarm_id
        );
        for member in &swarm.members {
            ensure!(
                member.record.swarm_id.as_deref() == Some(&swarm.swarm_id),
                "member swarm mismatch"
            );
            ensure!(
                members.insert(member.record.session_id.clone()),
                "duplicate swarm member"
            );
        }
    }
    Ok(())
}

/// All state-changing operations use an exclusive global admission reservation.
/// This intentionally rejects even unrelated busy turns rather than guessing
/// whether detached tool callbacks can still mutate the selected state.
pub async fn handle(
    cmd: &str,
    sessions: &SessionAgents,
    state: &SwarmState,
) -> Result<Option<String>> {
    handle_with_gate(cmd, sessions, state, crate::runtime_activity::global()).await
}

async fn handle_with_gate(
    cmd: &str,
    sessions: &SessionAgents,
    state: &SwarmState,
    gate: &crate::runtime_activity::ActivityGate,
) -> Result<Option<String>> {
    let Some(rest) = cmd.strip_prefix("group_transfer:") else {
        return Ok(None);
    };
    // In particular, abort queues behind a prepare that timed out at its
    // caller. The cancellation tombstone also covers reverse arrival order.
    let _operation = TRANSFER_OPERATION.lock().await;
    let (action, json) = rest
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("group_transfer requires action:JSON"))?;
    let _idle = gate.try_idle()
        .ok_or_else(|| anyhow::anyhow!("daemon is busy; wait for all turns and background tasks to finish before group transfer"))?;
    match action {
        "inspect" | "prepare" => {
            let (selection, host, expected) = if action == "prepare" {
                let request: PrepareRequest = serde_json::from_str(json)?;
                ensure!(
                    !request.destination_host.trim().is_empty(),
                    "destination host is required"
                );
                validate_bundle(&request.expected)?;
                ensure!(
                    !cancellation_path(&request.expected).exists(),
                    "transfer was cancelled; inspect and journal a new transfer"
                );
                (
                    request.selection,
                    request.destination_host,
                    Some(request.expected),
                )
            } else {
                (serde_json::from_str(json)?, String::new(), None)
            };
            let all = snapshots(state).await;
            let (ids, swarm_ids) = expand_selection(&selection, &all)?;
            ensure!(
                storage::session_presence()
                    .iter()
                    .all(|presence| !ids.contains(&presence.session_id)
                        || presence.pid == std::process::id()),
                "a selected session is owned by another daemon/process"
            );
            let mut operation_guards = Vec::new();
            for id in &swarm_ids {
                operation_guards.push(super::swarm_operation_lock(id).lock_owned().await);
            }
            let all = snapshots(state).await;
            let (verified_ids, verified_swarms) = expand_selection(&selection, &all)?;
            ensure!(
                ids == verified_ids && swarm_ids == verified_swarms,
                "swarm changed while preparing transfer; retry"
            );
            let mut agents = Vec::new();
            let live = sessions.read().await;
            for id in &ids {
                if let Some(agent) = live.get(id) {
                    agents.push(
                        agent
                            .clone()
                            .try_lock_owned()
                            .map_err(|_| anyhow::anyhow!("session {id} is busy"))?,
                    );
                }
            }
            drop(live);
            if action == "prepare" {
                for agent in &mut agents {
                    agent.ensure_session_lease()?;
                    agent.persist_session_now()?;
                }
            }
            let mut bundle = GroupTransferBundle {
                version: 1,
                transfer_id: expected
                    .as_ref()
                    .map(|bundle| bundle.transfer_id.clone())
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                destination_host: host,
                sessions: Vec::new(),
                swarms: all
                    .into_iter()
                    .filter(|s| swarm_ids.contains(&s.swarm_id))
                    .collect(),
            };
            for id in &ids {
                let session = crate::session::Session::load(id)?;
                ensure!(
                    session.migration_lease_block().is_none(),
                    "session {id} is not owned here"
                );
                let previous_lease = read_transfer_lease(id)?;
                let epoch = session
                    .migration_epoch
                    .max(previous_lease.as_ref().map_or(0, |l| l.epoch))
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("migration epoch exhausted"))?;
                bundle.sessions.push(GroupTransferSession {
                    session_id: id.clone(),
                    epoch,
                    previous_lease,
                });
            }
            if let Some(expected) = expected {
                ensure!(
                    same_preparation(&bundle, &expected)?,
                    "transfer selection, epochs, leases or swarm state changed since inspect; no sessions fenced"
                );
            }
            if action == "prepare" {
                // No fence is written until all reservations and validation pass.
                let mut installed = Vec::new();
                for (index, session) in bundle.sessions.iter().enumerate() {
                    let lease = SessionLease {
                        session_id: session.session_id.clone(),
                        epoch: session.epoch,
                        away_host: Some(bundle.destination_host.clone()),
                        repo_root: session
                            .previous_lease
                            .as_ref()
                            .and_then(|l| l.repo_root.clone()),
                        last_sync_snapshot: session
                            .previous_lease
                            .as_ref()
                            .and_then(|l| l.last_sync_snapshot.clone()),
                        last_sync_head: session
                            .previous_lease
                            .as_ref()
                            .and_then(|l| l.last_sync_head.clone()),
                        updated_at: chrono::Utc::now().to_rfc3339(),
                    };
                    if let Err(error) = storage::write_session_lease(&lease) {
                        // Restore only leases this attempt actually installed.
                        for index in installed {
                            restore_previous(&bundle.sessions[index])?;
                        }
                        return Err(error);
                    }
                    installed.push(index);
                }
                // Evict only transferred agents. Any outstanding Arc remains
                // harmless because its durable away lease denies turns/saves.
                sessions.write().await.retain(|id, _| !ids.contains(id));
                let mut members = state.members.write().await;
                for id in &ids {
                    if let Some(member) = members.get_mut(id) {
                        member.status = "stopped".into();
                        member.detail = Some(format!("migrated to {}", bundle.destination_host));
                    }
                }
                drop(members);
                for id in &swarm_ids {
                    if let Err(error) = swarm_persistence::write_transfer_snapshot(
                        &swarm_persistence::transfer_snapshot(&state.load_runtime(id).await),
                    ) {
                        // The ownership fence, not this advisory stopped status,
                        // is authoritative. Still return the recovery bundle.
                        crate::logging::warn(&format!(
                            "Could not persist migrated swarm status {id}: {error}"
                        ));
                    }
                }
            }
            Ok(Some(serde_json::to_string(&bundle)?))
        }
        "abort" => {
            let bundle: GroupTransferBundle = serde_json::from_str(json)?;
            validate_bundle(&bundle)?;
            for session in &bundle.sessions {
                let current = read_transfer_lease(&session.session_id)?;
                if current != session.previous_lease {
                    verify_source_fence(session, &bundle.destination_host)?;
                }
            }
            storage::write_json_fast(&cancellation_path(&bundle), &bundle)?;
            for session in &bundle.sessions {
                if read_transfer_lease(&session.session_id)? != session.previous_lease {
                    restore_previous(session)?;
                }
            }
            Ok(Some("{\"aborted\":true}".into()))
        }
        "import" => {
            let bundle: GroupTransferBundle = serde_json::from_str(json)?;
            import_bundle(&bundle, sessions, state).await?;
            Ok(Some(serde_json::json!({"imported":bundle.sessions.iter().map(|s| &s.session_id).collect::<Vec<_>>(),"resumed":[]}).to_string()))
        }
        "check_import" => {
            let bundle: GroupTransferBundle = serde_json::from_str(json)?;
            validate_destination(&bundle, sessions, state).await?;
            Ok(Some("{\"ready\":true}".into()))
        }
        "reserve" => {
            let bundle: GroupTransferBundle = serde_json::from_str(json)?;
            validate_destination(&bundle, sessions, state).await?;
            // Validate the whole group before touching any lease. A retry of
            // the same reservation is harmless, but never fence a local owner.
            for session in &bundle.sessions {
                match read_transfer_lease(&session.session_id)? {
                    Some(lease)
                        if lease.away_host.as_deref() == Some(INBOUND_HOST)
                            && lease.epoch == session.epoch => {}
                    Some(lease) => ensure!(
                        !lease.is_here() && lease.epoch < session.epoch,
                        "destination session already owned or newer: {}",
                        session.session_id
                    ),
                    None => ensure!(
                        !crate::session::session_exists(&session.session_id),
                        "destination session already exists without a migration lease: {}",
                        session.session_id
                    ),
                }
            }
            for session in &bundle.sessions {
                let mut lease = transfer_lease(session, Some(INBOUND_HOST.into()));
                if let Some(previous) = read_transfer_lease(&session.session_id)? {
                    lease.repo_root = previous.repo_root;
                    lease.last_sync_snapshot = previous.last_sync_snapshot;
                    lease.last_sync_head = previous.last_sync_head;
                }
                storage::write_session_lease(&lease)?;
            }
            Ok(Some("{\"reserved\":true}".into()))
        }
        _ => bail!("unknown group_transfer action: {action}"),
    }
}

fn verify_source_fence(session: &GroupTransferSession, host: &str) -> Result<()> {
    let lease = read_transfer_lease(&session.session_id)?
        .ok_or_else(|| anyhow::anyhow!("missing transfer fence"))?;
    ensure!(
        lease.epoch == session.epoch && lease.away_host.as_deref() == Some(host),
        "transfer fence changed for {}",
        session.session_id
    );
    Ok(())
}

fn cancellation_path(bundle: &GroupTransferBundle) -> std::path::PathBuf {
    storage::durable_state_dir()
        .join("group-transfers")
        .join(format!("{}.cancelled.json", bundle.transfer_id))
}

fn read_transfer_lease(id: &str) -> Result<Option<SessionLease>> {
    let path = storage::session_leases_dir()
        .ok_or_else(|| anyhow::anyhow!("session lease directory unavailable"))?
        .join(format!("{id}.json"));
    if !path.exists() {
        return Ok(None);
    }
    let lease: SessionLease = storage::read_json(&path)?;
    ensure!(lease.session_id == id, "session lease identity mismatch");
    Ok(Some(lease))
}

fn same_preparation(actual: &GroupTransferBundle, expected: &GroupTransferBundle) -> Result<bool> {
    let normalize = |bundle: &GroupTransferBundle| -> Result<serde_json::Value> {
        let mut bundle = bundle.clone();
        bundle
            .sessions
            .sort_by(|a, b| a.session_id.cmp(&b.session_id));
        bundle.swarms.sort_by(|a, b| a.swarm_id.cmp(&b.swarm_id));
        for swarm in &mut bundle.swarms {
            swarm.updated_at_unix_ms = 0;
            swarm
                .members
                .sort_by(|a, b| a.record.session_id.cmp(&b.record.session_id));
            for member in &mut swarm.members {
                member.terminal_since_unix_ms = None;
            }
        }
        Ok(serde_json::to_value(bundle)?)
    };
    Ok(normalize(actual)? == normalize(expected)?)
}

fn transfer_lease(session: &GroupTransferSession, away_host: Option<String>) -> SessionLease {
    SessionLease {
        session_id: session.session_id.clone(),
        epoch: session.epoch,
        away_host,
        repo_root: session
            .previous_lease
            .as_ref()
            .and_then(|l| l.repo_root.clone()),
        last_sync_snapshot: session
            .previous_lease
            .as_ref()
            .and_then(|l| l.last_sync_snapshot.clone()),
        last_sync_head: session
            .previous_lease
            .as_ref()
            .and_then(|l| l.last_sync_head.clone()),
        updated_at: chrono::Utc::now().to_rfc3339(),
    }
}

fn restore_previous(session: &GroupTransferSession) -> Result<()> {
    if let Some(lease) = &session.previous_lease {
        storage::write_session_lease(lease)?;
    } else {
        let path = storage::session_leases_dir()
            .ok_or_else(|| anyhow::anyhow!("session lease directory unavailable"))?
            .join(format!("{}.json", session.session_id));
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

async fn validate_destination(
    bundle: &GroupTransferBundle,
    sessions: &SessionAgents,
    state: &SwarmState,
) -> Result<()> {
    validate_bundle(bundle)?;
    let ids: HashSet<_> = bundle
        .sessions
        .iter()
        .map(|s| s.session_id.as_str())
        .collect();
    // Refuse to replace a live destination copy, including an idle one.
    ensure!(
        sessions
            .read()
            .await
            .keys()
            .all(|id| !ids.contains(id.as_str())),
        "destination has a live session with a transferred id"
    );
    let existing = snapshots(state).await;
    for other in &existing {
        if !swarm_session_ids(other)
            .iter()
            .all(|id| !ids.contains(id.as_str()))
        {
            ensure!(
                bundle.swarms.iter().any(|s| s.swarm_id == other.swarm_id),
                "transferred session belongs to another destination swarm"
            );
        }
    }
    for swarm in &bundle.swarms {
        if let Some(durable) = swarm_persistence::read_transfer_snapshot(&swarm.swarm_id)? {
            ensure!(
                swarm_session_ids(&durable)
                    .iter()
                    .all(|id| ids.contains(id.as_str())),
                "destination durable swarm contains unrelated sessions: {}",
                swarm.swarm_id
            );
        }
        if let Some(other) = existing.iter().find(|s| s.swarm_id == swarm.swarm_id) {
            ensure!(
                swarm_session_ids(other)
                    .iter()
                    .all(|id| ids.contains(id.as_str())),
                "destination swarm contains unrelated sessions: {}",
                swarm.swarm_id
            );
        }
    }
    Ok(())
}

async fn import_bundle(
    bundle: &GroupTransferBundle,
    sessions: &SessionAgents,
    state: &SwarmState,
) -> Result<()> {
    validate_destination(bundle, sessions, state).await?;
    let mut destination_dirs = std::collections::HashMap::new();
    for transfer in &bundle.sessions {
        let session = crate::session::Session::load(&transfer.session_id)?;
        ensure!(
            session.migration_epoch == transfer.epoch,
            "destination session/lease epoch mismatch: {}",
            transfer.session_id
        );
        verify_source_fence(transfer, INBOUND_HOST)?;
        destination_dirs.insert(
            transfer.session_id.clone(),
            session.working_dir.map(std::path::PathBuf::from),
        );
    }
    let mut guards = Vec::new();
    let mut swarms = bundle.swarms.clone();
    for swarm in &mut swarms {
        for member in &mut swarm.members {
            // Session installation owns repository path remapping. Never
            // retain the source machine's checkout path in runtime metadata.
            if let Some(directory) = destination_dirs.get(&member.record.session_id) {
                member.record.working_dir = directory.clone();
            }
        }
    }
    swarms.sort_by(|a, b| a.swarm_id.cmp(&b.swarm_id));
    for swarm in &swarms {
        guards.push(
            super::swarm_operation_lock(&swarm.swarm_id)
                .lock_owned()
                .await,
        );
    }
    validate_destination(bundle, sessions, state).await?;
    // Durable writes happen before installation. No unrelated snapshot is read,
    // rewritten or pruned via the startup loader.
    for swarm in &swarms {
        swarm_persistence::write_transfer_snapshot(swarm)?;
    }
    // Deliberately leave inbound leases in place. The transport journals a
    // durable successful import before activating selected members. Retrying
    // this operation while still pending never starts a conversation turn.
    let mut members = state.members.write().await;
    let mut by_id = state.swarms_by_id.write().await;
    let mut plans = state.plans.write().await;
    let mut coordinators = state.coordinators.write().await;
    for swarm in swarms {
        members.retain(|_, member| member.swarm_id.as_deref() != Some(&swarm.swarm_id));
        let mut member_ids = HashSet::new();
        for member in swarm.members {
            let member = swarm_persistence::from_persisted_member(
                member,
                swarm.updated_at_unix_ms,
                swarm.updated_at_unix_ms,
                std::time::Duration::MAX,
            )
            .expect("transfer members never expire during import");
            member_ids.insert(member.session_id.clone());
            members.insert(member.session_id.clone(), member);
        }
        by_id.insert(swarm.swarm_id.clone(), member_ids);
        plans.remove(&swarm.swarm_id);
        if let Some(plan) = swarm.plan {
            plans.insert(
                swarm.swarm_id.clone(),
                swarm_persistence::from_persisted_plan(plan, swarm.updated_at_unix_ms),
            );
        }
        coordinators.remove(&swarm.swarm_id);
        if let Some(coordinator) = swarm.coordinator_session_id {
            coordinators.insert(swarm.swarm_id, coordinator);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "group_transfer_tests.rs"]
mod tests;
