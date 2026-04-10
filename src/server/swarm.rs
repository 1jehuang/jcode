use super::state::{MAX_EVENT_HISTORY, fanout_session_event};
use super::{FileAccess, SwarmEvent, SwarmEventType, SwarmMember, VersionedPlan};
use crate::agent::Agent;
use crate::plan::PlanItem;
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
use std::time::Instant;
use tokio::sync::{Mutex, RwLock, broadcast};

const DEFAULT_SWARM_STATUS_DEBOUNCE_MEMBER_THRESHOLD: usize = 16;
const DEFAULT_SWARM_STATUS_DEBOUNCE_MS: u64 = 30;

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
            .expect("pending swarm status broadcasts lock poisoned");
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
                .expect("pending swarm status broadcasts lock poisoned");
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
    let (version, items, mut participants): (u64, Vec<PlanItem>, Vec<String>) = {
        let plans = swarm_plans.read().await;
        let Some(vp) = plans.get(swarm_id) else {
            return;
        };
        let mut p: Vec<String> = vp.participants.iter().cloned().collect();
        p.sort();
        (vp.version, vp.items.clone(), p)
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

pub(super) async fn remove_session_channel_subscriptions(
    session_id: &str,
    channel_subscriptions: &Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>,
    channel_subscriptions_by_session: &Arc<
        RwLock<HashMap<String, HashMap<String, HashSet<String>>>>,
    >,
) {
    let session_subscriptions = {
        let mut reverse = channel_subscriptions_by_session.write().await;
        reverse.remove(session_id)
    };

    let mut subs = channel_subscriptions.write().await;

    if let Some(session_subscriptions) = session_subscriptions {
        for (swarm_id, channels) in session_subscriptions {
            let mut remove_swarm = false;
            if let Some(swarm_subs) = subs.get_mut(&swarm_id) {
                for channel_name in channels {
                    if let Some(members) = swarm_subs.get_mut(&channel_name) {
                        members.remove(session_id);
                        if members.is_empty() {
                            swarm_subs.remove(&channel_name);
                        }
                    }
                }
                remove_swarm = swarm_subs.is_empty();
            }
            if remove_swarm {
                subs.remove(&swarm_id);
            }
        }
        return;
    }

    let swarm_ids: Vec<String> = subs.keys().cloned().collect();

    for swarm_id in swarm_ids {
        let mut remove_swarm = false;
        if let Some(swarm_subs) = subs.get_mut(&swarm_id) {
            let channel_names: Vec<String> = swarm_subs.keys().cloned().collect();
            for channel_name in channel_names {
                if let Some(members) = swarm_subs.get_mut(&channel_name) {
                    members.remove(session_id);
                    if members.is_empty() {
                        swarm_subs.remove(&channel_name);
                    }
                }
            }
            remove_swarm = swarm_subs.is_empty();
        }
        if remove_swarm {
            subs.remove(&swarm_id);
        }
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

pub(super) async fn subscribe_session_to_channel(
    session_id: &str,
    swarm_id: &str,
    channel: &str,
    channel_subscriptions: &Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>,
    channel_subscriptions_by_session: &Arc<
        RwLock<HashMap<String, HashMap<String, HashSet<String>>>>,
    >,
) {
    {
        let mut subs = channel_subscriptions.write().await;
        subs.entry(swarm_id.to_string())
            .or_default()
            .entry(channel.to_string())
            .or_default()
            .insert(session_id.to_string());
    }

    let mut reverse = channel_subscriptions_by_session.write().await;
    reverse
        .entry(session_id.to_string())
        .or_default()
        .entry(swarm_id.to_string())
        .or_default()
        .insert(channel.to_string());
}

pub(super) async fn unsubscribe_session_from_channel(
    session_id: &str,
    swarm_id: &str,
    channel: &str,
    channel_subscriptions: &Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>,
    channel_subscriptions_by_session: &Arc<
        RwLock<HashMap<String, HashMap<String, HashSet<String>>>>,
    >,
) {
    {
        let mut subs = channel_subscriptions.write().await;
        let mut remove_swarm = false;
        if let Some(swarm_subs) = subs.get_mut(swarm_id) {
            if let Some(members) = swarm_subs.get_mut(channel) {
                members.remove(session_id);
                if members.is_empty() {
                    swarm_subs.remove(channel);
                }
            }
            remove_swarm = swarm_subs.is_empty();
        }
        if remove_swarm {
            subs.remove(swarm_id);
        }
    }

    let mut reverse = channel_subscriptions_by_session.write().await;
    let mut remove_session_entry = false;
    if let Some(session_subs) = reverse.get_mut(session_id) {
        let mut remove_swarm_entry = false;
        if let Some(channels) = session_subs.get_mut(swarm_id) {
            channels.remove(channel);
            remove_swarm_entry = channels.is_empty();
        }
        if remove_swarm_entry {
            session_subs.remove(swarm_id);
        }
        remove_session_entry = session_subs.is_empty();
    }
    if remove_session_entry {
        reverse.remove(session_id);
    }
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
    let (swarm_id, agent_name, member_changed, status_changed, old_status) = {
        let mut members = swarm_members.write().await;
        if let Some(member) = members.get_mut(session_id) {
            let previous_status = member.status.clone();
            let status_changed = member.status != status;
            let detail_changed = member.detail != detail;
            let member_changed = status_changed || detail_changed;
            if status_changed {
                member.last_status_change = Instant::now();
            }
            let name = member.friendly_name.clone();
            member.status = status.to_string();
            member.detail = detail;
            (
                member.swarm_id.clone(),
                name,
                member_changed,
                status_changed,
                previous_status,
            )
        } else {
            (None, None, false, false, String::new())
        }
    };
    if let Some(ref id) = swarm_id {
        if !member_changed {
            return;
        }

        if status_changed {
            if let (Some(history), Some(counter), Some(tx)) =
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
                        old_status,
                        new_status: status.to_string(),
                    },
                )
                .await;
            }
        }

        broadcast_swarm_status(id, swarm_members, swarms_by_id).await;

        if status_changed && status == "completed" {
            let coordinator_id = {
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
            if let Some(coord_id) = coordinator_id {
                let name = agent_name
                    .as_deref()
                    .unwrap_or(&session_id[..8.min(session_id.len())]);
                let msg = format!(
                    "Agent {} completed their work. Use assign_task to give them new work, or stop to remove them.",
                    name
                );
                let _ = fanout_session_event(
                    swarm_members,
                    &coord_id,
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
    for blocked in ["subagent", "task", "todowrite", "todoread"] {
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

    if let (Some(start), Some(end)) = (text.find('['), text.rfind(']')) {
        if start < end {
            if let Ok(tasks) = serde_json::from_str::<Vec<SwarmTaskSpec>>(&text[start..=end]) {
                return tasks;
            }
        }
    }

    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::{
        parse_swarm_tasks, remove_session_from_swarm, summarize_plan_items, truncate_detail,
        update_member_status,
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
                role: role.to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless,
            },
            event_rx,
        )
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
                    blocked_by: Vec::new(),
                    assigned_to: Some("coord".to_string()),
                }],
                version: 1,
                participants: HashSet::from(["coord".to_string()]),
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
}
