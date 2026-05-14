use super::*;
use crate::bus::BackgroundTaskStatus;
use std::ffi::OsStr;
use std::sync::{LazyLock, Mutex};

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct EnvVarGuard {
    key: &'static str,
    original: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
        let original = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, original }
    }

    fn remove(key: &'static str) -> Self {
        let original = std::env::var_os(key);
        crate::env::remove_var(key);
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.original {
            Some(value) => crate::env::set_var(self.key, value),
            None => crate::env::remove_var(self.key),
        }
    }
}

fn create_test_context(session_id: &str, working_dir: Option<std::path::PathBuf>) -> ToolContext {
    ToolContext {
        session_id: session_id.to_string(),
        message_id: "test-message".to_string(),
        tool_call_id: "test-tool-call".to_string(),
        working_dir,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: crate::tool::ToolExecutionMode::Direct,
    }
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

fn test_source_state(repo_dir: &std::path::Path) -> build::SourceState {
    build::SourceState {
        repo_scope: "test-repo-scope".to_string(),
        worktree_scope: build::worktree_scope_key(repo_dir)
            .unwrap_or_else(|_| "test-worktree".to_string()),
        short_hash: "test-build".to_string(),
        full_hash: "test-build-full".to_string(),
        dirty: true,
        fingerprint: "test-fingerprint".to_string(),
        version_label: "test-build".to_string(),
        changed_paths: 0,
    }
}

async fn wait_for_task_completion(task_id: &str) -> background::TaskStatusFile {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Some(status) = background::global().status(task_id).await
            && status.status != BackgroundTaskStatus::Running
        {
            return status;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for background task {}",
            task_id
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[test]
fn test_reload_context_serialization() {
    // Create test context with task info
    let ctx = ReloadContext {
        task_context: Some("Testing the reload feature".to_string()),
        version_before: "v0.1.100".to_string(),
        version_after: "abc1234".to_string(),
        session_id: "test-session-123".to_string(),
        timestamp: "2025-01-20T00:00:00Z".to_string(),
    };

    // Serialize and deserialize
    let json = serde_json::to_string(&ctx).unwrap();
    let loaded: ReloadContext = serde_json::from_str(&json).unwrap();

    assert_eq!(
        loaded.task_context,
        Some("Testing the reload feature".to_string())
    );
    assert_eq!(loaded.version_before, "v0.1.100");
    assert_eq!(loaded.version_after, "abc1234");
    assert_eq!(loaded.session_id, "test-session-123");
}

#[test]
fn test_reload_context_path() {
    // Just verify the session-scoped path function works
    let path = ReloadContext::path_for_session("test-session-123");
    assert!(path.is_ok());
    let path = path.unwrap();
    let path_str = path.to_string_lossy();
    assert!(path_str.contains("reload-context-test-session-123.json"));
}

#[test]
fn test_reload_context_save_and_load_for_session_uses_session_scoped_file() {
    let _storage_guard = crate::storage::lock_test_env();
    let _lock = lock_env();
    let temp_home = tempfile::TempDir::new().expect("temp home");
    let _home_guard = EnvVarGuard::set("JCODE_HOME", temp_home.path());

    let ctx = ReloadContext {
        task_context: Some("Testing scoped reload context".to_string()),
        version_before: "v0.1.100".to_string(),
        version_after: "abc1234".to_string(),
        session_id: "test-session-123".to_string(),
        timestamp: "2025-01-20T00:00:00Z".to_string(),
    };

    ctx.save().expect("save reload context");

    let path = ReloadContext::path_for_session("test-session-123").expect("context path");
    assert!(
        path.exists(),
        "session-scoped reload context file should exist"
    );

    let peeked = ReloadContext::peek_for_session("test-session-123")
        .expect("peek should succeed")
        .expect("context should exist");
    assert_eq!(peeked.session_id, "test-session-123");

    let loaded = ReloadContext::load_for_session("test-session-123")
        .expect("load should succeed")
        .expect("context should exist");
    assert_eq!(loaded.session_id, "test-session-123");
    assert!(
        !path.exists(),
        "load_for_session should consume the context file"
    );
}

#[test]
fn test_recovery_directive_prefers_reload_context_when_present() {
    let ctx = ReloadContext {
        task_context: Some("Resume a self-dev reload".to_string()),
        version_before: "old-build".to_string(),
        version_after: "new-build".to_string(),
        session_id: "session-123".to_string(),
        timestamp: "2026-04-19T00:00:00Z".to_string(),
    };

    let directive = ReloadContext::recovery_directive(
        Some(&ctx),
        true,
        "\nPersisted background task(s) detected.",
        Some(12),
    )
    .expect("directive should exist");

    assert_eq!(
        directive.reconnect_notice.as_deref(),
        Some("Reloaded with build new-build")
    );
    assert!(directive.continuation_message.contains("Reload succeeded"));
    assert!(
        directive
            .continuation_message
            .contains("Persisted background task(s)")
    );
    assert!(
        directive
            .continuation_message
            .contains("Session restored with 12 turns")
    );
}

#[test]
fn test_recovery_directive_uses_interrupted_message_without_reload_context() {
    let directive = ReloadContext::recovery_directive(None, true, "", None)
        .expect("interrupted sessions should get a directive");

    assert!(directive.reconnect_notice.is_none());
    assert!(
        directive
            .continuation_message
            .contains("interrupted by a server reload while a tool was running")
    );
}

#[test]
fn test_recovery_directive_returns_none_when_no_reload_recovery_needed() {
    assert!(ReloadContext::recovery_directive(None, false, "", None).is_none());
}

#[test]
fn reload_timeout_secs_defaults_to_15() {
    let _storage_guard = crate::storage::lock_test_env();
    let _lock = lock_env();
    let _guard = EnvVarGuard::remove("JCODE_SELFDEV_RELOAD_TIMEOUT_SECS");
    assert_eq!(SelfDevTool::reload_timeout_secs(), 15);
}

#[test]
fn reload_timeout_secs_honors_valid_env_override() {
    let _storage_guard = crate::storage::lock_test_env();
    let _lock = lock_env();
    let _guard = EnvVarGuard::set("JCODE_SELFDEV_RELOAD_TIMEOUT_SECS", "27");
    assert_eq!(SelfDevTool::reload_timeout_secs(), 27);
}

#[test]
fn reload_timeout_secs_ignores_empty_invalid_and_zero_values() {
    let _storage_guard = crate::storage::lock_test_env();
    let _lock = lock_env();
    let _guard = EnvVarGuard::set("JCODE_SELFDEV_RELOAD_TIMEOUT_SECS", "   ");
    assert_eq!(SelfDevTool::reload_timeout_secs(), 15);
    drop(_guard);

    let _guard = EnvVarGuard::set("JCODE_SELFDEV_RELOAD_TIMEOUT_SECS", "abc");
    assert_eq!(SelfDevTool::reload_timeout_secs(), 15);
    drop(_guard);

    let _guard = EnvVarGuard::set("JCODE_SELFDEV_RELOAD_TIMEOUT_SECS", "0");
    assert_eq!(SelfDevTool::reload_timeout_secs(), 15);
}

#[test]
fn schema_only_advertises_core_selfdev_fields() {
    let schema = SelfDevTool::new().parameters_schema();
    let props = schema["properties"]
        .as_object()
        .expect("selfdev schema should have properties");

    assert!(props.contains_key("action"));
    assert!(props.contains_key("prompt"));
    assert!(props.contains_key("context"));
    assert!(props.contains_key("reason"));
    assert!(props.contains_key("target"));
    assert!(props.contains_key("command"));
    assert!(props.contains_key("request_id"));
    assert!(props.contains_key("task_id"));
    assert!(!props.contains_key("notify"));
    assert!(!props.contains_key("wake"));
}

#[tokio::test]
async fn test_action_queues_command_in_test_mode() {
    let _storage_guard = crate::storage::lock_test_env();
    let _lock = lock_env();
    let temp_home = tempfile::TempDir::new().expect("temp home");
    let _home_guard = EnvVarGuard::set("JCODE_HOME", temp_home.path());
    let _test_guard = EnvVarGuard::set("JCODE_TEST_SESSION", "1");
    let repo = create_repo_fixture();

    let tool = SelfDevTool::new();
    let ctx = create_test_context(
        "session-selfdev-test-action",
        Some(repo.path().to_path_buf()),
    );
    let output = tool
        .execute(
            json!({
                "action": "test",
                "command": "cargo test -p jcode selfdev_build_command",
                "reason": "verify selfdev test queue"
            }),
            ctx,
        )
        .await
        .expect("selfdev test should queue");

    assert!(output.output.contains("Self-dev test queued"));
    assert!(
        output
            .output
            .contains("cargo test -p jcode selfdev_build_command")
    );
}

#[tokio::test]
async fn do_reload_returns_after_ack_in_direct_mode() {
    let request_id = server::send_reload_signal("direct-hash".to_string(), None, true);
    let waiter = tokio::spawn({
        let request_id = request_id.clone();
        async move { server::wait_for_reload_ack(&request_id, std::time::Duration::from_secs(1)).await }
    });

    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    server::acknowledge_reload_signal(&crate::server::ReloadSignal {
        hash: "direct-hash".to_string(),
        triggering_session: None,
        prefer_selfdev_binary: true,
        request_id: "ignored".to_string(),
    });

    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    server::acknowledge_reload_signal(&crate::server::ReloadSignal {
        hash: "direct-hash".to_string(),
        triggering_session: None,
        prefer_selfdev_binary: true,
        request_id,
    });

    let ack = waiter
        .await
        .expect("waiter task should complete")
        .expect("ack should be received");
    assert_eq!(ack.hash, "direct-hash");
}

#[test]
fn reload_repo_resolver_uses_working_dir_when_primary_detection_fails() {
    let repo = create_repo_fixture();
    let nested = repo.path().join("crates").join("jcode-build-support");
    std::fs::create_dir_all(&nested).expect("nested dir");

    let resolved = reload::resolve_selfdev_reload_repo_dir_from(None, Some(&nested), None);
    assert_eq!(resolved.as_deref(), Some(repo.path()));
}

#[test]
fn reload_repo_resolver_uses_recent_build_repo_when_other_detection_fails() {
    let repo = create_repo_fixture();
    let nested = repo.path().join("src").join("cli");
    std::fs::create_dir_all(&nested).expect("nested dir");

    let resolved = reload::resolve_selfdev_reload_repo_dir_from(None, None, Some(nested));
    assert_eq!(resolved.as_deref(), Some(repo.path()));
}

#[test]
fn reload_repo_resolver_prefers_working_dir_before_recent_build_repo() {
    let repo = create_repo_fixture();
    let recent_repo = create_repo_fixture();
    let nested = repo.path().join("src").join("tool");
    let recent_nested = recent_repo.path().join("src").join("cli");
    std::fs::create_dir_all(&nested).expect("nested dir");
    std::fs::create_dir_all(&recent_nested).expect("recent nested dir");

    let resolved = reload::resolve_selfdev_reload_repo_dir_from(
        None,
        Some(&nested),
        Some(recent_nested),
    );
    assert_eq!(resolved.as_deref(), Some(repo.path()));
}

#[test]
fn reload_repo_resolver_prefers_recent_build_repo_before_primary_fallback() {
    let primary_repo = create_repo_fixture();
    let recent_repo = create_repo_fixture();
    let recent_nested = recent_repo.path().join("src").join("tool");
    std::fs::create_dir_all(&recent_nested).expect("recent nested dir");

    let resolved = reload::resolve_selfdev_reload_repo_dir_from(
        Some(primary_repo.path().to_path_buf()),
        None,
        Some(recent_nested),
    );
    assert_eq!(resolved.as_deref(), Some(recent_repo.path()));
}

#[tokio::test]
async fn enter_creates_selfdev_session_in_test_mode() {
    let _storage_guard = crate::storage::lock_test_env();
    let _lock = lock_env();
    let temp_home = tempfile::TempDir::new().expect("temp home");
    let _home_guard = EnvVarGuard::set("JCODE_HOME", temp_home.path());
    let _test_guard = EnvVarGuard::set("JCODE_TEST_SESSION", "1");
    let repo = create_repo_fixture();

    let mut parent = session::Session::create(None, Some("Origin Session".to_string()));
    parent.working_dir = Some("/tmp/origin-project".to_string());
    parent.model = Some("gpt-test".to_string());
    parent.provider_key = Some("openai".to_string());
    parent.subagent_model = Some("gpt-subagent".to_string());
    parent.add_message(
        crate::message::Role::User,
        vec![crate::message::ContentBlock::Text {
            text: "hello from parent".to_string(),
            cache_control: None,
        }],
    );
    parent.compaction = Some(session::StoredCompactionState {
        summary_text: "summary".to_string(),
        openai_encrypted_content: None,
        covers_up_to_turn: 1,
        original_turn_count: 1,
        compacted_count: 1,
    });
    parent.record_replay_display_message("system", None, "remember this context");
    parent.save().expect("save parent session");

    let tool = SelfDevTool::new();
    let ctx = create_test_context(&parent.id, Some(repo.path().to_path_buf()));
    let output = tool
        .execute(
            json!({"action": "enter", "prompt": "Work on jcode itself"}),
            ctx,
        )
        .await
        .expect("selfdev enter should succeed in test mode");

    assert!(output.output.contains("Created self-dev session"));
    assert!(
        output
            .output
            .contains("Test mode skipped launching a new terminal")
    );
    assert!(
        output.output.contains("Seed prompt captured"),
        "test-mode enter should still report captured prompt"
    );

    let metadata = output.metadata.expect("metadata");
    let session_id = metadata["session_id"]
        .as_str()
        .expect("session id metadata");
    assert_eq!(metadata["inherited_context"].as_bool(), Some(true));
    let session = session::Session::load(session_id).expect("load spawned session");
    assert!(
        session.is_canary,
        "spawned session should be canary/self-dev"
    );
    assert_eq!(session.testing_build.as_deref(), Some("self-dev"));
    assert_eq!(
        session.working_dir.as_deref(),
        Some(repo.path().to_string_lossy().as_ref())
    );
    assert_eq!(session.parent_id.as_deref(), Some(parent.id.as_str()));
    assert_eq!(session.messages.len(), parent.messages.len());
    assert_eq!(session.messages[0].content_preview(), "hello from parent");
    assert_eq!(session.compaction, parent.compaction);
    assert_eq!(session.model, parent.model);
    assert_eq!(session.provider_key, parent.provider_key);
    assert_eq!(session.subagent_model, parent.subagent_model);
    assert_eq!(session.replay_events, parent.replay_events);
}

#[tokio::test]
async fn enter_falls_back_to_fresh_session_when_parent_missing() {
    let _storage_guard = crate::storage::lock_test_env();
    let _lock = lock_env();
    let temp_home = tempfile::TempDir::new().expect("temp home");
    let _home_guard = EnvVarGuard::set("JCODE_HOME", temp_home.path());
    let _test_guard = EnvVarGuard::set("JCODE_TEST_SESSION", "1");
    let repo = create_repo_fixture();

    let tool = SelfDevTool::new();
    let ctx = create_test_context("missing-parent", Some(repo.path().to_path_buf()));
    let output = tool
        .execute(json!({"action": "enter"}), ctx)
        .await
        .expect("selfdev enter should succeed without a persisted parent session");

    let metadata = output.metadata.expect("metadata");
    let session_id = metadata["session_id"]
        .as_str()
        .expect("session id metadata");
    assert_eq!(metadata["inherited_context"].as_bool(), Some(false));

    let session = session::Session::load(session_id).expect("load spawned session");
    assert!(session.messages.is_empty());
    assert!(session.parent_id.is_none());
    assert_eq!(
        session.working_dir.as_deref(),
        Some(repo.path().to_string_lossy().as_ref())
    );
}

#[tokio::test]
async fn reload_requires_selfdev_session() {
    let _storage_guard = crate::storage::lock_test_env();
    let _lock = lock_env();
    let temp_home = tempfile::TempDir::new().expect("temp home");
    let _home_guard = EnvVarGuard::set("JCODE_HOME", temp_home.path());

    let mut session = session::Session::create(None, Some("Normal Session".to_string()));
    session.save().expect("save session");

    let tool = SelfDevTool::new();
    let ctx = create_test_context(&session.id, session.working_dir.clone().map(Into::into));
    let output = tool
        .execute(json!({"action": "reload"}), ctx)
        .await
        .expect("reload should return guidance instead of failing");

    assert!(
        output
            .output
            .contains("only available inside a self-dev session")
    );
    assert!(output.output.contains("selfdev enter"));
}

#[tokio::test]
async fn build_requires_reason() {
    let _storage_guard = crate::storage::lock_test_env();
    let _lock = lock_env();
    let temp_home = tempfile::TempDir::new().expect("temp home");
    let _home_guard = EnvVarGuard::set("JCODE_HOME", temp_home.path());
    let _test_guard = EnvVarGuard::set("JCODE_TEST_SESSION", "1");
    let repo = create_repo_fixture();

    let tool = SelfDevTool::new();
    let ctx = create_test_context("build-session", Some(repo.path().to_path_buf()));
    let err = tool
        .execute(json!({"action": "build"}), ctx)
        .await
        .expect_err("build without reason should fail");

    assert!(err.to_string().contains("requires a non-empty `reason`"));
}

#[tokio::test]
async fn build_queues_background_tasks_and_reports_queue_status() {
    let _storage_guard = crate::storage::lock_test_env();
    let _lock = lock_env();
    let temp_home = tempfile::TempDir::new().expect("temp home");
    let _home_guard = EnvVarGuard::set("JCODE_HOME", temp_home.path());
    let _test_guard = EnvVarGuard::set("JCODE_TEST_SESSION", "1");
    let repo = create_repo_fixture();

    let mut session_one = session::Session::create(None, Some("First build session".to_string()));
    session_one.short_name = Some("alpha".to_string());
    session_one.save().expect("save session one");

    let mut session_two = session::Session::create(None, Some("Second build session".to_string()));
    session_two.short_name = Some("beta".to_string());
    session_two.save().expect("save session two");

    let tool = SelfDevTool::new();
    let first = tool
        .execute(
            json!({"action": "build", "reason": "first reason"}),
            create_test_context(&session_one.id, Some(repo.path().to_path_buf())),
        )
        .await
        .expect("first build should queue");
    let second = tool
        .execute(
            json!({"action": "build", "reason": "second reason"}),
            create_test_context(&session_two.id, Some(repo.path().to_path_buf())),
        )
        .await
        .expect("second build should queue");

    let first_meta = first.metadata.expect("first metadata");
    let second_meta = second.metadata.expect("second metadata");
    let first_task_id = first_meta["task_id"].as_str().expect("first task id");
    let second_task_id = second_meta["task_id"].as_str().expect("second task id");

    assert_eq!(first_meta["queue_position"].as_u64(), Some(1));
    assert_eq!(second_meta["deduped"].as_bool(), Some(true));
    assert!(
        second
            .output
            .contains("attached instead of spawning a duplicate build")
    );

    let status_output = selfdev_status_output().expect("status output");
    assert!(status_output.output.contains("## Build Queue"));
    assert!(status_output.output.contains("first reason"));
    assert!(status_output.output.contains("Attached watchers: 1"));
    assert!(
        status_output
            .output
            .contains("Target version: `test-build`")
    );

    let first_status = wait_for_task_completion(first_task_id).await;
    let second_status = wait_for_task_completion(second_task_id).await;
    assert_eq!(first_status.status, BackgroundTaskStatus::Completed);
    assert_eq!(second_status.status, BackgroundTaskStatus::Completed);

    let request_one =
        BuildRequest::load(first_meta["request_id"].as_str().expect("first request id"))
            .expect("load request one")
            .expect("request one exists");
    let request_two = BuildRequest::load(
        second_meta["request_id"]
            .as_str()
            .expect("second request id"),
    )
    .expect("load request two")
    .expect("request two exists");
    assert_eq!(request_one.state, BuildRequestState::Completed);
    assert_eq!(request_two.state, BuildRequestState::Completed);
}

#[tokio::test]
async fn build_dedupes_identical_reason_and_version_with_attached_watcher() {
    let _storage_guard = crate::storage::lock_test_env();
    let _lock = lock_env();
    let temp_home = tempfile::TempDir::new().expect("temp home");
    let _home_guard = EnvVarGuard::set("JCODE_HOME", temp_home.path());
    let _test_guard = EnvVarGuard::set("JCODE_TEST_SESSION", "1");
    let repo = create_repo_fixture();

    let mut session_one = session::Session::create(None, Some("Build A".to_string()));
    session_one.short_name = Some("alpha".to_string());
    session_one.save().expect("save session one");

    let mut session_two = session::Session::create(None, Some("Build B".to_string()));
    session_two.short_name = Some("beta".to_string());
    session_two.save().expect("save session two");

    let tool = SelfDevTool::new();
    let first = tool
        .execute(
            json!({"action": "build", "reason": "same reason"}),
            create_test_context(&session_one.id, Some(repo.path().to_path_buf())),
        )
        .await
        .expect("first build should queue");
    let second = tool
        .execute(
            json!({"action": "build", "reason": "same reason"}),
            create_test_context(&session_two.id, Some(repo.path().to_path_buf())),
        )
        .await
        .expect("second build should attach");

    let first_meta = first.metadata.expect("first metadata");
    let second_meta = second.metadata.expect("second metadata");
    assert_eq!(second_meta["deduped"].as_bool(), Some(true));
    assert_eq!(
        second_meta["duplicate_of"]["request_id"].as_str(),
        first_meta["request_id"].as_str()
    );

    let status_output = selfdev_status_output().expect("status output");
    assert!(status_output.output.contains("Attached watchers: 1"));
    assert!(status_output.output.contains("alpha"));
    assert!(status_output.output.contains("beta"));

    let first_status = wait_for_task_completion(first_meta["task_id"].as_str().unwrap()).await;
    let second_status = wait_for_task_completion(second_meta["task_id"].as_str().unwrap()).await;
    assert_eq!(first_status.status, BackgroundTaskStatus::Completed);
    assert_eq!(second_status.status, BackgroundTaskStatus::Completed);

    let producer_request = BuildRequest::load(first_meta["request_id"].as_str().unwrap())
        .expect("load producer request")
        .expect("producer request exists");
    assert!(producer_request.validated);

    let watcher_request = BuildRequest::load(second_meta["request_id"].as_str().unwrap())
        .expect("load watcher request")
        .expect("watcher request exists");
    assert_eq!(watcher_request.state, BuildRequestState::Completed);
    assert_eq!(
        watcher_request.attached_to_request_id.as_deref(),
        first_meta["request_id"].as_str()
    );
    assert!(
        watcher_request.validated,
        "attached watcher should inherit validated producer metadata for reload discovery"
    );
    assert_eq!(watcher_request.built_source, producer_request.built_source);
    assert_eq!(
        watcher_request.published_version,
        producer_request.published_version
    );
    assert_eq!(
        reload::latest_completed_build_repo_dir_for_session(&session_two.id).as_deref(),
        Some(repo.path())
    );
}

#[tokio::test]
async fn cancel_build_marks_request_cancelled_and_removes_it_from_queue() {
    let _storage_guard = crate::storage::lock_test_env();
    let _lock = lock_env();
    let temp_home = tempfile::TempDir::new().expect("temp home");
    let _home_guard = EnvVarGuard::set("JCODE_HOME", temp_home.path());
    let _test_guard = EnvVarGuard::set("JCODE_TEST_SESSION", "1");
    let repo = create_repo_fixture();

    let mut session_one = session::Session::create(None, Some("Build A".to_string()));
    session_one.short_name = Some("alpha".to_string());
    session_one.save().expect("save session one");

    let mut session_two = session::Session::create(None, Some("Build B".to_string()));
    session_two.short_name = Some("beta".to_string());
    session_two.save().expect("save session two");

    let tool = SelfDevTool::new();
    let first = tool
        .execute(
            json!({"action": "build", "reason": "keep building"}),
            create_test_context(&session_one.id, Some(repo.path().to_path_buf())),
        )
        .await
        .expect("first build should queue");
    let second = tool
        .execute(
            json!({"action": "build", "reason": "cancel me"}),
            create_test_context(&session_two.id, Some(repo.path().to_path_buf())),
        )
        .await
        .expect("second build should queue");

    let second_meta = second.metadata.expect("second metadata");
    let cancel = tool
        .execute(
            json!({
                "action": "cancel-build",
                "request_id": second_meta["request_id"].as_str().unwrap()
            }),
            create_test_context(&session_two.id, Some(repo.path().to_path_buf())),
        )
        .await
        .expect("cancel should succeed");

    assert!(cancel.output.contains("Cancelled self-dev build request"));

    let second_status = wait_for_task_completion(second_meta["task_id"].as_str().unwrap()).await;
    assert_eq!(second_status.status, BackgroundTaskStatus::Failed);

    let cancelled_request = BuildRequest::load(second_meta["request_id"].as_str().unwrap())
        .expect("load cancelled request")
        .expect("cancelled request exists");
    assert_eq!(cancelled_request.state, BuildRequestState::Cancelled);

    let status_output = selfdev_status_output().expect("status output");
    assert!(status_output.output.contains("keep building"));
    assert!(!status_output.output.contains("cancel me"));

    let first_meta = first.metadata.expect("first metadata");
    let first_status = wait_for_task_completion(first_meta["task_id"].as_str().unwrap()).await;
    assert_eq!(first_status.status, BackgroundTaskStatus::Completed);
}

#[test]
fn status_output_prunes_stale_pending_requests() {
    let _storage_guard = crate::storage::lock_test_env();
    let _lock = lock_env();
    let temp_home = tempfile::TempDir::new().expect("temp home");
    let _home_guard = EnvVarGuard::set("JCODE_HOME", temp_home.path());

    let mut session = session::Session::create(None, Some("Stale Build".to_string()));
    session.short_name = Some("ghost".to_string());
    session.save().expect("save session");

    let stale_status_path = temp_home.path().join("missing-selfdev.status.json");
    let source = test_source_state(std::path::Path::new("/tmp/jcode"));
    let request = BuildRequest {
        request_id: "stale-request".to_string(),
        background_task_id: Some("missing-task".to_string()),
        session_id: session.id.clone(),
        session_short_name: session.short_name.clone(),
        session_title: Some("Stale Build".to_string()),
        reason: "stale reason".to_string(),
        repo_dir: "/tmp/jcode".to_string(),
        repo_scope: source.repo_scope.clone(),
        worktree_scope: source.worktree_scope.clone(),
        command: "scripts/dev_cargo.sh build --profile selfdev -p jcode --bin jcode".to_string(),
        requested_at: Utc::now().to_rfc3339(),
        started_at: Some(Utc::now().to_rfc3339()),
        completed_at: None,
        state: BuildRequestState::Building,
        version: Some("stale-build".to_string()),
        dedupe_key: Some("stale-dedupe".to_string()),
        requested_source: Some(source),
        built_source: None,
        published_version: None,
        last_progress: Some("building".to_string()),
        validated: false,
        error: None,
        output_file: None,
        status_file: Some(stale_status_path.display().to_string()),
        attached_to_request_id: None,
    };
    request.save().expect("save stale request");

    let status_output = selfdev_status_output().expect("status output");
    assert!(
        !status_output.output.contains("stale reason"),
        "stale request should be pruned from queue output"
    );

    let request = BuildRequest::load("stale-request")
        .expect("load stale request")
        .expect("stale request exists");
    assert_eq!(request.state, BuildRequestState::Failed);
    assert!(
        request
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("pruning stale self-dev build request"),
        "stale request should record why it was pruned"
    );
}

#[tokio::test]
async fn build_ignores_stale_pending_requests_when_computing_queue_position() {
    let _storage_guard = crate::storage::lock_test_env();
    let _lock = lock_env();
    let temp_home = tempfile::TempDir::new().expect("temp home");
    let _home_guard = EnvVarGuard::set("JCODE_HOME", temp_home.path());
    let _test_guard = EnvVarGuard::set("JCODE_TEST_SESSION", "1");
    let repo = create_repo_fixture();

    let mut stale_session = session::Session::create(None, Some("Stale Build".to_string()));
    stale_session.short_name = Some("ghost".to_string());
    stale_session.save().expect("save stale session");

    let stale_status_path = temp_home.path().join("stale-running.status.json");
    storage::write_json(
        &stale_status_path,
        &background::TaskStatusFile {
            task_id: "stale-task".to_string(),
            tool_name: "selfdev-build".to_string(),
            display_name: Some("selfdev build".to_string()),
            session_id: stale_session.id.clone(),
            status: BackgroundTaskStatus::Running,
            exit_code: None,
            error: None,
            started_at: Utc::now().to_rfc3339(),
            completed_at: None,
            duration_secs: None,
            pid: None,
            detached: false,
            notify: true,
            wake: true,
            progress: None,
            event_history: Vec::new(),
        },
    )
    .expect("write stale status file");

    let source = test_source_state(repo.path());
    let stale_request = BuildRequest {
        request_id: "stale-queued-request".to_string(),
        background_task_id: Some("stale-task".to_string()),
        session_id: stale_session.id.clone(),
        session_short_name: stale_session.short_name.clone(),
        session_title: Some("Stale Build".to_string()),
        reason: "stale blocker".to_string(),
        repo_dir: repo.path().display().to_string(),
        repo_scope: source.repo_scope.clone(),
        worktree_scope: source.worktree_scope.clone(),
        command: "scripts/dev_cargo.sh build --profile selfdev -p jcode --bin jcode".to_string(),
        requested_at: Utc::now().to_rfc3339(),
        started_at: Some(Utc::now().to_rfc3339()),
        completed_at: None,
        state: BuildRequestState::Queued,
        version: Some("test-build".to_string()),
        dedupe_key: Some("stale-dedupe".to_string()),
        requested_source: Some(source),
        built_source: None,
        published_version: None,
        last_progress: Some("queued".to_string()),
        validated: false,
        error: None,
        output_file: None,
        status_file: Some(stale_status_path.display().to_string()),
        attached_to_request_id: None,
    };
    stale_request.save().expect("save stale queued request");

    let mut live_session = session::Session::create(None, Some("Live Build".to_string()));
    live_session.short_name = Some("alpha".to_string());
    live_session.save().expect("save live session");

    let tool = SelfDevTool::new();
    let output = tool
        .execute(
            json!({"action": "build", "reason": "fresh build"}),
            create_test_context(&live_session.id, Some(repo.path().to_path_buf())),
        )
        .await
        .expect("build should queue");

    let metadata = output.metadata.expect("build metadata");
    assert_eq!(metadata["queue_position"].as_u64(), Some(1));
    assert!(
        !output.output.contains("Currently blocked by"),
        "stale queued requests should not block new builds"
    );

    let stale_request = BuildRequest::load("stale-queued-request")
        .expect("load stale queued request")
        .expect("stale queued request exists");
    assert_eq!(stale_request.state, BuildRequestState::Failed);

    let task_id = metadata["task_id"].as_str().expect("task id");
    let status = wait_for_task_completion(task_id).await;
    assert_eq!(status.status, BackgroundTaskStatus::Completed);
}

#[test]
fn reconcile_pending_state_maps_superseded_background_status() {
    let _storage_guard = crate::storage::lock_test_env();
    let _lock = lock_env();
    let temp_home = tempfile::TempDir::new().expect("temp home");
    let _home_guard = EnvVarGuard::set("JCODE_HOME", temp_home.path());

    let mut session = session::Session::create(None, Some("Superseded Build".to_string()));
    session.short_name = Some("alpha".to_string());
    session.save().expect("save session");

    let status_path = temp_home.path().join("superseded.status.json");
    storage::write_json(
        &status_path,
        &background::TaskStatusFile {
            task_id: "superseded-task".to_string(),
            tool_name: "selfdev-build".to_string(),
            display_name: Some("selfdev build".to_string()),
            session_id: session.id.clone(),
            status: BackgroundTaskStatus::Superseded,
            exit_code: Some(0),
            error: Some("Build completed, but source changed before activation".to_string()),
            started_at: Utc::now().to_rfc3339(),
            completed_at: Some(Utc::now().to_rfc3339()),
            duration_secs: Some(1.0),
            pid: None,
            detached: false,
            notify: true,
            wake: true,
            progress: None,
            event_history: Vec::new(),
        },
    )
    .expect("write superseded status file");

    let source = test_source_state(std::path::Path::new("/tmp/jcode"));
    let request = BuildRequest {
        request_id: "superseded-request".to_string(),
        background_task_id: Some("superseded-task".to_string()),
        session_id: session.id.clone(),
        session_short_name: session.short_name.clone(),
        session_title: Some("Superseded Build".to_string()),
        reason: "superseded reason".to_string(),
        repo_dir: "/tmp/jcode".to_string(),
        repo_scope: source.repo_scope.clone(),
        worktree_scope: source.worktree_scope.clone(),
        command: "scripts/dev_cargo.sh build --profile selfdev -p jcode --bin jcode".to_string(),
        requested_at: Utc::now().to_rfc3339(),
        started_at: Some(Utc::now().to_rfc3339()),
        completed_at: None,
        state: BuildRequestState::Building,
        version: Some("superseded-build".to_string()),
        dedupe_key: Some("superseded-dedupe".to_string()),
        requested_source: Some(source),
        built_source: None,
        published_version: None,
        last_progress: Some("building".to_string()),
        validated: false,
        error: None,
        output_file: None,
        status_file: Some(status_path.display().to_string()),
        attached_to_request_id: None,
    };
    request.save().expect("save superseded request");

    let mut request = BuildRequest::load("superseded-request")
        .expect("load superseded request")
        .expect("request exists");
    assert!(
        !request
            .reconcile_pending_state()
            .expect("reconcile superseded request")
    );
    assert_eq!(request.state, BuildRequestState::Superseded);
    assert!(
        request
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("source changed before activation")
    );
}
