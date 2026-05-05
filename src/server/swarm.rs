use super::state::{MAX_EVENT_HISTORY, fanout_session_event};
use super::{FileAccess, SwarmEvent, SwarmEventType, SwarmMember, SwarmState, VersionedPlan};
use super::{persist_swarm_state_for, remove_persisted_swarm_state_for};
use crate::agent::Agent;
use crate::plan::{PlanItem, newly_ready_item_ids};
use crate::protocol::{NotificationType, ServerEvent};
use crate::session::Session;
use anyhow::Result;
use futures::future::try_join_all;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, RwLock, broadcast};

fn status_age_secs(last_status_change: Instant) -> u64 {
    last_status_change.elapsed().as_secs()
}

const DEFAULT_SWARM_STATUS_DEBOUNCE_MEMBER_THRESHOLD: usize = 2;
const DEFAULT_SWARM_STATUS_DEBOUNCE_MS: u64 = 75;
const DEFAULT_SWARM_TASK_HEARTBEAT_SECS: u64 = 10;
const DEFAULT_SWARM_TASK_STALE_AFTER_SECS: u64 = 45;
const DEFAULT_SWARM_TASK_SWEEP_INTERVAL_SECS: u64 = 5;
const MAX_SWARM_COMPLETION_REPORT_CHARS: usize = 4000;
const SWARM_COMPLETION_REPORT_MARKER: &str = "SWARM COMPLETION REPORT REQUIRED";

pub(super) fn append_swarm_completion_report_instructions(message: &str) -> String {
    if message.contains(SWARM_COMPLETION_REPORT_MARKER) {
        return message.to_string();
    }

    let mut out = message.trim_end().to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str("<system-reminder>\n");
    out.push_str(SWARM_COMPLETION_REPORT_MARKER);
    out.push_str(
        "\nBefore finishing, call the swarm tool with action=\"report\" to submit your completion report. \
Include a concise message, validation/tests performed, and blockers or follow-ups. \
After the report tool succeeds, also write a brief final assistant response. \
Do not finish with only tool output, a lifecycle status change, or no final response. \
Do not send a separate DM for the final report unless you need interactive coordination before finishing.\n",
    );
    out.push_str("</system-reminder>");
    out
}

pub(super) fn format_structured_completion_report(
    message: &str,
    validation: Option<&str>,
    follow_up: Option<&str>,
) -> String {
    let mut report = message.trim().to_string();
    if let Some(validation) = validation.map(str::trim).filter(|value| !value.is_empty()) {
        if !report.is_empty() {
            report.push_str("\n\n");
        }
        report.push_str("Validation:\n");
        report.push_str(validation);
    }
    if let Some(follow_up) = follow_up.map(str::trim).filter(|value| !value.is_empty()) {
        if !report.is_empty() {
            report.push_str("\n\n");
        }
        report.push_str("Follow-ups/blockers:\n");
        report.push_str(follow_up);
    }
    report
}

fn normalize_completion_report(report: Option<String>) -> Option<String> {
    let report = report?.trim().to_string();
    if report.is_empty() {
        return None;
    }

    let char_count = report.chars().count();
    if char_count <= MAX_SWARM_COMPLETION_REPORT_CHARS {
        return Some(report);
    }

    let suffix = "\n\n[Report truncated by jcode before delivery.]";
    let keep_chars = MAX_SWARM_COMPLETION_REPORT_CHARS.saturating_sub(suffix.chars().count());
    let mut truncated: String = report.chars().take(keep_chars).collect();
    truncated.push_str(suffix);
    Some(truncated)
}

fn completion_status_intro(name: &str, status: &str) -> String {
    match status {
        "ready" => format!("Agent {} finished their work and is ready for more.", name),
        "failed" => format!("Agent {} finished with status failed.", name),
        "stopped" => format!("Agent {} stopped.", name),
        _ => format!("Agent {} completed their work.", name),
    }
}

fn completion_followup(status: &str, has_report: bool) -> &'static str {
    match (status, has_report) {
        ("ready", true) => {
            "Use assign_task to give them more work, stop to remove them, or summary/read_context for full context."
        }
        ("ready", false) => {
            "Use summary/read_context to inspect results, assign_task for more work, or stop to remove them."
        }
        ("failed", true) => {
            "Use summary/read_context for full context, retry with guidance, or stop to remove them."
        }
        ("failed", false) => {
            "Use summary/read_context to inspect results, assign_task to retry with guidance, or stop to remove them."
        }
        ("stopped", _) => "Use summary/read_context to inspect results or stop to remove them.",
        (_, true) => {
            "Use assign_task to give them new work, stop to remove them, or summary/read_context for full context."
        }
        (_, false) => "Use assign_task to give them new work, or stop to remove them.",
    }
}

fn completion_notification_message(name: &str, status: &str, report: Option<&str>) -> String {
    let intro = completion_status_intro(name, status);
    let followup = completion_followup(status, report.is_some());
    match report {
        Some(report) => format!("{intro}\n\nReport:\n{report}\n\n{followup}"),
        None => format!("{intro}\n\nNo final textual report was produced. {followup}"),
    }
}

#[derive(Default, Clone, Copy)]
struct PendingSwarmStatusBroadcast {
    scheduled: bool,
    dirty: bool,
}

fn pending_swarm_status_broadcasts()
-> &'static StdMutex<HashMap<String, PendingSwarmStatusBroadcast>> {
    static PENDING: OnceLock<StdMutex<HashMap<String, PendingSwarmStatusBroadcast>>> =
        OnceLock::new();
    PENDING.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn swarm_status_debounce_member_threshold() -> usize {
    static CACHED: OnceLock<AtomicUsize> = OnceLock::new();
    CACHED
        .get_or_init(|| {
            let configured = std::env::var("JCODE_SWARM_STATUS_DEBOUNCE_MEMBER_THRESHOLD")
                .ok()
                .and_then(|value| value.trim().parse::<usize>().ok())
                .filter(|value| *value > 0)
                .unwrap_or(DEFAULT_SWARM_STATUS_DEBOUNCE_MEMBER_THRESHOLD);
            AtomicUsize::new(configured)
        })
        .load(Ordering::Relaxed)
}

