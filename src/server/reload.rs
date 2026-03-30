use crate::agent::Agent;
use crate::server::{SwarmEvent, SwarmEventType, SwarmMember};
use jcode_agent_runtime::InterruptSignal;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock, broadcast, watch};

const RELOAD_GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

fn prepare_server_exec(cmd: &mut std::process::Command, socket_path: &std::path::Path) {
    // The replacement daemon must own the published socket paths. Unlink them
    // before exec so we never inherit a stale on-disk endpoint through reload.
    crate::server::cleanup_socket_pair(socket_path);
    cmd.env_remove("JCODE_READY_FD");

    // The shared daemon may have inherited stderr from the client process that
    // originally spawned it. Once that client exits, later reload execs can hit
    // SIGPIPE during boot when they emit provider/model notices to stderr,
    // killing the replacement server before it binds the socket. The daemon
    // logs to the file logger, so detach stdio for exec-based reloads.
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
}

async fn receive_reload_signal(
    rx: &mut watch::Receiver<Option<crate::server::ReloadSignal>>,
) -> Option<crate::server::ReloadSignal> {
    if let Some(signal) = rx.borrow_and_update().clone() {
        return Some(signal);
    }

    loop {
        if rx.changed().await.is_err() {
            return None;
        }

        if let Some(signal) = rx.borrow_and_update().clone() {
            return Some(signal);
        }
    }
}

pub(super) async fn await_reload_signal(
    sessions: Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    shutdown_signals: Arc<RwLock<HashMap<String, InterruptSignal>>>,
    swarm_event_tx: broadcast::Sender<SwarmEvent>,
) {
    use std::process::Command as ProcessCommand;

    let mut rx = super::reload_state::reload_signal().1.clone();

    loop {
        let signal = match receive_reload_signal(&mut rx).await {
            Some(signal) => signal,
            None => return,
        };

        crate::logging::info(&format!(
            "Server: reload signal received via channel request={} hash={} triggering_session={:?} prefer_selfdev_binary={}",
            signal.request_id, signal.hash, signal.triggering_session, signal.prefer_selfdev_binary
        ));
        let reload_started = std::time::Instant::now();
        crate::server::write_reload_state(
            &signal.request_id,
            &signal.hash,
            crate::server::ReloadPhase::Starting,
            signal.triggering_session.clone(),
        );
        super::acknowledge_reload_signal(&signal);

        if std::env::var("JCODE_TEST_SESSION")
            .map(|value| {
                let trimmed = value.trim();
                !trimmed.is_empty() && trimmed != "0" && !trimmed.eq_ignore_ascii_case("false")
            })
            .unwrap_or(false)
        {
            crate::logging::info(
                "Server: JCODE_TEST_SESSION set, skipping process exec for reload test",
            );
            continue;
        }

        graceful_shutdown_sessions(
            &sessions,
            &swarm_members,
            &shutdown_signals,
            &swarm_event_tx,
        )
        .await;
        crate::logging::info(&format!(
            "Server: graceful shutdown completed for reload request={} after {}ms state={}",
            signal.request_id,
            reload_started.elapsed().as_millis(),
            crate::server::reload_state_summary(std::time::Duration::from_secs(60))
        ));

        let prefers_selfdev = signal.prefer_selfdev_binary;

        if let Some((binary, label)) = super::server_update_candidate(prefers_selfdev) {
            if binary.exists() {
                let socket = super::socket_path();
                crate::logging::info(&format!(
                    "Server: exec'ing into {} binary {:?} (socket: {:?}, prep={}ms, state={})",
                    label,
                    binary,
                    socket,
                    reload_started.elapsed().as_millis(),
                    crate::server::reload_state_summary(std::time::Duration::from_secs(60))
                ));
                let mut cmd = ProcessCommand::new(&binary);
                cmd.arg("serve").arg("--socket").arg(socket.as_os_str());
                prepare_server_exec(&mut cmd, &socket);
                let err = crate::platform::replace_process(&mut cmd);
                crate::server::write_reload_state(
                    &signal.request_id,
                    &signal.hash,
                    crate::server::ReloadPhase::Failed,
                    Some(err.to_string()),
                );
                crate::logging::error(&format!(
                    "Failed to exec into {} {:?}: {}",
                    label, binary, err
                ));
            } else {
                crate::server::write_reload_state(
                    &signal.request_id,
                    &signal.hash,
                    crate::server::ReloadPhase::Failed,
                    Some(format!("missing binary: {}", binary.display())),
                );
            }
        } else {
            crate::server::write_reload_state(
                &signal.request_id,
                &signal.hash,
                crate::server::ReloadPhase::Failed,
                Some("no reloadable binary found".to_string()),
            );
        }
        std::process::exit(42);
    }
}

