//! Tests for the session `working_dir` isolation guarantees (P2.4/P2.5):
//! project MCP registration and the command/path tools must resolve against
//! the session working directory, never the daemon process cwd.

use super::*;

/// P2.4 / issue #420: the unpooled branch of `register_mcp_tools_for_dir` must
/// honor `working_dir` just like the pooled branch. It used to call
/// `McpManager::new()`, which resolves project-local config against the process
/// cwd -- one arbitrary project in a multi-project daemon -- so a session with
/// no pool silently read whichever project started the daemon.
#[tokio::test]
async fn unpooled_registration_resolves_project_mcp_config_against_working_dir() {
    let _env_lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().expect("create isolated JCODE_HOME");
    let _home_guard = TestHomeGuard::new(home.path());
    let working_dir = tempfile::tempdir().expect("create isolated MCP working directory");

    std::fs::write(
        working_dir.path().join(".mcp.json"),
        r#"{"mcpServers": {"project_only": {"command": "/bin/sh", "args": ["-c", "echo hi"]}}}"#,
    )
    .expect("write project-local MCP config");

    let registry = Registry::empty();
    // No shared pool: this is the branch that used to ignore `working_dir`.
    registry
        .register_mcp_tools_for_dir(None, None, None, Some(working_dir.path().to_path_buf()))
        .await;

    let output = registry
        .execute(
            "mcp",
            serde_json::json!({"action": "list"}),
            mcp_test_context(working_dir.path()),
        )
        .await
        .expect("mcp list should succeed");

    assert!(
        output.output.contains("project_only"),
        "the unpooled manager ignored working_dir and did not load .mcp.json from it; output: {}",
        output.output
    );
}

/// Positive control for the sibling above: the same session, same file layout,
/// with a pool, must see the same project-local server. Guards against the test
/// passing because config loading or list rendering changed, rather than
/// because the unpooled branch stopped skipping `working_dir`.
#[tokio::test]
async fn pooled_registration_resolves_project_mcp_config_against_working_dir() {
    let _env_lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().expect("create isolated JCODE_HOME");
    let _home_guard = TestHomeGuard::new(home.path());
    let working_dir = tempfile::tempdir().expect("create isolated MCP working directory");

    std::fs::write(
        working_dir.path().join(".mcp.json"),
        r#"{"mcpServers": {"project_only": {"command": "/bin/sh", "args": ["-c", "echo hi"]}}}"#,
    )
    .expect("write project-local MCP config");

    let registry = Registry::empty();
    let pool = Arc::new(crate::mcp::SharedMcpPool::new(
        crate::mcp::McpConfig::default(),
    ));
    registry
        .register_mcp_tools_for_dir(
            None,
            Some(pool),
            Some("mcp-pooled-control".to_string()),
            Some(working_dir.path().to_path_buf()),
        )
        .await;

    let output = registry
        .execute(
            "mcp",
            serde_json::json!({"action": "list"}),
            mcp_test_context(working_dir.path()),
        )
        .await
        .expect("mcp list should succeed");

    assert!(
        output.output.contains("project_only"),
        "pooled manager did not load .mcp.json from working_dir; output: {}",
        output.output
    );
}

#[tokio::test]
async fn mcp_list_remains_available_while_background_connect_is_handshaking() {
    let _env_lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().expect("create isolated JCODE_HOME");
    let _home_guard = TestHomeGuard::new(home.path());
    let working_dir = tempfile::tempdir().expect("create isolated MCP working directory");

    std::fs::write(
        home.path().join("mcp.json"),
        r#"{
            "mcpServers": {
                "slow": {
                    "command": "/bin/sh",
                    "args": ["-c", "sleep 2"],
                    "timeout_secs": 86400
                }
            }
        }"#,
    )
    .expect("write slow MCP config");

    let registry = Registry::empty();
    registry
        .register_mcp_tools_for_dir(None, None, None, Some(working_dir.path().to_path_buf()))
        .await;

    // Let the background connection task acquire its read guard and enter the
    // slow initialize handshake before asking the management tool to list.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let output = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        registry.execute(
            "mcp",
            serde_json::json!({"action": "list"}),
            mcp_test_context(working_dir.path()),
        ),
    )
    .await
    .expect("mcp list must not wait for a slow background handshake")
    .expect("mcp list should succeed");

    assert!(
        output.output.contains("slow"),
        "unexpected output: {}",
        output.output
    );
}

