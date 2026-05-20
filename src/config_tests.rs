use super::{
    AmbientConfig, Config, DiffDisplayMode, DisplayConfig, ProviderConfig,
    SessionPickerResumeAction, SwarmSpawnMode, config_env_fingerprint,
};
use std::ffi::OsString;
use std::path::Path;

fn restore_env_var(key: &str, previous: Option<OsString>) {
    if let Some(previous) = previous {
        crate::env::set_var(key, previous);
    } else {
        crate::env::remove_var(key);
    }
}

#[test]
fn test_openai_reasoning_effort_defaults_to_low() {
    assert_eq!(
        ProviderConfig::default().openai_reasoning_effort.as_deref(),
        Some("low")
    );
}

#[test]
fn test_openai_fast_mode_defaults_to_priority() {
    assert_eq!(
        ProviderConfig::default().openai_service_tier.as_deref(),
        Some("priority")
    );
}

#[test]
fn swarm_spawn_mode_defaults_to_visible() {
    assert_eq!(
        Config::default().agents.swarm_spawn_mode,
        SwarmSpawnMode::Visible
    );
}

#[test]
fn swarm_spawn_mode_parses_supported_values() {
    let cfg: Config = toml::from_str("[agents]\nswarm_spawn_mode = \"headless\"\n")
        .expect("headless swarm_spawn_mode should parse");
    assert_eq!(cfg.agents.swarm_spawn_mode, SwarmSpawnMode::Headless);

    let cfg: Config = toml::from_str("[agents]\nswarm_spawn_mode = \"auto\"\n")
        .expect("auto swarm_spawn_mode should parse");
    assert_eq!(cfg.agents.swarm_spawn_mode, SwarmSpawnMode::Auto);

    let cfg: Config = toml::from_str("[agents]\nswarm_spawn_mode = \"visible\"\n")
        .expect("visible swarm_spawn_mode should parse");
    assert_eq!(cfg.agents.swarm_spawn_mode, SwarmSpawnMode::Visible);
}

#[test]
fn swarm_spawn_mode_rejects_invalid_values() {
    let result = toml::from_str::<Config>("[agents]\nswarm_spawn_mode = \"background\"\n");
    assert!(result.is_err());
}

