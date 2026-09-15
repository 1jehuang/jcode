//! Config-save tests for the declared-removal signal.
//!
//! A serialized struct can only express a deletion by omitting a key, which at
//! the file level is indistinguishable from "not modeled". So the write path
//! takes an explicit list of dotted paths to delete. This file starts with that
//! signal's unit tests (recorded per config path, consumed by one save); the
//! preserving save and its file-level tests build on it.

use super::Config;

/// A declared removal is recorded for the active config path and consumed by
/// the save that follows, so it cannot leak onto a later write.
#[test]
fn declared_removals_are_recorded_and_consumed() {
    let _guard = crate::storage::lock_test_env();
    let previous = std::env::var_os("JCODE_HOME");
    let dir = tempfile::TempDir::new().expect("tempdir");
    crate::env::set_var("JCODE_HOME", dir.path());
    Config::invalidate_cache();
    let path = Config::path().expect("config path");
    std::fs::create_dir_all(path.parent().expect("config parent")).expect("create parent");
    std::fs::write(&path, "[display]\ncentered = false\n").expect("write config");
    Config::invalidate_cache();

    Config::declare_removal("display.colors");
    assert_eq!(
        Config::pending_removals(),
        vec!["display.colors".to_string()],
        "the removal must be recorded for this config path"
    );

    Config::default().save().expect("save");
    assert!(
        Config::pending_removals().is_empty(),
        "save must consume the declared removal"
    );

    match previous {
        Some(prev) => crate::env::set_var("JCODE_HOME", prev),
        None => crate::env::remove_var("JCODE_HOME"),
    }
    Config::invalidate_cache();
}

/// Removals are keyed by config path: one recorded under a different
/// `JCODE_HOME` must never be applied to another home's file.
#[test]
fn declared_removals_are_scoped_to_the_config_path() {
    let _guard = crate::storage::lock_test_env();
    let previous = std::env::var_os("JCODE_HOME");
    let first = tempfile::TempDir::new().expect("tempdir");
    let second = tempfile::TempDir::new().expect("tempdir");

    // Record a removal under the first home.
    crate::env::set_var("JCODE_HOME", first.path());
    Config::invalidate_cache();
    Config::declare_removal("display.colors");

    // Saving under the second home must neither apply nor consume it.
    crate::env::set_var("JCODE_HOME", second.path());
    Config::invalidate_cache();
    let path = Config::path().expect("config path");
    std::fs::create_dir_all(path.parent().expect("config parent")).expect("create parent");
    std::fs::write(&path, "[display.colors]\nerror = \"#1050f0\"\n").expect("write config");
    Config::load_for_update()
        .expect("load")
        .save()
        .expect("save in the other home");
    assert!(
        std::fs::read_to_string(&path)
            .expect("read")
            .contains("#1050f0"),
        "the other home's file must keep its colors"
    );

    // Back in the first home, the declaration is still pending.
    crate::env::set_var("JCODE_HOME", first.path());
    Config::invalidate_cache();
    assert_eq!(
        Config::pending_removals(),
        vec!["display.colors".to_string()],
        "the declaration must survive until its own home is saved"
    );

    match previous {
        Some(prev) => crate::env::set_var("JCODE_HOME", prev),
        None => crate::env::remove_var("JCODE_HOME"),
    }
    Config::invalidate_cache();
}
