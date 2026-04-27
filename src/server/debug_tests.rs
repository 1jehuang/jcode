mod tests {
    use super::super::*;
    use crate::server::debug_jobs::DebugJobStatus;

    #[test]
    fn client_debug_state_registers_unregisters_and_falls_back() {
        let mut state = ClientDebugState::default();
        let (tx1, _rx1) = mpsc::unbounded_channel();
        let (tx2, _rx2) = mpsc::unbounded_channel();

        state.register("client-a".to_string(), tx1.clone());
        state.register("client-b".to_string(), tx2.clone());

        let (active_id, _sender) = state.active_sender().expect("active sender present");
        assert_eq!(active_id, "client-b");

        state.unregister("client-b");
        let (fallback_id, _sender) = state.active_sender().expect("fallback sender present");
        assert_eq!(fallback_id, "client-a");

        state.unregister("client-a");
        assert!(state.active_sender().is_none());
    }

    #[test]
    fn debug_job_payloads_include_expected_fields() {
        let now = Instant::now();
        let job = DebugJob {
            id: "job_123".to_string(),
            status: DebugJobStatus::Completed,
            command: "message:hello".to_string(),
            session_id: Some("session_abc".to_string()),
            created_at: now,
            started_at: Some(now),
            finished_at: Some(now),
            output: Some("done".to_string()),
            error: None,
        };

        let summary = job.summary_payload();
        assert_eq!(summary.get("id").and_then(|v| v.as_str()), Some("job_123"));
        assert_eq!(
            summary.get("status").and_then(|v| v.as_str()),
            Some("completed")
        );
        assert_eq!(
            summary.get("session_id").and_then(|v| v.as_str()),
            Some("session_abc")
        );

        let status = job.status_payload();
        assert_eq!(status.get("output").and_then(|v| v.as_str()), Some("done"));
        assert!(status.get("error").is_some());
    }

    #[test]
    fn debug_help_text_mentions_key_namespaces_and_commands() {
        let help = debug_help_text();
        assert!(help.contains("SERVER COMMANDS"));
        assert!(help.contains("CLIENT COMMANDS"));
        assert!(help.contains("TESTER COMMANDS"));
        assert!(help.contains("message_async:<text>"));
        assert!(help.contains("client:frame"));
    }

    #[test]
    fn swarm_debug_help_text_mentions_core_swarm_sections() {
        let help = swarm_debug_help_text();
        assert!(help.contains("MEMBERS & STRUCTURE"));
        assert!(help.contains("PLAN PROPOSALS"));
        assert!(help.contains("REAL-TIME EVENTS"));
        assert!(help.contains("swarm:list"));
    }

    #[test]
    fn parse_namespaced_command_defaults_to_server_namespace() {
        assert_eq!(parse_namespaced_command("state"), ("server", "state"));
        assert_eq!(
            parse_namespaced_command("swarm:list"),
            ("server", "swarm:list")
        );
    }

    #[test]
    fn parse_namespaced_command_recognizes_known_namespaces() {
        assert_eq!(
            parse_namespaced_command("client:frame"),
            ("client", "frame")
        );
        assert_eq!(parse_namespaced_command("tester:list"), ("tester", "list"));
        assert_eq!(
            parse_namespaced_command("server:state"),
            ("server", "state")
        );
    }
}

mod transcript_routing_tests {
    use super::super::{
        ClientConnectionInfo, ClientDebugState, resolve_client_debug_sender,
        resolve_transcript_target_session,
    };
    use crate::protocol::ServerEvent;
    use crate::server::SwarmMember;
    use std::collections::HashMap;
    #[cfg(target_os = "linux")]
    use std::ffi::OsString;
    use std::sync::Arc;
    use std::time::Instant;
    use tokio::sync::{RwLock, mpsc};

