use super::*;

struct EnvGuard {
    vars: Vec<(String, Option<String>)>,
}

impl EnvGuard {
    fn save(keys: &[&str]) -> Self {
        let vars = keys
            .iter()
            .map(|key| (key.to_string(), std::env::var(key).ok()))
            .collect();
        Self { vars }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in &self.vars {
            if let Some(value) = value {
                crate::env::set_var(key, value);
            } else {
                crate::env::remove_var(key);
            }
        }
    }
}

#[test]
fn matrix_profiles_have_unique_ids_and_safe_metadata() {
    let mut ids = HashSet::new();
    for profile in openai_compatible_profiles() {
        assert!(
            ids.insert(profile.id),
            "duplicate provider profile id: {}",
            profile.id
        );
        assert!(is_safe_env_key_name(profile.api_key_env));
        assert!(is_safe_env_file_name(profile.env_file));
        assert_eq!(
            normalize_api_base(profile.api_base).as_deref(),
            Some(profile.api_base)
        );
    }
}

#[test]
fn matrix_login_provider_aliases_resolve_to_canonical_ids() {
    assert_eq!(
        resolve_login_provider("subscription").map(|provider| provider.id),
        Some("jcode")
    );
    assert_eq!(
        resolve_login_provider("anthropic").map(|provider| provider.id),
        Some("claude")
    );
    assert_eq!(
        resolve_login_provider("opencodego").map(|provider| provider.id),
        Some("opencode-go")
    );
    assert_eq!(
        resolve_login_provider("z.ai").map(|provider| provider.id),
        Some("zai")
    );
    assert_eq!(
        resolve_login_provider("compat").map(|provider| provider.id),
        Some("openai-compatible")
    );
    assert_eq!(
        resolve_login_provider("aoai").map(|provider| provider.id),
        Some("azure")
    );
    assert_eq!(
        resolve_login_provider("cerberascode").map(|provider| provider.id),
        Some("cerebras")
    );
    assert_eq!(
        resolve_login_provider("bailian").map(|provider| provider.id),
        Some("alibaba-coding-plan")
    );
    assert_eq!(
        resolve_login_provider("gmail").map(|provider| provider.id),
        Some("google")
    );
}

#[test]
fn auth_issue_profile_metadata_matches_direct_provider_endpoints() {
    assert_eq!(ZAI_PROFILE.api_base, "https://api.z.ai/api/coding/paas/v4");
    assert_eq!(ZAI_PROFILE.default_model, Some("glm-4.5"));
    assert_eq!(DEEPSEEK_PROFILE.api_base, "https://api.deepseek.com");
    assert_eq!(DEEPSEEK_PROFILE.default_model, Some("deepseek-v4-flash"));
    assert_eq!(DEEPSEEK_PROFILE.setup_url, "https://api-docs.deepseek.com/");
    assert_eq!(COMTEGRA_PROFILE.api_base, "https://llm.comtegra.cloud/v1");
    assert_eq!(COMTEGRA_PROFILE.default_model, Some("glm-51-nvfp4"));
    assert_eq!(COMTEGRA_PROFILE.api_key_env, "COMTEGRA_API_KEY");
    assert!(!OPENAI_COMPAT_PROFILE.setup_url.contains("opencode.ai"));
}

#[test]
fn auth_issue_runtime_display_name_tracks_direct_compatible_profiles() {
    let _lock = crate::storage::lock_test_env();
    let _guard = EnvGuard::save(&[
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_OPENROUTER_PROVIDER_FEATURES",
        "JCODE_OPENROUTER_ALLOW_NO_AUTH",
        "JCODE_NAMED_PROVIDER_PROFILE",
        "JCODE_PROVIDER_PROFILE_ACTIVE",
    ]);

    apply_openai_compatible_profile_env(Some(DEEPSEEK_PROFILE));
    assert_eq!(runtime_provider_display_name("openrouter"), "DeepSeek");

    apply_openai_compatible_profile_env(Some(ZAI_PROFILE));
    assert_eq!(runtime_provider_display_name("openrouter"), "Z.AI");
}

#[test]
fn matrix_login_provider_ids_and_aliases_are_unique() {
    let mut seen = std::collections::HashSet::new();
    for provider in login_providers() {
        assert!(
            seen.insert(provider.id),
            "duplicate login provider identifier: {}",
            provider.id
        );
        for alias in provider.aliases {
            assert!(
                seen.insert(*alias),
                "duplicate login provider alias: {}",
                alias
            );
        }
    }
}

