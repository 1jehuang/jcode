mod tests {
    use super::super::*;
    use crate::server::debug_jobs::DebugJobStatus;

    #[test]
    fn production_group_transfer_does_not_enable_arbitrary_debug_control() {
        let bundle = serde_json::json!({
            "version": 1, "transfer_id": "test-transfer", "destination_host": "cloud",
            "sessions": [], "swarms": [],
        });
        let prepare = serde_json::json!({
            "session_ids": ["session_one"], "destination_host": "cloud", "expected": bundle,
        });
        for command in [
            "group_transfer:inspect:{\"session_ids\":[\"session_one\"]}".to_string(),
            " server:group_transfer:inspect:{\"swarm_ids\":[\"swarm_one\"]} ".to_string(),
            format!("group_transfer:prepare:{prepare}"),
            format!("group_transfer:reserve:{bundle}"),
            format!("group_transfer:import:{bundle}"),
            format!("group_transfer:abort:{bundle}"),
            format!("group_transfer:check_import:{bundle}"),
        ] {
            assert!(debug_command_authorized(&command, false), "{command}");
        }
        for command in [
            "message:run arbitrary code",
            "server:tool:bash {\"command\":\"true\"}",
            "create_session",
            "destroy_session:session_one",
            "reload",
            "state",
            "client:group_transfer:inspect:{}",
            "tester:group_transfer:inspect:{}",
            "group_transfer:unknown:{}",
            "group_transfer:inspect",
            "group_transfer:inspect:not-json",
            "group_transfer:inspect:[]",
            "group_transfer:prepare:{}",
            "group_transfer:import:{}",
            "group_transfer:inspect:{}\nmessage:run arbitrary code",
            "group_transfer:inspect:{};reload",
            "group_transfer:inspect: {} {}",
        ] {
            assert!(!debug_command_authorized(command, false), "{command}");
        }
        assert!(debug_command_authorized("message:hello", true));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn production_group_transfer_scopes_admission_and_keeps_debug_socket_alive() {
        use super::debug_execution_tests::TestProvider;
        use crate::server::Server;
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        // A child test process isolates the global gate from other tests' turns
        // and keeps all fixture sessions and leases outside user directories.
        const CHILD: &str = "JCODE_TEST_SCOPED_TRANSFER_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let home = tempfile::tempdir().unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "production_group_transfer_scopes_admission_and_keeps_debug_socket_alive",
                    "--test-threads=1",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .env("JCODE_HOME", home.path())
                .env("JCODE_RUNTIME_DIR", home.path().join("runtime"))
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "isolated transport regression failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
            return;
        }
        crate::session::Session::create_with_id("selected".into(), None, None)
            .save_prepared()
            .unwrap();
        let server = Server::new_with_name(Arc::new(TestProvider), Some("transfer-test".into()));
        let (client, stream) = tokio::net::UnixStream::pair().unwrap();
        let handler = tokio::spawn(super::super::handle_debug_client(
            stream,
            server.sessions,
            server.is_processing,
            server.session_id,
            server.provider,
            server.client_connections,
            server.swarm_state.members,
            server.swarm_state.swarms_by_id,
            server.shared_context,
            server.swarm_state.plans,
            server.swarm_state.coordinators,
            server.file_touch,
            server.channel_subscriptions,
            server.channel_subscriptions_by_session,
            server.client_debug_state,
            server.client_debug_response_tx,
            server.debug_jobs,
            server.event_history,
            server.event_counter,
            server.swarm_event_tx,
            server.identity,
            Instant::now(),
            None,
            None,
            server.shutdown_signals,
            server.soft_interrupt_queues,
        ));
        let (reader, mut writer) = client.into_split();
        let mut reader = BufReader::new(reader);
        let mut id = 0;
        for (owner, expect_success) in [
            (None, false),
            (Some("selected"), false),
            (Some("unrelated"), true),
        ] {
            let gate = crate::runtime_activity::global();
            let _activity = match owner {
                Some(session) => gate.try_enter_session(session).unwrap(),
                None => gate.try_enter_global().unwrap(),
            };
            for command in [
                "group_transfer:inspect:{\"session_ids\":[\"selected\"]}",
                "server:group_transfer:inspect:{\"session_ids\":[\"selected\"]}",
            ] {
                id += 1;
                let request = serde_json::json!({
                    "type": "debug_command", "id": id, "command": command,
                });
                writer
                    .write_all(format!("{request}\n").as_bytes())
                    .await
                    .unwrap();
                let mut line = String::new();
                tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    reader.read_line(&mut line),
                )
                .await
                .unwrap()
                .unwrap();
                let response: serde_json::Value = serde_json::from_str(&line).unwrap();
                assert_eq!(response["type"], "debug_response");
                assert_eq!(response["id"], id);
                assert_eq!(
                    response["ok"], expect_success,
                    "owner={owner:?}: {response}"
                );
                let output = response["output"].as_str().unwrap();
                if expect_success {
                    let bundle: serde_json::Value = serde_json::from_str(output).unwrap();
                    assert_eq!(bundle["sessions"][0]["session_id"], "selected");
                    assert_eq!(bundle["sessions"].as_array().unwrap().len(), 1);
                } else {
                    assert!(
                        output.contains("selected sessions or unscoped background tasks are busy"),
                        "{output}"
                    );
                    assert!(
                        output.contains("action=inspect phase=idle_admission"),
                        "{output}"
                    );
                }
            }
        }
        writer
            .write_all(b"{\"type\":\"ping\",\"id\":7}\n")
            .await
            .unwrap();
        let mut line = String::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            reader.read_line(&mut line),
        )
        .await
        .unwrap()
        .unwrap();
        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["type"], "pong");
        assert_eq!(response["id"], 7);
        drop(writer);
        drop(reader);
        handler.await.unwrap().unwrap();
    }

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
        assert!(help.contains("client:picker"));
        assert!(help.contains("agent:context"));
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
            latest_completion_report: None,
            role: "agent".to_string(),
            joined_at: now,
            last_status_change: now,
            is_headless: false,
            output_tail: None,
            todo_progress: None,
            todo_items: Vec::new(),
            runtime: crate::protocol::SwarmMemberRuntime::default(),
            task_label: None,
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
            terminal_env: Vec::new(),
            disconnect_tx: mpsc::unbounded_channel().0,
        }
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
        crate::client_focus::remember_last_focused_session("session_focus")
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
        crate::client_focus::remember_last_focused_session("session_stale")
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

    fn lock_env() -> crate::storage::TestEnvGuard {
        crate::storage::lock_test_env()
    }

    struct EnvVarGuard {
        _lock: crate::storage::TestEnvGuard,
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

    pub(super) struct TestProvider;

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
    async fn resolve_debug_session_never_defaults_to_a_dead_current_session() {
        // Issue #1650: the recorded current session can be a temporary id a
        // client abandoned by resuming another session.
        let agent = test_agent().await;
        let live_id = {
            let agent = agent.lock().await;
            agent.session_id().to_string()
        };
        let sessions = Arc::new(RwLock::new(HashMap::from([(
            live_id.clone(),
            agent.clone(),
        )])));
        let current = Arc::new(RwLock::new("session_ox_dead".to_string()));

        let (resolved_id, resolved_agent) = resolve_debug_session(&sessions, &current, None)
            .await
            .expect("a sole live session is the default");
        assert_eq!(resolved_id, live_id);
        assert!(Arc::ptr_eq(&resolved_agent, &agent));

        let other = test_agent().await;
        let other_id = {
            let agent = other.lock().await;
            agent.session_id().to_string()
        };
        sessions.write().await.insert(other_id.clone(), other);
        let err = match resolve_debug_session(&sessions, &current, None).await {
            Ok((id, _)) => panic!("ambiguous default resolved to {id}"),
            Err(err) => err.to_string(),
        };
        assert!(err.contains("session_ox_dead"), "{err}");
        assert!(err.contains(&live_id) && err.contains(&other_id), "{err}");
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