    fn live_member(session_id: &str, connection_id: &str) -> SwarmMember {
        let (event_tx, _event_rx) = mpsc::unbounded_channel::<ServerEvent>();
        let now = Instant::now();
        SwarmMember {
            session_id: session_id.to_string(),
            event_tx: event_tx.clone(),
            event_txs: HashMap::from([(connection_id.to_string(), event_tx)]),
            working_dir: None,
            swarm_id: None,
            swarm_enabled: false,
            status: "ready".to_string(),
            detail: None,
            friendly_name: None,
            report_back_to_session_id: None,
            role: "agent".to_string(),
            joined_at: now,
            last_status_change: now,
            is_headless: false,
        }
    }

    fn connection(
        session_id: &str,
        debug_client_id: &str,
        last_seen: Instant,
    ) -> ClientConnectionInfo {
        ClientConnectionInfo {
            client_id: format!("conn-{session_id}"),
            session_id: session_id.to_string(),
            client_instance_id: None,
            debug_client_id: Some(debug_client_id.to_string()),
            connected_at: last_seen,
            last_seen,
            is_processing: false,
            current_tool_name: None,
            disconnect_tx: mpsc::unbounded_channel().0,
        }
    }

    #[cfg(target_os = "linux")]
    struct EnvVarGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    #[cfg(target_os = "linux")]
    impl EnvVarGuard {
        fn set<K: AsRef<std::ffi::OsStr>>(key: &'static str, value: K) -> Self {
            let previous = std::env::var_os(key);
            crate::env::set_var(key, value);
            Self { key, previous }
        }
    }

