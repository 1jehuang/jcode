use super::await_members_state::{
    PersistedAwaitMembersState, all_pending_await_members_including_expired, ensure_pending_state,
    load_state, persist_final_response, request_key, save_state,
};
use super::{AwaitMembersRuntime, SwarmEvent, SwarmMember};
use crate::bus::{Bus, BusEvent, SwarmAwaitCompleted, UiActivity};
use crate::protocol::{AwaitedMemberStatus, ServerEvent, format_comm_awaited_members_with_reports};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{RwLock, broadcast, mpsc};

pub(super) async fn awaited_member_statuses(
    req_session_id: &str,
    swarm_id: &str,
    requested_ids: &[String],
    target_status: &[String],
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
) -> Vec<AwaitedMemberStatus> {
    let watch_ids: Vec<String> = if requested_ids.is_empty() {
        let mut watch_ids: Vec<String> = {
            let swarms = swarms_by_id.read().await;
            swarms
                .get(swarm_id)
                .map(|sessions| {
                    sessions
                        .iter()
                        .filter(|session_id| session_id.as_str() != req_session_id)
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        };
        watch_ids.sort();
        watch_ids
    } else {
        requested_ids.to_vec()
    };

    let members = swarm_members.read().await;
    watch_ids
        .iter()
        .map(|session_id| {
            let (name, status, completion_report) = members
                .get(session_id)
                .map(|member| {
                    (
                        member.friendly_name.clone(),
                        member.status.clone(),
                        member.latest_completion_report.clone(),
                    )
                })
                .unwrap_or((None, "unknown".to_string(), None));
            let done = target_status.contains(&status)
                || (status == "unknown"
                    && (target_status.contains(&"stopped".to_string())
                        || target_status.contains(&"completed".to_string())));
            AwaitedMemberStatus {
                session_id: session_id.clone(),
                friendly_name: name,
                status,
                done,
                completion_report,
            }
        })
        .collect()
}

fn short_member_name(member: &AwaitedMemberStatus) -> String {
    member
        .friendly_name
        .clone()
        .unwrap_or_else(|| member.session_id[..8.min(member.session_id.len())].to_string())
}

pub(super) fn timeout_summary(member_statuses: &[AwaitedMemberStatus]) -> String {
    let pending: Vec<String> = member_statuses
        .iter()
        .filter(|member| !member.done)
        .map(|member| format!("{} ({})", short_member_name(member), member.status))
        .collect();
    format!("Timed out. Still waiting on: {}", pending.join(", "))
}

fn completion_summary(member_statuses: &[AwaitedMemberStatus]) -> String {
    let done_names: Vec<String> = member_statuses.iter().map(short_member_name).collect();
    format!(
        "All {} members are done: {}",
        done_names.len(),
        done_names.join(", ")
    )
}

pub(super) fn completion_mode(mode: Option<&str>) -> &str {
    match mode {
        Some("any") => "any",
        _ => "all",
    }
}

pub(super) fn mode_satisfied(member_statuses: &[AwaitedMemberStatus], mode: Option<&str>) -> bool {
    match completion_mode(mode) {
        "any" => member_statuses.iter().any(|status| status.done),
        _ => member_statuses.iter().all(|status| status.done),
    }
}

pub(super) fn mode_summary(member_statuses: &[AwaitedMemberStatus], mode: Option<&str>) -> String {
    match completion_mode(mode) {
        "any" => {
            let matching: Vec<String> = member_statuses
                .iter()
                .filter(|member| member.done)
                .map(short_member_name)
                .collect();
            format!(
                "Matched {} member{}: {}",
                matching.len(),
                if matching.len() == 1 { "" } else { "s" },
                matching.join(", ")
            )
        }
        _ => completion_summary(member_statuses),
    }
}

pub(super) fn deadline_to_instant(deadline_unix_ms: u64) -> tokio::time::Instant {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    tokio::time::Instant::now() + Duration::from_millis(deadline_unix_ms.saturating_sub(now_ms))
}

pub(super) async fn respond_to_waiters(
    runtime: &AwaitMembersRuntime,
    key: &str,
    completed: bool,
    members: Vec<AwaitedMemberStatus>,
    summary: String,
) {
    for (request_id, client_event_tx) in runtime.take_waiters(key).await {
        let _ = client_event_tx.send(ServerEvent::CommAwaitMembersResponse {
            id: request_id,
            completed,
            members: members.clone(),
            summary: summary.clone(),
            background_started: false,
        });
    }
    runtime.clear_active(key).await;
}

/// Build the swarm-flavored completion notification body delivered to the
/// requesting agent when a backgrounded await finishes. Reuses the same
/// member-status + completion-report rendering as the blocking tool result so
/// the agent sees consistent output whether it waited inline or in the
/// background.
fn background_completion_notification(
    completed: bool,
    summary: &str,
    members: &[AwaitedMemberStatus],
) -> String {
    let reports = HashMap::new();
    let body = format_comm_awaited_members_with_reports(completed, summary, members, &reports);
    format!("🐝 **Swarm await finished**\n\n{}", body)
}

/// Reload the latest persisted pending state for `state.key`, if any. Delivery
/// prefs (background/notify/wake) can be updated by duplicate requests after a
/// watcher captured its own copy at spawn, so re-reading before exit/finalize
/// keeps the watcher in sync with what the requesting tool was last told.
fn refresh_pending_state(state: &PersistedAwaitMembersState) -> Option<PersistedAwaitMembersState> {
    load_state(&state.key).filter(PersistedAwaitMembersState::is_pending)
}

/// Persist the terminal result, reply to any blocking socket waiters, and, when
/// the await was started in background mode, publish a `SwarmAwaitCompleted`
/// bus event so the server's bus monitor can wake/notify the requesting agent
/// the same way background tasks do.
async fn finalize_await(
    runtime: &AwaitMembersRuntime,
    state: &PersistedAwaitMembersState,
    completed: bool,
    members: Vec<AwaitedMemberStatus>,
    summary: String,
) {
    // Deliver with the latest persisted prefs: a duplicate request may have
    // changed background/notify/wake after the caller captured this copy.
    let state = refresh_pending_state(state).unwrap_or_else(|| state.clone());
    let _ = persist_final_response(&state, completed, members.clone(), summary.clone());

    if state.background && (state.notify || state.wake) {
        let notification = background_completion_notification(completed, &summary, &members);
        Bus::global().publish(BusEvent::SwarmAwaitCompleted(SwarmAwaitCompleted {
            session_id: state.session_id.clone(),
            completed,
            summary: summary.clone(),
            notification,
            notify: state.notify,
            wake: state.wake,
        }));
    }

    respond_to_waiters(runtime, &state.key, completed, members, summary).await;
}

pub(super) async fn spawn_or_resume_await_members(
    state: PersistedAwaitMembersState,
    req_session_id: String,
    swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_event_tx: broadcast::Sender<SwarmEvent>,
    await_members_runtime: AwaitMembersRuntime,
) {
    let key = state.key.clone();
    let swarm_id = state.swarm_id.clone();
    let requested_ids = state.requested_ids.clone();
    let target_status = state.target_status.clone();
    let mode = state.mode.clone();

    tokio::spawn(async move {
        let mut event_rx = swarm_event_tx.subscribe();
        let deadline = deadline_to_instant(state.deadline_unix_ms);

        loop {
            let member_statuses = awaited_member_statuses(
                &req_session_id,
                &swarm_id,
                &requested_ids,
                &target_status,
                &swarm_members,
                &swarms_by_id,
            )
            .await;

            if member_statuses.is_empty() {
                let summary = "No other members in swarm to wait for.".to_string();
                finalize_await(&await_members_runtime, &state, true, vec![], summary).await;
                return;
            }

            if mode_satisfied(&member_statuses, mode.as_deref()) {
                let summary = mode_summary(&member_statuses, mode.as_deref());
                finalize_await(
                    &await_members_runtime,
                    &state,
                    true,
                    member_statuses,
                    summary,
                )
                .await;
                return;
            }

            // Blocking waits stop watching once every socket waiter has
            // disconnected. Background watchers have no socket waiter, so they
            // keep running until they resolve or hit the deadline, delivering
            // the result via notify/wake. Re-read the persisted prefs here: a
            // duplicate request may have upgraded this wait to background mode
            // after this watcher was spawned with a blocking-state copy.
            let is_background = refresh_pending_state(&state)
                .map(|latest| latest.background)
                .unwrap_or(state.background);
            if !is_background && await_members_runtime.retain_open_waiters(&key).await == 0 {
                await_members_runtime.clear_active(&key).await;
                return;
            }

            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => {
                    let summary = timeout_summary(&member_statuses);
                    finalize_await(&await_members_runtime, &state, false, member_statuses, summary).await;
                    return;
                }
                event = event_rx.recv() => {
                    match event {
                        Ok(event) => {
                            if event.swarm_id.as_deref() != Some(swarm_id.as_str()) {
                                continue;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            // Dropped events are recoverable: the loop re-reads
                            // member statuses from shared state at the top, so
                            // just keep watching instead of orphaning the wait.
                            crate::logging::info(&format!(
                                "await_members watcher lagged by {} swarm events; re-checking statuses",
                                n
                            ));
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            await_members_runtime.clear_active(&key).await;
                            return;
                        }
                    }
                }
            }
        }
    });
}

pub(super) struct CommAwaitMembersContext<'a> {
    pub client_event_tx: &'a mpsc::UnboundedSender<ServerEvent>,
    pub swarm_members: &'a Arc<RwLock<HashMap<String, SwarmMember>>>,
    pub swarms_by_id: &'a Arc<RwLock<HashMap<String, HashSet<String>>>>,
    pub swarm_event_tx: &'a broadcast::Sender<SwarmEvent>,
    pub await_members_runtime: &'a AwaitMembersRuntime,
}

#[expect(
    clippy::too_many_arguments,
    reason = "await request carries protocol fields plus delivery flags; grouping would churn many call sites"
)]
pub(super) async fn handle_comm_await_members(
    id: u64,
    req_session_id: String,
    target_status: Vec<String>,
    requested_ids: Vec<String>,
    mode: Option<String>,
    timeout_secs: Option<u64>,
    background: bool,
    notify: bool,
    wake: bool,
    ctx: CommAwaitMembersContext<'_>,
) {
    let swarm_id = {
        let members = ctx.swarm_members.read().await;
        members
            .get(&req_session_id)
            .and_then(|member| member.swarm_id.clone())
    };

    if let Some(swarm_id) = swarm_id {
        let key = request_key(
            &req_session_id,
            &swarm_id,
            &requested_ids,
            &target_status,
            mode.as_deref(),
        );
        let mut persisted = load_state(&key);

        let initial_statuses = awaited_member_statuses(
            &req_session_id,
            &swarm_id,
            &requested_ids,
            &target_status,
            ctx.swarm_members,
            ctx.swarms_by_id,
        )
        .await;

        if let Some(final_response) = persisted
            .as_ref()
            .and_then(|state| state.final_response.clone())
        {
            let current_still_satisfies =
                initial_statuses.is_empty() || mode_satisfied(&initial_statuses, mode.as_deref());
            if current_still_satisfies {
                let _ = ctx
                    .client_event_tx
                    .send(ServerEvent::CommAwaitMembersResponse {
                        id,
                        completed: final_response.completed,
                        members: final_response.members,
                        summary: final_response.summary,
                        background_started: false,
                    });
                return;
            }

            persisted = None;
        }

        if initial_statuses.is_empty() {
            let _ = ctx
                .client_event_tx
                .send(ServerEvent::CommAwaitMembersResponse {
                    id,
                    completed: true,
                    members: vec![],
                    summary: "No other members in swarm to wait for.".to_string(),
                    background_started: false,
                });
            return;
        }

        // Already satisfied right now: answer inline regardless of background
        // mode. There is nothing to wait for, so the agent should get the
        // result immediately instead of a "watching in background" stub.
        if mode_satisfied(&initial_statuses, mode.as_deref()) {
            let summary = mode_summary(&initial_statuses, mode.as_deref());
            let _ = ctx
                .client_event_tx
                .send(ServerEvent::CommAwaitMembersResponse {
                    id,
                    completed: true,
                    members: initial_statuses,
                    summary,
                    background_started: false,
                });
            return;
        }

        let requested_deadline = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
            + Duration::from_secs(timeout_secs.unwrap_or(3600)).as_millis() as u64;
        let mut state = persisted.unwrap_or_else(|| {
            ensure_pending_state(
                &key,
                &req_session_id,
                &swarm_id,
                &requested_ids,
                &target_status,
                mode.as_deref(),
                requested_deadline,
                background,
                notify,
                wake,
            )
        });

        // When reusing a persisted pending state (e.g. a resumed call after
        // reload, or a duplicate request), let the latest call's delivery prefs
        // win so the watcher and tool response stay in sync. The deadline is
        // intentionally preserved from the original request.
        if state.background != background || state.notify != notify || state.wake != wake {
            state.background = background;
            state.notify = notify;
            state.wake = wake;
            save_state(&state);
        }

        let already_expired = state.deadline_unix_ms
            <= SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;

        // Background mode: hand off to a detached watcher and answer the tool
        // immediately so the requesting turn stays responsive. Completion is
        // delivered later via notify/wake.
        if background {
            if already_expired {
                let summary = timeout_summary(&initial_statuses);
                finalize_await(
                    ctx.await_members_runtime,
                    &state,
                    false,
                    initial_statuses.clone(),
                    summary.clone(),
                )
                .await;
                // Answer the requesting tool call directly: no waiter was
                // registered for this request (waiters are only added in the
                // blocking branch), so without this the socket call would hang
                // until its client-side timeout.
                let _ = ctx
                    .client_event_tx
                    .send(ServerEvent::CommAwaitMembersResponse {
                        id,
                        completed: false,
                        members: initial_statuses,
                        summary,
                        background_started: false,
                    });
                return;
            }

            if ctx.await_members_runtime.mark_active_if_new(&key).await {
                publish_await_started_card(&state, &initial_statuses);
                spawn_or_resume_await_members(
                    state,
                    req_session_id,
                    ctx.swarm_members.clone(),
                    ctx.swarms_by_id.clone(),
                    ctx.swarm_event_tx.clone(),
                    ctx.await_members_runtime.clone(),
                )
                .await;
            }

            let summary = background_started_summary(&initial_statuses, mode.as_deref(), wake);
            let _ = ctx
                .client_event_tx
                .send(ServerEvent::CommAwaitMembersResponse {
                    id,
                    completed: false,
                    members: initial_statuses,
                    summary,
                    background_started: true,
                });
            return;
        }

        // Blocking mode: register a socket waiter that the watcher resolves.
        ctx.await_members_runtime
            .add_waiter(&key, id, ctx.client_event_tx)
            .await;

        if already_expired {
            let summary = timeout_summary(&initial_statuses);
            let _ =
                persist_final_response(&state, false, initial_statuses.clone(), summary.clone());
            respond_to_waiters(
                ctx.await_members_runtime,
                &key,
                false,
                initial_statuses,
                summary,
            )
            .await;
            return;
        }

        if ctx.await_members_runtime.mark_active_if_new(&key).await {
            spawn_or_resume_await_members(
                state,
                req_session_id,
                ctx.swarm_members.clone(),
                ctx.swarms_by_id.clone(),
                ctx.swarm_event_tx.clone(),
                ctx.await_members_runtime.clone(),
            )
            .await;
        }
    } else {
        let _ = ctx.client_event_tx.send(ServerEvent::Error {
            id,
            message: "Not in a swarm. Use a git repository to enable swarm features.".to_string(),
            retry_after_secs: None,
            server_resumes: false,
        });
    }
}

