//! Config tests for user-configurable colors.
//!
//! Split out of `config_tests.rs` to keep that file under the test-size
//! ratchet, and because these exercise one coherent contract: the `/colors`
//! surface writing to and reading from a real config file.

use super::Config;

/// The color config a user actually writes must survive a real file round trip.
///
/// The template tests check the string we ship; this checks the whole path a
/// user takes: jcode writes the default file, the user uncomments the color
/// example, and jcode loads it back through the same cache the running process
/// uses. A schema or template mistake that only shows up on disk lands here.
#[test]
fn configured_colors_survive_a_real_config_file_round_trip() {
    let _guard = crate::storage::lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let dir = tempfile::TempDir::new().expect("tempdir");
    crate::env::set_var("JCODE_HOME", dir.path());
    Config::invalidate_cache();

    // The file jcode writes for a new user must document colors and parse.
    let path = Config::create_default_config_file().expect("create default config file");
    let generated = std::fs::read_to_string(&path).expect("read generated config");
    assert!(
        generated.contains("[display.colors]"),
        "the generated config should document how to configure colors"
    );
    assert!(
        generated.contains("/colors generate"),
        "the generated config should point at the palette generator"
    );

    // A user setting colors by hand, alongside an unrelated existing setting.
    std::fs::write(
        &path,
        "[display]\ncentered = false\n\n[display.colors]\nerror = \"#1050f0\"\nai = \"#ffaa00\"\n",
    )
    .expect("write user config");
    Config::invalidate_cache();

    let loaded = crate::config::config();
    assert_eq!(
        loaded.display.colors.get("error").map(String::as_str),
        Some("#1050f0"),
        "a hand-written color must load"
    );
    assert_eq!(loaded.display.colors.len(), 2);
    assert!(!loaded.display.centered, "unrelated settings must survive");

    // The summary users read must reflect it, or the setting is invisible.
    let summary = loaded.display_string();
    assert!(
        summary.contains("Custom colors") && summary.contains("error"),
        "config summary should report customized roles: {summary}"
    );

    // A typo must be skipped, never fatal, and must not take valid entries with
    // it: losing a whole palette to one bad line would be the worst outcome.
    // The palette-side half of that contract is asserted in `jcode-tui-style`
    // (`from_pairs_reports_errors_without_dropping_valid_entries`), which owns
    // the parsing; here we only require the config layer to keep both entries
    // and stay loadable.
    std::fs::write(
        &path,
        "[display.colors]\nerror = \"nonsense\"\nai = \"#ffaa00\"\n",
    )
    .expect("write config with a typo");
    Config::invalidate_cache();
    let recovered = crate::config::config();
    assert_eq!(
        recovered.display.colors.len(),
        2,
        "an invalid value must not drop entries at the config layer"
    );

    if let Some(prev) = prev_home {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}
