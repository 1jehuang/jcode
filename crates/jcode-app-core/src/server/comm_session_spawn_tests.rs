//! Tests for swarm spawn effort resolution and visible-spawn working-dir fallback.

use super::*;

#[test]
fn swarm_spawn_effort_prefers_explicit_then_config_pin_then_inherit() {
    use super::super::resolve_swarm_spawn_effort;

    // Explicit spawn argument wins over the config pin (#1165).
    assert_eq!(
        resolve_swarm_spawn_effort(Some("low"), Some("medium")),
        Some("low".to_string())
    );
    // A missing or blank spawn argument falls back to `agents.swarm_effort`.
    assert_eq!(
        resolve_swarm_spawn_effort(None, Some("medium")),
        Some("medium".to_string())
    );
    assert_eq!(
        resolve_swarm_spawn_effort(Some("  "), Some(" medium ")),
        Some("medium".to_string())
    );
    // With neither, the worker inherits the provider-wide effort.
    assert_eq!(resolve_swarm_spawn_effort(None, None), None);
    assert_eq!(resolve_swarm_spawn_effort(Some(""), Some("")), None);
}

/// A visible spawn whose spawner has no project must fail rather than open the
/// new session in the daemon's cwd, which belongs to whichever repository
/// happened to start this process (P2.5).
///
/// This drives `prepare_visible_spawn_session`, the real seam, rather than the
/// helper: per the P1.3 lesson a helper test cannot prove the call site consults
/// it. The process cwd is pointed at a real repo so the pre-fix fallback would
/// resolve to it and be observable, and the launch closure records what it was
/// handed so the assertion is about the actual directory, not just the error.
#[test]
fn visible_spawn_without_a_working_dir_errors_instead_of_using_the_daemon_cwd() {
    let _lock = crate::storage::lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let prev_cwd = std::env::current_dir().expect("cwd");
    let home = tempfile::TempDir::new().expect("temp home");
    crate::env::set_var("JCODE_HOME", home.path());

    let repo = tempfile::TempDir::new().expect("repo dir");
    std::env::set_current_dir(repo.path()).expect("set cwd");

    let launched_dir = std::cell::RefCell::new(None::<std::path::PathBuf>);
    let result = {
        let launched_dir = &launched_dir;
        prepare_visible_spawn_session(
            None,
            None,
            None,
            None,
            None,
            false,
            None,
            move |_session_id, cwd, _selfdev, _provider_key| {
                *launched_dir.borrow_mut() = Some(cwd.to_path_buf());
                Ok(true)
            },
        )
    };

    let error =
        result.expect_err("a spawn with no working dir must fail instead of using the daemon cwd");
    let message = error.to_string();
    assert!(
        message.contains("working directory"),
        "the error must be actionable, got: {message}"
    );
    assert!(
        launched_dir.borrow().is_none(),
        "no session window may be launched without a working dir, saw cwd: {:?}",
        launched_dir.borrow()
    );

    // Positive control: with an explicit directory the spawn proceeds and the
    // window is launched in *that* directory, not the daemon's.
    let target = tempfile::TempDir::new().expect("target dir");
    let launched_dir = std::cell::RefCell::new(None::<std::path::PathBuf>);
    let (session_id, launched) = {
        let launched_dir = &launched_dir;
        prepare_visible_spawn_session(
            Some(&target.path().to_string_lossy()),
            None,
            None,
            None,
            None,
            false,
            None,
            move |_session_id, cwd, _selfdev, _provider_key| {
                *launched_dir.borrow_mut() = Some(cwd.to_path_buf());
                Ok(true)
            },
        )
        .expect("positive control: an explicit working dir still spawns")
    };
    assert!(
        launched,
        "positive control: the session window should launch"
    );
    assert!(
        !session_id.is_empty(),
        "positive control: a session id is returned"
    );
    assert_eq!(
        launched_dir.borrow().as_deref(),
        Some(target.path()),
        "positive control: the window must open in the requested directory"
    );

    super::super::cleanup_prepared_visible_spawn_session(&session_id);

    std::env::set_current_dir(prev_cwd).expect("restore cwd");
    if let Some(value) = prev_home {
        crate::env::set_var("JCODE_HOME", value);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}
