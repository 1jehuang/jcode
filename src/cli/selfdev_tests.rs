use super::wait_for_reloading_server;
use crate::build;
use crate::{provider, session, storage, tool};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    storage::lock_test_env()
}

struct EnvVarGuard {
    vars: Vec<(&'static str, Option<OsString>)>,
}

impl EnvVarGuard {
    fn capture(names: &[&'static str]) -> Self {
        Self {
            vars: names
                .iter()
                .map(|name| (*name, std::env::var_os(name)))
                .collect(),
        }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        for (name, value) in &self.vars {
            if let Some(value) = value {
                crate::env::set_var(name, value);
            } else {
                crate::env::remove_var(name);
            }
        }
    }
}

fn set_socket_test_env(socket_path: &Path, runtime_dir: &Path) -> EnvVarGuard {
    let guard = EnvVarGuard::capture(&["JCODE_SOCKET", "JCODE_RUNTIME_DIR"]);
    crate::server::set_socket_path(socket_path.to_str().expect("utf8 socket path"));
    crate::env::set_var("JCODE_RUNTIME_DIR", runtime_dir);
    guard
}

struct TestEnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    _env: EnvVarGuard,
    _temp_home: tempfile::TempDir,
}

impl TestEnvGuard {
    fn new() -> anyhow::Result<Self> {
        let lock = lock_env();
        let temp_home = tempfile::Builder::new()
            .prefix("jcode-selfdev-test-home-")
            .tempdir()?;
        let env = EnvVarGuard::capture(&["JCODE_HOME", "JCODE_TEST_SESSION"]);

        crate::env::set_var("JCODE_HOME", temp_home.path());
        crate::env::set_var("JCODE_TEST_SESSION", "1");

        Ok(Self {
            _lock: lock,
            _env: env,
            _temp_home: temp_home,
        })
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
        Err(anyhow::anyhow!(
            "TestProvider should not be used for streaming completions in selfdev tests"
        ))
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
    let socket_path = temp.path().join("jcode.sock");
    let _env = set_socket_test_env(&socket_path, temp.path());
    crate::server::write_reload_state(
        "reload-test",
        "hash",
        crate::server::ReloadPhase::Failed,
        Some("boom".to_string()),
    );

    assert!(!wait_for_reloading_server().await);

    crate::server::clear_reload_marker();
}

#[tokio::test]
async fn test_wait_for_reloading_server_returns_true_for_live_listener() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let socket_path = temp.path().join("jcode.sock");
    let _env = set_socket_test_env(&socket_path, temp.path());
    let _listener = crate::transport::Listener::bind(&socket_path).expect("bind listener");

    assert!(wait_for_reloading_server().await);
}

fn isolated_launcher_env() -> (
    std::sync::MutexGuard<'static, ()>,
    EnvVarGuard,
    tempfile::TempDir,
) {
    let lock = lock_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let env = EnvVarGuard::capture(&["JCODE_INSTALL_DIR", "JCODE_HOME", "HOME", "USERPROFILE"]);
    crate::env::set_var("HOME", temp.path());
    crate::env::set_var("USERPROFILE", temp.path());
    crate::env::remove_var("JCODE_INSTALL_DIR");
    crate::env::remove_var("JCODE_HOME");
    (lock, env, temp)
}

fn set_var<T: AsRef<OsStr>>(name: &str, value: T) {
    crate::env::set_var(name, value);
}

fn create_repo_fixture() -> tempfile::TempDir {
    let temp = tempfile::TempDir::new().expect("temp repo");
    std::fs::create_dir_all(temp.path().join(".git")).expect("git dir");
    std::fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"jcode\"\nversion = \"0.1.0\"\n",
    )
    .expect("cargo toml");
    temp
}

#[test]
fn selfdev_cli_repo_resolver_prefers_resumed_session_working_dir() {
    let _env = setup_test_env();
    let repo = create_repo_fixture();
    let other_repo = create_repo_fixture();
    let nested = repo.path().join("src").join("tool");
    std::fs::create_dir_all(&nested).expect("nested dir");

    let mut session = session::Session::create(None, Some("Selfdev Resume".to_string()));
    session.set_canary("self-dev");
    session.working_dir = Some(nested.to_string_lossy().to_string());
    let session_id = session.id.clone();
    session.save().expect("save session");

    let resolved = super::resolve_selfdev_cli_repo_dir_from(
        Some(other_repo.path().to_path_buf()),
        Some(&session_id),
    );

    assert_eq!(resolved.as_deref(), Some(repo.path()));
}