#[test]
fn matrix_tui_login_selection_supports_numbers_and_names() {
    let providers = tui_login_providers();
    assert_eq!(
        resolve_login_selection("1", &providers).map(|provider| provider.id),
        Some("auto-import")
    );
    assert_eq!(
        resolve_login_selection("2", &providers).map(|provider| provider.id),
        Some("claude")
    );
    assert_eq!(
        resolve_login_selection("15", &providers).map(|provider| provider.id),
        Some("cursor")
    );
    assert_eq!(
        resolve_login_selection("compat", &providers).map(|provider| provider.id),
        Some("openai-compatible")
    );
    assert_eq!(
        resolve_login_selection("cgc", &providers).map(|provider| provider.id),
        Some("comtegra")
    );
    assert!(resolve_login_selection("google", &providers).is_none());
}

#[test]
fn matrix_cli_login_selection_preserves_existing_order() {
    let providers = cli_login_providers();
    assert_eq!(
        resolve_login_selection("1", &providers).map(|provider| provider.id),
        Some("auto-import")
    );
    assert_eq!(
        resolve_login_selection("4", &providers).map(|provider| provider.id),
        Some("jcode")
    );
    assert_eq!(
        resolve_login_selection("5", &providers).map(|provider| provider.id),
        Some("copilot")
    );
    assert_eq!(
        resolve_login_selection("6", &providers).map(|provider| provider.id),
        Some("openrouter")
    );
    assert_eq!(
        resolve_login_selection("7", &providers).map(|provider| provider.id),
        Some("azure")
    );
    assert_eq!(
        resolve_login_selection("17", &providers).map(|provider| provider.id),
        Some("gemini")
    );
    assert_eq!(
        resolve_login_selection("18", &providers).map(|provider| provider.id),
        Some("google")
    );
}

#[test]
fn matrix_openrouter_like_sources_include_all_static_profiles() {
    let _lock = crate::storage::lock_test_env();
    let guard = EnvGuard::save(&[
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENAI_COMPAT_API_KEY_NAME",
        "JCODE_OPENAI_COMPAT_ENV_FILE",
    ]);
    crate::env::remove_var("JCODE_OPENROUTER_API_KEY_NAME");
    crate::env::remove_var("JCODE_OPENROUTER_ENV_FILE");
    crate::env::remove_var("JCODE_OPENAI_COMPAT_API_KEY_NAME");
    crate::env::remove_var("JCODE_OPENAI_COMPAT_ENV_FILE");

    let sources = openrouter_like_api_key_sources();
    drop(guard);

    assert!(sources.contains(&(
        "OPENROUTER_API_KEY".to_string(),
        "openrouter.env".to_string()
    )));
    for profile in openai_compatible_profiles() {
        if profile.requires_api_key {
            assert!(sources.contains(&(
                profile.api_key_env.to_string(),
                profile.env_file.to_string()
            )));
        }
    }
}

#[test]
fn matrix_openrouter_like_sources_accept_valid_overrides() {
    let _lock = crate::storage::lock_test_env();
    let _guard = EnvGuard::save(&[
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENAI_COMPAT_API_KEY_NAME",
        "JCODE_OPENAI_COMPAT_ENV_FILE",
    ]);

    crate::env::set_var("JCODE_OPENROUTER_API_KEY_NAME", "ALT_OPENROUTER_KEY");
    crate::env::set_var("JCODE_OPENROUTER_ENV_FILE", "alt-openrouter.env");
    crate::env::set_var("JCODE_OPENAI_COMPAT_API_KEY_NAME", "ALT_COMPAT_KEY");
    crate::env::set_var("JCODE_OPENAI_COMPAT_ENV_FILE", "alt-compat.env");

    let sources = openrouter_like_api_key_sources();
    assert!(sources.contains(&(
        "ALT_OPENROUTER_KEY".to_string(),
        "alt-openrouter.env".to_string()
    )));
    assert!(sources.contains(&("ALT_COMPAT_KEY".to_string(), "alt-compat.env".to_string())));
}

