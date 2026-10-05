use super::{
    ClientConnectionInfo, FileTouchService, SessionInterruptQueues, SwarmEvent, SwarmMember,
    SwarmState, VersionedPlan, broadcast_swarm_status, fanout_live_client_event,
    persist_swarm_state_for, remove_background_tool_signal, remove_plan_participant,
    remove_session_channel_subscriptions, remove_session_from_swarm,
    remove_session_interrupt_queue, send_swarm_plan_to_session, swarm_id_for_session,
    unregister_session_event_sender, update_member_status,
};
use crate::agent::Agent;
use crate::protocol::{NotificationType, ServerEvent};
use crate::tool::Registry;
use futures::FutureExt;
use jcode_agent_runtime::InterruptSignal;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

use super::client_session::{
    ChannelSubscriptions, SessionAgents, ensure_client_swarm_member, mark_remote_reload_started,
    session_working_dir_for_client,
};

/// Record a client event that could not be delivered, instead of discarding the failure.
///
/// A closed receiver means the client disconnected, which is routine and not worth a
/// warning. It is still worth a debug line: these events are load-bearing, and losing one
/// leaves the client with no way to learn it happened. Losing `SessionId` means a remote
/// client cannot reattach; losing `Done` means the caller waits forever.
///
/// Returns whether the event was delivered, so callers and tests can observe delivery
/// instead of assuming it.
fn notify_client<T>(client_event_tx: &mpsc::UnboundedSender<T>, event: T, what: &str) -> bool {
    match client_event_tx.send(event) {
        Ok(()) => true,
        Err(err) => {
            crate::logging::debug(&format!(
                "notify_client: {} undeliverable ({}); the client has disconnected",
                what, err
            ));
            false
        }
    }
}

pub(super) fn effective_subscribe_working_dir(
    current: Option<&str>,
    reported: &str,
    home: Option<&Path>,
) -> String {
    match subscribe_working_dir_replacement(current, reported, home) {
        Some(accepted) => accepted,
        None => current
            .map(str::to_string)
            .unwrap_or_else(|| reported.trim().to_string()),
    }
}

/// Decide whether a client-reported subscribe cwd may replace the session's
/// current working directory.
///
/// Requiring a subscribe cwd to be non-empty and absolute (the earlier
/// require-cwd change) is necessary but not sufficient: a client that launches
/// with an inherited environment can report the user's *home* directory even
/// though the real project lives elsewhere. Accepting that silently re-pins the
/// session to home, so bash/file tools run against home while the header still
/// shows the project path (issue #481).
///
/// The rule is deliberately narrow so it cannot break legitimate directory
/// changes: a reported cwd that is exactly the home directory is ignored *only*
/// when the session already has a different working directory. Working in home
/// on purpose (no prior cwd, or a session already pinned to home) still works,
/// and every other path is accepted as before.
pub(super) fn subscribe_working_dir_replacement(
    current: Option<&str>,
    reported: &str,
    home: Option<&Path>,
) -> Option<String> {
    let reported_trimmed = reported.trim();
    if reported_trimmed.is_empty() {
        return None;
    }
    let current = current.map(str::trim).filter(|dir| !dir.is_empty());
    if current == Some(reported_trimmed) {
        return None;
    }
    if let (Some(current), Some(home)) = (current, home)
        && Path::new(reported_trimmed) == home
        && Path::new(current) != home
    {
        return None;
    }
    Some(reported_trimmed.to_string())
}

/// Why a client-reported subscribe directory was refused.
///
/// The two reasons need distinct wording. A *home* report means the client inherited
/// `$HOME` instead of its project (issue #481). Any other differing report means the
/// client is in a different project than the session it attached to, which the
/// creation-only rule in `session_working_dir_for_client` refuses. Logging the home
/// wording for a cross-project attach would name the wrong cause.
fn log_ignored_subscribe_working_dir(
    session_id: &str,
    current: &str,
    reported: &str,
    reason: SubscribeWorkingDirRefusal,
) {
    crate::logging::warn(&subscribe_working_dir_refusal_message(
        session_id, current, reported, reason,
    ));
}

