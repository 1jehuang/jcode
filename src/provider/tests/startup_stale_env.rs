/// Regression test: when stale named-provider env vars leak into the
/// process from a previous jcode session's shell environment,
/// `new_with_auth_status` must clear them before the guard so
/// config.toml's `default_provider` can take effect. The key assertion
/// is that the stale profile is NOT used and config.toml wins.
#[test]
fn startup_clears_stale_named_provider_env_vars_when_no_cli_override() {
    with_clean_provider_test_env(|| {
        let cfg_path = crate::config::Config::path().expect("config path in test");
        std::fs::create_dir_all(cfg_path.parent().expect("config parent"))
            .expect("create config dir");
        let toml = r#"
[providers.my-profile]
provider_type = "openai-compatible"
base_url = "https://llm.example.test/v1"
default_model = "my-gpt"

[provider]
default_provider = "my-profile"
"#;
        std::fs::write(&cfg_path, toml).expect("write config.toml");

        // Stale env vars simulating a previous session's shell env.
        // JCODE_PROVIDER_PROFILE_ACTIVE is NOT set, so no CLI override.
        crate::env::set_var("JCODE_NAMED_PROVIDER_PROFILE", "stale-previous-profile");
        crate::env::set_var("JCODE_OPENROUTER_CACHE_NAMESPACE", "stale-namespace");
        crate::env::set_var("JCODE_ACTIVE_PROVIDER", "openrouter");
        crate::env::set_var("JCODE_RUNTIME_PROVIDER", "stale-runtime");

        crate::config::Config::invalidate_cache();

        let auth = crate::auth::AuthStatus::check();
        let provider = MultiProvider::new_with_auth_status(auth);

        // The config.toml default_provider="my-profile" should have
        // taken effect (the stale vars were cleared first, then the
        // guard applied the correct profile).
        assert_eq!(
            std::env::var("JCODE_PROVIDER_PROFILE_ACTIVE")
                .ok()
                .as_deref(),
            Some("1"),
            "profile should be active after default_provider is applied"
        );
        assert_eq!(
            std::env::var("JCODE_NAMED_PROVIDER_PROFILE")
                .ok()
                .as_deref(),
            Some("my-profile"),
            "named profile should be config.toml's my-profile, not stale-previous-profile"
        );

        // The provider may resolve to Claude as a fallback when
        // the named profile has no live credentials, but the env
        // vars confirm the correct profile was applied.
        let _ = provider.active_provider();
    });
}

/// When JCODE_PROVIDER_PROFILE_ACTIVE is explicitly set (CLI override),
/// stale env vars should NOT be cleared and the explicit profile should
/// be preserved.
#[test]
fn startup_preserves_explicit_cli_profile_override() {
    with_clean_provider_test_env(|| {
        let cfg_path = crate::config::Config::path().expect("config path in test");
        std::fs::create_dir_all(cfg_path.parent().expect("config parent"))
            .expect("create config dir");
        let toml = r#"
[providers.cli-profile]
provider_type = "openai-compatible"
base_url = "https://cli.example.test/v1"
default_model = "cli-model"

[provider]
default_provider = "my-profile"
"#;
        std::fs::write(&cfg_path, toml).expect("write config.toml");

        // Explicit CLI override: JCODE_PROVIDER_PROFILE_ACTIVE is set.
        crate::env::set_var("JCODE_PROVIDER_PROFILE_ACTIVE", "1");
        crate::env::set_var("JCODE_NAMED_PROVIDER_PROFILE", "cli-profile");
        crate::env::set_var("JCODE_OPENROUTER_CACHE_NAMESPACE", "cli-profile");

        crate::config::Config::invalidate_cache();

        let auth = crate::auth::AuthStatus::check();
        let provider = MultiProvider::new_with_auth_status(auth);

        // With an explicit CLI override, the stale env vars should NOT
        // be cleared.
        assert_eq!(
            std::env::var("JCODE_PROVIDER_PROFILE_ACTIVE")
                .ok()
                .as_deref(),
            Some("1"),
            "explicit CLI override should be preserved"
        );
        assert_eq!(
            std::env::var("JCODE_NAMED_PROVIDER_PROFILE")
                .ok()
                .as_deref(),
            Some("cli-profile"),
            "explicit profile should be preserved"
        );

        let _ = provider; // just ensure construction succeeded
    });
}
