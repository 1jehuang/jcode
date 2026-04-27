use super::*;
use crate::transport::Listener;

struct ReloadTestEnv {
    prev_socket: Option<std::ffi::OsString>,
    prev_runtime: Option<std::ffi::OsString>,
    socket_path: std::path::PathBuf,
}

impl ReloadTestEnv {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let socket_path = temp.path().join("jcode.sock");
        let prev_socket = std::env::var_os("JCODE_SOCKET");
        let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
        crate::server::set_socket_path(socket_path.to_str().expect("utf8 socket path"));
        crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());
        // Keep tempdir alive for the duration of the test helper.
        let _ = temp.keep();
        Self {
            prev_socket,
            prev_runtime,
            socket_path,
        }
    }
}

impl Drop for ReloadTestEnv {
    fn drop(&mut self) {
        crate::server::clear_reload_marker();
        let _ = std::fs::remove_file(&self.socket_path);
        if let Some(prev_socket) = &self.prev_socket {
            crate::env::set_var("JCODE_SOCKET", prev_socket);
        } else {
            crate::env::remove_var("JCODE_SOCKET");
        }
        if let Some(prev_runtime) = &self.prev_runtime {
            crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
        } else {
            crate::env::remove_var("JCODE_RUNTIME_DIR");
        }
    }
}

#[cfg(unix)]
#[test]
fn spawn_lock_serializes_shared_server_bootstrap() {
    let temp = tempfile::tempdir().expect("tempdir");
    let socket_path = temp.path().join("jcode.sock");
    let lock_path = spawn_lock_path(&socket_path);

    let first = try_acquire_spawn_lock(&lock_path)
        .expect("acquire first lock")
        .expect("first lock should succeed");
    let second = try_acquire_spawn_lock(&lock_path).expect("acquire second lock");
    assert!(
        second.is_none(),
        "second lock should be held by first guard"
    );

    drop(first);

    let third = try_acquire_spawn_lock(&lock_path)
        .expect("acquire third lock")
        .expect("third lock should succeed after release");
    drop(third);

    assert!(
        !lock_path.exists(),
        "lock file should be cleaned up when the guard drops"
    );
}

#[test]
fn resolve_resume_id_imports_raw_codex_session_ids() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    crate::env::set_var("JCODE_HOME", temp.path());

    let codex_dir = temp.path().join("external/.codex/sessions/2026/04/16");
    std::fs::create_dir_all(&codex_dir).expect("create codex dir");
    std::fs::write(
            codex_dir.join("rollout.jsonl"),
            concat!(
                "{\"timestamp\":\"2026-04-16T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"codex-cli-resume-test\",\"timestamp\":\"2026-04-16T09:59:00Z\",\"cwd\":\"/tmp/codex-cli-resume\"}}\n",
                "{\"timestamp\":\"2026-04-16T10:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"Resume this Codex session\"}]}}\n",
                "{\"timestamp\":\"2026-04-16T10:00:02Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Imported\"}]}}\n"
            ),
        )
        .expect("write codex transcript");

    let resolved = resolve_resume_id("codex-cli-resume-test").expect("resolve codex id");
    let imported_id = crate::import::imported_codex_session_id("codex-cli-resume-test");
    assert_eq!(resolved, imported_id);

    let session = crate::session::Session::load(&resolved).expect("load imported session");
    assert_eq!(session.messages.len(), 2);

    crate::env::remove_var("JCODE_HOME");
}

#[tokio::test]
async fn wait_for_existing_reload_server_uses_reloading_server_instead_of_spawning() {
    let _guard = crate::storage::lock_test_env();
    let env = ReloadTestEnv::new();
    crate::server::write_reload_state(
        "reload-test",
        "hash",
        crate::server::ReloadPhase::Starting,
        None,
    );

    let bind_path = env.socket_path.clone();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let bind_task = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let listener = Listener::bind(&bind_path).expect("bind replacement listener");
        crate::server::write_reload_state(
            "reload-test",
            "hash",
            crate::server::ReloadPhase::SocketReady,
            None,
        );
        let _listener = listener;
        let _ = release_rx.await;
    });

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        wait_for_existing_reload_server("test"),
    )
    .await
    .expect("reload wait should not hang");
    let _ = release_tx.send(());
    bind_task.await.expect("bind task");
    assert!(result);
}

#[tokio::test]
async fn wait_for_existing_reload_server_returns_false_for_failed_reload() {
    let _guard = crate::storage::lock_test_env();
    let _env = ReloadTestEnv::new();
    crate::server::write_reload_state(
        "reload-test",
        "hash",
        crate::server::ReloadPhase::Failed,
        Some("boom".to_string()),
    );

    assert!(!wait_for_existing_reload_server("test").await);
}

#[tokio::test]
async fn wait_for_resuming_server_detects_delayed_listener_without_marker() {
    let _guard = crate::storage::lock_test_env();
    let env = ReloadTestEnv::new();

    let bind_path = env.socket_path.clone();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let bind_task = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let listener = Listener::bind(&bind_path).expect("bind delayed listener");
        let _listener = listener;
        let _ = release_rx.await;
    });

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        wait_for_resuming_server("test", std::time::Duration::from_secs(1)),
    )
    .await
    .expect("resume wait should not hang");
    let _ = release_tx.send(());
    bind_task.await.expect("bind task");
    assert!(
        result,
        "resume wait should detect a delayed server without requiring a reload marker"
    );
}

#[tokio::test]
async fn wait_for_reloading_server_returns_false_when_idle() {
    let _guard = crate::storage::lock_test_env();
    let _env = ReloadTestEnv::new();

    assert!(!wait_for_reloading_server().await);
}

#[tokio::test]
async fn wait_for_reloading_server_returns_false_when_reload_failed() {
    let _guard = crate::storage::lock_test_env();
    let _env = ReloadTestEnv::new();
    crate::server::write_reload_state(
        "reload-test",
        "hash",
        crate::server::ReloadPhase::Failed,
        Some("boom".to_string()),
    );

    assert!(!wait_for_reloading_server().await);
}

#[tokio::test]
async fn wait_for_reloading_server_returns_true_for_live_listener() {
    let _guard = crate::storage::lock_test_env();
    let env = ReloadTestEnv::new();
    let _listener = Listener::bind(&env.socket_path).expect("bind listener");

    assert!(wait_for_reloading_server().await);
}

#[tokio::test]
async fn server_is_running_at_treats_live_listener_as_running_without_pong() {
    let temp = tempfile::tempdir().expect("tempdir");
    let socket_path = temp.path().join("jcode.sock");

    let _listener = Listener::bind(&socket_path).expect("bind listener");

    assert!(
        server_is_running_at(&socket_path).await,
        "a live listener should prevent duplicate server spawns even if ping is slow or absent"
    );
}