#[test]
fn selfdev_cli_repo_resolver_falls_back_to_primary_without_resumed_repo() {
    let _env = setup_test_env();
    let repo = create_repo_fixture();

    let mut session = session::Session::create(None, Some("Selfdev Resume".to_string()));
    session.set_canary("self-dev");
    session.working_dir = Some("/not/a/jcode/repo".to_string());
    let session_id = session.id.clone();
    session.save().expect("save session");

    let resolved =
        super::resolve_selfdev_cli_repo_dir_from(Some(repo.path().to_path_buf()), Some(&session_id));

    assert_eq!(resolved.as_deref(), Some(repo.path()));
}

#[test]
fn selfdev_repo_discovery_error_mentions_attempted_paths() {
    let _env = setup_test_env();
    let mut session = session::Session::create(None, Some("Selfdev Resume".to_string()));
    session.set_canary("self-dev");
    session.working_dir = Some("/not/a/jcode/repo".to_string());
    let session_id = session.id.clone();
    session.save().expect("save session");

    let error = super::selfdev_repo_discovery_error(Some(&session_id)).to_string();

    assert!(error.contains("/not/a/jcode/repo"));
    assert!(error.contains("current_dir="));
    assert!(error.contains("build/executable fallbacks"));
}

#[test]
fn test_launcher_dir_uses_trimmed_install_dir_before_jcode_home() {
    let (_lock, _env, temp) = isolated_launcher_env();
    let install_dir = temp.path().join("install bin");
    let jcode_home = temp.path().join("jcode-home");
    set_var(
        "JCODE_INSTALL_DIR",
        format!("  {}  ", install_dir.display()),
    );
    set_var("JCODE_HOME", &jcode_home);

    assert_eq!(build::launcher_dir().expect("launcher dir"), install_dir);
}

#[test]
fn test_launcher_dir_ignores_blank_overrides_and_uses_home_default() {
    let (_lock, _env, temp) = isolated_launcher_env();
    set_var("JCODE_INSTALL_DIR", "   ");
    set_var("JCODE_HOME", "\t");

    let expected = default_launcher_dir(temp.path());
    assert_eq!(build::launcher_dir().expect("launcher dir"), expected);
}

fn default_launcher_dir(home: &Path) -> PathBuf {
    if cfg!(windows) {
        home.join("AppData").join("Local").join("jcode").join("bin")
    } else {
        home.join(".local").join("bin")
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
    assert_eq!(build.args.first().map(String::as_str), Some("-lc"));
    let command = build.args.get(1).expect("shell command");
    assert!(command.contains("dev_cargo.sh' build --profile selfdev -p jcode --bin jcode"));
    assert!(!command.contains("jcode-desktop"));
    assert!(build.display.contains("-p jcode --bin jcode"));
    assert!(!build.display.contains("jcode-desktop"));
}

#[test]
fn test_selfdev_build_command_falls_back_to_cargo_when_wrapper_missing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let build = build::selfdev_build_command(temp.path());
    assert_eq!(build.program, "bash");
    assert_eq!(build.args.first().map(String::as_str), Some("-lc"));
    let command = build.args.get(1).expect("shell command");
    assert!(command.contains("cargo build --profile selfdev -p jcode --bin jcode"));
    assert!(!command.contains("jcode-desktop"));
}

#[test]
fn test_selfdev_build_command_can_target_all() {
    let temp = tempfile::tempdir().expect("tempdir");
    let build =
        build::selfdev_build_command_for_target(temp.path(), build::SelfDevBuildTarget::All);
    assert!(build.display.contains("-p jcode --bin jcode"));
    assert!(
        build
            .display
            .contains("-p jcode-desktop --bin jcode-desktop")
    );
}

#[test]
fn test_selfdev_build_command_can_target_tui_only() {
    let temp = tempfile::tempdir().expect("tempdir");
    let build =
        build::selfdev_build_command_for_target(temp.path(), build::SelfDevBuildTarget::Tui);
    assert!(build.display.contains("-p jcode --bin jcode"));
    assert!(!build.display.contains("jcode-desktop"));
}

#[test]
fn test_selfdev_build_command_can_target_desktop_only() {
    let temp = tempfile::tempdir().expect("tempdir");
    let build =
        build::selfdev_build_command_for_target(temp.path(), build::SelfDevBuildTarget::Desktop);
    assert!(!build.display.contains("-p jcode --bin jcode"));
    assert!(
        build
            .display
            .contains("-p jcode-desktop --bin jcode-desktop")
    );
}
