//! Regression tests for the jade-relay launch working dir (plan item P5.3).
//!
//! One `jcode` daemon serves sessions for many projects and its process cwd is
//! whichever project happened to start it. `create_launch_session` used to
//! answer a missing `working_dir` with that cwd, so a relay event that named no
//! directory silently produced a session rooted in a *different* project. These
//! tests live in their own file because `jade_relay.rs` is already over the
//! oversized-file budget and the fix must not grow it further.
//!
//! Each test has a positive control, so neither can pass merely because every
//! launch now fails.

#![cfg(test)]

use super::jade_relay::{LaunchRequest, create_launch_session};

/// A launch with no directory of its own must fail, not inherit the daemon's.
///
/// This asserts the refusal, because the refusal is the behaviour that keeps
/// the projects apart. The control below covers the other half: a launch that
/// *does* name a directory still gets a session there.
#[test]
fn launch_without_a_working_dir_is_refused_instead_of_using_the_daemon_cwd() {
    let _lock = crate::storage::lock_test_env();
    let daemon_repo = tempfile::TempDir::new().expect("daemon repo dir");
    let prev_cwd = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(daemon_repo.path()).expect("set cwd");

    let request = LaunchRequest {
        text: "launch from the device".to_string(),
        working_dir: None,
        model: None,
        provider_key: None,
        selfdev: false,
    };
    let error = create_launch_session(&request)
        .expect_err("a launch with no working_dir must not fall back to the daemon cwd");
    let message = error.to_string();
    assert!(
        message.contains("working_dir") && message.contains("jade_relay_launch_working_dir"),
        "the error must name both fixes, got: {message}"
    );

    std::env::set_current_dir(prev_cwd).expect("restore cwd");
}

/// Positive control for the test above: an explicit directory is honoured.
#[test]
fn launch_with_an_explicit_working_dir_uses_it() {
    let _lock = crate::storage::lock_test_env();
    let project = tempfile::TempDir::new().expect("project dir");
    let request = LaunchRequest {
        text: "launch from the device".to_string(),
        working_dir: Some(project.path().display().to_string()),
        model: None,
        provider_key: None,
        selfdev: false,
    };
    let (_session_id, cwd) = create_launch_session(&request).expect("explicit dir launches");
    assert_eq!(
        cwd,
        project.path(),
        "the session must be rooted in the requested directory"
    );
}