/// The text of a refusal, kept separate from the logging call so it can be asserted.
///
/// The reason has to reach a human reading the log, not just a match arm. Building the
/// string here means a test can check that a cross-project refusal does not describe a
/// home directory and vice versa.
pub(super) fn subscribe_working_dir_refusal_message(
    session_id: &str,
    current: &str,
    reported: &str,
    reason: SubscribeWorkingDirRefusal,
) -> String {
    let cause = match reason {
        SubscribeWorkingDirRefusal::HomeDirectory => {
            "the client reported the home directory while the session is bound to a project (issue #481)"
        }
        SubscribeWorkingDirRefusal::CrossProject => {
            "a client-reported directory is creation-only and never moves an existing session to another project"
        }
    };
    format!(
        "Ignoring subscribe working_dir {reported} for session {session_id}: {cause}; the session stays bound to {current}"
    )
}

/// Whether two reported directories are the same project.
///
/// `subscribe_working_dir_replacement` compares the strings literally, so
/// `/repo` and `/repo/.` disagree with each other and the caller would then log a
/// refusal for a directory that is in fact unchanged. Compare canonically so a
/// cosmetic difference does not read as a cross-project attach.
fn same_working_dir(a: &str, b: &str) -> bool {
    super::util::canonicalize_or(a.into()) == super::util::canonicalize_or(b.into())
}

/// Classify why a reported directory was refused, or `None` when it was merely
/// unchanged and there is nothing worth warning about.
///
/// Kept next to [`SubscribeWorkingDirRefusal`] so the reason a log line claims and
/// the reason the code took cannot drift apart.
pub(super) fn subscribe_working_dir_refusal_reason(
    current: &str,
    reported: &str,
    home: Option<&Path>,
) -> Option<SubscribeWorkingDirRefusal> {
    if same_working_dir(current, reported) {
        return None;
    }
    let reported = reported.trim();
    if let Some(home) = home
        && Path::new(reported) == home
        && !same_working_dir(current, &home.to_string_lossy())
    {
        return Some(SubscribeWorkingDirRefusal::HomeDirectory);
    }
    Some(SubscribeWorkingDirRefusal::CrossProject)
}

/// Why `apply_or_defer_subscribe_working_dir` refused a reported directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SubscribeWorkingDirRefusal {
    /// The report was the home directory and the session had a different project.
    HomeDirectory,
    /// The report names a different project than the session belongs to.
    CrossProject,
    // No `Unchanged` variant on purpose: an unchanged report is not a refusal, so
    // `subscribe_working_dir_refusal_reason` returns `None` for it instead of a
    // reason that would have to be handled as "nothing happened".
}

