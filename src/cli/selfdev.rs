use anyhow::Result;

use crate::{build, logging, session, startup_profile};

use super::output;
use super::provider_init::ProviderChoice;

pub const CLIENT_SELFDEV_ENV: &str = "JCODE_CLIENT_SELFDEV_MODE";

pub fn client_selfdev_requested() -> bool {
    std::env::var(CLIENT_SELFDEV_ENV).is_ok()
}

async fn wait_for_reloading_server() -> bool {
    match crate::server::await_reload_handoff(
        &crate::server::socket_path(),
        std::time::Duration::from_secs(30),
    )
    .await
    {
        crate::server::ReloadWaitStatus::Ready => true,
        crate::server::ReloadWaitStatus::Failed(detail) => {
            logging::warn(&format!(
                "Reload handoff failed while resuming self-dev session: {}",
                detail.unwrap_or_else(|| "unknown reload failure".to_string())
            ));
            false
        }
        crate::server::ReloadWaitStatus::Idle => false,
        crate::server::ReloadWaitStatus::Waiting { .. } => false,
    }
}

pub async fn run_self_dev(should_build: bool, resume_session: Option<String>) -> Result<()> {
    startup_profile::mark("run_self_dev_enter");
    crate::env::set_var(CLIENT_SELFDEV_ENV, "1");

    let repo_dir =
        build::get_repo_dir().ok_or_else(|| anyhow::anyhow!("Could not find jcode repository"))?;

    startup_profile::mark("selfdev_session_create");
    let is_resume = resume_session.is_some();
    let session_id = if let Some(id) = resume_session {
        if let Ok(mut session) = session::Session::load(&id)
            && !session.is_canary
        {
            session.set_canary("self-dev");
            let _ = session.save();
        }
        id
    } else {
        let mut session =
            session::Session::create(None, Some("Self-development session".to_string()));
        session.set_canary("self-dev");
        session.id.clone()
    };

    crate::process_title::set_client_session_title(&session_id, true);

    if should_build {
        let source = build::current_source_state(&repo_dir)?;
        let build = build::selfdev_build_command(&repo_dir);
        output::stderr_info(format!("Building with {}...", build.display));

        build::run_selfdev_build(&repo_dir)?;
        build::ensure_source_state_matches(&repo_dir, &source)?;

        build::publish_local_current_build_for_source(&repo_dir, &source)?;

        output::stderr_info("✓ Build complete; updated current launcher");
    }

    let target_binary = build::client_update_candidate(true)
        .map(|(path, _)| path)
        .or_else(|| build::find_dev_binary(&repo_dir))
        .unwrap_or_else(|| build::release_binary_path(&repo_dir));

    if !target_binary.exists() {
        anyhow::bail!(
            "No binary found at {:?}\n\
             Run 'jcode self-dev --build' first, or build with '{}' and then publish current.",
            target_binary,
            build::selfdev_build_command(&repo_dir).display,
        );
    }

    let hash = build::current_git_hash(&repo_dir)?;
    startup_profile::mark("selfdev_git_hash");

    if !is_resume {
        output::stderr_info(format!("Starting self-dev session with {}...", hash));
    } else {
        logging::info(&format!("Resuming self-dev session with {}...", hash));
    }

    if is_resume {
        crate::env::set_var("JCODE_RESUMING", "1");
    }

    let mut server_running = super::dispatch::server_is_running().await;
    if !server_running && std::env::var("JCODE_RESUMING").is_ok() {
        if let Some(state) = crate::server::recent_reload_state(std::time::Duration::from_secs(30))
        {
            match state.phase {
                crate::server::ReloadPhase::Starting => {
                    logging::info(
                        "Reload state=starting while resuming self-dev session; waiting for existing server to come back",
                    );
                    server_running = wait_for_reloading_server().await;
                }
                crate::server::ReloadPhase::Failed => {
                    if let Ok(Some(version)) =
                        build::rollback_pending_activation_for_session(&session_id)
                    {
                        logging::warn(&format!(
                            "Rolled back failed pending activation for build {} while resuming self-dev session",
                            version
                        ));
                    }
                    logging::warn(&format!(
                        "Reload state=failed while resuming self-dev session: {}",
                        state
                            .detail
                            .unwrap_or_else(|| "unknown reload failure".to_string())
                    ));
                }
                crate::server::ReloadPhase::SocketReady => {}
            }
        }

        if !server_running {
            server_running = super::dispatch::wait_for_resuming_server(
                "self-dev resume without reload marker",
                std::time::Duration::from_secs(5),
            )
            .await;
        }
    }

    if server_running {
        if let Ok(Some(version)) = build::complete_pending_activation_for_session(&session_id) {
            logging::info(&format!(
                "Marked pending self-dev activation as successful for build {}",
                version
            ));
        }
    }

    if !server_running {
        super::dispatch::maybe_prompt_server_bootstrap_login(&ProviderChoice::Auto).await?;
        super::dispatch::spawn_server(&ProviderChoice::Auto, None).await?;
    }

    if std::env::var("JCODE_RESUMING").is_err() && server_running {
        output::stderr_info("Connecting to shared server...");
    }

    output::stderr_info("Starting self-dev TUI...");

    super::tui_launch::run_tui_client(Some(session_id), None, !server_running, false).await
}
#[cfg(test)]
mod tests {
    use super::wait_for_reloading_server;
    use crate::build;
    use crate::{provider, session, storage, tool};
    use std::ffi::OsString;
    use std::sync::Arc;

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        storage::lock_test_env()
    }

    struct TestEnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        prev_home: Option<OsString>,
        prev_test_session: Option<OsString>,
        _temp_home: tempfile::TempDir,
    }

    impl TestEnvGuard {
        fn new() -> anyhow::Result<Self> {
            let lock = lock_env();
            let temp_home = tempfile::Builder::new()
                .prefix("jcode-selfdev-test-home-")
                .tempdir()?;
            let prev_home = std::env::var_os("JCODE_HOME");
            let prev_test_session = std::env::var_os("JCODE_TEST_SESSION");

            crate::env::set_var("JCODE_HOME", temp_home.path());
            crate::env::set_var("JCODE_TEST_SESSION", "1");

            Ok(Self {
                _lock: lock,
                prev_home,
                prev_test_session,
                _temp_home: temp_home,
            })
        }
    }

    impl Drop for TestEnvGuard {
        fn drop(&mut self) {
            if let Some(prev_home) = &self.prev_home {
                crate::env::set_var("JCODE_HOME", prev_home);
            } else {
                crate::env::remove_var("JCODE_HOME");
            }

            if let Some(prev_test_session) = &self.prev_test_session {
                crate::env::set_var("JCODE_TEST_SESSION", prev_test_session);
            } else {
                crate::env::remove_var("JCODE_TEST_SESSION");
            }
        }
    }

    fn setup_test_env() -> TestEnvGuard {
        TestEnvGuard::new().expect("failed to setup isolated test environment")
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
            unimplemented!()
        }

        fn fork(&self) -> Arc<dyn provider::Provider> {
            Arc::new(TestProvider)
        }
    }

    #[tokio::test]
    async fn test_selfdev_tool_registration() {
        let _env = setup_test_env();

        let mut session = session::Session::create(None, Some("Test".to_string()));
        session.set_canary("test");
        assert!(session.is_canary, "Session should be marked as canary");

        let provider = Arc::new(TestProvider) as Arc<dyn provider::Provider>;
        let registry = tool::Registry::new(provider).await;

        let tools_before: Vec<String> = registry.tool_names().await;
        let has_selfdev_before = tools_before.contains(&"selfdev".to_string());

        registry.register_selfdev_tools().await;

        let tools_after: Vec<String> = registry.tool_names().await;
        let has_selfdev_after = tools_after.contains(&"selfdev".to_string());

        println!(
            "Before: selfdev={}, tools={:?}",
            has_selfdev_before,
            tools_before.len()
        );
        println!(
            "After: selfdev={}, tools={:?}",
            has_selfdev_after,
            tools_after.len()
        );

        assert!(has_selfdev_after, "selfdev should be registered");
    }

    #[tokio::test]
    async fn test_selfdev_session_and_registry() {
        let _env = setup_test_env();

        let mut session = session::Session::create(None, Some("Test E2E".to_string()));
        session.set_canary("test-build");
        let session_id = session.id.clone();
        session.save().expect("Failed to save session");

        let loaded = session::Session::load(&session_id).expect("Failed to load session");
        assert!(loaded.is_canary, "Loaded session should be canary");

        let provider = Arc::new(TestProvider) as Arc<dyn provider::Provider>;
        let registry = tool::Registry::new(provider.clone()).await;

        let tools_before = registry.tool_names().await;
        assert!(
            tools_before.contains(&"selfdev".to_string()),
            "selfdev should be available in all sessions initially"
        );

        registry.register_selfdev_tools().await;

        let tools_after = registry.tool_names().await;
        assert!(
            tools_after.contains(&"selfdev".to_string()),
            "selfdev SHOULD be registered after register_selfdev_tools"
        );

        let ctx = tool::ToolContext {
            session_id: session_id.clone(),
            message_id: "test".to_string(),
            tool_call_id: "test".to_string(),
            working_dir: None,
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: tool::ToolExecutionMode::Direct,
        };
        let result = registry
            .execute("selfdev", serde_json::json!({"action": "status"}), ctx)
            .await;

        println!("selfdev status result: {:?}", result);
        assert!(result.is_ok(), "selfdev tool should execute successfully");

        let _ = std::fs::remove_file(
            storage::jcode_dir()
                .unwrap()
                .join("sessions")
                .join(format!("{}.json", session_id)),
        );
    }

    #[tokio::test]
    async fn test_wait_for_reloading_server_returns_false_when_reload_failed() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let prev_socket = std::env::var_os("JCODE_SOCKET");
        let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
        let socket_path = temp.path().join("jcode.sock");
        crate::server::set_socket_path(socket_path.to_str().expect("utf8 socket path"));
        crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());
        crate::server::write_reload_state(
            "reload-test",
            "hash",
            crate::server::ReloadPhase::Failed,
            Some("boom".to_string()),
        );

        assert!(!wait_for_reloading_server().await);

        crate::server::clear_reload_marker();
        if let Some(prev_socket) = prev_socket {
            crate::env::set_var("JCODE_SOCKET", prev_socket);
        } else {
            crate::env::remove_var("JCODE_SOCKET");
        }
        if let Some(prev_runtime) = prev_runtime {
            crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
        } else {
            crate::env::remove_var("JCODE_RUNTIME_DIR");
        }
    }

    #[tokio::test]
    async fn test_wait_for_reloading_server_returns_true_for_live_listener() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let prev_socket = std::env::var_os("JCODE_SOCKET");
        let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
        let socket_path = temp.path().join("jcode.sock");
        crate::server::set_socket_path(socket_path.to_str().expect("utf8 socket path"));
        crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());
        let _listener = crate::transport::Listener::bind(&socket_path).expect("bind listener");

        assert!(wait_for_reloading_server().await);

        if let Some(prev_socket) = prev_socket {
            crate::env::set_var("JCODE_SOCKET", prev_socket);
        } else {
            crate::env::remove_var("JCODE_SOCKET");
        }
        if let Some(prev_runtime) = prev_runtime {
            crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
        } else {
            crate::env::remove_var("JCODE_RUNTIME_DIR");
        }
    }

    #[test]
    fn test_selfdev_build_command_prefers_repo_wrapper_when_present() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scripts_dir = temp.path().join("scripts");
        std::fs::create_dir_all(&scripts_dir).expect("create scripts dir");
        std::fs::write(scripts_dir.join("dev_cargo.sh"), "#!/usr/bin/env bash\n")
            .expect("write wrapper");

        let build = build::selfdev_build_command(temp.path());
        assert_eq!(build.program, "bash");
        assert_eq!(
            build.args,
            vec![
                temp.path()
                    .join("scripts")
                    .join("dev_cargo.sh")
                    .to_string_lossy()
                    .into_owned(),
                "build".to_string(),
                "--release".to_string(),
                "--bin".to_string(),
                "jcode".to_string(),
            ]
        );
    }

    #[test]
    fn test_selfdev_build_command_falls_back_to_cargo_when_wrapper_missing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let build = build::selfdev_build_command(temp.path());
        assert_eq!(build.program, "cargo");
        assert_eq!(
            build.args,
            vec![
                "build".to_string(),
                "--release".to_string(),
                "--bin".to_string(),
                "jcode".to_string(),
            ]
        );
    }
}
