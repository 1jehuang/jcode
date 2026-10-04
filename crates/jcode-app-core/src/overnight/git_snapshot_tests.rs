//! Tests for `gather_git_snapshot`, which resolves git state for an overnight
//! run.
//!
//! Sibling file so the addition does not grow `overnight.rs`, which is already
//! over the code-size ratchet and is measured with its `#[cfg(test)]` module
//! stripped, so the two cannot both be satisfied by rebaselining.

use std::path::Path;

use super::gather_git_snapshot;

/// A manifest with no working_dir must not be described by the git state of
/// whatever directory the process happens to be in. Before the fix this
/// resolved `Path::new(".")` and reported the daemon cwd's branch and dirty
/// count, which is a different project's repository entirely.
///
/// The positive control matters: the same call with a real directory still
/// has to produce git output, or the test would pass for the wrong reason.
#[test]
fn git_snapshot_without_working_dir_reports_nothing() {
    let absent = gather_git_snapshot(None);
    assert!(
        absent.branch.is_none(),
        "branch must stay unknown, got {:?}",
        absent.branch
    );
    assert!(
        absent.dirty_count.is_none(),
        "dirty count must stay unknown, got {:?}",
        absent.dirty_count
    );
    assert!(absent.dirty_summary.is_empty());
    let error = absent
        .error
        .expect("a missing working dir must be reported, not papered over");
    assert!(
        error.contains("no working directory"),
        "error should say why, got {error:?}"
    );

    // Positive control: a real directory is still inspected. Use a temp dir
    // inside the repo so `git status --short` is a real command either way,
    // and only assert that the guard clause did not fire.
    let temp = tempfile::tempdir().expect("tempdir");
    let present = gather_git_snapshot(Some(temp.path()));
    assert!(
        !present
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("no working directory"),
        "a supplied working dir must not be treated as absent: {:?}",
        present.error
    );
}