pub(super) fn apply_or_defer_subscribe_working_dir(
    agent: &Arc<Mutex<Agent>>,
    working_dir: &str,
    session_id: &str,
) {
    let home = dirs::home_dir();
    if let Ok(mut agent_guard) = agent.try_lock() {
        match subscribe_working_dir_replacement(
            agent_guard.working_dir(),
            working_dir,
            home.as_deref(),
        ) {
            Some(accepted) => {
                // An existing project directory always wins over a client-reported
                // one. See `session_working_dir_for_client`: a client's directory
                // is creation-only, and a target attachment reports the attaching
                // client's project, not this session's.
                let accepted = match agent_guard.working_dir() {
                    Some(existing) => {
                        session_working_dir_for_client(Some(existing), Some(&accepted), false)
                    }
                    None => Some(accepted),
                };
                if let Some(accepted) = accepted {
                    agent_guard.set_working_dir(&accepted);
                } else if let Some(current) = agent_guard.working_dir()
                    && !same_working_dir(current, working_dir)
                {
                    log_ignored_subscribe_working_dir(
                        session_id,
                        current,
                        working_dir,
                        SubscribeWorkingDirRefusal::CrossProject,
                    );
                }
            }
            None => {
                if let Some(current) = agent_guard.working_dir()
                    && current != working_dir
                    && let Some(reason) =
                        subscribe_working_dir_refusal_reason(current, working_dir, home.as_deref())
                {
                    log_ignored_subscribe_working_dir(session_id, current, working_dir, reason);
                }
            }
        }
        return;
    }

    let agent = Arc::clone(agent);
    let working_dir = working_dir.to_string();
    let session_id = session_id.to_string();
    tokio::spawn(async move {
        let mut agent_guard = agent.lock().await;
        match subscribe_working_dir_replacement(
            agent_guard.working_dir(),
            &working_dir,
            home.as_deref(),
        ) {
            Some(accepted) => {
                // Same rule as the synchronous branch above. Without it the guard
                // would hold only while the session was idle and quietly lapse
                // mid-turn, which is exactly when a desktop attaches to a live
                // session.
                let accepted = match agent_guard.working_dir() {
                    Some(existing) => {
                        session_working_dir_for_client(Some(existing), Some(&accepted), false)
                    }
                    None => Some(accepted),
                };
                match accepted {
                    Some(accepted) => {
                        agent_guard.set_working_dir(&accepted);
                        crate::logging::info(&format!(
                            "Applied deferred subscribe working directory for session {}",
                            session_id
                        ));
                    }
                    None => {
                        if let Some(current) = agent_guard.working_dir()
                            && !same_working_dir(current, &working_dir)
                        {
                            log_ignored_subscribe_working_dir(
                                &session_id,
                                current,
                                &working_dir,
                                SubscribeWorkingDirRefusal::CrossProject,
                            );
                        }
                    }
                }
            }
            None => {
                if let Some(current) = agent_guard.working_dir()
                    && current != working_dir
                    && let Some(reason) =
                        subscribe_working_dir_refusal_reason(current, &working_dir, home.as_deref())
                {
                    log_ignored_subscribe_working_dir(&session_id, current, &working_dir, reason);
                }
            }
        }
    });
}

