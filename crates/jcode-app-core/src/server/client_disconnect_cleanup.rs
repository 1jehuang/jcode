use super::{
    ClientConnectionInfo, ClientDebugState, FileTouchService, SessionInterruptQueues, SwarmEvent,
    SwarmEventType, SwarmMember, VersionedPlan, record_swarm_event, remove_background_tool_signal,
    remove_session_channel_subscriptions, remove_session_from_swarm,
    remove_session_interrupt_queue, unregister_session_event_sender, update_member_status,
};
use crate::agent::Agent;
use anyhow::Result;
use jcode_agent_runtime::InterruptSignal;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

type SessionAgents = Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>;
type ChannelSubscriptions = Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>;

const RELOAD_DISCONNECT_MARKER_MAX_AGE: Duration = Duration::from_secs(30);
pub(super) const IDLE_RECONNECT_GRACE: Duration = Duration::from_secs(30);

// The last registered event sender remains on the member after it detaches.
// It is therefore also an ownership witness: an old grace timer must not
// remove a successor's session even if that successor has disconnected again.
async fn attachment_was_replaced(
    members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    session_id: &str,
    original: &mpsc::UnboundedSender<crate::protocol::ServerEvent>,
) -> bool {
    members
        .read()
        .await
        .get(session_id)
        .is_none_or(|member| !member.event_tx.same_channel(original))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DisconnectDisposition {
    Closed,
    Crashed,
    Reloading,
}

fn disconnect_disposition(disconnected_while_processing: bool) -> DisconnectDisposition {
    // Losing the UI is only a session crash when it interrupts unfinished work.
    // In particular, force-quitting Desktop after Done is an ordinary close.
    if !disconnected_while_processing {
        return DisconnectDisposition::Closed;
    }

    if crate::server::reload_marker_active(RELOAD_DISCONNECT_MARKER_MAX_AGE) {
        DisconnectDisposition::Reloading
    } else {
        DisconnectDisposition::Crashed
    }
}

fn disconnected_while_processing(
    client_is_processing: bool,
    processing_task: Option<&tokio::task::JoinHandle<()>>,
) -> bool {
    // Socket EOF is prioritized over processing_done_rx. A finished task is
    // authoritative even if the client's cached processing flag is still set.
    processing_task
        .map(|handle| !handle.is_finished())
        .unwrap_or(client_is_processing)
}

/// Release transport-owned state without changing the live session or turn.
pub(super) async fn detach_client_attachment(
    session_id: &str,
    connection_id: &str,
    debug_id: &str,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    client_debug_state: &Arc<RwLock<ClientDebugState>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) {
    client_debug_state.write().await.unregister(debug_id);
    client_connections.write().await.remove(connection_id);
    unregister_session_event_sender(swarm_members, session_id, connection_id).await;
}

#[expect(
    clippy::too_many_arguments,
    reason = "disconnect cleanup updates sessions, swarms, files, channels, debug state, and shutdown signals together"
)]
pub(super) async fn cleanup_client_connection(
    sessions: &SessionAgents,
    client_session_id: &str,
    client_is_processing: bool,
    processing_task: &mut Option<tokio::task::JoinHandle<()>>,
    event_handle: tokio::task::JoinHandle<()>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    file_touch: &FileTouchService,
    channel_subscriptions: &ChannelSubscriptions,
    channel_subscriptions_by_session: &ChannelSubscriptions,
    client_debug_state: &Arc<RwLock<ClientDebugState>>,
    client_debug_id: &str,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    client_connection_id: &str,
    shutdown_signals: &Arc<RwLock<HashMap<String, InterruptSignal>>>,
    soft_interrupt_queues: &SessionInterruptQueues,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    client_event_tx: &mpsc::UnboundedSender<crate::protocol::ServerEvent>,
    idle_reconnect_grace: Duration,
) -> Result<()> {
    let disposition = disconnect_disposition(disconnected_while_processing(
        client_is_processing,
        processing_task.as_ref(),
    ));
    let allow_reconnect = if disposition == DisconnectDisposition::Closed
        && !crate::session::session_exists(client_session_id)
    {
        let agent = sessions.read().await.get(client_session_id).cloned();
        agent.is_some_and(|agent| {
            agent
                .try_lock()
                .is_ok_and(|agent| agent.visible_conversation_message_count() == 0)
        })
    } else {
        false
    };

    // A live processing task owns the agent mutex. Abort it before trying to
    // persist the disconnect disposition; otherwise cleanup waits two seconds,
    // times out, and leaves the durable session `Active` precisely when an
    // interrupted desktop turn must become `Crashed`.
    if let Some(handle) = processing_task.take() {
        handle.abort();
    }

    detach_client_attachment(
        client_session_id,
        client_connection_id,
        client_debug_id,
        client_connections,
        client_debug_state,
        swarm_members,
    )
    .await;

    if allow_reconnect {
        // Empty roots intentionally have no snapshot. A replacement UI/SDK
        // connection needs a bounded opportunity to reclaim the live Agent,
        // without creating history entries for every briefly opened panel.
        // No registry or agent lock is held across the wait. Processing/crash
        // cleanup never enters this path.
        event_handle.abort();
        crate::logging::info(&format!(
            "Retaining idle unsaved session {} for reconnect grace",
            client_session_id
        ));
        let deadline = tokio::time::Instant::now() + idle_reconnect_grace;
        loop {
            if attachment_was_replaced(swarm_members, client_session_id, client_event_tx).await
                || client_connections
                    .read()
                    .await
                    .values()
                    .any(|info| info.session_id == client_session_id)
            {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep_until(std::cmp::min(
                deadline,
                tokio::time::Instant::now() + Duration::from_millis(25),
            ))
            .await;
        }
    }

    // Release stale live ownership before slower cleanup so a reconnecting TUI can
    // reclaim the same session without tripping duplicate-attach guards.
    tokio::task::yield_now().await;

    // Resume claims use this same lock before accessing the sessions map.
    // Keep it through destructive cleanup so a successor cannot be claimed
    // between the check and session/status/control-handle removal.
    let connections = client_connections.write().await;
    let successor_connected = connections
        .values()
        .any(|info| info.session_id == client_session_id);
    if successor_connected
        || (allow_reconnect
            && attachment_was_replaced(swarm_members, client_session_id, client_event_tx).await)
    {
        crate::logging::info(&format!(
            "Skipping destructive disconnect cleanup for {} because another client is still attached",
            client_session_id
        ));
        event_handle.abort();
        return Ok(());
    }

    {
        if let Some(agent_arc) = super::remove_session_entry(sessions, client_session_id).await {
            let lock_result =
                tokio::time::timeout(std::time::Duration::from_secs(2), agent_arc.lock()).await;

            match lock_result {
                Ok(mut agent) => {
                    match disposition {
                        DisconnectDisposition::Closed => {
                            agent.mark_closed();
                        }
                        DisconnectDisposition::Reloading => {
                            agent.mark_crashed(Some(
                                "Server reload interrupted processing".to_string(),
                            ));
                        }
                        DisconnectDisposition::Crashed => {
                            agent.mark_crashed(Some(
                                "Client disconnected while processing".to_string(),
                            ));
                        }
                    }

                    let memory_enabled = agent.memory_enabled();
                    let transcript = if memory_enabled {
                        Some(agent.build_transcript_for_extraction())
                    } else {
                        None
                    };
                    let sid = client_session_id.to_string();
                    let working_dir = agent.working_dir().map(|dir| dir.to_string());
                    drop(agent);
                    let event = match disposition {
                        DisconnectDisposition::Closed => {
                            crate::runtime_memory_log::RuntimeMemoryLogEvent::new(
                                "session_closed",
                                "client_disconnected",
                            )
                        }
                        DisconnectDisposition::Crashed => {
                            crate::runtime_memory_log::RuntimeMemoryLogEvent::new(
                                "session_crashed",
                                "client_disconnected_while_processing",
                            )
                        }
                        DisconnectDisposition::Reloading => {
                            crate::runtime_memory_log::RuntimeMemoryLogEvent::new(
                                "session_reloading",
                                "server_reload_disconnect",
                            )
                        }
                    }
                    .with_session_id(sid.clone())
                    .force_attribution();
                    crate::runtime_memory_log::emit_event(event);
                    if let Some(transcript) = transcript {
                        crate::memory_agent::trigger_final_extraction_with_dir(
                            transcript,
                            sid,
                            working_dir,
                        );
                    }
                }
                Err(_) => {
                    crate::logging::warn(&format!(
                        "Session {} cleanup timed out waiting for agent lock (stuck task); skipping graceful shutdown",
                        client_session_id
                    ));
                }
            }
        }
    }

    {
        let (status, detail) = match disposition {
            DisconnectDisposition::Closed => ("stopped", Some("disconnected".to_string())),
            DisconnectDisposition::Crashed => {
                ("crashed", Some("disconnect while running".to_string()))
            }
            DisconnectDisposition::Reloading => {
                ("stopped", Some("server reload in progress".to_string()))
            }
        };
        update_member_status(
            client_session_id,
            status,
            detail,
            swarm_members,
            swarms_by_id,
            Some(event_history),
            Some(event_counter),
            Some(swarm_event_tx),
        )
        .await;

        let (swarm_id, removed_name) = {
            let mut members = swarm_members.write().await;
            if let Some(member) = members.remove(client_session_id) {
                (member.swarm_id, member.friendly_name)
            } else {
                (None, None)
            }
        };
        crate::session_metrics::forget(client_session_id);
        crate::session_effort::forget_session_effort(client_session_id);

        if let Some(ref swarm_id) = swarm_id {
            record_swarm_event(
                event_history,
                event_counter,
                swarm_event_tx,
                client_session_id.to_string(),
                removed_name.clone(),
                Some(swarm_id.clone()),
                SwarmEventType::MemberChange {
                    action: "left".to_string(),
                },
            )
            .await;
            remove_session_from_swarm(
                client_session_id,
                swarm_id,
                swarm_members,
                swarms_by_id,
                swarm_coordinators,
                swarm_plans,
            )
            .await;
        }
        remove_session_channel_subscriptions(
            client_session_id,
            channel_subscriptions,
            channel_subscriptions_by_session,
        )
        .await;
        file_touch.clear_session(client_session_id).await;
    }

    {
        let mut signals = shutdown_signals.write().await;
        signals.remove(client_session_id);
    }
    remove_background_tool_signal(client_session_id);
    remove_session_interrupt_queue(soft_interrupt_queues, client_session_id).await;

    drop(connections);
    event_handle.abort();
    Ok(())
}

#[cfg(test)]
#[path = "client_disconnect_grace_tests.rs"]
mod grace_tests;

#[cfg(test)]
mod tests {
    use super::{DisconnectDisposition, disconnect_disposition, disconnected_while_processing};

    #[test]
    fn idle_disconnect_is_closed() {
        assert_eq!(disconnect_disposition(false), DisconnectDisposition::Closed);
    }

    #[tokio::test]
    async fn completed_task_overrides_stale_processing_flag() {
        let task = tokio::spawn(async {});
        while !task.is_finished() {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            disconnect_disposition(disconnected_while_processing(true, Some(&task))),
            DisconnectDisposition::Closed
        );
    }

    #[tokio::test]
    async fn unfinished_task_is_processing_even_without_cached_flag() {
        let task = tokio::spawn(std::future::pending::<()>());
        assert!(disconnected_while_processing(false, Some(&task)));
        task.abort();
        assert!(disconnected_while_processing(true, None));
        assert!(!disconnected_while_processing(false, None));
    }

    #[test]
    fn running_disconnect_without_reload_is_crash() {
        let _guard = crate::storage::lock_test_env();
        crate::server::clear_reload_marker();
        assert_eq!(disconnect_disposition(true), DisconnectDisposition::Crashed);
    }

    #[test]
    fn running_disconnect_during_reload_is_expected() {
        let _guard = crate::storage::lock_test_env();
        let runtime = tempfile::TempDir::new().expect("create runtime dir");
        crate::env::set_var("JCODE_RUNTIME_DIR", runtime.path());
        crate::server::clear_reload_marker();
        crate::server::write_reload_state(
            "test-request",
            "test-hash",
            crate::server::ReloadPhase::Starting,
            None,
        );
        assert_eq!(
            disconnect_disposition(true),
            DisconnectDisposition::Reloading
        );
        assert_eq!(disconnect_disposition(false), DisconnectDisposition::Closed);
        crate::server::clear_reload_marker();
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }

    #[test]
    fn running_disconnect_during_recent_socket_ready_reload_is_expected() {
        let _guard = crate::storage::lock_test_env();
        let runtime = tempfile::TempDir::new().expect("create runtime dir");
        crate::env::set_var("JCODE_RUNTIME_DIR", runtime.path());
        crate::server::clear_reload_marker();
        crate::server::write_reload_state(
            "test-request",
            "test-hash",
            crate::server::ReloadPhase::SocketReady,
            None,
        );
        assert_eq!(
            disconnect_disposition(true),
            DisconnectDisposition::Reloading
        );
        crate::server::clear_reload_marker();
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}