fn swarm_status_debounce_ms() -> u64 {
    static CACHED: OnceLock<AtomicU64> = OnceLock::new();
    CACHED
        .get_or_init(|| {
            let configured = std::env::var("JCODE_SWARM_STATUS_DEBOUNCE_MS")
                .ok()
                .and_then(|value| value.trim().parse::<u64>().ok())
                .filter(|value| *value > 0)
                .unwrap_or(DEFAULT_SWARM_STATUS_DEBOUNCE_MS);
            AtomicU64::new(configured)
        })
        .load(Ordering::Relaxed)
}

fn configured_positive_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

pub(super) fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub(super) fn swarm_task_heartbeat_interval() -> Duration {
    Duration::from_secs(configured_positive_u64(
        "JCODE_SWARM_TASK_HEARTBEAT_SECS",
        DEFAULT_SWARM_TASK_HEARTBEAT_SECS,
    ))
}

pub(super) fn swarm_task_stale_after() -> Duration {
    Duration::from_secs(configured_positive_u64(
        "JCODE_SWARM_TASK_STALE_AFTER_SECS",
        DEFAULT_SWARM_TASK_STALE_AFTER_SECS,
    ))
}

pub(super) fn swarm_task_sweep_interval() -> Duration {
    Duration::from_secs(configured_positive_u64(
        "JCODE_SWARM_TASK_SWEEP_INTERVAL_SECS",
        DEFAULT_SWARM_TASK_SWEEP_INTERVAL_SECS,
    ))
}

#[expect(
    clippy::too_many_arguments,
    reason = "task progress touch updates durable progress plus swarm persistence and coordinator-facing state in one helper"
)]
pub(super) async fn touch_swarm_task_progress(
    swarm_id: &str,
    task_id: &str,
    assigned_session_id: Option<&str>,
    detail: Option<String>,
    checkpoint_summary: Option<String>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
) -> bool {
    let now_ms = now_unix_ms();
    let revived = {
        let mut plans = swarm_plans.write().await;
        let Some(plan) = plans.get_mut(swarm_id) else {
            return false;
        };
        let Some(item) = plan.items.iter_mut().find(|item| item.id == task_id) else {
            return false;
        };
        let progress = plan.task_progress.entry(task_id.to_string()).or_default();
        if let Some(session_id) = assigned_session_id {
            progress.assigned_session_id = Some(session_id.to_string());
        }
        progress.last_heartbeat_unix_ms = Some(now_ms);
        progress.heartbeat_count = Some(progress.heartbeat_count.unwrap_or(0) + 1);
        if let Some(detail) = detail {
            progress.last_detail = Some(truncate_detail(&detail, 120));
        }
        if let Some(summary) = checkpoint_summary {
            progress.last_checkpoint_unix_ms = Some(now_ms);
            progress.checkpoint_summary = Some(truncate_detail(&summary, 120));
            progress.checkpoint_count = Some(progress.checkpoint_count.unwrap_or(0) + 1);
        }
        if item.status == "running_stale" {
            item.status = "running".to_string();
            progress.stale_since_unix_ms = None;
            plan.version += 1;
            true
        } else {
            false
        }
    };
    let swarm_state = SwarmState {
        members: Arc::clone(swarm_members),
        swarms_by_id: Arc::clone(swarms_by_id),
        plans: Arc::clone(swarm_plans),
        coordinators: Arc::clone(swarm_coordinators),
    };
    persist_swarm_state_for(swarm_id, &swarm_state).await;
    revived
}

pub(super) async fn refresh_swarm_task_staleness(
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
) {
    let now_ms = now_unix_ms();
    let stale_after_ms = swarm_task_stale_after().as_millis() as u64;
    let changed_swarm_ids = {
        let mut plans = swarm_plans.write().await;
        let mut changed = Vec::new();
        for (swarm_id, plan) in plans.iter_mut() {
            let mut swarm_changed = false;
            for item in &mut plan.items {
                if !matches!(item.status.as_str(), "running" | "running_stale") {
                    continue;
                }
                let progress = plan.task_progress.entry(item.id.clone()).or_default();
                let last_heartbeat = progress
                    .last_heartbeat_unix_ms
                    .or(progress.started_at_unix_ms)
                    .or(progress.assigned_at_unix_ms);
                let is_stale = last_heartbeat
                    .map(|ts| now_ms.saturating_sub(ts) >= stale_after_ms)
                    .unwrap_or(true);
                match (item.status.as_str(), is_stale) {
                    ("running", true) => {
                        item.status = "running_stale".to_string();
                        progress.stale_since_unix_ms.get_or_insert(now_ms);
                        plan.version += 1;
                        swarm_changed = true;
                    }
                    ("running_stale", false) => {
                        item.status = "running".to_string();
                        progress.stale_since_unix_ms = None;
                        plan.version += 1;
                        swarm_changed = true;
                    }
                    _ => {}
                }
            }
            if swarm_changed {
                changed.push(swarm_id.clone());
            }
        }
        changed
    };

    for swarm_id in changed_swarm_ids {
        let swarm_state = SwarmState {
            members: Arc::clone(swarm_members),
            swarms_by_id: Arc::clone(swarms_by_id),
            plans: Arc::clone(swarm_plans),
            coordinators: Arc::clone(swarm_coordinators),
        };
        persist_swarm_state_for(&swarm_id, &swarm_state).await;
        broadcast_swarm_plan(
            &swarm_id,
            Some("task_staleness_changed".to_string()),
            swarm_plans,
            swarm_members,
            swarms_by_id,
        )
        .await;
    }
}

fn swarm_broadcast_key(
    swarm_id: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
) -> String {
    format!(
        "{:p}:{:p}:{swarm_id}",
        Arc::as_ptr(swarm_members),
        Arc::as_ptr(swarms_by_id)
    )
}

