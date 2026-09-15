//! Config-save tests for format preservation.
//!
//! `Config::save` overlays the serialized struct onto the parsed existing file
//! instead of rewriting it wholesale, so the user's comments and any section a
//! newer build wrote survive a settings change. A serialized struct can only
//! express a removal by omitting a key, which at the file level is
//! indistinguishable from "not modeled", so deliberate deletions are declared
//! explicitly and applied on top of the overlay. These tests pin both halves:
//! what must survive, and what a declared removal must actually delete.

use super::Config;
use std::ffi::OsString;

/// Points `JCODE_HOME` at a fresh directory for the duration of a test.
struct HomeGuard {
    previous: Option<OsString>,
    _dir: tempfile::TempDir,
}

impl HomeGuard {
    fn new() -> Self {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let previous = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", dir.path());
        Config::invalidate_cache();
        Self {
            previous,
            _dir: dir,
        }
    }

    fn path(&self) -> std::path::PathBuf {
        Config::path().expect("config path")
    }

    fn write(&self, content: &str) {
        let path = self.path();
        std::fs::create_dir_all(path.parent().expect("config parent")).expect("create parent");
        std::fs::write(&path, content).expect("write config");
        Config::invalidate_cache();
    }

    fn read(&self) -> String {
        std::fs::read_to_string(self.path()).expect("read config")
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(prev) => crate::env::set_var("JCODE_HOME", prev),
            None => crate::env::remove_var("JCODE_HOME"),
        }
        Config::invalidate_cache();
    }
}

fn load_for_update() -> Config {
    Config::load_for_update().expect("load config for update")
}

/// A settings write must not erase the user's comments.
///
/// `DisplayConfig.centered` has no `skip_serializing_if`, so it is always
/// written back; the assertion is that the write happens *around* the comments
/// instead of replacing the whole file.
#[test]
fn settings_write_preserves_comments() {
    let _guard = crate::storage::lock_test_env();
    let home = HomeGuard::new();
    home.write("# my theme\n[display]\n# center everything\ncentered = false\n");

    let mut cfg = load_for_update();
    cfg.display.centered = true;
    cfg.save().expect("save");

    let written = home.read();
    assert!(
        written.contains("# my theme"),
        "the comment above the section must survive: {written}"
    );
    assert!(
        written.contains("# center everything"),
        "the comment on the changed key must survive: {written}"
    );
    assert!(
        written.contains("centered = true"),
        "the change must still be persisted: {written}"
    );
}

/// A section this build does not model must survive, because it may have been
/// written by a newer build (or a plugin) that this one must not silently undo.
#[test]
fn undeclared_unmodeled_section_survives_a_save() {
    let _guard = crate::storage::lock_test_env();
    let home = HomeGuard::new();
    home.write("[display]\ncentered = false\n\n[from_a_newer_build]\nshiny = true\n");

    let mut cfg = load_for_update();
    cfg.display.centered = true;
    cfg.save().expect("save");

    let written = home.read();
    assert!(
        written.contains("[from_a_newer_build]"),
        "an unmodeled section must survive: {written}"
    );
    assert!(
        written.contains("shiny = true"),
        "its values must survive too: {written}"
    );
    assert!(written.contains("centered = true"));
}

/// `/colors reset` (all roles) declares `display.colors`; the emptied section
/// must actually be gone from the file, or the next load restores it.
#[test]
fn declared_removal_deletes_a_section_from_the_file() {
    let _guard = crate::storage::lock_test_env();
    let home = HomeGuard::new();
    home.write("[display]\ncentered = false\n\n[display.colors]\nerror = \"#1050f0\"\n");

    Config::update_removing(&["display.colors"], |cfg| cfg.display.colors.clear())
        .expect("persist reset");

    let written = home.read();
    assert!(
        !written.contains("#1050f0"),
        "the reset color must be gone from the file: {written}"
    );
    assert!(
        !written.contains("[display.colors]"),
        "the emptied section must be gone from the file: {written}"
    );
    assert!(
        written.contains("centered = false"),
        "unrelated settings must still be present: {written}"
    );
}

/// Resetting one role is deletion by omission for that key only: the key must
/// disappear while the other roles stay.
#[test]
fn declared_removal_deletes_a_single_key() {
    let _guard = crate::storage::lock_test_env();
    let home = HomeGuard::new();
    home.write("[display.colors]\nerror = \"#1050f0\"\nai = \"#ffaa00\"\n");

    Config::update_removing(&["display.colors.error"], |cfg| {
        cfg.display.colors.remove("error");
    })
    .expect("persist reset");

    let written = home.read();
    assert!(
        !written.contains("#1050f0"),
        "the reset role must be gone: {written}"
    );
    assert!(
        written.contains("#ffaa00"),
        "the untouched role must survive: {written}"
    );
}

/// The frozen-sponsors repair happens in memory only, so the save has to be
/// told the section must vanish; otherwise the preserving save keeps it and the
/// freeze recurs.
#[test]
fn sponsors_repair_removes_the_section_from_the_file() {
    let _guard = crate::storage::lock_test_env();
    let home = HomeGuard::new();
    home.write(
        "[display]\ncentered = false\n\n[sponsors]\nenabled = false\nendpoint = \
         \"https://api.jcode.sh/v1/discovery\"\n",
    );

    let loaded = Config::load();
    assert!(loaded.sponsors.enabled, "the repair must run on load");
    loaded.save().expect("save");

    let written = home.read();
    assert!(
        !written.contains("[sponsors]"),
        "the repaired section must be gone so the freeze cannot recur: {written}"
    );
}

/// Clearing a provider default is deletion by omission too: `None` is not
/// serialized, so the setter has to declare the key or the file keeps it.
#[test]
fn clearing_the_default_model_removes_the_key_from_the_file() {
    let _guard = crate::storage::lock_test_env();
    let home = HomeGuard::new();
    home.write("[provider]\ndefault_model = \"claude-fable-5\"\n");

    Config::set_default_model(None, None).expect("clear defaults");

    let written = home.read();
    assert!(
        !written.contains("claude-fable-5"),
        "the cleared default must be gone from the file: {written}"
    );
    assert!(
        !written.contains("default_model"),
        "the key itself must be gone, not just its value: {written}"
    );
}

/// An unparseable file takes the plain-write fallback rather than being merged
/// against a document that cannot be understood.
#[test]
fn unparseable_file_falls_back_to_a_plain_write() {
    let _guard = crate::storage::lock_test_env();
    let home = HomeGuard::new();
    let broken = "[display\ncolors = {}\n";
    home.write(broken);

    let cfg = Config::default();
    cfg.save().expect("save");

    let written = home.read();
    assert_ne!(
        written, broken,
        "the fallback must replace the unreadable file: {written}"
    );
    toml::from_str::<Config>(&written)
        .unwrap_or_else(|err| panic!("fallback content must be valid TOML: {err}\n{written}"));
}

/// A declared removal is recorded for the active config path and consumed by
/// the save that follows, so it cannot leak onto a later write.
#[test]
fn declared_removals_are_recorded_and_consumed() {
    let _guard = crate::storage::lock_test_env();
    let home = HomeGuard::new();
    home.write("[display]\ncentered = false\n");

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
