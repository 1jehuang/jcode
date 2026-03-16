use crate::agent::Agent;
use crate::server::{SwarmEvent, SwarmEventType, SwarmMember};
use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc, watch};

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

pub(super) fn get_repo_dir() -> Option<PathBuf> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = PathBuf::from(manifest_dir);
    if path.join(".git").exists() {
        return Some(path);
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(repo) = exe
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
        {
            if repo.join(".git").exists() {
                return Some(repo.to_path_buf());
            }
        }
    }

    None
}

#[allow(dead_code)]
pub(super) fn do_server_reload() -> Result<()> {
    let repo_dir =
        get_repo_dir().ok_or_else(|| anyhow::anyhow!("Could not find jcode repository"))?;

    crate::logging::info("Server hot-reload starting...");
    crate::logging::info("Pulling latest changes...");
    if let Err(e) = crate::update::run_git_pull_ff_only(&repo_dir, true) {
        crate::logging::info(&format!("Warning: {}. Continuing with current code.", e));
    }

    crate::logging::info("Building...");
    let build = std::process::Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(&repo_dir)
        .status()?;

    if !build.success() {
        anyhow::bail!("Build failed");
    }

    if let Err(e) = crate::build::install_local_release(&repo_dir) {
        crate::logging::info(&format!("Warning: install failed: {}", e));
    }

    crate::logging::info("✓ Build complete, restarting server...");

    let exe = crate::build::release_binary_path(&repo_dir);
    if !exe.exists() {
        anyhow::bail!("Built executable not found at {:?}", exe);
    }

    let socket = crate::server::socket_path();
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("serve");
    prepare_server_exec(&mut cmd, &socket);
    let err = crate::platform::replace_process(&mut cmd);
    Err(anyhow::anyhow!("Failed to exec {:?}: {}", exe, err))
}

pub(super) async fn do_server_reload_with_progress(
    tx: mpsc::UnboundedSender<crate::protocol::ServerEvent>,
    request_id: String,
    provider_arg: Option<String>,
    model_arg: Option<String>,
    socket_arg: String,
    is_selfdev_session: bool,
) -> Result<()> {
    let send_progress =
        |step: &str, message: &str, success: Option<bool>, output: Option<String>| {
            let _ = tx.send(crate::protocol::ServerEvent::ReloadProgress {
                step: step.to_string(),
                message: message.to_string(),
                success,
                output,
            });
        };

    send_progress("init", "🔄 Starting hot-reload...", None, None);

    let repo_dir = get_repo_dir();
    if let Some(repo_dir) = &repo_dir {
        send_progress(
            "init",
            &format!("📁 Repository: {}", repo_dir.display()),
            Some(true),
            None,
        );
    } else {
        send_progress("init", "📁 Repository: (not found)", Some(true), None);
    }

    let (exe, exe_label) = super::server_update_candidate(is_selfdev_session)
        .ok_or_else(|| anyhow::anyhow!("No reloadable binary found"))?;
    if !exe.exists() {
        send_progress("verify", "❌ No reloadable binary found", Some(false), None);
        send_progress(
            "verify",
            "💡 Run 'cargo build --release' first, then use 'selfdev reload'",
            Some(false),
            None,
        );
        anyhow::bail!("No binary found. Build first with 'cargo build --release'");
    }

    let metadata = std::fs::metadata(&exe)?;
    let size_mb = metadata.len() as f64 / (1024.0 * 1024.0);
    let modified = metadata.modified().ok();

    let age_str = if let Some(mod_time) = modified {
        if let Ok(elapsed) = mod_time.elapsed() {
            let secs = elapsed.as_secs();
            if secs < 60 {
                format!("{} seconds ago", secs)
            } else if secs < 3600 {
                format!("{} minutes ago", secs / 60)
            } else if secs < 86400 {
                format!("{} hours ago", secs / 3600)
            } else {
                format!("{} days ago", secs / 86400)
            }
        } else {
            "unknown".to_string()
        }
    } else {
        "unknown".to_string()
    };

    send_progress(
        "verify",
        &format!(
            "✓ Binary ({}): {:.1} MB, built {}",
            exe_label, size_mb, age_str
        ),
        Some(true),
        None,
    );

    if let Some(repo_dir) = &repo_dir {
        let head_output = std::process::Command::new("git")
            .args(["log", "--oneline", "-1"])
            .current_dir(repo_dir)
            .output();

        if let Ok(output) = head_output {
            let head_str = String::from_utf8_lossy(&output.stdout);
            send_progress(
                "git",
                &format!("📍 HEAD: {}", head_str.trim()),
                Some(true),
                None,
            );
        }
    }

    send_progress(
        "exec",
        "🚀 Restarting server with existing binary...",
        None,
        None,
    );

    crate::logging::info(&format!("Exec'ing into binary: {:?}", exe));

    let socket_path = PathBuf::from(&socket_arg);
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("serve").arg("--socket").arg(socket_arg);
    if let Some(provider) = provider_arg {
        cmd.arg("--provider").arg(provider);
    }
    if let Some(model) = model_arg {
        cmd.arg("--model").arg(model);
    }
    prepare_server_exec(&mut cmd, &socket_path);
    let err = crate::platform::replace_process(&mut cmd);
    crate::server::write_reload_state(
        &request_id,
        env!("JCODE_VERSION"),
        crate::server::ReloadPhase::Failed,
        Some(err.to_string()),
    );

    Err(anyhow::anyhow!("Failed to exec {:?}: {}", exe, err))
}