async fn broadcast_swarm_status_now(
    session_ids: Vec<String>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) {
    if session_ids.is_empty() {
        return;
    }

    let members_guard = swarm_members.read().await;
    let members_list: Vec<crate::protocol::SwarmMemberStatus> = session_ids
        .iter()
        .filter_map(|sid| {
            members_guard
                .get(sid)
                .map(|m| crate::protocol::SwarmMemberStatus {
                    session_id: m.session_id.clone(),
                    friendly_name: m.friendly_name.clone(),
                    status: m.status.clone(),
                    detail: m.detail.clone(),
                    role: Some(m.role.clone()),
                    is_headless: Some(m.is_headless),
                    live_attachments: Some(m.event_txs.len()),
                    status_age_secs: Some(status_age_secs(m.last_status_change)),
                })
        })
        .collect();

    drop(members_guard);
    let event = ServerEvent::SwarmStatus {
        members: members_list,
    };
    for sid in session_ids {
        let _ = fanout_session_event(swarm_members, &sid, event.clone()).await;
    }
}

pub(super) async fn broadcast_swarm_status(
    swarm_id: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
) {
    let session_ids: Vec<String> = {
        let swarms = swarms_by_id.read().await;
        swarms
            .get(swarm_id)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    };
    if session_ids.is_empty() {
        return;
    }

    if session_ids.len() < swarm_status_debounce_member_threshold() {
        broadcast_swarm_status_now(session_ids, swarm_members).await;
        return;
    }

    let key = swarm_broadcast_key(swarm_id, swarm_members, swarms_by_id);
    let should_spawn = {
        let mut pending = pending_swarm_status_broadcasts()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = pending.entry(key.clone()).or_default();
        if entry.scheduled {
            entry.dirty = true;
            false
        } else {
            entry.scheduled = true;
            entry.dirty = false;
            true
        }
    };

    if !should_spawn {
        return;
    }

    let swarm_id = swarm_id.to_string();
    let swarm_members = Arc::clone(swarm_members);
    let swarms_by_id = Arc::clone(swarms_by_id);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(swarm_status_debounce_ms())).await;
            let session_ids: Vec<String> = {
                let swarms = swarms_by_id.read().await;
                swarms
                    .get(&swarm_id)
                    .map(|s| s.iter().cloned().collect())
                    .unwrap_or_default()
            };
            broadcast_swarm_status_now(session_ids, &swarm_members).await;

            let mut pending = pending_swarm_status_broadcasts()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(entry) = pending.get_mut(&key) else {
                break;
            };
            if entry.dirty {
                entry.dirty = false;
                continue;
            }
            pending.remove(&key);
            break;
        }
    });
}

/// Broadcast the authoritative swarm plan snapshot.
///
/// Plan snapshots are sent to explicit plan participants. If a plan has no
/// participants yet, fall back to all current swarm members.
pub(super) async fn broadcast_swarm_plan(
    swarm_id: &str,
    reason: Option<String>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
) {
    broadcast_swarm_plan_with_previous(
        swarm_id,
        reason,
        None,
        swarm_plans,
        swarm_members,
        swarms_by_id,
    )
    .await;
}

pub(super) async fn broadcast_swarm_plan_with_previous(
    swarm_id: &str,
    reason: Option<String>,
    previous_items: Option<&[PlanItem]>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
) {
    let (version, items, summary, mut participants): (
        u64,
        Vec<PlanItem>,
        crate::protocol::PlanGraphStatus,
        Vec<String>,
    ) = {
        let plans = swarm_plans.read().await;
        let Some(vp) = plans.get(swarm_id) else {
            return;
        };
        let newly_ready_ids = previous_items
            .map(|before| newly_ready_item_ids(before, &vp.items))
            .unwrap_or_default();
        let mut p: Vec<String> = vp.participants.iter().cloned().collect();
        p.sort();
        (
            vp.version,
            vp.items.clone(),
            crate::protocol::PlanGraphStatus::from_versioned_plan(
                swarm_id,
                vp,
                Some(3),
                newly_ready_ids,
            ),
            p,
        )
    };

    if participants.is_empty() {
        let swarms = swarms_by_id.read().await;
        participants = swarms
            .get(swarm_id)
            .map(|s| {
                let mut ids: Vec<String> = s.iter().cloned().collect();
                ids.sort();
                ids
            })
            .unwrap_or_default();
    }

    if participants.is_empty() {
        return;
    }

    let event = ServerEvent::SwarmPlan {
        swarm_id: swarm_id.to_string(),
        version,
        items,
        participants: participants.clone(),
        reason,
        summary: Some(summary),
    };

    let members = swarm_members.read().await;
    for sid in participants {
        if let Some(member) = members.get(&sid) {
            let _ = member.event_tx.send(event.clone());
        }
    }
}

pub(super) async fn rename_plan_participant(
    swarm_id: &str,
    old_session_id: &str,
    new_session_id: &str,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
) {
    let mut plans = swarm_plans.write().await;
    if let Some(vp) = plans.get_mut(swarm_id) {
        if vp.participants.remove(old_session_id) {
            vp.participants.insert(new_session_id.to_string());
        }
        for item in &mut vp.items {
            if item.assigned_to.as_deref() == Some(old_session_id) {
                item.assigned_to = Some(new_session_id.to_string());
            }
        }
    }
}

pub(super) async fn remove_plan_participant(
    swarm_id: &str,
    session_id: &str,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
) {
    let mut plans = swarm_plans.write().await;
    if let Some(vp) = plans.get_mut(swarm_id) {
        vp.participants.remove(session_id);
    }
}

pub(super) async fn remove_session_file_touches(
    session_id: &str,
    file_touches: &Arc<RwLock<HashMap<PathBuf, Vec<FileAccess>>>>,
    files_touched_by_session: &Arc<RwLock<HashMap<String, HashSet<PathBuf>>>>,
) {
    let touched_paths = {
        let mut reverse = files_touched_by_session.write().await;
        reverse.remove(session_id)
    };

    let mut touches = file_touches.write().await;
    if let Some(paths) = touched_paths {
        for path in paths {
            let mut remove_path = false;
            if let Some(accesses) = touches.get_mut(&path) {
                accesses.retain(|access| access.session_id != session_id);
                remove_path = accesses.is_empty();
            }
            if remove_path {
                touches.remove(&path);
            }
        }
        return;
    }

    touches.retain(|_, accesses| {
        accesses.retain(|access| access.session_id != session_id);
        !accesses.is_empty()
    });
}