#[test]
fn named_provider_config_accepts_openai_compatible_spelling() {
    let cfg: crate::config::Config = toml::from_str(
        r#"
        [providers.my-gateway]
        type = "openai-compatible"
        base_url = "https://llm.example.com/v1"
        auth = "bearer"
        api_key_env = "MY_GATEWAY_API_KEY"
        default_model = "opaque/model@id"

        [[providers.my-gateway.models]]
        id = "opaque/model@id"
        input = ["text"]
        "#,
    )
    .expect("config should parse");

    let profile = cfg.providers.get("my-gateway").expect("profile");
    assert_eq!(
        profile.provider_type,
        crate::config::NamedProviderType::OpenAiCompatible
    );
    assert_eq!(profile.base_url, "https://llm.example.com/v1");
    assert_eq!(profile.default_model.as_deref(), Some("opaque/model@id"));
    assert_eq!(profile.models[0].id, "opaque/model@id");
}

#[test]
fn named_provider_profile_maps_to_openai_compatible_runtime_env() {
    let _lock = crate::storage::lock_test_env();
    let _guard = EnvGuard::save(&[
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_OPENROUTER_PROVIDER_FEATURES",
        "JCODE_OPENROUTER_ALLOW_NO_AUTH",
        "JCODE_OPENROUTER_MODEL_CATALOG",
        "JCODE_OPENROUTER_MODEL",
        "JCODE_OPENROUTER_STATIC_MODELS",
        "JCODE_OPENROUTER_AUTH_HEADER",
        "JCODE_OPENROUTER_AUTH_HEADER_NAME",
        "JCODE_NAMED_PROVIDER_PROFILE",
        "MY_GATEWAY_API_KEY",
    ]);

    let cfg: crate::config::Config = toml::from_str(
        r#"
        [providers.my-gateway]
        type = "openai-compatible"
        base_url = "https://llm.example.com/v1/"
        auth = "header"
        auth_header = "x-api-key"
        api_key_env = "MY_GATEWAY_API_KEY"
        default_model = "opaque/model@id"
        model_catalog = false

        [[providers.my-gateway.models]]
        id = "opaque/model@id"

        [[providers.my-gateway.models]]
        id = "another-local-id"
        "#,
    )
    .expect("config should parse");

    apply_named_provider_profile_env_from_config("my-gateway", &cfg).expect("apply profile");

    assert_eq!(
        std::env::var("JCODE_OPENROUTER_API_BASE").ok().as_deref(),
        Some("https://llm.example.com/v1")
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_API_KEY_NAME")
            .ok()
            .as_deref(),
        Some("MY_GATEWAY_API_KEY")
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_PROVIDER_FEATURES")
            .ok()
            .as_deref(),
        Some("0")
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_MODEL_CATALOG")
            .ok()
            .as_deref(),
        Some("0")
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_MODEL").ok().as_deref(),
        Some("opaque/model@id")
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_STATIC_MODELS")
            .ok()
            .as_deref(),
        Some("opaque/model@id\nanother-local-id")
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_AUTH_HEADER")
            .ok()
            .as_deref(),
        Some("api-key")
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_AUTH_HEADER_NAME")
            .ok()
            .as_deref(),
        Some("x-api-key")
    );
    assert_eq!(
        std::env::var("JCODE_NAMED_PROVIDER_PROFILE")
            .ok()
            .as_deref(),
        Some("my-gateway")
    );
}

#[test]
fn named_provider_inline_api_key_is_private_runtime_fallback() {
    let _lock = crate::storage::lock_test_env();
    let _guard = EnvGuard::save(&[
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_OPENROUTER_PROVIDER_FEATURES",
        "JCODE_OPENROUTER_MODEL_CATALOG",
        "JCODE_NAMED_PROVIDER_PROFILE",
        "JCODE_PROVIDER_MY_GATEWAY_API_KEY",
    ]);

    let cfg: crate::config::Config = toml::from_str(
        r#"
        [providers.my-gateway]
        type = "openai-compatible"
        base_url = "https://llm.example.com/v1"
        api_key = "inline-secret"
        "#,
    )
    .expect("config should parse");

    apply_named_provider_profile_env_from_config("my-gateway", &cfg).expect("apply profile");

    assert_eq!(
        std::env::var("JCODE_OPENROUTER_API_KEY_NAME")
            .ok()
            .as_deref(),
        Some("JCODE_PROVIDER_MY_GATEWAY_API_KEY")
    );
    assert_eq!(
        std::env::var("JCODE_PROVIDER_MY_GATEWAY_API_KEY")
            .ok()
            .as_deref(),
        Some("inline-secret")
    );
}

