use super::{ReloadTargetDecision, guarded_reload_target};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

fn t(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

fn candidate(path: &str) -> (PathBuf, &'static str) {
    (PathBuf::from(path), "shared-server")
}

#[test]
fn same_binary_is_always_used() {
    // Reloading into ourselves never raises a version question, even with an
    // older mtime reading.
    let decision = guarded_reload_target(
        candidate("/x/current/jcode"),
        Path::new("/x/current/jcode"),
        Some(Path::new("/x/current/jcode")),
        Some(Path::new("/x/current/jcode")),
        Some(t(200)),
        Some(t(100)),
    );
    assert!(matches!(decision, ReloadTargetDecision::UseCandidate(_)));
}

#[test]
fn newer_candidate_is_used() {
    // The self-dev case: a freshly written candidate is newer, so apply it.
    let decision = guarded_reload_target(
        candidate("/x/shared-server/jcode"),
        Path::new("/x/builds/versions/new/jcode"),
        Some(Path::new("/x/builds/versions/old/jcode")),
        Some(Path::new("/x/builds/versions/old/jcode")),
        Some(t(100)),
        Some(t(200)),
    );
    match decision {
        ReloadTargetDecision::UseCandidate((path, _)) => {
            assert_eq!(path, PathBuf::from("/x/shared-server/jcode"));
        }
        other => panic!("expected candidate to be used, got {other:?}"),
    }
}

#[test]
fn equal_mtime_candidate_is_used() {
    // Same mtime is not a downgrade.
    let decision = guarded_reload_target(
        candidate("/x/shared-server/jcode"),
        Path::new("/x/builds/versions/same/jcode"),
        Some(Path::new("/x/builds/versions/current/jcode")),
        Some(Path::new("/x/builds/versions/current/jcode")),
        Some(t(100)),
        Some(t(100)),
    );
    assert!(matches!(decision, ReloadTargetDecision::UseCandidate(_)));
}

#[test]
fn strictly_older_candidate_is_blocked_and_uses_current_exe() {
    // The reported bug: shared-server channel points at an older build than
    // the running client. Force reload must NOT downgrade; it re-execs the
    // current binary instead.
    let decision = guarded_reload_target(
        candidate("/x/shared-server/jcode"),
        Path::new("/x/builds/versions/old-0.14.3/jcode"),
        Some(Path::new("/x/builds/versions/new/jcode")),
        Some(Path::new("/x/builds/versions/new/jcode")),
        Some(t(300)),
        Some(t(100)),
    );
    match decision {
        ReloadTargetDecision::DowngradeBlockedUseCurrent((path, _)) => {
            assert_eq!(path, PathBuf::from("/x/builds/versions/new/jcode"));
        }
        other => panic!("expected downgrade to be blocked, got {other:?}"),
    }
}

#[test]
fn unreadable_candidate_mtime_is_treated_as_downgrade() {
    let decision = guarded_reload_target(
        candidate("/x/shared-server/jcode"),
        Path::new("/x/builds/versions/unknown/jcode"),
        Some(Path::new("/x/builds/versions/new/jcode")),
        Some(Path::new("/x/builds/versions/new/jcode")),
        Some(t(300)),
        None,
    );
    assert!(matches!(
        decision,
        ReloadTargetDecision::DowngradeBlockedUseCurrent(_)
    ));
}

#[test]
fn downgrade_without_current_exe_falls_back_to_candidate() {
    // If we cannot identify the running exe we cannot re-exec it, so we have
    // to proceed with the candidate rather than refuse to reload entirely.
    let decision = guarded_reload_target(
        candidate("/x/shared-server/jcode"),
        Path::new("/x/builds/versions/old/jcode"),
        None,
        None,
        None,
        Some(t(100)),
    );
    assert!(matches!(
        decision,
        ReloadTargetDecision::DowngradeUnverifiable(_)
    ));
}
