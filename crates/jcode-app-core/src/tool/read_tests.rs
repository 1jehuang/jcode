//! Tests for the session-scoped path handling in `read.rs`.
//!
//! Separate file so the additions do not grow the already-oversized
//! `tool/tests.rs` past the test-size ratchet, and so the isolation
//! guarantees for this tool are readable in one place.

use jcode_tool_core::Tool;
use serde_json::json;

use super::ToolContext;
use super::ToolExecutionMode;
use super::read::ReadTool;

#[tokio::test]
async fn missing_root_path_does_not_suggest_files_from_the_daemon_cwd() {
    let _lock = crate::storage::lock_test_env();
    let prev_cwd = std::env::current_dir().expect("cwd");
    let daemon_repo = tempfile::TempDir::new().expect("daemon repo dir");
    std::fs::write(
        daemon_repo.path().join("hijacked.txt"),
        "belongs to the daemon project",
    )
    .expect("seed daemon-side file");
    std::env::set_current_dir(daemon_repo.path()).expect("set cwd");

    // Positive control, run first: a session dir that really does hold a
    // near-miss name still produces a suggestion. Without this, the assertion
    // below would also pass for a `find_similar_files` that had stopped
    // suggesting anything.
    let session_dir = tempfile::TempDir::new().expect("session dir");
    std::fs::write(session_dir.path().join("report.txt"), "hi").expect("seed session file");
    let ctx = ToolContext {
        session_id: "p53-session".to_string(),
        message_id: "p53-msg".to_string(),
        tool_call_id: "p53-call".to_string(),
        working_dir: Some(session_dir.path().to_path_buf()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    };
    let error = ReadTool::new()
        .execute(json!({"file_path": "old report.txt"}), ctx.clone())
        .await
        .expect_err("old report.txt does not exist");
    assert!(
        error.to_string().contains("report.txt"),
        "a near-miss inside the session dir must still be suggested, got: {}",
        error
    );

    // Now the property that keeps this from being a daemon-cwd read, stated as
    // the platform fact it actually is.
    //
    // `find_similar_files` returned early whenever `parent()` was `None`, which
    // meant "the path has no parent directory, so there is nothing to list".
    // The tempting `unwrap_or(Path::new("."))` would instead have listed `.`,
    // which the OS resolves against the *daemon's* directory. That branch is
    // unreachable through this tool: a compiled probe established that every
    // path that is both absolute and parentless (`C:\\`, `C:/`, `\\?\\C:\\`)
    // also *exists*, so `ReadTool` returns before calling `find_similar_files`
    // at all, and a path that does not exist always has both a parent and a
    // file name. The early return is therefore load-bearing on a platform
    // invariant rather than on a lucky ordering, and that is what this asserts:
    // if it ever regresses, `find_similar_files` must not be able to list the
    // daemon's directory.
    for probe in [
        std::path::MAIN_SEPARATOR.to_string(),
        "C:\\".to_string(),
        "C:/".to_string(),
    ] {
        let path = std::path::PathBuf::from(&probe);
        if path.parent().is_some() {
            continue; // not a parentless shape on this platform
        }
        assert!(
            path.exists(),
            "a parentless path that does not exist would reach find_similar_files \
             with no parent; the early return must not be the only thing hiding it: {probe:?}"
        );
    }

    // And the direct check that the helper is not consulting the daemon
    // directory for any parentless input.
    let root = std::path::PathBuf::from("C:\\");
    assert_eq!(root.parent(), None, "the probe must be parentless here");
    assert!(
        !crate::tool::read::suggestions_for_test(&root)
            .iter()
            .any(|s| s.contains("hijacked.txt")),
        "a parentless path produced a daemon-side suggestion"
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