    #[cfg(target_os = "linux")]
    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                crate::env::set_var(self.key, previous);
            } else {
                crate::env::remove_var(self.key);
            }
        }
    }

    #[cfg(target_os = "linux")]
    struct ChildGuard(std::process::Child);

    #[cfg(target_os = "linux")]
    impl ChildGuard {
        fn spawn_named(name: &str) -> Self {
            let child = std::process::Command::new("python3")
                .args([
                    "-c",
                    "import ctypes, sys, time; libc = ctypes.CDLL(None); libc.prctl(15, sys.argv[1].encode(), 0, 0, 0); time.sleep(30)",
                    name,
                ])
                .spawn()
                .expect("spawn named helper process");
            Self(child)
        }

        fn pid(&self) -> u32 {
            self.0.id()
        }
    }

    #[cfg(target_os = "linux")]
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[cfg(target_os = "linux")]
    fn install_fake_niri(bin_dir: &std::path::Path, pid: u32, title: &str) {
        use std::os::unix::fs::PermissionsExt;

        std::fs::create_dir_all(bin_dir).expect("create fake bin dir");
        let script = bin_dir.join("niri");
        let json = serde_json::json!({
            "pid": pid,
            "title": title,
            "app_id": "kitty"
        });
        std::fs::write(&script, format!("#!/bin/sh\nprintf '%s\\n' '{}'\n", json))
            .expect("write fake niri script");
        let mut perms = std::fs::metadata(&script)
            .expect("fake niri metadata")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).expect("chmod fake niri");
    }

    #[tokio::test]
    async fn resolve_transcript_target_session_uses_requested_connected_session() {
        let client_connections = Arc::new(RwLock::new(HashMap::from([(
            "conn-1".to_string(),
            connection("session_abc", "debug-1", Instant::now()),
        )])));
        let client_debug_state = Arc::new(RwLock::new(ClientDebugState::default()));
        let swarm_members = Arc::new(RwLock::new(HashMap::from([(
            "session_abc".to_string(),
            live_member("session_abc", "conn-1"),
        )])));

        let resolved = resolve_transcript_target_session(
            Some("session_abc".to_string()),
            &client_connections,
            &client_debug_state,
            &swarm_members,
        )
        .await
        .expect("resolve connected requested session");

        assert_eq!(resolved, "session_abc");
    }

    #[tokio::test]
    async fn resolve_transcript_target_session_prefers_last_focused_live_session() {
        let _guard = crate::storage::lock_test_env();
        let jcode_dir = crate::storage::jcode_dir().expect("jcode dir");
        let active_dir = jcode_dir.join("active_pids");
        std::fs::create_dir_all(&active_dir).expect("create active_pids");
        std::fs::write(active_dir.join("session_focus"), "12345").expect("write active pid");
        crate::dictation::remember_last_focused_session("session_focus")
            .expect("remember last focused session");

        let client_connections = Arc::new(RwLock::new(HashMap::from([(
            "conn-1".to_string(),
            connection("session_focus", "debug-1", Instant::now()),
        )])));
        let client_debug_state = Arc::new(RwLock::new(ClientDebugState::default()));
        let swarm_members = Arc::new(RwLock::new(HashMap::from([(
            "session_focus".to_string(),
            live_member("session_focus", "conn-1"),
        )])));

        let resolved = resolve_transcript_target_session(
            None,
            &client_connections,
            &client_debug_state,
            &swarm_members,
        )
        .await
        .expect("resolve last-focused session");

        assert_eq!(resolved, "session_focus");
    }

    #[tokio::test]
    async fn resolve_transcript_target_session_rejects_requested_session_without_connected_tui() {
        let client_connections = Arc::new(RwLock::new(HashMap::from([(
            "conn-1".to_string(),
            connection("session_abc", "debug-1", Instant::now()),
        )])));
        let client_debug_state = Arc::new(RwLock::new(ClientDebugState::default()));
        let swarm_members = Arc::new(RwLock::new(HashMap::new()));

        let err = resolve_transcript_target_session(
            Some("session_abc".to_string()),
            &client_connections,
            &client_debug_state,
            &swarm_members,
        )
        .await
        .expect_err("requested session without connected tui should error");

        assert!(
            err.to_string()
                .contains("does not have a connected TUI client")
        );
    }

    #[tokio::test]
    async fn resolve_transcript_target_session_falls_back_to_most_recent_live_tui_when_last_focused_not_connected()
     {
        let _guard = crate::storage::lock_test_env();
        let jcode_dir = crate::storage::jcode_dir().expect("jcode dir");
        let active_dir = jcode_dir.join("active_pids");
        std::fs::create_dir_all(&active_dir).expect("create active_pids");
        std::fs::write(active_dir.join("session_stale"), "12345").expect("write active pid");
        crate::dictation::remember_last_focused_session("session_stale")
            .expect("remember last focused session");

        let now = Instant::now();
        let client_connections = Arc::new(RwLock::new(HashMap::from([
            (
                "conn-1".to_string(),
                connection(
                    "session_stale_debug",
                    "debug-1",
                    now - std::time::Duration::from_secs(60),
                ),
            ),
            (
                "conn-2".to_string(),
                connection("session_recent", "debug-2", now),
            ),
        ])));
        let client_debug_state = Arc::new(RwLock::new(ClientDebugState {
            active_id: Some("debug-1".to_string()),
            clients: HashMap::new(),
        }));
        let swarm_members = Arc::new(RwLock::new(HashMap::from([(
            "session_recent".to_string(),
            live_member("session_recent", "conn-2"),
        )])));

        let resolved = resolve_transcript_target_session(
            None,
            &client_connections,
            &client_debug_state,
            &swarm_members,
        )
        .await
        .expect("resolve fallback live session");

        assert_eq!(resolved, "session_recent");
    }

    #[tokio::test]
    async fn resolve_transcript_target_session_ignores_non_live_requesting_clients() {
        let now = Instant::now();
        let client_connections = Arc::new(RwLock::new(HashMap::from([
            (
                "conn-cli".to_string(),
                connection("session_cli", "debug-cli", now),
            ),
            (
                "conn-tui".to_string(),
                connection(
                    "session_tui",
                    "debug-tui",
                    now - std::time::Duration::from_secs(30),
                ),
            ),
        ])));
        let client_debug_state = Arc::new(RwLock::new(ClientDebugState {
            active_id: Some("debug-cli".to_string()),
            clients: HashMap::new(),
        }));
        let swarm_members = Arc::new(RwLock::new(HashMap::from([(
            "session_tui".to_string(),
            live_member("session_tui", "conn-tui"),
        )])));

        let resolved = resolve_transcript_target_session(
            None,
            &client_connections,
            &client_debug_state,
            &swarm_members,
        )
        .await
        .expect("resolve live tui session");

        assert_eq!(resolved, "session_tui");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn resolve_transcript_target_session_prefers_current_niri_focused_session_over_last_focused()
     {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("tempdir");
        let _home = EnvVarGuard::set("JCODE_HOME", temp.path());

        let active_dir = temp.path().join("active_pids");
        std::fs::create_dir_all(&active_dir).expect("create active_pids");

        let fox = "session_fox_100";
        let swan = "session_swan_200";
        std::fs::write(active_dir.join(fox), "111").expect("write fox active pid");
        std::fs::write(active_dir.join(swan), "222").expect("write swan active pid");
        crate::dictation::remember_last_focused_session(fox).expect("remember fox session");

        let focused_process = ChildGuard::spawn_named("jcode:d:swan");
        let bin_dir = temp.path().join("bin");
        install_fake_niri(
            &bin_dir,
            focused_process.pid(),
            "🦢 jcode/cliff Swan [self-dev]",
        );
        let prev_path = std::env::var_os("PATH").unwrap_or_default();
        let mut path = OsString::from(bin_dir.as_os_str());
        path.push(":");
        path.push(prev_path);
        let _path = EnvVarGuard::set("PATH", path);

        let now = Instant::now();
        let client_connections = Arc::new(RwLock::new(HashMap::from([
            (
                "conn-fox".to_string(),
                connection(fox, "debug-fox", now - std::time::Duration::from_secs(30)),
            ),
            ("conn-swan".to_string(), connection(swan, "debug-swan", now)),
        ])));
        let client_debug_state = Arc::new(RwLock::new(ClientDebugState::default()));
        let swarm_members = Arc::new(RwLock::new(HashMap::from([
            (fox.to_string(), live_member(fox, "conn-fox")),
            (swan.to_string(), live_member(swan, "conn-swan")),
        ])));

        let resolved = resolve_transcript_target_session(
            None,
            &client_connections,
            &client_debug_state,
            &swarm_members,
        )
        .await
        .expect("resolve transcript target from focused session");

        assert_eq!(resolved, swan);
    }

    #[tokio::test]
    async fn resolve_client_debug_sender_uses_requested_session() {
        let (tx_target, _rx_target) = mpsc::unbounded_channel();
        let (tx_other, _rx_other) = mpsc::unbounded_channel();

        let client_debug_state = Arc::new(RwLock::new(ClientDebugState::default()));
        {
            let mut state = client_debug_state.write().await;
            state.register("debug-target".to_string(), tx_target.clone());
            state.register("debug-other".to_string(), tx_other.clone());
            state.active_id = Some("debug-other".to_string());
        }

        let now = Instant::now();
        let client_connections = Arc::new(RwLock::new(HashMap::from([
            (
                "target".to_string(),
                connection("session-target", "debug-target", now),
            ),
            (
                "other".to_string(),
                connection("session-other", "debug-other", now),
            ),
        ])));

        let (client_id, _sender) = resolve_client_debug_sender(
            Some("session-target"),
            &client_connections,
            &client_debug_state,
        )
        .await
        .expect("requested session should resolve");

        assert_eq!(client_id, "debug-target");
    }

    #[tokio::test]
    async fn resolve_client_debug_sender_prefers_most_recent_requested_session_connection() {
        let (tx_old, _rx_old) = mpsc::unbounded_channel();
        let (tx_new, _rx_new) = mpsc::unbounded_channel();

        let client_debug_state = Arc::new(RwLock::new(ClientDebugState::default()));
        {
            let mut state = client_debug_state.write().await;
            state.register("debug-old".to_string(), tx_old.clone());
            state.register("debug-new".to_string(), tx_new.clone());
        }

        let now = Instant::now();
        let client_connections = Arc::new(RwLock::new(HashMap::from([
            (
                "old".to_string(),
                connection(
                    "session-target",
                    "debug-old",
                    now - std::time::Duration::from_secs(30),
                ),
            ),
            (
                "new".to_string(),
                connection("session-target", "debug-new", now),
            ),
        ])));

        let (client_id, _sender) = resolve_client_debug_sender(
            Some("session-target"),
            &client_connections,
            &client_debug_state,
        )
        .await
        .expect("most recent session client should resolve");

        assert_eq!(client_id, "debug-new");
    }

    #[tokio::test]
    async fn resolve_client_debug_sender_without_request_uses_active_client() {
        let (tx_a, _rx_a) = mpsc::unbounded_channel();
        let (tx_b, _rx_b) = mpsc::unbounded_channel();

        let client_debug_state = Arc::new(RwLock::new(ClientDebugState::default()));
        {
            let mut state = client_debug_state.write().await;
            state.register("debug-a".to_string(), tx_a.clone());
            state.register("debug-b".to_string(), tx_b.clone());
        }

        let client_connections = Arc::new(RwLock::new(HashMap::new()));

        let (client_id, _sender) =
            resolve_client_debug_sender(None, &client_connections, &client_debug_state)
                .await
                .expect("active client should resolve");

        assert_eq!(client_id, "debug-b");
    }
}

