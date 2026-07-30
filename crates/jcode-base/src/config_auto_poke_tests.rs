//! Tests for the `[features] auto_poke` setting.
//!
//! Split into its own file so `config_tests.rs` stays under the
//! oversized-test budget instead of pushing it over.

use super::*;

#[test]
fn auto_poke_feature_defaults_on_and_parses_false() {
    // Default must stay on: this only exists so users who find the automatic
    // follow-ups disruptive can opt out (issue #664), not to change behaviour
    // for everyone else.
    assert!(Config::default().features.auto_poke);

    let cfg: Config =
        toml::from_str("[features]\nauto_poke = false\n").expect("features.auto_poke should parse");
    assert!(!cfg.features.auto_poke);
}

#[test]
fn auto_poke_environment_override_uses_standard_boolean_values() {
    let _guard = crate::storage::lock_test_env();
    let previous = std::env::var_os("JCODE_AUTO_POKE");
    crate::env::set_var("JCODE_AUTO_POKE", "off");

    let mut cfg = Config::default();
    cfg.apply_env_overrides();
    assert!(!cfg.features.auto_poke);

    match previous {
        Some(previous) => crate::env::set_var("JCODE_AUTO_POKE", previous),
        None => crate::env::remove_var("JCODE_AUTO_POKE"),
    }
}

#[test]
fn default_config_file_documents_auto_poke_and_round_trips() {
    // The shipped template is what most users edit, so a field missing from it
    // is effectively undiscoverable. Parse the template itself rather than
    // asserting on a substring, so this also catches a malformed default.
    let _guard = crate::storage::lock_test_env();
    let home = tempfile::tempdir().expect("tempdir");
    crate::env::set_var("JCODE_HOME", home.path());

    let path = Config::create_default_config_file().expect("write default config");
    let text = std::fs::read_to_string(&path).expect("read default config");
    assert!(
        text.contains("auto_poke = true"),
        "default config template should document auto_poke"
    );
    let parsed: Config = toml::from_str(&text).expect("default template should parse");
    assert!(parsed.features.auto_poke);

    crate::env::remove_var("JCODE_HOME");
}
