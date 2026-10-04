use super::pick_newest_candidate;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

fn t(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

fn entry(
    path: &str,
    label: &'static str,
    mtime: Option<SystemTime>,
) -> ((PathBuf, &'static str), PathBuf, Option<SystemTime>) {
    let p = PathBuf::from(path);
    ((p.clone(), label), p, mtime)
}

#[test]
fn other_flavor_wins_when_strictly_newer() {
    // The /update bug: the session's own (self-dev) flavor is pinned to an
    // OLD build, but the other (normal) flavor self-healed to a NEWER
    // release. The reload target must follow the newer release so the daemon
    // can actually apply the update it advertises.
    let chosen = pick_newest_candidate([
        entry(
            "/x/versions/old-selfdev/jcode",
            "shared-server",
            Some(t(100)),
        ),
        entry("/x/versions/new-release/jcode", "stable", Some(t(200))),
    ])
    .expect("a candidate");
    assert_eq!(chosen.0, PathBuf::from("/x/versions/new-release/jcode"));
}

#[test]
fn own_flavor_wins_on_tie() {
    // A deliberately-pinned self-dev build that is at least as fresh as the
    // other flavor must be preserved (self-dev pin protection).
    let chosen = pick_newest_candidate([
        entry("/x/versions/selfdev/jcode", "shared-server", Some(t(200))),
        entry("/x/versions/release/jcode", "stable", Some(t(200))),
    ])
    .expect("a candidate");
    assert_eq!(chosen.0, PathBuf::from("/x/versions/selfdev/jcode"));
}

#[test]
fn own_flavor_wins_when_strictly_newer() {
    let chosen = pick_newest_candidate([
        entry(
            "/x/versions/fresh-selfdev/jcode",
            "shared-server",
            Some(t(300)),
        ),
        entry("/x/versions/release/jcode", "stable", Some(t(200))),
    ])
    .expect("a candidate");
    assert_eq!(chosen.0, PathBuf::from("/x/versions/fresh-selfdev/jcode"));
}

#[test]
fn unknown_other_mtime_never_displaces_preferred() {
    // An unreadable mtime on the other flavor must not let it win, so we
    // never swap to an unverifiable binary.
    let chosen = pick_newest_candidate([
        entry("/x/versions/selfdev/jcode", "shared-server", Some(t(100))),
        entry("/x/versions/release/jcode", "stable", None),
    ])
    .expect("a candidate");
    assert_eq!(chosen.0, PathBuf::from("/x/versions/selfdev/jcode"));
}

#[test]
fn duplicate_canonical_paths_collapse() {
    // Both flavors resolving to the same binary must not double-count; the
    // first (preferred) occurrence wins.
    let chosen = pick_newest_candidate([
        entry("/x/versions/same/jcode", "shared-server", Some(t(100))),
        entry("/x/versions/same/jcode", "stable", Some(t(999))),
    ])
    .expect("a candidate");
    assert_eq!(chosen.1, "shared-server");
}

#[test]
fn empty_is_none() {
    assert!(pick_newest_candidate(std::iter::empty()).is_none());
}