/// One-line summary returned to the tool when a wait is handed off to a
/// background watcher.
fn background_started_summary(
    member_statuses: &[AwaitedMemberStatus],
    mode: Option<&str>,
    wake: bool,
) -> String {
    let pending: Vec<String> = member_statuses
        .iter()
        .filter(|member| !member.done)
        .map(short_member_name)
        .collect();
    let scope = match completion_mode(mode) {
        "any" => "any of",
        _ => "all of",
    };
    let delivery = if wake {
        "You'll be woken with the result when it resolves."
    } else {
        "A notification will appear when it resolves."
    };
    if pending.is_empty() {
        format!("Watching swarm members in the background. {}", delivery)
    } else {
        format!(
            "Watching {} {} in the background. {}",
            scope,
            pending.join(", "),
            delivery
        )
    }
}

/// Emit a swarm-flavored "await started" activity card so attached clients show
/// that a background watcher is now running for this session.
fn publish_await_started_card(
    state: &PersistedAwaitMembersState,
    member_statuses: &[AwaitedMemberStatus],
) {
    if !state.notify {
        return;
    }
    let pending: Vec<String> = member_statuses
        .iter()
        .filter(|member| !member.done)
        .map(short_member_name)
        .collect();
    let watching = if pending.is_empty() {
        "swarm members".to_string()
    } else {
        pending.join(", ")
    };
    Bus::global().publish(BusEvent::UiActivity(UiActivity::background(
        Some(state.session_id.clone()),
        format!(
            "🐝 **Swarm await started** · watching `{}`\n\nJcode is waiting for these members in the background and will report back when they finish.",
            watching
        ),
        Some(format!("Swarm await started · {}", watching)),
    )));
}

