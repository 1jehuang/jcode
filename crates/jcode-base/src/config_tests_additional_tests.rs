use super::*;

#[test]
fn hibernation_environment_override_accepts_seconds_and_zero_but_not_invalid_values() {
    let _lock = crate::storage::lock_test_env();
    let key = "JCODE_HIBERNATION_IDLE_SECS";
    let previous = std::env::var_os(key);
    for (value, expected) in [("60", 60), (" 0 ", 0), ("-1", 1800), ("invalid", 1800)] {
        crate::env::set_var(key, value);
        let mut config = Config::default();
        config.apply_env_overrides();
        assert_eq!(config.session.hibernation_idle_secs, expected, "{value}");
    }
    restore_env_var(key, previous);
}

#[test]
fn gemini_config_reload_does_not_export_sticky_environment_overrides() {
    let _lock = crate::storage::lock_test_env();
    let _env = GeminiConfigEnv::new();
    let mut cfg = Config::default();
    cfg.provider.gemini_force_oauth = true;
    cfg.provider.gemini_project = Some(" project-a ".into());
    cfg.save().unwrap();
    assert!(crate::auth::gemini::force_oauth());
    assert_eq!(
        crate::auth::gemini::cloud_project().as_deref(),
        Some("project-a")
    );
    assert!(std::env::var_os("JCODE_GEMINI_FORCE_OAUTH").is_none());
    assert!(std::env::var_os("GOOGLE_CLOUD_PROJECT").is_none());

    cfg.provider.gemini_force_oauth = false;
    cfg.provider.gemini_project = Some("project-b".into());
    cfg.save().unwrap();
    assert!(!crate::auth::gemini::force_oauth());
    assert_eq!(
        crate::auth::gemini::cloud_project().as_deref(),
        Some("project-b")
    );

    cfg.provider.gemini_project = None;
    cfg.save().unwrap();
    assert_eq!(crate::auth::gemini::cloud_project(), None);
}

#[test]
fn gemini_environment_overrides_config_without_changing_the_file() {
    let _lock = crate::storage::lock_test_env();
    let _env = GeminiConfigEnv::new();
    let mut cfg = Config::default();
    cfg.provider.gemini_force_oauth = true;
    cfg.provider.gemini_project = Some("from-config".into());
    cfg.save().unwrap();
    crate::env::set_var("JCODE_GEMINI_FORCE_OAUTH", "off");
    crate::env::set_var("GOOGLE_CLOUD_PROJECT_ID", "from-alias");
    Config::invalidate_cache();
    assert!(!crate::auth::gemini::force_oauth());
    assert_eq!(
        crate::auth::gemini::cloud_project().as_deref(),
        Some("from-alias")
    );
    crate::env::set_var("GOOGLE_CLOUD_PROJECT", "from-env");
    Config::invalidate_cache();
    assert_eq!(
        crate::auth::gemini::cloud_project().as_deref(),
        Some("from-env")
    );

    for key in [
        "JCODE_GEMINI_FORCE_OAUTH",
        "GOOGLE_CLOUD_PROJECT",
        "GOOGLE_CLOUD_PROJECT_ID",
    ] {
        crate::env::remove_var(key);
    }
    Config::invalidate_cache();
    assert!(crate::auth::gemini::force_oauth());
    assert_eq!(
        crate::auth::gemini::cloud_project().as_deref(),
        Some("from-config")
    );
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
fn preserve_reasoning_context_defaults_to_enabled() {
    assert!(ProviderConfig::default().preserve_reasoning_context);
}

#[test]
fn swarm_spawn_mode_defaults_to_inline() {
    assert_eq!(
        Config::default().agents.swarm_spawn_mode,
        SwarmSpawnMode::Inline
    );
}