pub(super) async fn graceful_shutdown_sessions(
    _sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    shutdown_signals: &Arc<RwLock<HashMap<String, InterruptSignal>>>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) {
    graceful_shutdown_sessions_with_timeout(
        _sessions,
        swarm_members,
        shutdown_signals,
        swarm_event_tx,
        RELOAD_GRACEFUL_SHUTDOWN_TIMEOUT,
    )
    .await;
}

async fn graceful_shutdown_sessions_with_timeout(
    _sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    shutdown_signals: &Arc<RwLock<HashMap<String, InterruptSignal>>>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    timeout: Duration,
) {
    let actively_generating: Vec<String> = {
        let members = swarm_members.read().await;
        members
            .iter()
            .filter(|(_, m)| m.status == "running")
            .map(|(id, _)| id.clone())
            .collect()
    };

    let (signalable_sessions, unsignalable_sessions) = {
        let signals = shutdown_signals.read().await;
        actively_generating
            .into_iter()
            .partition::<Vec<_>, _>(|session_id| signals.contains_key(session_id))
    };

    if !unsignalable_sessions.is_empty() {
        crate::logging::warn(&format!(
            "Server: {} running session(s) had no shutdown signal and will not block reload: {:?}",
            unsignalable_sessions.len(),
            unsignalable_sessions
        ));
    }

    if signalable_sessions.is_empty() {
        crate::logging::info(
            "Server: no sessions actively generating, proceeding with reload immediately",
        );
        return;
    }

    crate::logging::info(&format!(
        "Server: signaling {} actively generating session(s) to checkpoint: {:?}",
        signalable_sessions.len(),
        signalable_sessions
    ));

    {
        let signals = shutdown_signals.read().await;
        for session_id in &signalable_sessions {
            let signal = signals
                .get(session_id)
                .expect("signalable sessions were filtered against shutdown_signals");
            signal.fire();
            crate::logging::info(&format!(
                "Server: sent graceful shutdown signal to session {}",
                session_id
            ));
        }
    }

    let watched: std::collections::HashSet<String> = signalable_sessions.into_iter().collect();
    let mut event_rx = swarm_event_tx.subscribe();
    let deadline = Instant::now() + timeout;

    loop {
        let still_running: Vec<String> = {
            let members = swarm_members.read().await;
            watched
                .iter()
                .filter(|id| {
                    members
                        .get(*id)
                        .map(|m| m.status == "running")
                        .unwrap_or(false)
                })
                .cloned()
                .collect()
        };

        if still_running.is_empty() {
            crate::logging::info("Server: all sessions checkpointed, proceeding with reload");
            break;
        }

        crate::logging::info(&format!(
            "Server: waiting for {} session(s) to checkpoint before reload: {:?}",
            still_running.len(),
            still_running
        ));

        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            crate::logging::warn(&format!(
                "Server: reload graceful shutdown timed out after {}ms; proceeding with still-running sessions: {:?}",
                timeout.as_millis(),
                still_running
            ));
            break;
        }

        match tokio::time::timeout(remaining, event_rx.recv()).await {
            Ok(Ok(event)) => match &event.event {
                SwarmEventType::StatusChange { .. } if watched.contains(&event.session_id) => {}
                SwarmEventType::MemberChange { action }
                    if action == "left" && watched.contains(&event.session_id) => {}
                _ => continue,
            },
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(broadcast::error::RecvError::Closed)) => {
                crate::logging::warn(
                    "Server: swarm event channel closed while waiting for reload checkpoint",
                );
                break;
            }
            Err(_) => {
                crate::logging::warn(&format!(
                    "Server: reload graceful shutdown timed out after {}ms; proceeding without waiting for remaining checkpoint events",
                    timeout.as_millis()
                ));
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        graceful_shutdown_sessions, graceful_shutdown_sessions_with_timeout, receive_reload_signal,
    };
    use jcode_agent_runtime::InterruptSignal;
    use crate::server::{ReloadSignal, SwarmEvent, SwarmEventType, SwarmMember};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Instant;
    use tokio::sync::{RwLock, broadcast, mpsc, watch};

    fn member(session_id: &str, status: &str) -> SwarmMember {
        let (event_tx, _event_rx) = mpsc::unbounded_channel();
        SwarmMember {
            session_id: session_id.to_string(),
            event_tx,
            working_dir: None,
            swarm_id: None,
            swarm_enabled: false,
            status: status.to_string(),
            detail: None,
            friendly_name: None,
            role: "agent".to_string(),
            joined_at: Instant::now(),
            last_status_change: Instant::now(),
            is_headless: false,
        }
    }

    #[tokio::test]
    async fn receive_reload_signal_consumes_already_pending_value() {
        let (tx, mut rx) = watch::channel(None::<ReloadSignal>);
        tx.send(Some(ReloadSignal {
            hash: "abc1234".to_string(),
            triggering_session: Some("sess-1".to_string()),
            prefer_selfdev_binary: true,
            request_id: "reload-1".to_string(),
        }))
        .expect("send pending reload signal");

        let signal = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            receive_reload_signal(&mut rx),
        )
        .await
        .expect("pending signal should be observed immediately")
        .expect("channel should still be open");

        assert_eq!(signal.hash, "abc1234");
        assert_eq!(signal.triggering_session.as_deref(), Some("sess-1"));
        assert!(signal.prefer_selfdev_binary);
        assert_eq!(signal.request_id, "reload-1");
    }

    #[tokio::test]
    async fn receive_reload_signal_waits_for_future_value_when_initially_empty() {
        let (tx, mut rx) = watch::channel(None::<ReloadSignal>);

        let waiter = tokio::spawn(async move { receive_reload_signal(&mut rx).await });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        tx.send(Some(ReloadSignal {
            hash: "def5678".to_string(),
            triggering_session: Some("sess-2".to_string()),
            prefer_selfdev_binary: false,
            request_id: "reload-2".to_string(),
        }))
        .expect("send future reload signal");

        let signal = tokio::time::timeout(std::time::Duration::from_millis(100), waiter)
            .await
            .expect("future signal should wake waiter")
            .expect("waiter task should succeed")
            .expect("channel should still be open");

        assert_eq!(signal.hash, "def5678");
        assert_eq!(signal.triggering_session.as_deref(), Some("sess-2"));
        assert!(!signal.prefer_selfdev_binary);
        assert_eq!(signal.request_id, "reload-2");
    }

    #[tokio::test]
    async fn graceful_shutdown_sessions_signals_all_running_sessions_including_initiator() {
        let sessions = Arc::new(RwLock::new(HashMap::new()));
        let swarm_members = Arc::new(RwLock::new(HashMap::from([
            ("initiator".to_string(), member("initiator", "running")),
            ("peer".to_string(), member("peer", "running")),
        ])));
        let initiator_signal = InterruptSignal::new();
        let peer_signal = InterruptSignal::new();
        let shutdown_signals = Arc::new(RwLock::new(HashMap::from([
            ("initiator".to_string(), initiator_signal.clone()),
            ("peer".to_string(), peer_signal.clone()),
        ])));
        let (swarm_event_tx, _) = broadcast::channel(8);
        let swarm_members_for_task = swarm_members.clone();
        let swarm_event_tx_for_task = swarm_event_tx.clone();

        let checkpoint_task = tokio::spawn(async move {
            tokio::task::yield_now().await;
            {
                let mut members = swarm_members_for_task.write().await;
                members.get_mut("initiator").expect("initiator").status = "ready".to_string();
                members.get_mut("peer").expect("peer").status = "ready".to_string();
            }
            let _ = swarm_event_tx_for_task.send(SwarmEvent {
                id: 1,
                session_id: "initiator".to_string(),
                session_name: None,
                swarm_id: None,
                event: SwarmEventType::StatusChange {
                    old_status: "running".to_string(),
                    new_status: "ready".to_string(),
                },
                timestamp: Instant::now(),
                absolute_time: std::time::SystemTime::now(),
            });
            let _ = swarm_event_tx_for_task.send(SwarmEvent {
                id: 2,
                session_id: "peer".to_string(),
                session_name: None,
                swarm_id: None,
                event: SwarmEventType::StatusChange {
                    old_status: "running".to_string(),
                    new_status: "ready".to_string(),
                },
                timestamp: Instant::now(),
                absolute_time: std::time::SystemTime::now(),
            });
        });

        graceful_shutdown_sessions(
            &sessions,
            &swarm_members,
            &shutdown_signals,
            &swarm_event_tx,
        )
        .await;
        checkpoint_task.await.expect("checkpoint task");

        assert!(
            initiator_signal.is_set(),
            "initiating selfdev session should also be interrupted so reload tool cannot hang"
        );
        assert!(
            peer_signal.is_set(),
            "other running sessions should be interrupted too"
        );
    }

    #[tokio::test]
    async fn graceful_shutdown_sessions_skips_idle_sessions() {
        let sessions = Arc::new(RwLock::new(HashMap::new()));
        let swarm_members = Arc::new(RwLock::new(HashMap::from([(
            "idle".to_string(),
            member("idle", "ready"),
        )])));
        let idle_signal = InterruptSignal::new();
        let shutdown_signals = Arc::new(RwLock::new(HashMap::from([(
            "idle".to_string(),
            idle_signal.clone(),
        )])));
        let (swarm_event_tx, _) = broadcast::channel(8);

        graceful_shutdown_sessions(
            &sessions,
            &swarm_members,
            &shutdown_signals,
            &swarm_event_tx,
        )
        .await;

        assert!(
            !idle_signal.is_set(),
            "idle sessions should not be interrupted during reload"
        );
    }

    #[tokio::test]
    async fn graceful_shutdown_sessions_does_not_wait_on_running_sessions_without_signal() {
        let sessions = Arc::new(RwLock::new(HashMap::new()));
        let swarm_members = Arc::new(RwLock::new(HashMap::from([(
            "orphan_running".to_string(),
            member("orphan_running", "running"),
        )])));
        let shutdown_signals = Arc::new(RwLock::new(HashMap::new()));
        let (swarm_event_tx, _) = broadcast::channel(8);

        let started = Instant::now();
        graceful_shutdown_sessions(
            &sessions,
            &swarm_members,
            &shutdown_signals,
            &swarm_event_tx,
        )
        .await;

        assert!(
            started.elapsed() < std::time::Duration::from_millis(100),
            "running sessions without shutdown signals should not consume the reload grace period"
        );
    }

    #[tokio::test]
    async fn graceful_shutdown_sessions_waits_until_target_status_change_arrives() {
        let sessions = Arc::new(RwLock::new(HashMap::new()));
        let swarm_members = Arc::new(RwLock::new(HashMap::from([(
            "target".to_string(),
            member("target", "running"),
        )])));
        let signal = InterruptSignal::new();
        let shutdown_signals = Arc::new(RwLock::new(HashMap::from([(
            "target".to_string(),
            signal.clone(),
        )])));
        let (swarm_event_tx, _) = broadcast::channel(8);

        let mut waiter = tokio::spawn({
            let sessions = sessions.clone();
            let swarm_members = swarm_members.clone();
            let shutdown_signals = shutdown_signals.clone();
            let swarm_event_tx = swarm_event_tx.clone();
            async move {
                graceful_shutdown_sessions(
                    &sessions,
                    &swarm_members,
                    &shutdown_signals,
                    &swarm_event_tx,
                )
                .await;
            }
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            signal.is_set(),
            "running target should be interrupted promptly"
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut waiter)
                .await
                .is_err(),
            "reload shutdown should stay pending until target leaves running"
        );

        {
            let mut members = swarm_members.write().await;
            members.get_mut("target").expect("target").status = "ready".to_string();
        }
        let _ = swarm_event_tx.send(SwarmEvent {
            id: 1,
            session_id: "target".to_string(),
            session_name: None,
            swarm_id: None,
            event: SwarmEventType::StatusChange {
                old_status: "running".to_string(),
                new_status: "ready".to_string(),
            },
            timestamp: Instant::now(),
            absolute_time: std::time::SystemTime::now(),
        });

        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("waiter should complete after target checkpoint")
            .expect("waiter task should succeed");
    }

    #[tokio::test]
    async fn graceful_shutdown_sessions_ignores_unrelated_events_until_target_leaves() {
        let sessions = Arc::new(RwLock::new(HashMap::new()));
        let swarm_members = Arc::new(RwLock::new(HashMap::from([
            ("target".to_string(), member("target", "running")),
            ("other".to_string(), member("other", "running")),
        ])));
        let signal = InterruptSignal::new();
        let shutdown_signals =
            Arc::new(RwLock::new(HashMap::from([("target".to_string(), signal)])));
        let (swarm_event_tx, _) = broadcast::channel(8);

        let mut waiter = tokio::spawn({
            let sessions = sessions.clone();
            let swarm_members = swarm_members.clone();
            let shutdown_signals = shutdown_signals.clone();
            let swarm_event_tx = swarm_event_tx.clone();
            async move {
                graceful_shutdown_sessions(
                    &sessions,
                    &swarm_members,
                    &shutdown_signals,
                    &swarm_event_tx,
                )
                .await;
            }
        });

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        {
            let mut members = swarm_members.write().await;
            members.get_mut("other").expect("other").status = "ready".to_string();
        }
        let _ = swarm_event_tx.send(SwarmEvent {
            id: 1,
            session_id: "other".to_string(),
            session_name: None,
            swarm_id: None,
            event: SwarmEventType::StatusChange {
                old_status: "running".to_string(),
                new_status: "ready".to_string(),
            },
            timestamp: Instant::now(),
            absolute_time: std::time::SystemTime::now(),
        });

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut waiter)
                .await
                .is_err(),
            "unrelated status changes should not unblock reload shutdown"
        );

        {
            let mut members = swarm_members.write().await;
            members.get_mut("target").expect("target").status = "stopped".to_string();
        }
        let _ = swarm_event_tx.send(SwarmEvent {
            id: 2,
            session_id: "target".to_string(),
            session_name: None,
            swarm_id: None,
            event: SwarmEventType::StatusChange {
                old_status: "running".to_string(),
                new_status: "stopped".to_string(),
            },
            timestamp: Instant::now(),
            absolute_time: std::time::SystemTime::now(),
        });

        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("waiter should complete after target transition")
            .expect("waiter task should succeed");
    }

    #[tokio::test]
    async fn graceful_shutdown_sessions_treats_member_left_as_unblocked() {
        let sessions = Arc::new(RwLock::new(HashMap::new()));
        let swarm_members = Arc::new(RwLock::new(HashMap::from([(
            "target".to_string(),
            member("target", "running"),
        )])));
        let signal = InterruptSignal::new();
        let shutdown_signals =
            Arc::new(RwLock::new(HashMap::from([("target".to_string(), signal)])));
        let (swarm_event_tx, _) = broadcast::channel(8);

        let waiter = tokio::spawn({
            let sessions = sessions.clone();
            let swarm_members = swarm_members.clone();
            let shutdown_signals = shutdown_signals.clone();
            let swarm_event_tx = swarm_event_tx.clone();
            async move {
                graceful_shutdown_sessions(
                    &sessions,
                    &swarm_members,
                    &shutdown_signals,
                    &swarm_event_tx,
                )
                .await;
            }
        });

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        {
            let mut members = swarm_members.write().await;
            members.remove("target");
        }
        let _ = swarm_event_tx.send(SwarmEvent {
            id: 1,
            session_id: "target".to_string(),
            session_name: None,
            swarm_id: None,
            event: SwarmEventType::MemberChange {
                action: "left".to_string(),
            },
            timestamp: Instant::now(),
            absolute_time: std::time::SystemTime::now(),
        });

        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("waiter should complete after member leaves")
            .expect("waiter task should succeed");
    }

    #[tokio::test]
    async fn graceful_shutdown_sessions_times_out_and_proceeds() {
        let sessions = Arc::new(RwLock::new(HashMap::new()));
        let swarm_members = Arc::new(RwLock::new(HashMap::from([(
            "target".to_string(),
            member("target", "running"),
        )])));
        let signal = InterruptSignal::new();
        let shutdown_signals = Arc::new(RwLock::new(HashMap::from([(
            "target".to_string(),
            signal.clone(),
        )])));
        let (swarm_event_tx, _) = broadcast::channel(8);

        let started = Instant::now();
        graceful_shutdown_sessions_with_timeout(
            &sessions,
            &swarm_members,
            &shutdown_signals,
            &swarm_event_tx,
            std::time::Duration::from_millis(50),
        )
        .await;

        assert!(
            signal.is_set(),
            "running target should still be signaled promptly"
        );
        assert!(
            started.elapsed() >= std::time::Duration::from_millis(50)
                && started.elapsed() < std::time::Duration::from_millis(250),
            "graceful shutdown should honor the timeout instead of waiting indefinitely"
        );
    }
}
