use super::*;

#[test]
fn test_env_override_trusted_external_auth_splits_source_and_path_entries() {
    let _guard = crate::storage::lock_test_env();
    let prev = std::env::var_os("JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES");
    crate::env::set_var(
        "JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES",
        "legacy_source,claude_code_credentials|/tmp/auth.json",
    );

    let mut cfg = Config::default();
    cfg.apply_env_overrides();

    assert_eq!(cfg.auth.trusted_external_sources, vec!["legacy_source"]);
    assert_eq!(
        cfg.auth.trusted_external_source_paths,
        vec!["claude_code_credentials|/tmp/auth.json"]
    );

    if let Some(prev) = prev {
        crate::env::set_var("JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES", prev);
    } else {
        crate::env::remove_var("JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES");
    }
}

#[test]
fn test_external_auth_source_allowed_for_path_matches_saved_entry() {
    let _guard = crate::storage::lock_test_env();
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("auth.json");
    std::fs::write(&path, "{}\n").expect("write auth file");

    let canonical = std::fs::canonicalize(&path).expect("canonical path");
    let mut cfg = Config::default();
    cfg.auth.trusted_external_source_paths = vec![format!(
        "test_source|{}",
        canonical.to_string_lossy().to_ascii_lowercase()
    )];

    assert!(cfg.external_auth_source_allowed_for_path_config("test_source", &path));
}

#[test]
fn test_external_auth_source_allowed_for_path_ignores_broad_legacy_entry() {
    let _guard = crate::storage::lock_test_env();
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("auth.json");
    std::fs::write(&path, "{}\n").expect("write auth file");

    let mut cfg = Config::default();
    cfg.auth.trusted_external_sources = vec!["test_source".to_string()];

    assert!(!cfg.external_auth_source_allowed_for_path_config("test_source", &path));
}

/// Regression test for issue #349: a removed/unknown `update_channel` value
/// (older configs could contain `"manual"`) must not fail the whole config
/// parse. A hard parse failure during the reload handoff left the reload
/// marker stuck in `starting` and clients re-requested the reload forever.
#[test]
fn unknown_update_channel_value_falls_back_to_stable_instead_of_failing_parse() {
    let cfg: Config = toml::from_str("[features]\nupdate_channel = \"manual\"\n")
        .expect("unknown update_channel must not fail config parse");
    assert_eq!(
        cfg.features.update_channel,
        crate::config::UpdateChannel::Stable,
        "unknown channel should fall back to the default"
    );

    // Other settings in the same config must survive the fallback.
    let cfg: Config = toml::from_str(
        "[features]\nupdate_channel = \"manual\"\nmemory = false\n\n[display]\ncentered = true\n",
    )
    .expect("config with unknown update_channel should parse");
    assert_eq!(
        cfg.features.update_channel,
        crate::config::UpdateChannel::Stable
    );
    assert!(!cfg.features.memory);
    assert!(cfg.display.centered);
}

#[test]
fn known_update_channel_values_still_parse() {
    let cfg: Config = toml::from_str("[features]\nupdate_channel = \"preview\"\n").unwrap();
    assert_eq!(
        cfg.features.update_channel,
        crate::config::UpdateChannel::Preview
    );
    let encoded = toml::to_string(&cfg).unwrap();
    let decoded: Config = toml::from_str(&encoded).unwrap();
    assert_eq!(
        decoded.features.update_channel,
        crate::config::UpdateChannel::Preview
    );
    assert_eq!(
        crate::config::UpdateChannel::default(),
        crate::config::UpdateChannel::Stable
    );
    let cfg: Config = toml::from_str("[features]\nupdate_channel = \"main\"\n")
        .expect("main update_channel should parse");
    assert_eq!(
        cfg.features.update_channel,
        crate::config::UpdateChannel::Main
    );

    let cfg: Config = toml::from_str("[features]\nupdate_channel = \"stable\"\n")
        .expect("stable update_channel should parse");
    assert_eq!(
        cfg.features.update_channel,
        crate::config::UpdateChannel::Stable
    );
}

#[test]
fn update_channel_parse_accepts_known_aliases_and_rejects_unknown() {
    use crate::config::UpdateChannel;
    assert_eq!(UpdateChannel::parse("stable"), Some(UpdateChannel::Stable));
    assert_eq!(UpdateChannel::parse("release"), Some(UpdateChannel::Stable));
    assert_eq!(
        UpdateChannel::parse(" Preview "),
        Some(UpdateChannel::Preview)
    );
    assert_eq!(UpdateChannel::parse("main"), Some(UpdateChannel::Main));
    assert_eq!(UpdateChannel::parse("nightly"), Some(UpdateChannel::Main));
    assert_eq!(UpdateChannel::parse("edge"), Some(UpdateChannel::Main));
    assert_eq!(UpdateChannel::parse(" Main "), Some(UpdateChannel::Main));
    assert_eq!(UpdateChannel::parse("manual"), None);
    assert_eq!(UpdateChannel::parse(""), None);
}

impl Config {
    fn external_auth_source_allowed_for_path_config(&self, source_id: &str, path: &Path) -> bool {
        let Ok(entry) = Self::trusted_external_auth_path_entry(source_id, path) else {
            return false;
        };
        self.auth
            .trusted_external_source_paths
            .iter()
            .any(|value| value.trim().eq_ignore_ascii_case(&entry))
    }
}