pub(super) async fn remove_session_from_swarm(
    session_id: &str,
    swarm_id: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
) {
    remove_plan_participant(swarm_id, session_id, swarm_plans).await;

    {
        let mut swarms = swarms_by_id.write().await;
        if let Some(swarm) = swarms.get_mut(swarm_id) {
            swarm.remove(session_id);
            if swarm.is_empty() {
                swarms.remove(swarm_id);
            }
        }
    }

    let was_coordinator = {
        let coordinators = swarm_coordinators.read().await;
        coordinators
            .get(swarm_id)
            .map(|id| id == session_id)
            .unwrap_or(false)
    };

    if was_coordinator {
        let new_coordinator = {
            let swarms = swarms_by_id.read().await;
            let members = swarm_members.read().await;
            swarms.get(swarm_id).and_then(|swarm| {
                swarm
                    .iter()
                    .filter_map(|id| {
                        members
                            .get(id)
                            .filter(|member| !member.is_headless)
                            .map(|_| id.clone())
                    })
                    .min()
            })
        };

        {
            let mut coordinators = swarm_coordinators.write().await;
            coordinators.remove(swarm_id);
            if let Some(ref new_id) = new_coordinator {
                coordinators.insert(swarm_id.to_string(), new_id.clone());
            }
        }

        if let Some(new_id) = new_coordinator {
            {
                let mut members = swarm_members.write().await;
                if let Some(member) = members.get_mut(&new_id) {
                    member.role = "coordinator".to_string();
                }
            }
            let mut plans = swarm_plans.write().await;
            if let Some(vp) = plans.get_mut(swarm_id) {
                vp.participants.insert(new_id.clone());
            }
            let members = swarm_members.read().await;
            if let Some(member) = members.get(&new_id) {
                let _ = member.event_tx.send(ServerEvent::Notification {
                    from_session: new_id.clone(),
                    from_name: member.friendly_name.clone(),
                    notification_type: NotificationType::Message {
                        scope: Some("swarm".to_string()),
                        channel: None,
                    },
                    message: "You are now the coordinator for this swarm.".to_string(),
                });
            }
        }
    }

    {
        let mut members = swarm_members.write().await;
        if let Some(member) = members.get_mut(session_id) {
            member.role = "agent".to_string();
        }
    }

    if swarm_plans.read().await.contains_key(swarm_id) {
        let swarm_state = SwarmState {
            members: Arc::clone(swarm_members),
            swarms_by_id: Arc::clone(swarms_by_id),
            plans: Arc::clone(swarm_plans),
            coordinators: Arc::clone(swarm_coordinators),
        };
        persist_swarm_state_for(swarm_id, &swarm_state).await;
    } else {
        let swarm_state = SwarmState {
            members: Arc::clone(swarm_members),
            swarms_by_id: Arc::clone(swarms_by_id),
            plans: Arc::clone(swarm_plans),
            coordinators: Arc::clone(swarm_coordinators),
        };
        remove_persisted_swarm_state_for(swarm_id, &swarm_state).await;
    }

    broadcast_swarm_status(swarm_id, swarm_members, swarms_by_id).await;
}

pub(super) async fn record_swarm_event(
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    session_id: String,
    session_name: Option<String>,
    swarm_id: Option<String>,
    event: SwarmEventType,
) {
    let swarm_event = SwarmEvent {
        id: event_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
        session_id,
        session_name,
        swarm_id,
        event,
        timestamp: Instant::now(),
        absolute_time: std::time::SystemTime::now(),
    };
    let _ = swarm_event_tx.send(swarm_event.clone());
    let mut history = event_history.write().await;
    history.push_back(swarm_event);
    if history.len() > MAX_EVENT_HISTORY {
        history.pop_front();
    }
}

pub(super) async fn record_swarm_event_for_session(
    session_id: &str,
    event: SwarmEventType,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) {
    let (session_name, swarm_id) = {
        let members = swarm_members.read().await;
        if let Some(member) = members.get(session_id) {
            (member.friendly_name.clone(), member.swarm_id.clone())
        } else {
            (None, None)
        }
    };
    record_swarm_event(
        event_history,
        event_counter,
        swarm_event_tx,
        session_id.to_string(),
        session_name,
        swarm_id,
        event,
    )
    .await;
}

#[expect(
    clippy::too_many_arguments,
    reason = "member status updates need swarm membership, broadcast state, and optional event history sinks"
)]
pub(super) async fn update_member_status(
    session_id: &str,
    status: &str,
    detail: Option<String>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    event_history: Option<&Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>>,
    event_counter: Option<&Arc<std::sync::atomic::AtomicU64>>,
    swarm_event_tx: Option<&broadcast::Sender<SwarmEvent>>,
) {
    update_member_status_with_report(
        session_id,
        status,
        detail,
        None,
        swarm_members,
        swarms_by_id,
        event_history,
        event_counter,
        swarm_event_tx,
    )
    .await;
}