fn apply_or_defer_subscribe_selfdev(agent: &Arc<Mutex<Agent>>, session_id: &str) {
    if let Ok(mut agent_guard) = agent.try_lock() {
        if !agent_guard.is_canary() {
            agent_guard.set_canary("self-dev");
        }
        return;
    }

    let agent = Arc::clone(agent);
    let session_id = session_id.to_string();
    tokio::spawn(async move {
        let mut agent_guard = agent.lock().await;
        if !agent_guard.is_canary() {
            agent_guard.set_canary("self-dev");
        }
        crate::logging::info(&format!(
            "Applied deferred self-dev subscribe metadata for session {}",
            session_id
        ));
    });
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_subscribe(
    id: u64,
    subscribe_working_dir: Option<String>,
    selfdev: Option<bool>,
    register_mcp_tools: bool,
    client_selfdev: &mut bool,
    client_session_id: &str,
    client_connection_id: &str,
    friendly_name: &Option<String>,
    agent: &Arc<Mutex<Agent>>,
    registry: &Registry,
    swarm_enabled: bool,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    channel_subscriptions: &ChannelSubscriptions,
    channel_subscriptions_by_session: &ChannelSubscriptions,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    mcp_pool: &Arc<crate::mcp::SharedMcpPool>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) {
    let subscribe_start = Instant::now();
    crate::logging::event_info(
        "SESSION_LIFECYCLE",
        vec![
            ("phase", "subscribe_start".to_string()),
            ("request_id", id.to_string()),
            ("session_id", client_session_id.to_string()),
            ("client_connection_id", client_connection_id.to_string()),
            (
                "working_dir_set",
                subscribe_working_dir.is_some().to_string(),
            ),
            ("register_mcp_tools", register_mcp_tools.to_string()),
            ("swarm_enabled", swarm_enabled.to_string()),
        ],
    );
    let inserted_swarm_member = ensure_client_swarm_member(
        client_session_id,
        client_connection_id,
        friendly_name,
        client_event_tx,
        agent,
        swarm_enabled,
        swarm_members,
        swarms_by_id,
        event_history,
        event_counter,
        swarm_event_tx,
    )
    .await;

    if let Some(ref dir) = subscribe_working_dir {
        // A client-reported directory never re-pins a session that already has
        // one. The desktop sends a Subscribe with its own cwd straight after a
        // target-aware resume, so honoring it here would undo the directory the
        // resume just preserved and move a session into the attaching client's
        // project. `apply_or_defer_subscribe_working_dir` applies the same
        // decision, so the agent and the swarm/mcp resolution below agree.
        let existing_dir = current_working_dir(agent, client_session_id);
        let bound_dir = session_working_dir_for_client(existing_dir.as_deref(), Some(dir), true);
        if bound_dir.as_deref() != Some(dir) {
            crate::logging::warn(&format!(
                "Ignoring subscribe working_dir {} for session {}: the session is already bound to {} (a client-reported directory is creation-only)",
                dir,
                client_session_id,
                existing_dir.as_deref().unwrap_or("<unset>"),
            ));
        }
        apply_or_defer_subscribe_working_dir(agent, dir, client_session_id);

        // Swarm grouping must use the *bound* directory, not the raw report, or a
        // subscribe would re-key the session's swarm even though its agent stayed
        // put. Two rules decide that bound directory and they must be applied
        // together: a reported home directory never displaces an established
        // project (issue #481), and a client-reported directory never displaces an
        // existing one at all (creation-only). Applying only the home rule here
        // left the swarm keyed to the attaching client's project while the
        // session's own tools ran in its own.
        let bound_dir = {
            let current = current_working_dir(agent, client_session_id);
            let after_home_rule = effective_subscribe_working_dir(
                current.as_deref(),
                dir,
                dirs::home_dir().as_deref(),
            );
            session_working_dir_for_client(current.as_deref(), Some(&after_home_rule), false)
                .unwrap_or(after_home_rule)
        };
        let new_path = PathBuf::from(&bound_dir);
        let mut old_swarm_id: Option<String> = None;
        let mut updated_swarm_id: Option<String> = None;
        {
            let mut members = swarm_members.write().await;
            if let Some(member) = members.get_mut(client_session_id) {
                old_swarm_id = member.swarm_id.clone();
                // Existing members include reconnects and daemon-restored
                // sessions. Keep their persisted swarm id so an intentional
                // resume retains its workers and plan. Only a newly inserted
                // root receives the new session-scoped identity.
                let new_swarm_id = if inserted_swarm_member {
                    swarm_id_for_session(client_session_id)
                } else {
                    member
                        .swarm_id
                        .clone()
                        .or_else(|| swarm_id_for_session(client_session_id))
                };
                member.working_dir = Some(new_path);
                member.swarm_id = if member.swarm_enabled {
                    new_swarm_id.clone()
                } else {
                    None
                };
                updated_swarm_id = member.swarm_id.clone();
            }
        }

        if let Some(ref old_id) = old_swarm_id {
            if updated_swarm_id.as_ref() != Some(old_id) {
                remove_session_channel_subscriptions(
                    client_session_id,
                    channel_subscriptions,
                    channel_subscriptions_by_session,
                )
                .await;
            }
            let mut swarms = swarms_by_id.write().await;
            if let Some(swarm) = swarms.get_mut(old_id) {
                swarm.remove(client_session_id);
                if swarm.is_empty() {
                    swarms.remove(old_id);
                }
            }
        }

        if let Some(ref new_id) = updated_swarm_id {
            let mut swarms = swarms_by_id.write().await;
            swarms
                .entry(new_id.clone())
                .or_insert_with(HashSet::new)
                .insert(client_session_id.to_string());
        }

        if updated_swarm_id != old_swarm_id {
            crate::logging::event_info(
                "SESSION_LIFECYCLE",
                vec![
                    ("phase", "subscribe_swarm_changed".to_string()),
                    ("session_id", client_session_id.to_string()),
                    ("client_connection_id", client_connection_id.to_string()),
                    (
                        "old_swarm_id",
                        old_swarm_id.clone().unwrap_or_else(|| "none".to_string()),
                    ),
                    (
                        "new_swarm_id",
                        updated_swarm_id
                            .clone()
                            .unwrap_or_else(|| "none".to_string()),
                    ),
                ],
            );
            let mut members = swarm_members.write().await;
            if let Some(member) = members.get_mut(client_session_id) {
                member.role = "agent".to_string();
            }
        }

        if let Some(old_id) = old_swarm_id.clone() {
            let was_coordinator = {
                let coordinators = swarm_coordinators.read().await;
                coordinators
                    .get(&old_id)
                    .map(|session_id| session_id == client_session_id)
                    .unwrap_or(false)
            };
            if was_coordinator {
                let mut new_coordinator: Option<String> = None;
                {
                    let swarms = swarms_by_id.read().await;
                    if let Some(swarm) = swarms.get(&old_id) {
                        new_coordinator = swarm.iter().min().cloned();
                    }
                }
                {
                    let mut coordinators = swarm_coordinators.write().await;
                    coordinators.remove(&old_id);
                    if let Some(ref new_id) = new_coordinator {
                        coordinators.insert(old_id.clone(), new_id.clone());
                    }
                }
                if let Some(new_id) = new_coordinator.clone() {
                    let members = swarm_members.read().await;
                    if let Some(member) = members.get(&new_id) {
                        notify_client(
                            &member.event_tx,
                            ServerEvent::Notification {
                                from_session: new_id.clone(),
                                from_name: member.friendly_name.clone(),
                                notification_type: NotificationType::Message {
                                    scope: Some("swarm".to_string()),
                                    channel: None,
                                    tldr: None,
                                },
                                message: "You are now the coordinator for this swarm.".to_string(),
                            },
                            "swarm member notification",
                        );
                    }
                }
            }
        }

        if let Some(old_id) = old_swarm_id.clone() {
            if updated_swarm_id.as_ref() != Some(&old_id) {
                remove_plan_participant(&old_id, client_session_id, swarm_plans).await;
                let swarm_state = SwarmState {
                    members: Arc::clone(swarm_members),
                    swarms_by_id: Arc::clone(swarms_by_id),
                    plans: Arc::clone(swarm_plans),
                    coordinators: Arc::clone(swarm_coordinators),
                };
                persist_swarm_state_for(&old_id, &swarm_state).await;
            }
            broadcast_swarm_status(&old_id, swarm_members, swarms_by_id).await;
        }
        if let Some(new_id) = updated_swarm_id
            && old_swarm_id.as_ref() != Some(&new_id)
        {
            broadcast_swarm_status(&new_id, swarm_members, swarms_by_id).await;
        }
    }

    let should_selfdev = *client_selfdev || matches!(selfdev, Some(true));

    if should_selfdev {
        *client_selfdev = true;
        apply_or_defer_subscribe_selfdev(agent, client_session_id);
        registry.register_selfdev_tools().await;
    }

    let mcp_register_ms = if register_mcp_tools {
        let mcp_register_start = Instant::now();
        // Resolve project-local MCP config against the session working dir,
        // not the server process cwd (issue #420). Prefer the subscribe
        // request's dir; fall back to the agent's stored session dir.
        let mcp_working_dir = match subscribe_working_dir.as_ref() {
            // Resolve against the bound directory so a rejected home-dir report
            // cannot point project-local MCP discovery at home (issue #481).
            Some(dir) => {
                let current = current_working_dir(agent, client_session_id);
                Some(PathBuf::from(effective_subscribe_working_dir(
                    current.as_deref(),
                    dir,
                    dirs::home_dir().as_deref(),
                )))
            }
            None => current_working_dir(agent, client_session_id).map(PathBuf::from),
        };
        registry
            .register_mcp_tools_for_dir(
                Some(client_event_tx.clone()),
                Some(Arc::clone(mcp_pool)),
                Some(client_session_id.to_string()),
                mcp_working_dir,
            )
            .await;
        mcp_register_start.elapsed().as_millis()
    } else {
        0
    };

    crate::logging::info(&format!(
        "[TIMING] handle_subscribe: session={}, working_dir_set={}, selfdev={}, mcp_register={}ms, total={}ms",
        client_session_id,
        subscribe_working_dir.is_some(),
        should_selfdev,
        mcp_register_ms,
        subscribe_start.elapsed().as_millis(),
    ));
    crate::logging::event_info(
        "SESSION_LIFECYCLE",
        vec![
            ("phase", "subscribe_done".to_string()),
            ("request_id", id.to_string()),
            ("session_id", client_session_id.to_string()),
            ("client_connection_id", client_connection_id.to_string()),
            ("mcp_register_ms", mcp_register_ms.to_string()),
            (
                "elapsed_ms",
                subscribe_start.elapsed().as_millis().to_string(),
            ),
        ],
    );

    if subscribe_should_mark_ready(client_session_id, swarm_members).await {
        update_member_status(
            client_session_id,
            "ready",
            None,
            swarm_members,
            swarms_by_id,
            Some(event_history),
            Some(event_counter),
            Some(swarm_event_tx),
        )
        .await;
    }

    // Re-send the current swarm plan so a reconnecting client renders the
    // plan graph immediately instead of waiting for the next plan mutation.
    send_swarm_plan_to_session(client_session_id, swarm_members, swarm_plans).await;

    // Tell the client which session it is bound to. Local clients learn this
    // from their own launch state, but a remote client (gateway/WebSocket) has
    // no other source, and without it a dropped connection cannot reattach:
    // the next Subscribe carries no `target_session_id`, so the server hands
    // it a brand-new session and the in-flight turn becomes unreachable.
    notify_client(
        client_event_tx,
        ServerEvent::SessionId {
            session_id: client_session_id.to_string(),
        },
        "SessionId",
    );
    notify_client(client_event_tx, ServerEvent::Done { id }, "Done");
    prewarm_idle_agent(agent);
}

pub(super) fn prewarm_idle_agent(agent: &Arc<Mutex<Agent>>) -> bool {
    // Poll local preparation once, without holding the agent across a yield.
    // If a registry/provider lock would wait, abandon this optional attempt.
    // Only the provider's network task can outlive this call.
    let Ok(guard) = agent.try_lock() else {
        return false;
    };
    guard.prewarm_provider().now_or_never().is_some()
}

pub(super) async fn subscribe_should_mark_ready(
    client_session_id: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> bool {
    let members = swarm_members.read().await;
    members
        .get(client_session_id)
        .is_none_or(|member| member.status != "running")
}

pub(super) async fn rename_swarm_member_session(
    old_session_id: &str,
    new_session_id: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
) {
    // Never hold both swarm maps at once. Coordinator cleanup reads them in the
    // opposite order, so retaining the member write guard while waiting for the
    // swarm map can permanently deadlock reconnects and every later subscribe.
    let renamed_swarm_id = {
        let mut members = swarm_members.write().await;
        let renamed_swarm_id = members.remove(old_session_id).and_then(|mut member| {
            let swarm_id = member.swarm_id.clone();
            member.session_id = new_session_id.to_string();
            member.status = "ready".to_string();
            member.detail = None;
            members.insert(new_session_id.to_string(), member);
            swarm_id
        });

        // Keep the spawn tree intact across the rename: children that reported
        // back to the old session id must follow it.
        for member in members.values_mut() {
            if member.report_back_to_session_id.as_deref() == Some(old_session_id) {
                member.report_back_to_session_id = Some(new_session_id.to_string());
            }
        }
        renamed_swarm_id
    };

    if let Some(swarm_id) = renamed_swarm_id {
        let mut swarms = swarms_by_id.write().await;
        if let Some(swarm) = swarms.get_mut(&swarm_id) {
            swarm.remove(old_session_id);
            swarm.insert(new_session_id.to_string());
        }
    }
}

pub(super) async fn handle_reload(
    id: u64,
    force: bool,
    client_session_id: &str,
    agent: &Arc<Mutex<Agent>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    // A non-forced reload (e.g. `jcode server reload`) is a graceful upgrade
    // request: only reload when this server is provably running older code than
    // an available reload candidate. This keeps us from downgrading a newer
    // server (such as a self-dev daemon next to an older release client) and
    // from re-entering the reload-loop family (#277), where a server that merely
    // "differs" can never make the difference go away by reloading.
    if !force && !super::server_has_newer_binary() {
        crate::logging::info(&format!(
            "handle_reload: skipping non-forced reload for client_session_id={} (no strictly-newer binary)",
            client_session_id
        ));
        // Tell the requester this was a deliberate no-op (not a silent success)
        // so callers like `jcode server reload` can report "already up to date"
        // distinctly from an actual reload.
        notify_client(
            client_event_tx,
            ServerEvent::ReloadProgress {
                step: "skip".to_string(),
                message: "Server already running the newest binary; no reload needed.".to_string(),
                success: Some(true),
                output: None,
            },
            "ReloadProgress",
        );
        notify_client(client_event_tx, ServerEvent::Done { id }, "Done");
        return;
    }

    let request_id = crate::id::new_id("reload");
    mark_remote_reload_started(&request_id);

    let (triggering_session, prefer_selfdev_binary) = match agent.try_lock() {
        Ok(agent_guard) => (
            Some(agent_guard.session_id().to_string()),
            agent_guard.is_canary(),
        ),
        Err(_) => {
            crate::logging::warn(&format!(
                "SERVER_RELOAD_AGENT_BUSY request_id={} client_session_id={} fallback_triggering_session={} prefer_selfdev_binary=false",
                request_id, client_session_id, client_session_id
            ));
            (Some(client_session_id.to_string()), false)
        }
    };

    let live_sessions = {
        let members = swarm_members.read().await;
        members
            .iter()
            .filter_map(|(session_id, member)| {
                if member.event_txs.is_empty() {
                    None
                } else {
                    Some(session_id.clone())
                }
            })
            .collect::<Vec<_>>()
    };

    let mut delivered = 0;
    for session_id in &live_sessions {
        delivered += fanout_live_client_event(
            swarm_members,
            session_id,
            ServerEvent::Reloading { new_socket: None },
        )
        .await;
    }
    if delivered == 0 {
        notify_client(
            client_event_tx,
            ServerEvent::Reloading { new_socket: None },
            "Reloading",
        );
    }

    let hash = jcode_build_meta::git_hash().to_string();
    let signal_request_id =
        crate::server::send_reload_signal(hash, triggering_session.clone(), prefer_selfdev_binary);

    crate::logging::info(&format!(
        "handle_reload: queued reload signal {} from remote client request {} (triggering_session={:?}, prefer_selfdev_binary={}, reload_notified_sessions={}, reload_notified_clients={})",
        signal_request_id,
        request_id,
        triggering_session,
        prefer_selfdev_binary,
        live_sessions.len(),
        delivered
    ));

    notify_client(client_event_tx, ServerEvent::Done { id }, "Done");
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn cleanup_detached_source_session_if_unused(
    old_session_id: &str,
    client_connection_id: &str,
    source_agent: &Arc<Mutex<Agent>>,
    sessions: &SessionAgents,
    shutdown_signals: &Arc<RwLock<HashMap<String, InterruptSignal>>>,
    soft_interrupt_queues: &SessionInterruptQueues,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    file_touch: &FileTouchService,
    channel_subscriptions: &ChannelSubscriptions,
    channel_subscriptions_by_session: &ChannelSubscriptions,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
) {
    unregister_session_event_sender(swarm_members, old_session_id, client_connection_id).await;

    if !remove_detached_source_if_unclaimed(
        old_session_id,
        client_connection_id,
        source_agent,
        sessions,
        client_connections,
    )
    .await
    {
        return;
    }

    {
        let mut agent_guard = source_agent.lock().await;
        agent_guard.mark_closed();
    }

    {
        let mut signals = shutdown_signals.write().await;
        signals.remove(old_session_id);
    }
    remove_background_tool_signal(old_session_id);
    remove_session_interrupt_queue(soft_interrupt_queues, old_session_id).await;
    remove_session_channel_subscriptions(
        old_session_id,
        channel_subscriptions,
        channel_subscriptions_by_session,
    )
    .await;
    file_touch.clear_session(old_session_id).await;

    let removed_swarm_id = {
        let mut members = swarm_members.write().await;
        members
            .remove(old_session_id)
            .and_then(|member| member.swarm_id)
    };
    if let Some(swarm_id) = removed_swarm_id {
        remove_session_from_swarm(
            old_session_id,
            &swarm_id,
            swarm_members,
            swarms_by_id,
            swarm_coordinators,
            swarm_plans,
        )
        .await;
    }
}

/// Removes a detached source only while holding the same connection-registry
/// write lock used to claim a live resume target. The connection registry is
/// the attachment authority, so the lock order for transitions is always
/// `client_connections` then `sessions`.
pub(super) async fn remove_detached_source_if_unclaimed(
    old_session_id: &str,
    client_connection_id: &str,
    source_agent: &Arc<Mutex<Agent>>,
    sessions: &SessionAgents,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
) -> bool {
    let connections = client_connections.write().await;
    if connections
        .values()
        .any(|info| info.client_id != client_connection_id && info.session_id == old_session_id)
    {
        return false;
    }

    let mut sessions_guard = sessions.write().await;
    let owns_source = sessions_guard
        .get(old_session_id)
        .map(|existing| Arc::ptr_eq(existing, source_agent))
        .unwrap_or(false);
    if owns_source {
        sessions_guard.remove(old_session_id);
    }
    owns_source
}

/// Read a session's working directory without ever blocking on the agent lock.
///
/// `try_lock` is not an answer here. A lock held by an in-flight turn is not the
/// same fact as a session with no working directory, and reading it as one sent a
/// busy session's directory back to the *client's* report: the agent stayed put,
/// because `apply_or_defer_subscribe_working_dir` refuses to move a bound
/// session, while the swarm member and project-local MCP discovery were re-keyed
/// to the attaching client's project. That is the disagreement these rules exist
/// to prevent, and a desktop attaches to live sessions routinely, so contention
/// was the ordinary case rather than a race.
///
/// The persisted session is read first for the same reason
/// `ensure_client_swarm_member` does: it is the same session file the agent
/// saves its directory to, it needs no lock, and a missing or partial file is an
/// ordinary outcome rather than an error. Only when it yields nothing does the
/// live agent get asked, and then through `try_lock` so this still cannot stall a
/// subscribe behind an in-flight turn.
fn current_working_dir(agent: &Arc<Mutex<Agent>>, session_id: &str) -> Option<String> {
    match crate::session::Session::load_startup_stub(session_id) {
        Ok(session) => match session.working_dir {
            Some(dir) => return Some(dir),
            // A stub that parsed but carries no directory is still evidence about
            // this session, so ask the live agent before falling back to the
            // client's report.
            None => {}
        },
        Err(err) => {
            crate::logging::warn(&format!(
                "Could not read the persisted working directory for session {}: {:#}",
                session_id, err
            ));
        }
    }
    let guard = match agent.try_lock() {
        Ok(guard) => guard,
        Err(_) => {
            crate::logging::debug(&format!(
                "current_working_dir: agent {} is busy; falling back to the client's report",
                session_id
            ));
            return None;
        }
    };
    guard.working_dir().map(str::to_string)
}

#[cfg(test)]
#[path = "subscribe_working_dir_tests.rs"]
mod subscribe_working_dir_tests;
