//! Tests for `#[path]`-attributed module `project_key_tests` of `memory.rs`.
//!
//! Extracted from the parent file: an inline `#[cfg(test)] mod` counts toward
//! the code-size budget, which only exempts `*_tests.rs` files.

use crate::memory::project_memory_file;
use crate::project_scope::{legacy_project_key, project_key};
use crate::storage;
use std::path::PathBuf;

/// `project_memory_file` must name its file with the stable key, and two
/// spellings of one repo must land on one file.
///
/// This drives the public call site rather than `project_key`, because a
/// helper-level test passes even when a caller still hashes the raw path
/// itself, which is exactly what all three sites used to do.
#[test]
fn project_memory_file_uses_the_stable_key_and_unifies_path_spellings() {
    let _lock = storage::lock_test_env();
    let repo = tempfile::tempdir().expect("tempdir");
    let path = project_memory_file(repo.path()).expect("memory file");
    let name = path
        .file_name()
        .expect("file name")
        .to_string_lossy()
        .into_owned();

    assert_eq!(
        name,
        format!("{}.json", project_key(repo.path())),
        "the memory file must be named with the stable project key"
    );

    // The pre-P3.1 name must no longer be produced.
    assert_ne!(
        name,
        format!("{}.json", legacy_project_key(repo.path())),
        "positive control: the legacy key must differ, or this proves nothing"
    );

    // Positive control: a different spelling of the same repo, one file.
    let dotted = PathBuf::from(repo.path()).join(".");
    assert_eq!(
        project_memory_file(&dotted).expect("dotted"),
        path,
        "two spellings of one repo must not fragment its memories"
    );
}

/// An existing store written under the legacy key must survive the upgrade.
///
/// Without the carry-forward in `project_memory_file`, upgrading jcode would
/// silently present every existing user with an empty memory.
#[test]
fn project_memory_file_carries_a_legacy_store_forward() {
    let _lock = storage::lock_test_env();
    let repo = tempfile::tempdir().expect("tempdir");
    let memory_dir = storage::jcode_dir()
        .expect("jcode dir")
        .join("memory")
        .join("projects");
    std::fs::create_dir_all(&memory_dir).expect("create memory dir");

    let legacy = memory_dir.join(format!("{}.json", legacy_project_key(repo.path())));
    std::fs::write(&legacy, "{\"memories\":[\"kept\"]}").expect("seed legacy store");

    let path = project_memory_file(repo.path()).expect("memory file");
    assert_eq!(
        std::fs::read_to_string(&path).expect("migrated store is readable"),
        "{\"memories\":[\"kept\"]}",
        "a store written under the legacy key must be carried forward, not orphaned"
    );
}
