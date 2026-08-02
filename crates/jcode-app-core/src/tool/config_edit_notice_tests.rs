use super::*;

/// Point the process at a temp jcode home and return it with a restore guard.
fn temp_jcode_home() -> (tempfile::TempDir, Option<std::ffi::OsString>) {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let prev = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", dir.path());
    crate::config::Config::invalidate_cache();
    (dir, prev)
}

fn restore_jcode_home(prev: Option<std::ffi::OsString>) {
    if let Some(prev) = prev {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
    crate::config::Config::invalidate_cache();
}

#[test]
fn writing_the_active_config_reports_what_changed() {
    let _guard = crate::storage::lock_test_env();
    let (_dir, prev) = temp_jcode_home();

    let path = crate::config::Config::path().expect("config path");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    std::fs::write(&path, "[keybindings]\nscroll_up = \"ctrl+y\"\n").expect("write");

    let notice = config_edit_notice(
        &path,
        "[keybindings]\nscroll_up = \"ctrl+k\"\n",
        "[keybindings]\nscroll_up = \"ctrl+y\"\n",
    )
    .expect("an active-config edit should be reported");

    assert!(notice.contains("keybindings.scroll_up"), "{notice}");
    assert!(notice.contains("live now"), "{notice}");

    restore_jcode_home(prev);
}

#[test]
fn writing_an_unrelated_file_reports_nothing() {
    let _guard = crate::storage::lock_test_env();
    let (dir, prev) = temp_jcode_home();

    let other = dir.path().join("notes.toml");
    std::fs::write(&other, "[display]\ncentered = true\n").expect("write");

    assert!(
        config_edit_notice(&other, "", "[display]\ncentered = true\n").is_none(),
        "only the active config file should get a change report"
    );

    restore_jcode_home(prev);
}

#[test]
fn comment_only_config_edit_reports_nothing() {
    let _guard = crate::storage::lock_test_env();
    let (_dir, prev) = temp_jcode_home();

    let path = crate::config::Config::path().expect("config path");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    let after = "# note\n[display]\ncentered = true\n";
    std::fs::write(&path, after).expect("write");

    assert!(
        config_edit_notice(&path, "[display]\ncentered = true\n", after).is_none(),
        "a comment-only edit must not claim a settings change"
    );

    restore_jcode_home(prev);
}

#[test]
fn restart_required_sections_say_so() {
    let _guard = crate::storage::lock_test_env();
    let (_dir, prev) = temp_jcode_home();

    let path = crate::config::Config::path().expect("config path");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    let after = "[gateway]\nport = 8888\n";
    std::fs::write(&path, after).expect("write");

    let notice =
        config_edit_notice(&path, "[gateway]\nport = 7777\n", after).expect("report expected");
    assert!(
        notice.contains("Restart required for: gateway.port"),
        "{notice}"
    );

    restore_jcode_home(prev);
}

#[test]
fn the_notice_leaves_the_config_cache_current() {
    let _guard = crate::storage::lock_test_env();
    let (_dir, prev) = temp_jcode_home();

    let path = crate::config::Config::path().expect("config path");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    std::fs::write(&path, "[display]\ncentered = false\n").expect("write");
    assert!(!crate::config::config().display.centered);

    // Rewrite immediately: without the notice's explicit invalidation this can
    // land inside the config cache's staleness throttle.
    let after = "[display]\ncentered = true\n";
    std::fs::write(&path, after).expect("rewrite");
    let notice =
        config_edit_notice(&path, "[display]\ncentered = false\n", after).expect("report expected");

    assert!(notice.contains("live now"), "{notice}");
    assert!(
        crate::config::config().display.centered,
        "claiming 'live now' requires the config cache to already reflect the edit"
    );

    restore_jcode_home(prev);
}

#[test]
fn a_config_write_that_breaks_toml_syntax_is_reported_loudly() {
    let _guard = crate::storage::lock_test_env();
    let (_dir, prev) = temp_jcode_home();

    let path = crate::config::Config::path().expect("config path");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    let broken = "[display\ncentered = true\n";
    std::fs::write(&path, broken).expect("write");

    let notice = config_edit_notice(&path, "[display]\ncentered = true\n", broken)
        .expect("a config file that stopped parsing must never be silent");
    assert!(notice.contains("WARNING"), "{notice}");
    assert!(notice.contains("no longer parses"), "{notice}");

    restore_jcode_home(prev);
}

/// End-to-end through the real `write` tool: the path an agent actually takes
/// when a user says "change this setting".
#[tokio::test]
async fn the_write_tool_reports_config_changes_end_to_end() {
    use crate::tool::{Tool, ToolContext};

    let _guard = crate::storage::lock_test_env();
    let (dir, prev) = temp_jcode_home();

    let path = crate::config::Config::path().expect("config path");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    std::fs::write(
        &path,
        "[display]\ncentered = false\n\n[gateway]\nport = 7777\n",
    )
    .expect("seed config");
    assert!(!crate::config::config().display.centered);

    let ctx = ToolContext {
        session_id: "test".to_string(),
        message_id: "test".to_string(),
        tool_call_id: "test".to_string(),
        working_dir: Some(dir.path().to_path_buf()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: crate::tool::ToolExecutionMode::Direct,
    };

    let output = crate::tool::write::WriteTool
        .execute(
            serde_json::json!({
                "file_path": path.to_string_lossy(),
                "content": "[display]\ncentered = true\n\n[gateway]\nport = 8888\n",
            }),
            ctx,
        )
        .await
        .expect("write should succeed");

    let body = output.output;
    assert!(body.contains("display.centered"), "{body}");
    assert!(body.contains("live now"), "{body}");
    assert!(
        body.contains("Restart required for: gateway.port"),
        "{body}"
    );
    assert!(
        crate::config::config().display.centered,
        "the display change should be live in-process immediately after the write"
    );

    restore_jcode_home(prev);
}

/// `apply_patch` reaches config.toml through its own write paths, so it gets
/// the same report as write/edit.
#[tokio::test]
async fn apply_patch_reports_config_changes() {
    use crate::tool::{Tool, ToolContext};

    let _guard = crate::storage::lock_test_env();
    let (dir, prev) = temp_jcode_home();

    let path = crate::config::Config::path().expect("config path");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    std::fs::write(&path, "[display]\ncentered = false\n").expect("seed config");
    assert!(!crate::config::config().display.centered);

    let ctx = ToolContext {
        session_id: "test".to_string(),
        message_id: "test".to_string(),
        tool_call_id: "test".to_string(),
        working_dir: Some(dir.path().to_path_buf()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: crate::tool::ToolExecutionMode::Direct,
    };

    let patch_text = format!(
        "*** Begin Patch\n*** Update File: {}\n@@\n-centered = false\n+centered = true\n*** End Patch\n",
        path.display()
    );
    let output = crate::tool::apply_patch::ApplyPatchTool
        .execute(serde_json::json!({ "patch_text": patch_text }), ctx)
        .await
        .expect("patch should apply");

    let body = output.output;
    assert!(body.contains("display.centered"), "{body}");
    assert!(body.contains("live now"), "{body}");
    assert!(
        crate::config::config().display.centered,
        "the patched setting should be live immediately"
    );

    restore_jcode_home(prev);
}