#[test]
fn matrix_openrouter_like_sources_reject_invalid_overrides() {
    let _lock = crate::storage::lock_test_env();
    let _guard = EnvGuard::save(&[
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENAI_COMPAT_API_KEY_NAME",
        "JCODE_OPENAI_COMPAT_ENV_FILE",
    ]);

    crate::env::set_var("JCODE_OPENROUTER_API_KEY_NAME", "bad-key-name");
    crate::env::set_var("JCODE_OPENROUTER_ENV_FILE", "../bad.env");
    crate::env::set_var("JCODE_OPENAI_COMPAT_API_KEY_NAME", "bad key");
    crate::env::set_var("JCODE_OPENAI_COMPAT_ENV_FILE", "../bad-compat.env");

    let sources = openrouter_like_api_key_sources();
    assert!(
        !sources
            .iter()
            .any(|(key, _)| key == "bad-key-name" || key == "bad key")
    );
    assert!(
        !sources
            .iter()
            .any(|(_, file)| file == "../bad.env" || file == "../bad-compat.env")
    );
}

#[test]
fn matrix_openai_compatible_profile_overrides_apply_when_valid() {
    let _lock = crate::storage::lock_test_env();
    let _guard = EnvGuard::save(&[
        "JCODE_OPENAI_COMPAT_API_BASE",
        "JCODE_OPENAI_COMPAT_API_KEY_NAME",
        "JCODE_OPENAI_COMPAT_ENV_FILE",
        "JCODE_OPENAI_COMPAT_DEFAULT_MODEL",
    ]);

    crate::env::set_var(
        "JCODE_OPENAI_COMPAT_API_BASE",
        "https://api.groq.com/openai/v1/",
    );
    crate::env::set_var("JCODE_OPENAI_COMPAT_API_KEY_NAME", "GROQ_API_KEY");
    crate::env::set_var("JCODE_OPENAI_COMPAT_ENV_FILE", "groq.env");
    crate::env::set_var("JCODE_OPENAI_COMPAT_DEFAULT_MODEL", "openai/gpt-oss-120b");

    let resolved = resolve_openai_compatible_profile(OPENAI_COMPAT_PROFILE);
    assert_eq!(resolved.api_base, "https://api.groq.com/openai/v1");
    assert_eq!(resolved.api_key_env, "GROQ_API_KEY");
    assert_eq!(resolved.env_file, "groq.env");
    assert_eq!(
        resolved.default_model.as_deref(),
        Some("openai/gpt-oss-120b")
    );
}

#[test]
fn matrix_openai_compatible_profile_overrides_reject_invalid_values() {
    let _lock = crate::storage::lock_test_env();
    let _guard = EnvGuard::save(&[
        "JCODE_OPENAI_COMPAT_API_BASE",
        "JCODE_OPENAI_COMPAT_API_KEY_NAME",
        "JCODE_OPENAI_COMPAT_ENV_FILE",
    ]);

    crate::env::set_var("JCODE_OPENAI_COMPAT_API_BASE", "http://example.com/v1");
    crate::env::set_var("JCODE_OPENAI_COMPAT_API_KEY_NAME", "bad-key-name");
    crate::env::set_var("JCODE_OPENAI_COMPAT_ENV_FILE", "../bad.env");

    let resolved = resolve_openai_compatible_profile(OPENAI_COMPAT_PROFILE);
    assert_eq!(resolved.api_base, OPENAI_COMPAT_PROFILE.api_base);
    assert_eq!(resolved.api_key_env, OPENAI_COMPAT_PROFILE.api_key_env);
    assert_eq!(resolved.env_file, OPENAI_COMPAT_PROFILE.env_file);
}

#[test]
fn matrix_openai_compatible_profile_overrides_read_from_env_file() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let config_root = temp.path().join("config").join("jcode");
    std::fs::create_dir_all(&config_root).expect("config dir");

    let _guard = EnvGuard::save(&[
        "JCODE_HOME",
        "JCODE_OPENAI_COMPAT_API_BASE",
        "JCODE_OPENAI_COMPAT_API_KEY_NAME",
        "JCODE_OPENAI_COMPAT_ENV_FILE",
        "JCODE_OPENAI_COMPAT_DEFAULT_MODEL",
    ]);
    crate::env::set_var("JCODE_HOME", temp.path());
    crate::env::remove_var("JCODE_OPENAI_COMPAT_API_BASE");
    crate::env::remove_var("JCODE_OPENAI_COMPAT_API_KEY_NAME");
    crate::env::remove_var("JCODE_OPENAI_COMPAT_ENV_FILE");
    crate::env::remove_var("JCODE_OPENAI_COMPAT_DEFAULT_MODEL");
    std::fs::write(
        config_root.join(OPENAI_COMPAT_PROFILE.env_file),
        concat!(
            "JCODE_OPENAI_COMPAT_API_BASE=https://api.example.com/v1\n",
            "JCODE_OPENAI_COMPAT_API_KEY_NAME=EXAMPLE_API_KEY\n",
            "JCODE_OPENAI_COMPAT_ENV_FILE=example.env\n",
            "JCODE_OPENAI_COMPAT_DEFAULT_MODEL=example/model\n",
        ),
    )
    .expect("env file");

    let resolved = resolve_openai_compatible_profile(OPENAI_COMPAT_PROFILE);
    assert_eq!(resolved.api_base, "https://api.example.com/v1");
    assert_eq!(resolved.api_key_env, "EXAMPLE_API_KEY");
    assert_eq!(resolved.env_file, "example.env");
    assert_eq!(resolved.default_model.as_deref(), Some("example/model"));
}