/// A session with no project has no directory to run a command in. Every spawn
/// path in `bash.rs` only calls `current_dir` when `ctx.working_dir` is `Some`,
/// so without a guard the child would inherit the daemon's cwd, i.e. whichever
/// repository happened to start the daemon (P2.5).
///
/// All three dispatch shapes are checked, because the guard has to sit in front
/// of the foreground, detached, and background spawns rather than in any one of
/// them. This lives here rather than in `bash_tests.rs` because that module is
/// `not(windows)`, and the guard it covers is cross-platform.
///
/// The assertion is that no shape runs at all, so the check does not depend on
/// parsing a shell's directory output, which differs per platform and shell.
#[tokio::test]
async fn bash_refuses_to_run_without_a_working_dir_instead_of_using_the_daemon_cwd() {
    let _lock = crate::storage::lock_test_env();
    let prev_cwd = std::env::current_dir().expect("cwd");
    let daemon_repo = tempfile::TempDir::new().expect("daemon repo dir");
    std::env::set_current_dir(daemon_repo.path()).expect("set cwd");

    // `cd` prints the working directory on every supported platform's shell, and
    // this build's `build_shell_command` supplies one.
    let print_cwd = if cfg!(windows) { "cd" } else { "pwd" };

    let tool = crate::tool::bash::BashTool::new();
    let mut ctx = ToolContext {
        session_id: "p25c-session".to_string(),
        message_id: "p25c-msg".to_string(),
        tool_call_id: "p25c-call".to_string(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    };

    for (label, extra) in [
        ("foreground", serde_json::json!({})),
        ("detached", serde_json::json!({"run_in_background": true})),
        ("background", serde_json::json!({"timeout_ms": 50})),
    ] {
        let mut input = serde_json::json!({"command": print_cwd});
        if let (serde_json::Value::Object(base), serde_json::Value::Object(more)) =
            (&mut input, extra)
        {
            base.extend(more);
        }
        let error = tool
            .execute(input, ctx.clone())
            .await
            .expect_err(&format!("{label} must refuse to run without a working dir"));
        let message = error.to_string();
        assert!(
            message.contains("working directory"),
            "{label}: the error must be actionable, got: {message}"
        );
    }

    // Positive control: the same command with a working dir runs, and it runs in
    // that directory rather than the daemon's.
    let target = tempfile::TempDir::new().expect("target dir");
    ctx.working_dir = Some(target.path().to_path_buf());
    tool.execute(serde_json::json!({"command": print_cwd}), ctx)
        .await
        .expect("positive control: a session with a working dir still runs commands");

    std::env::set_current_dir(prev_cwd).expect("restore cwd");
}

/// A relative path with no session working directory must not be resolved
/// against the daemon's own directory.
///
/// This drives the real tools rather than `resolve_path` itself: a test of the
/// helper would pass even if a call site stopped propagating its error, which is
/// the half that actually regresses. The process cwd is pointed at a directory
/// holding a file with the exact name the tools are asked to write, so a silent
/// fallback would succeed and overwrite it, and the assertions below would have
/// to notice that.
#[tokio::test]
async fn relative_paths_are_refused_without_a_working_dir_instead_of_using_the_daemon_cwd() {
    let _lock = crate::storage::lock_test_env();
    let prev_cwd = std::env::current_dir().expect("cwd");
    let daemon_repo = tempfile::TempDir::new().expect("daemon repo dir");
    std::fs::write(
        daemon_repo.path().join("victim.txt"),
        "belongsto the daemon project",
    )
    .expect("seed daemon-side file");
    std::env::set_current_dir(daemon_repo.path()).expect("set cwd");

    let target = tempfile::TempDir::new().expect("target dir");
    let mut ctx = ToolContext {
        session_id: "p25d-session".to_string(),
        message_id: "p25d-msg".to_string(),
        tool_call_id: "p25d-call".to_string(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    };

    // --- write: the smallest tool that both resolves and mutates. ---
    let error = crate::tool::write::WriteTool::new()
        .execute(
            serde_json::json!({"file_path": "victim.txt", "content": "overwritten"}),
            ctx.clone(),
        )
        .await
        .expect_err("write must refuse a relative path with no session working dir");
    let message = error.to_string();
    assert!(
        message.contains("working directory") && message.contains("victim.txt"),
        "the write error must name both the problem and the path, got: {message}"
    );
    assert_eq!(
        std::fs::read_to_string(daemon_repo.path().join("victim.txt")).expect("read"),
        "belongsto the daemon project",
        "the daemon-side file must be untouched: the path resolved against the daemon cwd"
    );

    // --- read: the same resolution, on the read side. ---
    let error = crate::tool::read::ReadTool::new()
        .execute(serde_json::json!({"file_path": "victim.txt"}), ctx.clone())
        .await
        .expect_err("read must refuse a relative path with no session working dir");
    assert!(
        error.to_string().contains("working directory"),
        "got: {error}"
    );

    // --- apply_patch: resolves once up front, so a bad path in any hunk must
    // stop the whole patch before it writes anything. ---
    let error = crate::tool::apply_patch::ApplyPatchTool::new()
        .execute(
            serde_json::json!({"patch_text": "*** Begin Patch\n*** Add File: victim.txt\n+overwritten\n*** End Patch"}),
            ctx.clone(),
        )
        .await
        .expect_err("apply_patch must refuse a relative path with no session working dir");
    assert!(
        error.to_string().contains("working directory"),
        "got: {error}"
    );
    assert_eq!(
        std::fs::read_to_string(daemon_repo.path().join("victim.txt")).expect("read"),
        "belongsto the daemon project",
        "apply_patch must not have written through the daemon cwd"
    );

    // --- Positive control. With a working dir set, the same relative write
    // succeeds and lands in that directory, proving the refusals above came
    // from the missing working dir and not from the path being unusable. ---
    ctx.working_dir = Some(target.path().to_path_buf());
    crate::tool::write::WriteTool::new()
        .execute(
            serde_json::json!({"file_path": "victim.txt", "content": "written"}),
            ctx,
        )
        .await
        .expect("positive control: a session with a working dir can still write");
    assert_eq!(
        std::fs::read_to_string(target.path().join("victim.txt")).expect("read"),
        "written",
        "the positive control must land in the session's directory"
    );

    std::env::set_current_dir(prev_cwd).expect("restore cwd");
}

/// `apply_patch` must not partially apply: a patch whose *second* hunk has an
/// unresolvable path must leave the first hunk's file unwritten.
///
/// The single up-front resolution pass is what makes this true. Resolving per
/// hunk instead would write the first file and only then fail, leaving the
/// project in a state neither the agent nor the user asked for.
#[tokio::test]
async fn apply_patch_writes_nothing_when_any_path_in_the_patch_is_unresolvable() {
    let _lock = crate::storage::lock_test_env();
    let prev_cwd = std::env::current_dir().expect("cwd");
    let daemon_repo = tempfile::TempDir::new().expect("daemon repo dir");
    std::env::set_current_dir(daemon_repo.path()).expect("set cwd");

    let target = tempfile::TempDir::new().expect("target dir");
    let ctx = ToolContext {
        session_id: "p25d-atomic".to_string(),
        message_id: "p25d-msg".to_string(),
        tool_call_id: "p25d-call".to_string(),
        // No working dir: both hunks are relative, and must both be refused.
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    };

    crate::tool::apply_patch::ApplyPatchTool::new()
        .execute(
            serde_json::json!({"patch_text": "*** Begin Patch\n*** Add File: first.txt\n+one\n*** Add File: second.txt\n+two\n*** End Patch"}),
            ctx.clone(),
        )
        .await
        .expect_err("a patch naming unresolvable paths must be refused");

    for name in ["first.txt", "second.txt"] {
        assert!(
            !daemon_repo.path().join(name).exists(),
            "{name} must not exist in the daemon directory"
        );
        assert!(
            !target.path().join(name).exists(),
            "{name} must not exist anywhere: the patch was never partially applied"
        );
    }

    // Positive control: the same two-hunk patch with a working dir writes both.
    let mut ok_ctx = ctx;
    ok_ctx.working_dir = Some(target.path().to_path_buf());
    crate::tool::apply_patch::ApplyPatchTool::new()
        .execute(
            serde_json::json!({"patch_text": "*** Begin Patch\n*** Add File: first.txt\n+one\n*** Add File: second.txt\n+two\n*** End Patch"}),
            ok_ctx,
        )
        .await
        .expect("positive control: a session with a working dir can still patch");
    assert!(
        target.path().join("first.txt").exists(),
        "first hunk applied"
    );
    assert!(
        target.path().join("second.txt").exists(),
        "second hunk applied"
    );

    std::env::set_current_dir(prev_cwd).expect("restore cwd");
}