/// Re-spawn detached watchers for every pending background `await_members`
/// state after a server (re)start. Blocking waits are intentionally skipped:
/// their requesting tool call is parked on a socket that no longer exists, and
/// the agent is told to rerun the wait after reload. Background waits, by
/// contrast, deliver via notify/wake, so they can resume transparently.
pub(super) async fn resume_background_awaits(
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    await_members_runtime: &AwaitMembersRuntime,
) {
    let pending: Vec<PersistedAwaitMembersState> = all_pending_await_members_including_expired()
        .into_iter()
        .filter(|state| state.background)
        .collect();
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let mut resumed = 0usize;
    let mut expired = 0usize;
    for state in pending {
        // Deadline passed while the server was down: the wait can never
        // resolve, so finalize it as a timeout now so the promised
        // notify/wake still fires instead of the await silently vanishing.
        if state.deadline_unix_ms <= now_ms {
            let member_statuses = awaited_member_statuses(
                &state.session_id,
                &state.swarm_id,
                &state.requested_ids,
                &state.target_status,
                swarm_members,
                swarms_by_id,
            )
            .await;
            let (completed, summary) = if member_statuses.is_empty() {
                (true, "No other members in swarm to wait for.".to_string())
            } else if mode_satisfied(&member_statuses, state.mode.as_deref()) {
                (true, mode_summary(&member_statuses, state.mode.as_deref()))
            } else {
                (false, timeout_summary(&member_statuses))
            };
            finalize_await(
                await_members_runtime,
                &state,
                completed,
                member_statuses,
                summary,
            )
            .await;
            expired += 1;
            continue;
        }

        let key = state.key.clone();
        if await_members_runtime.mark_active_if_new(&key).await {
            let req_session_id = state.session_id.clone();
            spawn_or_resume_await_members(
                state,
                req_session_id,
                swarm_members.clone(),
                swarms_by_id.clone(),
                swarm_event_tx.clone(),
                await_members_runtime.clone(),
            )
            .await;
            resumed += 1;
        }
    }

    if resumed > 0 || expired > 0 {
        crate::logging::info(&format!(
            "Resumed {} background swarm await watcher(s) after startup ({} finalized as expired)",
            resumed, expired
        ));
    }
}
