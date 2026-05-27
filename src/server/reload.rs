use crate::agent::Agent;
use crate::server::reload_recovery::ReloadRecoveryRole;
use crate::server::{SwarmEvent, SwarmEventType, SwarmMember};
use crate::tool::selfdev::ReloadContext;
use jcode_agent_runtime::InterruptSignal;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock, broadcast, watch};

type SessionAgents = Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>;

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
        super::reload_trace::record_value(
            &signal.request_id,
            "signal_received",
            serde_json::json!({
                "hash": signal.hash,
                "triggering_session": signal.triggering_session,
                "prefer_selfdev_binary": signal.prefer_selfdev_binary,
            }),
        );
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

        persist_reload_recovery_intents(
            &signal.request_id,
            &swarm_members,
            signal.triggering_session.as_deref(),
        )
        .await;
        super::reload_trace::record_value(
            &signal.request_id,
            "intent_persistence_complete",
            serde_json::json!({}),
        );

        graceful_shutdown_sessions(
            &signal.request_id,
            &sessions,
            &swarm_members,
            &shutdown_signals,
            &swarm_event_tx,
            signal.triggering_session.as_deref(),
        )
        .await;
        crate::logging::info(&format!(
            "Server: graceful shutdown completed for reload request={} after {}ms state={}",
            signal.request_id,
            reload_started.elapsed().as_millis(),
            crate::server::reload_state_summary(std::time::Duration::from_secs(60))
        ));
        super::reload_trace::record_value(
            &signal.request_id,
            "graceful_shutdown_complete",
            serde_json::json!({
                "elapsed_ms": reload_started.elapsed().as_millis(),
                "state": crate::server::reload_state_summary(std::time::Duration::from_secs(60)),
            }),
        );

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
                super::reload_trace::record_value(
                    &signal.request_id,
                    "exec_start",
                    serde_json::json!({
                        "binary_label": label,
                        "binary": binary,
                        "socket": socket,
                        "elapsed_ms": reload_started.elapsed().as_millis(),
                    }),
                );
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

async fn persist_reload_recovery_intents(
    reload_id: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    triggering_session: Option<&str>,
) {
    let mut candidates: Vec<(String, bool)> = {
        let members = swarm_members.read().await;
        let snapshot = members
            .iter()
            .map(|(session_id, member)| {
                serde_json::json!({
                    "session_id": session_id,
                    "status": member.status,
                    "is_headless": member.is_headless,
                    "swarm_id": member.swarm_id,
                    "role": member.role,
                })
            })
            .collect::<Vec<_>>();
        super::reload_trace::record_value(
            reload_id,
            "candidate_snapshot",
            serde_json::json!({
                "triggering_session": triggering_session,
                "members": snapshot,
            }),
        );
        members
            .iter()
            .filter(|(_, member)| member.status == "running")
            .map(|(session_id, member)| (session_id.clone(), member.is_headless))
            .collect()
    };

    if let Some(triggering_session) = triggering_session
        && !candidates
            .iter()
            .any(|(session_id, _)| session_id == triggering_session)
    {
        candidates.push((triggering_session.to_string(), false));
    }

    candidates.sort_by(|a, b| a.0.cmp(&b.0));
    candidates.dedup_by(|a, b| a.0 == b.0);

    for (session_id, is_headless) in candidates {
        let reload_ctx = ReloadContext::peek_for_session(&session_id).ok().flatten();
        let is_triggering = Some(session_id.as_str()) == triggering_session;
        let Some(directive) = ReloadContext::recovery_directive_for_session(
            &session_id,
            reload_ctx.as_ref(),
            is_headless || !is_triggering,
            None,
        ) else {
            super::reload_trace::record_value(
                reload_id,
                "intent_skipped",
                serde_json::json!({
                    "session_id": session_id,
                    "triggering": is_triggering,
                    "is_headless": is_headless,
                    "has_reload_ctx": reload_ctx.is_some(),
                    "reason": "no directive generated",
                }),
            );
            crate::logging::info(&format!(
                "reload recovery store: no directive generated for reload_id={} session={} triggering={} headless={} has_reload_ctx={}",
                reload_id,
                session_id,
                is_triggering,
                is_headless,
                reload_ctx.is_some()
            ));
            continue;
        };

        let role = if is_headless {
            ReloadRecoveryRole::Headless
        } else if is_triggering {
            ReloadRecoveryRole::Initiator
        } else {
            ReloadRecoveryRole::InterruptedPeer
        };
        let reason = if is_triggering {
            "triggering session for reload"
        } else if is_headless {
            "headless session running during reload"
        } else {
            "attached peer session running during reload"
        };

        if let Err(err) =
            super::reload_recovery::persist_intent(reload_id, &session_id, role, directive, reason)
        {
            super::reload_trace::record_value(
                reload_id,
                "intent_persist_failed",
                serde_json::json!({
                    "session_id": session_id,
                    "error": err.to_string(),
                }),
            );
            crate::logging::warn(&format!(
                "reload recovery store: failed to persist intent reload_id={} session={}: {}",
                reload_id, session_id, err
            ));
        } else {
            super::reload_trace::record_value(
                reload_id,
                "intent_persisted",
                serde_json::json!({
                    "session_id": session_id,
                    "triggering": is_triggering,
                    "is_headless": is_headless,
                }),
            );
        }
    }
}

pub(super) async fn graceful_shutdown_sessions(
    reload_id: &str,
    _sessions: &SessionAgents,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    shutdown_signals: &Arc<RwLock<HashMap<String, InterruptSignal>>>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    triggering_session: Option<&str>,
) {
    graceful_shutdown_sessions_with_timeout(
        reload_id,
        _sessions,
        swarm_members,
        shutdown_signals,
        swarm_event_tx,
        RELOAD_GRACEFUL_SHUTDOWN_TIMEOUT,
        triggering_session,
    )
    .await;
}

async fn graceful_shutdown_sessions_with_timeout(
    reload_id: &str,
    _sessions: &SessionAgents,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    shutdown_signals: &Arc<RwLock<HashMap<String, InterruptSignal>>>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    timeout: Duration,
    triggering_session: Option<&str>,
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
        super::reload_trace::record_value(
            reload_id,
            "shutdown_unsignalable_sessions",
            serde_json::json!({
                "sessions": unsignalable_sessions,
            }),
        );
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
            let Some(signal) = signals.get(session_id) else {
                crate::logging::warn(&format!(
                    "Server: shutdown signal disappeared before graceful reload handoff for session {}",
                    session_id
                ));
                continue;
            };
            signal.fire();
            super::reload_trace::record_value(
                reload_id,
                "shutdown_signal_sent",
                serde_json::json!({
                    "session_id": session_id,
                    "triggering_session": triggering_session,
                }),
            );
            crate::logging::info(&format!(
                "Server: sent graceful shutdown signal to session {}",
                session_id
            ));
        }
    }

    let watched: std::collections::HashSet<String> = signalable_sessions
        .into_iter()
        .filter(|session_id| Some(session_id.as_str()) != triggering_session)
        .collect();

    if let Some(triggering_session) = triggering_session {
        crate::logging::info(&format!(
            "Server: excluding triggering session {} from reload checkpoint wait set",
            triggering_session
        ));
    }

    if watched.is_empty() {
        crate::logging::info(
            "Server: no non-triggering running sessions remain to checkpoint, proceeding with reload",
        );
        return;
    }

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
#[path = "reload_tests.rs"]
mod reload_tests;