#[test]
fn test_generated_default_config_uses_low_openai_reasoning_effort() {
    let _guard = crate::storage::lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let dir = tempfile::TempDir::new().expect("tempdir");
    crate::env::set_var("JCODE_HOME", dir.path());

    let path = Config::create_default_config_file().expect("create default config file");
    let content = std::fs::read_to_string(path).expect("read default config file");

    assert!(
        content.contains("openai_reasoning_effort = \"low\""),
        "generated default config should use low OpenAI reasoning effort"
    );
    assert!(
        content.contains("openai_service_tier = \"priority\""),
        "generated default config should enable OpenAI fast mode"
    );

    if let Some(prev) = prev_home {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn global_config_cache_reloads_after_manual_file_edit() {
    let _guard = crate::storage::lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let dir = tempfile::TempDir::new().expect("tempdir");
    crate::env::set_var("JCODE_HOME", dir.path());
    Config::invalidate_cache();

    let path = Config::path().expect("config path");
    std::fs::create_dir_all(path.parent().expect("config parent")).expect("create config parent");
    std::fs::write(&path, "[display]\ncentered = false\n").expect("write initial config");

    assert!(!crate::config::config().display.centered);

    // Different length as well as mtime so the metadata fingerprint notices the
    // manual edit even on filesystems with coarse timestamp resolution.
    std::fs::write(&path, "[display]\ncentered = true\n# edited\n").expect("edit config");

    assert!(crate::config::config().display.centered);

    restore_env_var("JCODE_HOME", prev_home);
    Config::invalidate_cache();
}

#[test]
fn config_save_invalidates_global_config_cache() {
    let _guard = crate::storage::lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let dir = tempfile::TempDir::new().expect("tempdir");
    crate::env::set_var("JCODE_HOME", dir.path());
    Config::invalidate_cache();

    let mut cfg = Config::default();
    cfg.display.centered = false;
    cfg.save().expect("save initial config");
    assert!(!crate::config::config().display.centered);

    cfg.display.centered = true;
    cfg.save().expect("save updated config");
    assert!(crate::config::config().display.centered);

    restore_env_var("JCODE_HOME", prev_home);
    Config::invalidate_cache();
}

#[test]
fn config_env_fingerprint_ignores_runtime_only_jcode_vars() {
    let _guard = crate::storage::lock_test_env();
    let prev_runtime_provider = std::env::var_os("JCODE_RUNTIME_PROVIDER");
    let prev_active_provider = std::env::var_os("JCODE_ACTIVE_PROVIDER");
    let prev_display_centered = std::env::var_os("JCODE_DISPLAY_CENTERED");

    crate::env::remove_var("JCODE_RUNTIME_PROVIDER");
    crate::env::remove_var("JCODE_ACTIVE_PROVIDER");
    crate::env::remove_var("JCODE_DISPLAY_CENTERED");
    let baseline = config_env_fingerprint();

    crate::env::set_var("JCODE_RUNTIME_PROVIDER", "openai");
    crate::env::set_var("JCODE_ACTIVE_PROVIDER", "openai");
    assert_eq!(baseline, config_env_fingerprint());

    crate::env::set_var("JCODE_DISPLAY_CENTERED", "1");
    assert_ne!(baseline, config_env_fingerprint());

    restore_env_var("JCODE_RUNTIME_PROVIDER", prev_runtime_provider);
    restore_env_var("JCODE_ACTIVE_PROVIDER", prev_active_provider);
    restore_env_var("JCODE_DISPLAY_CENTERED", prev_display_centered);
}

#[test]
fn config_env_fingerprint_tracks_every_apply_env_override_var() {
    let override_source = include_str!("config/env_overrides.rs");
    let mut missing = Vec::new();

    for line in override_source.lines() {
        let Some(start) = line.find("std::env::var(\"") else {
            continue;
        };
        let rest = &line[start + "std::env::var(\"".len()..];
        let Some(end) = rest.find('"') else {
            continue;
        };
        let key = &rest[..end];
        if !crate::config::CONFIG_ENV_KEYS.contains(&key) {
            missing.push(key.to_string());
        }
    }

    missing.sort();
    missing.dedup();
    assert!(
        missing.is_empty(),
        "CONFIG_ENV_KEYS must include every env var read by Config::apply_env_overrides; missing: {missing:?}"
    );
}

#[test]
fn cached_external_auth_trust_observes_manual_revocation() {
    let _guard = crate::storage::lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let dir = tempfile::TempDir::new().expect("tempdir");
    crate::env::set_var("JCODE_HOME", dir.path());
    Config::invalidate_cache();

    let auth_file = dir.path().join("external-auth.json");
    std::fs::write(&auth_file, "{}\n").expect("write external auth file");
    Config::allow_external_auth_source_for_path("test_source", &auth_file)
        .expect("trust external auth path");
    assert!(Config::external_auth_source_allowed_for_path_cached(
        "test_source",
        &auth_file
    ));

    let path = Config::path().expect("config path");
    std::fs::write(
        &path,
        "[auth]\ntrusted_external_source_paths = []\n# manually revoked\n",
    )
    .expect("manually revoke external auth trust");

    assert!(!Config::external_auth_source_allowed_for_path_cached(
        "test_source",
        &auth_file
    ));

    restore_env_var("JCODE_HOME", prev_home);
    Config::invalidate_cache();
}

#[test]
fn test_ambient_visible_defaults_to_true() {
    assert!(AmbientConfig::default().visible);
}

#[test]
fn test_display_auto_server_reload_defaults_to_true() {
    assert!(DisplayConfig::default().auto_server_reload);
}

#[test]
fn test_display_alignment_defaults_to_left() {
    assert!(!DisplayConfig::default().centered);
}

#[test]
fn test_provider_failover_defaults_match_new_behavior() {
    let provider = Config::default().provider;
    assert_eq!(
        provider.cross_provider_failover,
        super::CrossProviderFailoverMode::Countdown
    );
    assert!(provider.same_provider_account_failover);
}

#[test]
fn test_native_scrollbars_default_to_enabled() {
    let display = DisplayConfig::default();
    assert!(display.native_scrollbars.chat);
    assert!(display.native_scrollbars.side_panel);
}

#[test]
fn test_session_picker_resume_action_defaults_to_current_terminal() {
    assert_eq!(
        Config::default().keybindings.session_picker_enter,
        SessionPickerResumeAction::CurrentTerminal
    );
    assert_eq!(
        SessionPickerResumeAction::CurrentTerminal.alternate(),
        SessionPickerResumeAction::NewTerminal
    );
}

#[test]
fn test_session_picker_resume_action_deserializes_kebab_case() {
    let cfg: Config = toml::from_str(
        r#"
        [keybindings]
        session_picker_enter = "current-terminal"
        "#,
    )
    .expect("config should deserialize");

    assert_eq!(
        cfg.keybindings.session_picker_enter,
        SessionPickerResumeAction::CurrentTerminal
    );
}

#[test]
fn test_env_override_auto_server_reload() {
    let _guard = crate::storage::lock_test_env();
    let prev = std::env::var_os("JCODE_AUTO_SERVER_RELOAD");
    crate::env::set_var("JCODE_AUTO_SERVER_RELOAD", "false");

    let mut cfg = Config::default();
    cfg.apply_env_overrides();

    assert!(!cfg.display.auto_server_reload);

    if let Some(prev) = prev {
        crate::env::set_var("JCODE_AUTO_SERVER_RELOAD", prev);
    } else {
        crate::env::remove_var("JCODE_AUTO_SERVER_RELOAD");
    }
}

#[test]
fn test_env_override_native_scrollbars() {
    let _guard = crate::storage::lock_test_env();
    let prev_chat = std::env::var_os("JCODE_CHAT_NATIVE_SCROLLBAR");
    let prev_side = std::env::var_os("JCODE_SIDE_PANEL_NATIVE_SCROLLBAR");
    crate::env::set_var("JCODE_CHAT_NATIVE_SCROLLBAR", "true");
    crate::env::set_var("JCODE_SIDE_PANEL_NATIVE_SCROLLBAR", "false");

    let mut cfg = Config::default();
    cfg.apply_env_overrides();

    assert!(cfg.display.native_scrollbars.chat);
    assert!(!cfg.display.native_scrollbars.side_panel);

    if let Some(prev) = prev_chat {
        crate::env::set_var("JCODE_CHAT_NATIVE_SCROLLBAR", prev);
    } else {
        crate::env::remove_var("JCODE_CHAT_NATIVE_SCROLLBAR");
    }
    if let Some(prev) = prev_side {
        crate::env::set_var("JCODE_SIDE_PANEL_NATIVE_SCROLLBAR", prev);
    } else {
        crate::env::remove_var("JCODE_SIDE_PANEL_NATIVE_SCROLLBAR");
    }
}

#[test]
fn test_env_override_diff_mode_full_inline() {
    let _guard = crate::storage::lock_test_env();
    let prev = std::env::var_os("JCODE_DIFF_MODE");
    crate::env::set_var("JCODE_DIFF_MODE", "full-inline");

    let mut cfg = Config::default();
    cfg.apply_env_overrides();

    assert_eq!(cfg.display.diff_mode, DiffDisplayMode::FullInline);

    if let Some(prev) = prev {
        crate::env::set_var("JCODE_DIFF_MODE", prev);
    } else {
        crate::env::remove_var("JCODE_DIFF_MODE");
    }
}

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
