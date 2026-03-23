use super::{
    ClientConnectionInfo, ClientDebugState, SessionInterruptQueues, SwarmEvent, SwarmEventType,
    SwarmMember, VersionedPlan, record_swarm_event, remove_session_channel_subscriptions,
    remove_session_from_swarm, remove_session_interrupt_queue, update_member_status,
};
use crate::agent::{Agent, InterruptSignal};
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, broadcast};

const RELOAD_DISCONNECT_MARKER_MAX_AGE: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DisconnectDisposition {
    Closed,
    Crashed,
    Reloading,
}

fn disconnect_disposition(disconnected_while_processing: bool) -> DisconnectDisposition {
    if !disconnected_while_processing {
        return DisconnectDisposition::Closed;
    }

    if crate::server::reload_marker_active(RELOAD_DISCONNECT_MARKER_MAX_AGE) {
        DisconnectDisposition::Reloading
    } else {
        DisconnectDisposition::Crashed
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn cleanup_client_connection(
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    client_session_id: &str,
    client_is_processing: bool,
    processing_task: &mut Option<tokio::task::JoinHandle<()>>,
    event_handle: tokio::task::JoinHandle<()>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    channel_subscriptions: &Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>,
    client_debug_state: &Arc<RwLock<ClientDebugState>>,
    client_debug_id: &str,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    client_connection_id: &str,
    shutdown_signals: &Arc<RwLock<HashMap<String, InterruptSignal>>>,
    soft_interrupt_queues: &SessionInterruptQueues,
    event_history: &Arc<RwLock<Vec<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) -> Result<()> {
    let disconnected_while_processing = client_is_processing
        || processing_task
            .as_ref()
            .map(|handle| !handle.is_finished())
            .unwrap_or(false);
    let disposition = disconnect_disposition(disconnected_while_processing);

    {
        let mut sessions_guard = sessions.write().await;
        if let Some(agent_arc) = sessions_guard.remove(client_session_id) {
            drop(sessions_guard);
            let lock_result =
                tokio::time::timeout(std::time::Duration::from_secs(2), agent_arc.lock()).await;

            match lock_result {
                Ok(mut agent) => {
                    match disposition {
                        DisconnectDisposition::Closed | DisconnectDisposition::Reloading => {
                            agent.mark_closed();
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
        remove_session_channel_subscriptions(client_session_id, channel_subscriptions).await;
    }

    {
        let mut debug_state = client_debug_state.write().await;
        debug_state.unregister(client_debug_id);
    }
    {
        let mut connections = client_connections.write().await;
        connections.remove(client_connection_id);
    }
    {
        let mut signals = shutdown_signals.write().await;
        signals.remove(client_session_id);
    }
    remove_session_interrupt_queue(soft_interrupt_queues, client_session_id).await;

    if let Some(handle) = processing_task.take() {
        handle.abort();
    }

    event_handle.abort();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{DisconnectDisposition, disconnect_disposition};

    #[test]
    fn idle_disconnect_is_closed() {
        assert_eq!(disconnect_disposition(false), DisconnectDisposition::Closed);
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
        crate::server::clear_reload_marker();
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}