mod debug_execution_tests {
    use crate::agent::Agent;
    use crate::provider;
    use crate::server::debug_command_exec::{debug_message_timeout_secs, resolve_debug_session};
    use crate::tool::Registry;
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::sync::Arc;
    use tokio::sync::{Mutex as AsyncMutex, RwLock};

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        crate::storage::lock_test_env()
    }

    struct EnvVarGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let lock = lock_env();
            let previous = std::env::var_os(key);
            crate::env::set_var(key, value);
            Self {
                _lock: lock,
                key,
                previous,
            }
        }

        fn remove(key: &'static str) -> Self {
            let lock = lock_env();
            let previous = std::env::var_os(key);
            crate::env::remove_var(key);
            Self {
                _lock: lock,
                key,
                previous,
            }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(prev) = &self.previous {
                crate::env::set_var(self.key, prev);
            } else {
                crate::env::remove_var(self.key);
            }
        }
    }

    struct TestProvider;

    #[async_trait::async_trait]
    impl provider::Provider for TestProvider {
        fn name(&self) -> &str {
            "test"
        }

        fn model(&self) -> String {
            "test".to_string()
        }

        fn available_models(&self) -> Vec<&'static str> {
            vec![]
        }

        fn available_models_display(&self) -> Vec<String> {
            vec![]
        }

        async fn prefetch_models(&self) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_model(&self, _model: &str) -> anyhow::Result<()> {
            Ok(())
        }

        fn handles_tools_internally(&self) -> bool {
            false
        }

        async fn complete(
            &self,
            _messages: &[crate::message::Message],
            _tools: &[crate::message::ToolDefinition],
            _system: &str,
            _session_id: Option<&str>,
        ) -> anyhow::Result<crate::provider::EventStream> {
            Err(anyhow::anyhow!(
                "test provider complete should not be called in debug tests"
            ))
        }

        fn fork(&self) -> Arc<dyn provider::Provider> {
            Arc::new(TestProvider)
        }
    }

    async fn test_agent() -> Arc<AsyncMutex<Agent>> {
        let provider = Arc::new(TestProvider) as Arc<dyn provider::Provider>;
        let registry = Registry::new(provider.clone()).await;
        Arc::new(AsyncMutex::new(Agent::new(provider, registry)))
    }

    #[tokio::test]
    async fn resolve_debug_session_uses_requested_session_when_present() {
        let agent = test_agent().await;
        let session_id = {
            let agent = agent.lock().await;
            agent.session_id().to_string()
        };
        let sessions = Arc::new(RwLock::new(HashMap::from([(
            session_id.clone(),
            agent.clone(),
        )])));
        let current = Arc::new(RwLock::new(String::new()));

        let (resolved_id, resolved_agent) =
            resolve_debug_session(&sessions, &current, Some(session_id.clone()))
                .await
                .expect("resolve requested session");

        assert_eq!(resolved_id, session_id);
        assert!(Arc::ptr_eq(&resolved_agent, &agent));
    }

    #[tokio::test]
    async fn resolve_debug_session_falls_back_to_current_session() {
        let agent = test_agent().await;
        let session_id = {
            let agent = agent.lock().await;
            agent.session_id().to_string()
        };
        let sessions = Arc::new(RwLock::new(HashMap::from([(
            session_id.clone(),
            agent.clone(),
        )])));
        let current = Arc::new(RwLock::new(session_id.clone()));

        let (resolved_id, resolved_agent) = resolve_debug_session(&sessions, &current, None)
            .await
            .expect("resolve current session");

        assert_eq!(resolved_id, session_id);
        assert!(Arc::ptr_eq(&resolved_agent, &agent));
    }

    #[tokio::test]
    async fn resolve_debug_session_uses_only_session_when_singleton() {
        let agent = test_agent().await;
        let session_id = {
            let agent = agent.lock().await;
            agent.session_id().to_string()
        };
        let sessions = Arc::new(RwLock::new(HashMap::from([(
            session_id.clone(),
            agent.clone(),
        )])));
        let current = Arc::new(RwLock::new(String::new()));

        let (resolved_id, _) = resolve_debug_session(&sessions, &current, None)
            .await
            .expect("resolve single session");

        assert_eq!(resolved_id, session_id);
    }

    #[tokio::test]
    async fn resolve_debug_session_errors_for_unknown_or_missing_session() {
        let agent_a = test_agent().await;
        let id_a = {
            let agent = agent_a.lock().await;
            agent.session_id().to_string()
        };
        let agent_b = test_agent().await;
        let id_b = {
            let agent = agent_b.lock().await;
            agent.session_id().to_string()
        };

        let sessions = Arc::new(RwLock::new(HashMap::from([
            (id_a.clone(), agent_a),
            (id_b.clone(), agent_b),
        ])));
        let current = Arc::new(RwLock::new(String::new()));

        let unknown = resolve_debug_session(&sessions, &current, Some("missing".to_string())).await;
        let unknown_err = match unknown {
            Ok(_) => panic!("expected unknown session to error"),
            Err(err) => err,
        };
        assert!(unknown_err.to_string().contains("Unknown session_id"));

        let missing = resolve_debug_session(&sessions, &current, None).await;
        let missing_err = match missing {
            Ok(_) => panic!("expected missing active session to error"),
            Err(err) => err,
        };
        assert!(missing_err.to_string().contains("No active session found"));
    }

    #[test]
    fn debug_message_timeout_secs_reads_valid_env_values() {
        let _guard = EnvVarGuard::set("JCODE_DEBUG_MESSAGE_TIMEOUT_SECS", "17");
        assert_eq!(debug_message_timeout_secs(), Some(17));
    }

    #[test]
    fn debug_message_timeout_secs_ignores_missing_empty_invalid_and_zero() {
        let _guard = EnvVarGuard::remove("JCODE_DEBUG_MESSAGE_TIMEOUT_SECS");
        assert_eq!(debug_message_timeout_secs(), None);
        drop(_guard);

        let _guard = EnvVarGuard::set("JCODE_DEBUG_MESSAGE_TIMEOUT_SECS", "   ");
        assert_eq!(debug_message_timeout_secs(), None);
        drop(_guard);

        let _guard = EnvVarGuard::set("JCODE_DEBUG_MESSAGE_TIMEOUT_SECS", "abc");
        assert_eq!(debug_message_timeout_secs(), None);
        drop(_guard);

        let _guard = EnvVarGuard::set("JCODE_DEBUG_MESSAGE_TIMEOUT_SECS", "0");
        assert_eq!(debug_message_timeout_secs(), None);
    }
}