#[expect(
    clippy::too_many_arguments,
    reason = "member status updates need swarm membership, broadcast state, optional report text, and event history sinks"
)]
pub(super) async fn update_member_status_with_report(
    session_id: &str,
    status: &str,
    detail: Option<String>,
    completion_report: Option<String>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    event_history: Option<&Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>>,
    event_counter: Option<&Arc<std::sync::atomic::AtomicU64>>,
    swarm_event_tx: Option<&broadcast::Sender<SwarmEvent>>,
) {
    let completion_report = normalize_completion_report(completion_report);
    let (
        swarm_id,
        agent_name,
        member_changed,
        status_changed,
        old_status,
        _is_headless,
        report_back_to_session_id,
    ) = {
        let mut members = swarm_members.write().await;
        if let Some(member) = members.get_mut(session_id) {
            let previous_status = member.status.clone();
            let status_changed = member.status != status;
            let detail_changed = member.detail != detail;
            let report_changed =
                completion_report.is_some() && member.latest_completion_report != completion_report;
            let member_changed = status_changed || detail_changed || report_changed;
            if status_changed {
                member.last_status_change = Instant::now();
            }
            let name = member.friendly_name.clone();
            let is_headless = member.is_headless;
            let report_back_to_session_id = member.report_back_to_session_id.clone();
            member.status = status.to_string();
            member.detail = detail;
            if completion_report.is_some() {
                member.latest_completion_report = completion_report.clone();
            }
            (
                member.swarm_id.clone(),
                name,
                member_changed,
                status_changed,
                previous_status,
                is_headless,
                report_back_to_session_id,
            )
        } else {
            (None, None, false, false, String::new(), false, None)
        }
    };
    if let Some(ref id) = swarm_id {
        if !member_changed {
            return;
        }

        if status_changed
            && let (Some(history), Some(counter), Some(tx)) =
                (event_history, event_counter, swarm_event_tx)
        {
            record_swarm_event(
                history,
                counter,
                tx,
                session_id.to_string(),
                agent_name.clone(),
                Some(id.clone()),
                SwarmEventType::StatusChange {
                    old_status: old_status.clone(),
                    new_status: status.to_string(),
                },
            )
            .await;
        }

        broadcast_swarm_status(id, swarm_members, swarms_by_id).await;

        let should_notify_coordinator = status_changed
            && ((status == "completed")
                || (report_back_to_session_id.is_some()
                    && old_status == "running"
                    && matches!(status, "ready" | "failed" | "stopped")));
        if should_notify_coordinator {
            let fallback_coordinator_id =
                if report_back_to_session_id.as_deref() == Some(session_id) {
                    None
                } else {
                    let members = swarm_members.read().await;
                    members
                        .values()
                        .find(|m| {
                            m.swarm_id.as_deref() == Some(id)
                                && m.role == "coordinator"
                                && m.session_id != session_id
                        })
                        .map(|m| m.session_id.clone())
                };
            let recipient_session_id = report_back_to_session_id
                .clone()
                .filter(|owner_id| owner_id != session_id)
                .or(fallback_coordinator_id);
            if let Some(recipient_session_id) = recipient_session_id {
                let name = agent_name
                    .as_deref()
                    .unwrap_or(&session_id[..8.min(session_id.len())]);
                let msg =
                    completion_notification_message(name, status, completion_report.as_deref());
                let _ = fanout_session_event(
                    swarm_members,
                    &recipient_session_id,
                    ServerEvent::Notification {
                        from_session: session_id.to_string(),
                        from_name: agent_name.clone(),
                        notification_type: NotificationType::Message {
                            scope: Some("swarm".to_string()),
                            channel: None,
                        },
                        message: msg,
                    },
                )
                .await;
            }
        }
    }
}

pub(super) fn truncate_detail(text: &str, max_len: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim();
    let max_len = max_len.max(1);
    if trimmed.chars().count() <= max_len {
        return trimmed.to_string();
    }
    if max_len <= 3 {
        return trimmed.chars().take(max_len).collect();
    }
    let mut out: String = trimmed.chars().take(max_len - 3).collect();
    out.push_str("...");
    out
}

pub(super) fn summarize_plan_items(items: &[PlanItem], max_items: usize) -> String {
    if items.is_empty() {
        return "no items".to_string();
    }
    let mut parts: Vec<String> = Vec::new();
    for item in items.iter().take(max_items.max(1)) {
        parts.push(item.content.clone());
    }
    let mut summary = parts.join("; ");
    if items.len() > max_items.max(1) {
        summary.push_str(&format!(" (+{} more)", items.len() - max_items.max(1)));
    }
    summary
}

pub(super) async fn run_swarm_task(
    agent: Arc<Mutex<Agent>>,
    description: &str,
    subagent_type: &str,
    prompt: &str,
) -> Result<String> {
    let (provider, registry, session_id, working_dir, coordinator_model) = {
        let agent = agent.lock().await;
        (
            agent.provider_fork(),
            agent.registry(),
            agent.session_id().to_string(),
            agent.working_dir().map(PathBuf::from),
            agent.provider_model(),
        )
    };
    let mut session = Session::create(
        Some(session_id),
        Some(format!("{} (@{} swarm)", description, subagent_type)),
    );
    session.model = Some(coordinator_model);
    if let Some(dir) = working_dir {
        session.working_dir = Some(dir.display().to_string());
    }
    session.save()?;

    let mut allowed: HashSet<String> = registry.tool_names().await.into_iter().collect();
    for blocked in ["subagent", "task", "todo", "todowrite", "todoread"] {
        allowed.remove(blocked);
    }

    let mut worker = Agent::new_with_session(provider, registry, session, Some(allowed));
    let output = worker.run_once_capture(prompt).await?;
    Ok(output)
}

pub(super) async fn run_swarm_message(agent: Arc<Mutex<Agent>>, message: &str) -> Result<String> {
    let working_dir = {
        let agent = agent.lock().await;
        agent.working_dir().map(|dir| dir.to_string())
    };
    let working_dir_hint = working_dir
        .as_deref()
        .map(|dir| format!("Working directory: {}\n", dir))
        .unwrap_or_default();

    let planner_prompt = format!(
        "{working_dir_hint}You are a task planner. Break the request into 2-4 subtasks. \
Return ONLY a JSON array of objects with keys: description, prompt, subagent_type. \
No extra text.\n\nRequest:\n{message}"
    );

    let plan_text = {
        let mut agent = agent.lock().await;
        agent.run_once_capture(&planner_prompt).await?
    };

    let mut tasks = parse_swarm_tasks(&plan_text);
    if tasks.is_empty() {
        tasks.push(SwarmTaskSpec {
            description: "Main task".to_string(),
            prompt: message.to_string(),
            subagent_type: Some("general".to_string()),
        });
    }

    let task_futures = tasks.iter().map(|task| {
        let agent = agent.clone();
        let working_dir_hint = working_dir_hint.clone();
        let description = task.description.clone();
        let prompt = format!("{working_dir_hint}{}", task.prompt);
        let subagent_type = task
            .subagent_type
            .clone()
            .unwrap_or_else(|| "general".to_string());
        async move {
            let output = run_swarm_task(agent, &description, &subagent_type, &prompt).await?;
            Ok::<(String, String), anyhow::Error>((description, output))
        }
    });
    let task_outputs = try_join_all(task_futures).await?;

    let mut integration_prompt = String::new();
    integration_prompt.push_str(
        "You are the coordinator. Complete the original request using the subagent outputs below. ",
    );
    integration_prompt.push_str("Do not stop early; run any requested tests and fix failures.\n\n");
    integration_prompt.push_str("Original request:\n");
    integration_prompt.push_str(message);
    integration_prompt.push_str("\n\nSubagent outputs:\n");
    for (desc, output) in &task_outputs {
        integration_prompt.push_str(&format!("\n--- {} ---\n{}\n", desc, output));
    }
    integration_prompt.push_str("\nNow complete the task.\n");

    let final_output = {
        let mut agent = agent.lock().await;
        agent.run_once_capture(&integration_prompt).await?
    };

    Ok(final_output)
}