pub(super) fn provider_cli_arg(provider_name: &str) -> Option<String> {
    let lowered = provider_name.trim().to_lowercase();
    match lowered.as_str() {
        "openai" => Some("openai".to_string()),
        "claude" => Some("claude".to_string()),
        "cursor" => Some("cursor".to_string()),
        "copilot" => Some("copilot".to_string()),
        "gemini" => Some("gemini".to_string()),
        "antigravity" => Some("antigravity".to_string()),
        _ => None,
    }
}

pub(super) fn normalize_model_arg(model: String) -> Option<String> {
    let trimmed = model.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("unknown") {
        None
    } else {
        Some(trimmed.to_string())
    }
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
    shutdown_signals: Arc<RwLock<HashMap<String, crate::agent::InterruptSignal>>>,
    swarm_event_tx: broadcast::Sender<SwarmEvent>,
) {
    use std::process::Command as ProcessCommand;

    let mut rx = super::reload_signal().1.clone();

    loop {
        let signal = match receive_reload_signal(&mut rx).await {
            Some(signal) => signal,
            None => return,
        };

        crate::logging::info("Server: reload signal received via channel");
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

        let prefers_selfdev = signal.prefer_selfdev_binary;

        if let Some((binary, label)) = super::server_update_candidate(prefers_selfdev) {
            if binary.exists() {
                let socket = super::socket_path();
                crate::logging::info(&format!(
                    "Server: exec'ing into {} binary {:?} (socket: {:?}, prep={}ms)",
                    label,
                    binary,
                    socket,
                    reload_started.elapsed().as_millis()
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
    shutdown_signals: &Arc<RwLock<HashMap<String, crate::agent::InterruptSignal>>>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
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

        match event_rx.recv().await {
            Ok(event) => match &event.event {
                SwarmEventType::StatusChange { .. } if watched.contains(&event.session_id) => {}
                SwarmEventType::MemberChange { action }
                    if action == "left" && watched.contains(&event.session_id) => {}
                _ => continue,
            },
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => {
                crate::logging::warn(
                    "Server: swarm event channel closed while waiting for reload checkpoint",
                );
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{graceful_shutdown_sessions, receive_reload_signal};
    use crate::agent::InterruptSignal;
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
}
