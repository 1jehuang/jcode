use super::newer_binary_available;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

fn t(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

#[test]
fn reports_update_when_candidate_is_strictly_newer() {
    let candidates = vec![(PathBuf::from("/x/stable/jcode"), Some(t(200)))];
    assert!(newer_binary_available(
        Some(t(100)),
        Some(std::path::Path::new("/x/current/jcode")),
        candidates,
    ));
}

#[test]
fn ignores_candidate_that_is_not_newer() {
    let candidates = vec![(PathBuf::from("/x/stable/jcode"), Some(t(100)))];
    assert!(!newer_binary_available(
        Some(t(100)),
        Some(std::path::Path::new("/x/current/jcode")),
        candidates,
    ));
}

#[test]
fn never_reloads_into_self_even_if_paths_were_equal() {
    // Same canonical path must never count as an update, regardless of mtime.
    let candidates = vec![(PathBuf::from("/x/current/jcode"), Some(t(999)))];
    assert!(!newer_binary_available(
        Some(t(100)),
        Some(std::path::Path::new("/x/current/jcode")),
        candidates,
    ));
}

#[test]
fn suppresses_update_when_current_mtime_unavailable() {
    // Regression for issue #277: an unreadable current mtime previously fell
    // through to a path-difference heuristic that could loop forever.
    let candidates = vec![(PathBuf::from("/x/stable/jcode"), Some(t(200)))];
    assert!(!newer_binary_available(
        None,
        Some(std::path::Path::new("/x/current/jcode")),
        candidates,
    ));
}

#[test]
fn suppresses_update_when_candidate_mtime_unavailable() {
    // The dangerous case from issue #277: candidate path differs but its
    // mtime cannot be read. Must NOT report an update.
    let candidates = vec![(PathBuf::from("/x/stable/jcode"), None)];
    assert!(!newer_binary_available(
        Some(t(100)),
        Some(std::path::Path::new("/x/current/jcode")),
        candidates,
    ));
}

#[test]
fn reports_update_if_any_candidate_is_newer() {
    let candidates = vec![
        (PathBuf::from("/x/stable/jcode"), None),
        (PathBuf::from("/x/shared/jcode"), Some(t(300))),
    ];
    assert!(newer_binary_available(
        Some(t(100)),
        Some(std::path::Path::new("/x/current/jcode")),
        candidates,
    ));
}

#[test]
fn newer_server_is_not_outdated_by_older_channel_binary() {
    // Issue #291: a newer self-dev / shared-server daemon must NOT report an
    // update just because an *older* channel binary exists. Here the running
    // server (t=300) is newer than the only candidate (stable at t=100), so
    // there is no update. Previously a channel-version *mismatch* short-circuit
    // reported `true` here and told the newer server to downgrade itself.
    let candidates = vec![(PathBuf::from("/x/stable/jcode"), Some(t(100)))];
    assert!(!newer_binary_available(
        Some(t(300)),
        Some(std::path::Path::new("/x/builds/versions/dev/jcode")),
        candidates,
    ));
}

#[test]
fn equal_mtime_channel_binary_is_not_an_update() {
    // A candidate with the same mtime is not strictly newer, so it must not
    // trigger a reload (avoids the differ-but-not-newer reload loop, #277).
    let candidates = vec![(PathBuf::from("/x/stable/jcode"), Some(t(100)))];
    assert!(!newer_binary_available(
        Some(t(100)),
        Some(std::path::Path::new("/x/builds/versions/dev/jcode")),
        candidates,
    ));
}