#[derive(Debug, Deserialize)]
struct SwarmTaskSpec {
    description: String,
    prompt: String,
    #[serde(default)]
    subagent_type: Option<String>,
}

fn parse_swarm_tasks(text: &str) -> Vec<SwarmTaskSpec> {
    if let Ok(tasks) = serde_json::from_str::<Vec<SwarmTaskSpec>>(text) {
        return tasks;
    }

    if let (Some(start), Some(end)) = (text.find('['), text.rfind(']'))
        && start < end
        && let Ok(tasks) = serde_json::from_str::<Vec<SwarmTaskSpec>>(&text[start..=end])
    {
        return tasks;
    }

    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::{
        append_swarm_completion_report_instructions, broadcast_swarm_plan_with_previous,
        now_unix_ms, parse_swarm_tasks, refresh_swarm_task_staleness, remove_session_from_swarm,
        summarize_plan_items, touch_swarm_task_progress, truncate_detail, update_member_status,
        update_member_status_with_report,
    };
    use crate::plan::PlanItem;
    use crate::protocol::{NotificationType, ServerEvent};
    use crate::server::{SwarmMember, VersionedPlan};
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;
    use std::time::Instant;
    use tokio::sync::{RwLock, mpsc};

    fn plan_item(id: &str, content: &str) -> PlanItem {
        PlanItem {
            content: content.to_string(),
            status: "pending".to_string(),
            priority: "medium".to_string(),
            id: id.to_string(),
            subsystem: None,
            file_scope: Vec::new(),
            blocked_by: Vec::new(),
            assigned_to: None,
        }
    }

    #[test]
    fn truncate_detail_collapses_whitespace_and_ellipsizes() {
        assert_eq!(truncate_detail("hello   there\nworld", 11), "hello th...");
    }

    #[test]
    fn summarize_plan_items_limits_output() {
        let items = vec![
            plan_item("1", "inspect"),
            plan_item("2", "refactor"),
            plan_item("3", "test"),
        ];

        assert_eq!(
            summarize_plan_items(&items, 2),
            "inspect; refactor (+1 more)"
        );
    }

    #[test]
    fn parse_swarm_tasks_accepts_wrapped_json() {
        let text =
            "Plan:\n[{\"description\":\"A\",\"prompt\":\"B\",\"subagent_type\":\"general\"}]";
        let tasks = parse_swarm_tasks(text);

        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].description, "A");
        assert_eq!(tasks[0].prompt, "B");
        assert_eq!(tasks[0].subagent_type.as_deref(), Some("general"));
    }

    #[test]
    fn append_swarm_completion_report_instructions_is_idempotent() {
        let prompt = "Implement the task.";
        let with_instructions = append_swarm_completion_report_instructions(prompt);

        assert!(with_instructions.starts_with(prompt));
        assert!(with_instructions.contains("SWARM COMPLETION REPORT REQUIRED"));
        assert!(with_instructions.contains("automatically deliver"));
        assert_eq!(
            append_swarm_completion_report_instructions(&with_instructions),
            with_instructions
        );
    }

    fn swarm_member(
        session_id: &str,
        role: &str,
        is_headless: bool,
    ) -> (SwarmMember, mpsc::UnboundedReceiver<ServerEvent>) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        (
            SwarmMember {
                session_id: session_id.to_string(),
                event_tx,
                event_txs: HashMap::new(),
                working_dir: None,
                swarm_id: Some("swarm-1".to_string()),
                swarm_enabled: true,
                status: "ready".to_string(),
                detail: None,
                friendly_name: Some(session_id.to_string()),
                report_back_to_session_id: None,
                latest_completion_report: None,
                role: role.to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless,
            },
            event_rx,
        )
    }

    #[tokio::test]
    async fn broadcast_swarm_plan_with_previous_includes_newly_ready_ids() {
        let swarm_plans = Arc::new(RwLock::new(HashMap::from([(
            "swarm-1".to_string(),
            VersionedPlan {
                items: vec![
                    PlanItem {
                        content: "setup".to_string(),
                        status: "completed".to_string(),
                        priority: "high".to_string(),
                        id: "setup".to_string(),
                        subsystem: None,
                        file_scope: Vec::new(),
                        blocked_by: Vec::new(),
                        assigned_to: None,
                    },
                    PlanItem {
                        content: "follow-up".to_string(),
                        status: "queued".to_string(),
                        priority: "high".to_string(),
                        id: "follow-up".to_string(),
                        subsystem: None,
                        file_scope: Vec::new(),
                        blocked_by: vec!["setup".to_string()],
                        assigned_to: None,
                    },
                ],
                version: 2,
                participants: HashSet::from(["worker".to_string()]),
                task_progress: HashMap::new(),
            },
        )])));
        let (worker, mut worker_rx) = swarm_member("worker", "agent", false);
        let swarm_members = Arc::new(RwLock::new(HashMap::from([("worker".to_string(), worker)])));
        let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
            "swarm-1".to_string(),
            HashSet::from(["worker".to_string()]),
        )])));
        let previous_items = vec![
            PlanItem {
                content: "setup".to_string(),
                status: "running".to_string(),
                priority: "high".to_string(),
                id: "setup".to_string(),
                subsystem: None,
                file_scope: Vec::new(),
                blocked_by: Vec::new(),
                assigned_to: Some("worker".to_string()),
            },
            PlanItem {
                content: "follow-up".to_string(),
                status: "queued".to_string(),
                priority: "high".to_string(),
                id: "follow-up".to_string(),
                subsystem: None,
                file_scope: Vec::new(),
                blocked_by: vec!["setup".to_string()],
                assigned_to: None,
            },
        ];

        broadcast_swarm_plan_with_previous(
            "swarm-1",
            Some("task_completed".to_string()),
            Some(&previous_items),
            &swarm_plans,
            &swarm_members,
            &swarms_by_id,
        )
        .await;

        match worker_rx.recv().await.expect("swarm plan event") {
            ServerEvent::SwarmPlan {
                reason,
                summary: Some(summary),
                ..
            } => {
                assert_eq!(reason.as_deref(), Some("task_completed"));
                assert_eq!(summary.newly_ready_ids, vec!["follow-up".to_string()]);
                assert_eq!(summary.next_ready_ids, vec!["follow-up".to_string()]);
            }
            other => panic!("expected SwarmPlan event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn remove_session_from_swarm_reassigns_to_non_headless_member() {
        let swarm_members = Arc::new(RwLock::new(HashMap::new()));
        let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
            "swarm-1".to_string(),
            HashSet::from([
                "coord".to_string(),
                "headless".to_string(),
                "worker".to_string(),
            ]),
        )])));
        let swarm_coordinators = Arc::new(RwLock::new(HashMap::from([(
            "swarm-1".to_string(),
            "coord".to_string(),
        )])));
        let swarm_plans = Arc::new(RwLock::new(HashMap::from([(
            "swarm-1".to_string(),
            VersionedPlan {
                items: vec![PlanItem {
                    content: "task".to_string(),
                    status: "pending".to_string(),
                    priority: "medium".to_string(),
                    id: "1".to_string(),
                    subsystem: None,
                    file_scope: Vec::new(),
                    blocked_by: Vec::new(),
                    assigned_to: Some("coord".to_string()),
                }],
                version: 1,
                participants: HashSet::from(["coord".to_string()]),
                task_progress: HashMap::new(),
            },
        )])));

        let (coord, _coord_rx) = swarm_member("coord", "coordinator", false);
        let (headless, mut headless_rx) = swarm_member("headless", "agent", true);
        let (worker, mut worker_rx) = swarm_member("worker", "agent", false);
        {
            let mut members = swarm_members.write().await;
            members.insert("coord".to_string(), coord);
            members.insert("headless".to_string(), headless);
            members.insert("worker".to_string(), worker);
            members.remove("coord");
        }

        remove_session_from_swarm(
            "coord",
            "swarm-1",
            &swarm_members,
            &swarms_by_id,
            &swarm_coordinators,
            &swarm_plans,
        )
        .await;

        assert_eq!(
            swarm_coordinators
                .read()
                .await
                .get("swarm-1")
                .map(String::as_str),
            Some("worker")
        );
        assert!(
            swarm_plans
                .read()
                .await
                .get("swarm-1")
                .is_some_and(|plan| plan.participants.contains("worker"))
        );
        assert_eq!(
            swarm_members
                .read()
                .await
                .get("worker")
                .map(|member| member.role.as_str()),
            Some("coordinator")
        );
        assert_eq!(
            swarm_members
                .read()
                .await
                .get("headless")
                .map(|member| member.role.as_str()),
            Some("agent")
        );

        let headless_events: Vec<_> = std::iter::from_fn(|| headless_rx.try_recv().ok()).collect();
        assert!(headless_events.iter().all(|event| {
            !matches!(
                event,
                ServerEvent::Notification {
                    notification_type: NotificationType::Message { .. },
                    message,
                    ..
                } if message == "You are now the coordinator for this swarm."
            )
        }));

        let worker_events: Vec<_> = std::iter::from_fn(|| worker_rx.try_recv().ok()).collect();
        assert!(worker_events.iter().any(|event| {
            matches!(
                event,
                ServerEvent::Notification {
                    notification_type: NotificationType::Message { .. },
                    message,
                    ..
                } if message == "You are now the coordinator for this swarm."
            )
        }));
    }

    #[tokio::test]
    async fn update_member_status_notifies_coordinator_when_headless_worker_returns_ready() {
        let swarm_members = Arc::new(RwLock::new(HashMap::new()));
        let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
            "swarm-1".to_string(),
            HashSet::from(["coord".to_string(), "worker".to_string()]),
        )])));

        let (coord, mut coord_rx) = swarm_member("coord", "coordinator", false);
        let (mut worker, _worker_rx) = swarm_member("worker", "agent", true);
        worker.status = "running".to_string();
        worker.detail = Some("doing task".to_string());
        worker.report_back_to_session_id = Some("coord".to_string());
        {
            let mut members = swarm_members.write().await;
            members.insert("coord".to_string(), coord);
            members.insert("worker".to_string(), worker);
        }

        update_member_status(
            "worker",
            "ready",
            None,
            &swarm_members,
            &swarms_by_id,
            None,
            None,
            None,
        )
        .await;

        let events: Vec<_> = std::iter::from_fn(|| coord_rx.try_recv().ok()).collect();
        assert!(events.iter().any(|event| {
            matches!(
                event,
                ServerEvent::Notification {
                    notification_type: NotificationType::Message { .. },
                    message,
                    ..
                } if message.contains("finished their work and is ready for more")
            )
        }));
    }

    #[tokio::test]
    async fn update_member_status_prefers_explicit_report_back_owner_over_coordinator() {
        let swarm_members = Arc::new(RwLock::new(HashMap::new()));
        let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
            "swarm-1".to_string(),
            HashSet::from([
                "coord".to_string(),
                "owner".to_string(),
                "worker".to_string(),
            ]),
        )])));

        let (coord, mut coord_rx) = swarm_member("coord", "coordinator", false);
        let (owner, mut owner_rx) = swarm_member("owner", "agent", false);
        let (mut worker, _worker_rx) = swarm_member("worker", "agent", true);
        worker.status = "running".to_string();
        worker.detail = Some("doing task".to_string());
        worker.report_back_to_session_id = Some("owner".to_string());
        {
            let mut members = swarm_members.write().await;
            members.insert("coord".to_string(), coord);
            members.insert("owner".to_string(), owner);
            members.insert("worker".to_string(), worker);
        }

        update_member_status(
            "worker",
            "ready",
            None,
            &swarm_members,
            &swarms_by_id,
            None,
            None,
            None,
        )
        .await;

        let owner_events: Vec<_> = std::iter::from_fn(|| owner_rx.try_recv().ok()).collect();
        assert!(owner_events.iter().any(|event| {
            matches!(
                event,
                ServerEvent::Notification {
                    notification_type: NotificationType::Message { .. },
                    message,
                    ..
                } if message.contains("finished their work and is ready for more")
            )
        }));
        let coord_events: Vec<_> = std::iter::from_fn(|| coord_rx.try_recv().ok()).collect();
        assert!(coord_events.iter().all(|event| {
            !matches!(
                event,
                ServerEvent::Notification {
                    notification_type: NotificationType::Message { .. },
                    message,
                    ..
                } if message.contains("finished their work and is ready for more")
            )
        }));
    }

    #[tokio::test]
    async fn update_member_status_includes_completion_report_in_owner_notification() {
        let swarm_members = Arc::new(RwLock::new(HashMap::new()));
        let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
            "swarm-1".to_string(),
            HashSet::from(["coord".to_string(), "worker".to_string()]),
        )])));

        let (coord, mut coord_rx) = swarm_member("coord", "coordinator", false);
        let (mut worker, _worker_rx) = swarm_member("worker", "agent", true);
        worker.status = "running".to_string();
        worker.report_back_to_session_id = Some("coord".to_string());
        {
            let mut members = swarm_members.write().await;
            members.insert("coord".to_string(), coord);
            members.insert("worker".to_string(), worker);
        }

        update_member_status_with_report(
            "worker",
            "ready",
            None,
            Some("Validated the parser and all tests passed.".to_string()),
            &swarm_members,
            &swarms_by_id,
            None,
            None,
            None,
        )
        .await;

        let events: Vec<_> = std::iter::from_fn(|| coord_rx.try_recv().ok()).collect();
        assert!(events.iter().any(|event| {
            matches!(
                event,
                ServerEvent::Notification {
                    notification_type: NotificationType::Message { .. },
                    message,
                    ..
                } if message.contains("Report:\nValidated the parser")
                    && !message.contains("No final textual report")
            )
        }));
    }

    #[tokio::test]
    async fn update_member_status_skips_noop_broadcasts() {
        let swarm_members = Arc::new(RwLock::new(HashMap::new()));
        let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
            "swarm-1".to_string(),
            HashSet::from(["worker".to_string()]),
        )])));

        let (worker, mut worker_rx) = swarm_member("worker", "agent", false);
        swarm_members
            .write()
            .await
            .insert("worker".to_string(), worker);

        update_member_status(
            "worker",
            "ready",
            None,
            &swarm_members,
            &swarms_by_id,
            None,
            None,
            None,
        )
        .await;

        assert!(worker_rx.try_recv().is_err());

        update_member_status(
            "worker",
            "busy",
            Some("working".to_string()),
            &swarm_members,
            &swarms_by_id,
            None,
            None,
            None,
        )
        .await;

        assert!(matches!(
            worker_rx.try_recv(),
            Ok(ServerEvent::SwarmStatus { members }) if members.len() == 1
                && members[0].session_id == "worker"
                && members[0].status == "busy"
                && members[0].detail.as_deref() == Some("working")
        ));
    }

    #[tokio::test]
    async fn refresh_swarm_task_staleness_marks_running_tasks_stale_and_heartbeat_revives() {
        let swarm_members = Arc::new(RwLock::new(HashMap::new()));
        let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
            "swarm-1".to_string(),
            HashSet::from(["worker".to_string()]),
        )])));
        let swarm_coordinators = Arc::new(RwLock::new(HashMap::new()));
        let now_ms = now_unix_ms();
        let stale_age_ms = super::swarm_task_stale_after().as_millis() as u64 + 5_000;
        let swarm_plans = Arc::new(RwLock::new(HashMap::from([(
            "swarm-1".to_string(),
            VersionedPlan {
                items: vec![PlanItem {
                    content: "task".to_string(),
                    status: "running".to_string(),
                    priority: "medium".to_string(),
                    id: "task-1".to_string(),
                    subsystem: None,
                    file_scope: Vec::new(),
                    blocked_by: Vec::new(),
                    assigned_to: Some("worker".to_string()),
                }],
                version: 1,
                participants: HashSet::from(["worker".to_string()]),
                task_progress: HashMap::from([(
                    "task-1".to_string(),
                    crate::server::SwarmTaskProgress {
                        assigned_session_id: Some("worker".to_string()),
                        started_at_unix_ms: Some(now_ms.saturating_sub(stale_age_ms)),
                        last_heartbeat_unix_ms: Some(now_ms.saturating_sub(stale_age_ms)),
                        ..Default::default()
                    },
                )]),
            },
        )])));
        let (worker, _worker_rx) = swarm_member("worker", "agent", true);
        swarm_members
            .write()
            .await
            .insert("worker".to_string(), worker);

        refresh_swarm_task_staleness(
            &swarm_members,
            &swarms_by_id,
            &swarm_plans,
            &swarm_coordinators,
        )
        .await;

        {
            let plans = swarm_plans.read().await;
            let plan = plans.get("swarm-1").expect("plan");
            assert_eq!(plan.items[0].status, "running_stale");
            assert!(
                plan.task_progress
                    .get("task-1")
                    .and_then(|progress| progress.stale_since_unix_ms)
                    .is_some()
            );
        }

        let revived = touch_swarm_task_progress(
            "swarm-1",
            "task-1",
            Some("worker"),
            Some("still working".to_string()),
            Some("checkpoint saved".to_string()),
            &swarm_members,
            &swarms_by_id,
            &swarm_plans,
            &swarm_coordinators,
        )
        .await;
        assert!(revived);

        let plans = swarm_plans.read().await;
        let plan = plans.get("swarm-1").expect("plan");
        assert_eq!(plan.items[0].status, "running");
        let progress = plan.task_progress.get("task-1").expect("progress");
        assert_eq!(
            progress.checkpoint_summary.as_deref(),
            Some("checkpoint saved")
        );
        assert!(progress.stale_since_unix_ms.is_none());
    }
}