#[test]
fn matrix_openai_compatible_localhost_override_allows_no_auth() {
    let _lock = crate::storage::lock_test_env();
    let _guard = EnvGuard::save(&[
        "JCODE_OPENAI_COMPAT_API_BASE",
        "JCODE_OPENAI_COMPAT_API_KEY_NAME",
        "JCODE_OPENAI_COMPAT_ENV_FILE",
        "JCODE_OPENAI_COMPAT_LOCAL_ENABLED",
    ]);

    crate::env::set_var("JCODE_OPENAI_COMPAT_API_BASE", "http://localhost:11434/v1");
    crate::env::remove_var("JCODE_OPENAI_COMPAT_API_KEY_NAME");
    crate::env::remove_var("JCODE_OPENAI_COMPAT_ENV_FILE");
    crate::env::remove_var("JCODE_OPENAI_COMPAT_LOCAL_ENABLED");

    let resolved = resolve_openai_compatible_profile(OPENAI_COMPAT_PROFILE);
    assert_eq!(resolved.api_base, "http://localhost:11434/v1");
    assert!(!resolved.requires_api_key);
    assert!(openai_compatible_profile_is_configured(
        OPENAI_COMPAT_PROFILE
    ));
}

#[test]
fn matrix_load_api_key_from_env_or_config_prefers_env() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let config_root = temp.path().join("config").join("jcode");
    std::fs::create_dir_all(&config_root).expect("config dir");

    let _guard = EnvGuard::save(&["JCODE_HOME", "OPENCODE_API_KEY"]);
    crate::env::set_var("JCODE_HOME", temp.path());
    crate::env::set_var("OPENCODE_API_KEY", "env-secret");
    std::fs::write(
        config_root.join("opencode.env"),
        "OPENCODE_API_KEY=file-secret\n",
    )
    .expect("env file");

    assert_eq!(
        load_api_key_from_env_or_config("OPENCODE_API_KEY", "opencode.env").as_deref(),
        Some("env-secret")
    );
}

#[test]
fn matrix_load_api_key_from_env_or_config_reads_config_file() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let config_root = temp.path().join("config").join("jcode");
    std::fs::create_dir_all(&config_root).expect("config dir");

    let _guard = EnvGuard::save(&["JCODE_HOME", "OPENCODE_API_KEY"]);
    crate::env::set_var("JCODE_HOME", temp.path());
    crate::env::remove_var("OPENCODE_API_KEY");
    std::fs::write(
        config_root.join("opencode.env"),
        "OPENCODE_API_KEY=file-secret\n",
    )
    .expect("env file");

    assert_eq!(
        load_api_key_from_env_or_config("OPENCODE_API_KEY", "opencode.env").as_deref(),
        Some("file-secret")
    );
}

#[test]
fn load_api_key_accepts_legacy_zai_key_name() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let config_root = temp.path().join("config").join("jcode");
    std::fs::create_dir_all(&config_root).expect("config dir");

    let _guard = EnvGuard::save(&["JCODE_HOME", "ZHIPU_API_KEY", "ZAI_API_KEY"]);
    crate::env::set_var("JCODE_HOME", temp.path());
    crate::env::remove_var("ZHIPU_API_KEY");
    crate::env::remove_var("ZAI_API_KEY");
    std::fs::write(config_root.join("zai.env"), "ZAI_API_KEY=legacy-secret\n").expect("env file");

    assert_eq!(
        load_api_key_from_env_or_config("ZHIPU_API_KEY", "zai.env").as_deref(),
        Some("legacy-secret")
    );
}
