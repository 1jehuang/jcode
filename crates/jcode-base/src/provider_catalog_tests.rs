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
    assert_eq!(MINIMAX_PROFILE.api_base, "https://api.minimax.io/v1");
    assert_eq!(MINIMAX_PROFILE.api_key_env, "OPENAI_API_KEY");
    assert_eq!(
        ALIBABA_CODING_PLAN_PROFILE.api_base,
        "https://coding-intl.dashscope.aliyuncs.com/v1"
    );
    assert_eq!(COMTEGRA_PROFILE.api_base, "https://llm.comtegra.cloud/v1");
    assert_eq!(COMTEGRA_PROFILE.default_model, Some("glm-51-nvfp4"));
    assert_eq!(COMTEGRA_PROFILE.api_key_env, "COMTEGRA_API_KEY");
    assert_eq!(CEREBRAS_PROFILE.api_base, "https://api.cerebras.ai/v1");
    assert_eq!(CEREBRAS_PROFILE.default_model, Some("gpt-oss-120b"));
    assert!(!OPENAI_COMPAT_PROFILE.setup_url.contains("opencode.ai"));
}

#[test]
fn resolved_named_profile_suggests_newest_cached_live_release() {
    let _lock = crate::storage::lock_test_env();
    let _guard = EnvGuard::save(&["JCODE_HOME"]);
    let temp = tempfile::tempdir().expect("tempdir");
    crate::env::set_var("JCODE_HOME", temp.path());
    jcode_provider_openrouter::save_disk_cache_with_source_for_namespace(
        "cerebras",
        &[
            jcode_provider_openrouter::ModelInfo {
                id: "older-model".to_string(),
                name: String::new(),
                context_length: None,
                pricing: Default::default(),
                created: Some(1_700_000_000),
            },
            jcode_provider_openrouter::ModelInfo {
                id: "newer-model".to_string(),
                name: String::new(),
                context_length: None,
                pricing: Default::default(),
                created: Some(1_800_000_000),
            },
        ],
        Some(CEREBRAS_PROFILE.api_base),
    );

    let resolved = resolve_openai_compatible_profile(CEREBRAS_PROFILE);

    assert_eq!(resolved.default_model.as_deref(), Some("newer-model"));
}

#[test]
fn resolved_named_profile_skips_non_chat_models_when_picking_newest_default() {
    // Regression: a profile's auto-selected default must never be a non-chat
    // model (TTS/speech/embeddings/image/etc.). Catalogs such as Groq expose
    // their entire model list, and the newest-released entry is frequently a
    // non-chat model (e.g. `canopylabs/orpheus-*` TTS) which previously won the
    // newest-by-created tiebreak and became the chat default.
    let _lock = crate::storage::lock_test_env();
    let _guard = EnvGuard::save(&["JCODE_HOME"]);
    let temp = tempfile::tempdir().expect("tempdir");
    crate::env::set_var("JCODE_HOME", temp.path());
    jcode_provider_openrouter::save_disk_cache_with_source_for_namespace(
        "cerebras",
        &[
            jcode_provider_openrouter::ModelInfo {
                id: "older-chat-model".to_string(),
                name: String::new(),
                context_length: None,
                pricing: Default::default(),
                created: Some(1_700_000_000),
            },
            jcode_provider_openrouter::ModelInfo {
                id: "newer-chat-model".to_string(),
                name: String::new(),
                context_length: None,
                pricing: Default::default(),
                created: Some(1_800_000_000),
            },
            // Newest of all, but a non-chat (TTS) model that must be skipped.
            jcode_provider_openrouter::ModelInfo {
                id: "canopylabs/orpheus-v1-english".to_string(),
                name: String::new(),
                context_length: None,
                pricing: Default::default(),
                created: Some(1_900_000_000),
            },
            jcode_provider_openrouter::ModelInfo {
                id: "whisper-large-v3".to_string(),
                name: String::new(),
                context_length: None,
                pricing: Default::default(),
                created: Some(1_950_000_000),
            },
        ],
        Some(CEREBRAS_PROFILE.api_base),
    );

    let resolved = resolve_openai_compatible_profile(CEREBRAS_PROFILE);

    assert_eq!(
        resolved.default_model.as_deref(),
        Some("newer-chat-model"),
        "newest *chat* model must win; non-chat models (orpheus TTS, whisper STT) are skipped"
    );
}

#[test]
fn minimax_token_plan_keys_resolve_to_china_endpoint_without_changing_international_default() {
    let _lock = crate::storage::lock_test_env();
    let _guard = EnvGuard::save(&["OPENAI_API_KEY"]);
    crate::env::remove_var("OPENAI_API_KEY");

    let international = resolve_openai_compatible_profile(MINIMAX_PROFILE);
    assert_eq!(international.api_base, "https://api.minimax.io/v1");
    assert_eq!(
        international.setup_url,
        "https://platform.minimax.io/docs/guides/text-generation"
    );

    let china = resolve_openai_compatible_profile_with_api_key_hint(
        MINIMAX_PROFILE,
        Some("sk-cp-test-token"),
    );
    assert_eq!(china.api_base, MINIMAX_CHINA_API_BASE);
    assert_eq!(china.setup_url, MINIMAX_CHINA_SETUP_URL);
}

#[test]
fn auth_issue_lan_openai_compatible_bases_are_valid_for_local_model_servers() {
    assert_eq!(
        normalize_api_base("http://100.103.78.84:11434/v1").as_deref(),
        Some("http://100.103.78.84:11434/v1")
    );
    assert_eq!(
        normalize_api_base("http://hsv.local:11434/v1").as_deref(),
        Some("http://hsv.local:11434/v1")
    );
    assert_eq!(normalize_api_base("http://example.com/v1"), None);
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
        "JCODE_OPENROUTER_TRANSPORT_STATE",
        "JCODE_OPENROUTER_ALLOW_NO_AUTH",
        "JCODE_RUNTIME_PROVIDER",
        "JCODE_NAMED_PROVIDER_PROFILE",
        "JCODE_PROVIDER_PROFILE_ACTIVE",
    ]);

    crate::env::set_var("JCODE_RUNTIME_PROVIDER", "azure-openai");
    assert_eq!(runtime_provider_display_name("openrouter"), "Azure OpenAI");
    crate::env::remove_var("JCODE_RUNTIME_PROVIDER");

    apply_openai_compatible_profile_env(Some(DEEPSEEK_PROFILE));
    assert_eq!(runtime_provider_display_name("openrouter"), "DeepSeek");

    apply_openai_compatible_profile_env(Some(ZAI_PROFILE));
    assert_eq!(runtime_provider_display_name("openrouter"), "Z.AI");
}

#[test]
fn auth_profile_env_application_flushes_stale_openrouter_catalog_state() {
    let _lock = crate::storage::lock_test_env();
    let _guard = EnvGuard::save(&[
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_OPENROUTER_PROVIDER_FEATURES",
        "JCODE_OPENROUTER_TRANSPORT_STATE",
        "JCODE_OPENROUTER_ALLOW_NO_AUTH",
        "JCODE_OPENROUTER_MODEL_CATALOG",
        "JCODE_OPENROUTER_MODEL",
        "JCODE_OPENROUTER_STATIC_MODELS",
        "JCODE_OPENROUTER_AUTH_HEADER",
        "JCODE_OPENROUTER_AUTH_HEADER_NAME",
        "JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER",
        "JCODE_OPENROUTER_PROVIDER",
        "JCODE_OPENROUTER_NO_FALLBACK",
        "JCODE_NAMED_PROVIDER_PROFILE",
        "JCODE_PROVIDER_PROFILE_ACTIVE",
        "JCODE_PROVIDER_PROFILE_NAME",
    ]);

    crate::env::set_var("JCODE_OPENROUTER_API_BASE", "https://openrouter.ai/api/v1");
    crate::env::set_var("JCODE_OPENROUTER_API_KEY_NAME", "OPENROUTER_API_KEY");
    crate::env::set_var("JCODE_OPENROUTER_ENV_FILE", "openrouter.env");
    crate::env::set_var("JCODE_OPENROUTER_CACHE_NAMESPACE", "openrouter");
    crate::env::set_var("JCODE_OPENROUTER_PROVIDER_FEATURES", "1");
    crate::env::set_var("JCODE_OPENROUTER_TRANSPORT_STATE", "stale");
    crate::env::set_var("JCODE_OPENROUTER_ALLOW_NO_AUTH", "1");
    crate::env::set_var(
        "JCODE_OPENROUTER_MODEL_CATALOG",
        "stale-openrouter-catalog.json",
    );
    crate::env::set_var("JCODE_OPENROUTER_MODEL", "gpt-5.5");
    crate::env::set_var(
        "JCODE_OPENROUTER_STATIC_MODELS",
        "stale-openrouter-only-model",
    );
    crate::env::set_var("JCODE_OPENROUTER_AUTH_HEADER", "Bearer stale");
    crate::env::set_var("JCODE_OPENROUTER_AUTH_HEADER_NAME", "Authorization");
    crate::env::set_var("JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER", "openrouter");
    crate::env::set_var("JCODE_OPENROUTER_PROVIDER", "openrouter");
    crate::env::set_var("JCODE_OPENROUTER_NO_FALLBACK", "1");
    crate::env::set_var("JCODE_NAMED_PROVIDER_PROFILE", "openrouter");
    crate::env::set_var("JCODE_PROVIDER_PROFILE_ACTIVE", "1");
    crate::env::set_var("JCODE_PROVIDER_PROFILE_NAME", "openrouter");

    force_apply_openai_compatible_profile_env(Some(CEREBRAS_PROFILE));

    assert_eq!(
        std::env::var("JCODE_OPENROUTER_API_BASE").as_deref(),
        Ok("https://api.cerebras.ai/v1")
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_API_KEY_NAME").as_deref(),
        Ok("CEREBRAS_API_KEY")
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_ENV_FILE").as_deref(),
        Ok("cerebras.env")
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_CACHE_NAMESPACE").as_deref(),
        Ok("cerebras")
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_PROVIDER_FEATURES").as_deref(),
        Ok("0")
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_TRANSPORT_STATE").as_deref(),
        Ok("direct-api-key")
    );
    assert!(std::env::var_os("JCODE_OPENROUTER_ALLOW_NO_AUTH").is_none());
    assert!(std::env::var_os("JCODE_OPENROUTER_MODEL_CATALOG").is_none());
    assert!(std::env::var_os("JCODE_OPENROUTER_MODEL").is_none());
    assert!(std::env::var_os("JCODE_OPENROUTER_AUTH_HEADER").is_none());
    assert!(std::env::var_os("JCODE_OPENROUTER_AUTH_HEADER_NAME").is_none());
    assert!(std::env::var_os("JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER").is_none());
    assert!(std::env::var_os("JCODE_OPENROUTER_PROVIDER").is_none());
    assert!(std::env::var_os("JCODE_OPENROUTER_NO_FALLBACK").is_none());
    assert!(std::env::var_os("JCODE_NAMED_PROVIDER_PROFILE").is_none());
    assert!(std::env::var_os("JCODE_PROVIDER_PROFILE_ACTIVE").is_none());
    assert!(std::env::var_os("JCODE_PROVIDER_PROFILE_NAME").is_none());
    assert_ne!(
        std::env::var("JCODE_OPENROUTER_STATIC_MODELS")
            .ok()
            .as_deref(),
        Some("stale-openrouter-only-model")
    );
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
    // `anthropic-api` sits at 3 (between claude and openai), shifting the
    // rest of the list down one slot relative to the pre-May-2026 order.
    assert_eq!(
        resolve_login_selection("3", &providers).map(|provider| provider.id),
        Some("anthropic-api")
    );
    assert_eq!(
        resolve_login_selection("7", &providers).map(|provider| provider.id),
        Some("bedrock")
    );
    assert_eq!(
        resolve_login_selection("compat", &providers).map(|provider| provider.id),
        Some("openai-compatible")
    );
    assert_eq!(
        resolve_login_selection("cgc", &providers).map(|provider| provider.id),
        Some("comtegra")
    );
    assert_eq!(
        resolve_login_selection("bedrock", &providers).map(|provider| provider.id),
        Some("bedrock")
    );
    assert!(
        providers
            .iter()
            .take(7)
            .any(|provider| provider.id == "bedrock")
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
    // `anthropic-api` at 3 shifted everything after it down one slot.
    assert_eq!(
        resolve_login_selection("3", &providers).map(|provider| provider.id),
        Some("anthropic-api")
    );
    assert_eq!(
        resolve_login_selection("5", &providers).map(|provider| provider.id),
        Some("jcode")
    );
    assert_eq!(
        resolve_login_selection("6", &providers).map(|provider| provider.id),
        Some("copilot")
    );
    assert_eq!(
        resolve_login_selection("7", &providers).map(|provider| provider.id),
        Some("openrouter")
    );
    assert_eq!(
        resolve_login_selection("8", &providers).map(|provider| provider.id),
        Some("bedrock")
    );
    assert_eq!(
        resolve_login_selection("9", &providers).map(|provider| provider.id),
        Some("azure")
    );
    assert_eq!(
        resolve_login_selection("bedrock", &providers).map(|provider| provider.id),
        Some("bedrock")
    );
    assert!(
        providers
            .iter()
            .position(|provider| provider.id == "bedrock")
            < providers.iter().position(|provider| provider.id == "azure")
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
fn named_provider_profile_reports_malformed_config_instead_of_unknown_profile() {
    let _lock = crate::storage::lock_test_env();
    let previous_home = std::env::var_os("JCODE_HOME");
    let temp = tempfile::TempDir::new().expect("tempdir");
    crate::env::set_var("JCODE_HOME", temp.path());
    crate::config::Config::invalidate_cache();

    let config_path = crate::config::Config::path().expect("config path");
    std::fs::create_dir_all(config_path.parent().expect("config parent"))
        .expect("create config dir");
    std::fs::write(
        &config_path,
        r#"
        [providers.antigravity]
        type = "anthropic-compatible"
        base_url = "http://192.168.1.202:8080"
        api_key_env = "ANTIGRAVITY_API_KEY"
        default_model = "gemini-3.1-pro-low"

        [[providers.antigravity.models]]
        id = "gemini-3.1-pro-low"
        context_window = 128000
        "#,
    )
    .expect("write config");

    let err = apply_named_provider_profile_env("antigravity").expect_err("malformed config");
    let message = err.to_string();
    assert!(
        message.contains("Failed to parse config file"),
        "unexpected error: {message}"
    );
    assert!(
        message.contains("anthropic-compatible"),
        "unexpected error: {message}"
    );
    assert!(
        !message.contains("Unknown provider profile"),
        "unexpected error: {message}"
    );

    if let Some(previous_home) = previous_home {
        crate::env::set_var("JCODE_HOME", previous_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
    crate::config::Config::invalidate_cache();
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
        "JCODE_OPENROUTER_TRANSPORT_STATE",
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
        std::env::var("JCODE_OPENROUTER_TRANSPORT_STATE")
            .ok()
            .as_deref(),
        Some("direct-api-key")
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
        "JCODE_OPENROUTER_TRANSPORT_STATE",
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

#[test]
fn quality_tier_ranks_flagship_above_bare_above_cheap() {
    // Flagship-marked ids (max/pro/opus/coder/large/huge-param) -> tier 2.
    assert_eq!(openai_compatible_model_quality_tier("qwen3-max"), 2);
    assert_eq!(openai_compatible_model_quality_tier("claude-opus-4-8"), 2);
    assert_eq!(openai_compatible_model_quality_tier("qwen3-coder-480b"), 2);
    assert_eq!(openai_compatible_model_quality_tier("glm-4.6-pro"), 2);
    assert_eq!(
        openai_compatible_model_quality_tier("llama-3.1-405b-instruct"),
        2
    );

    // Bare frontier ids (no tier marker) -> tier 1.
    assert_eq!(openai_compatible_model_quality_tier("gpt-5.5"), 1);
    assert_eq!(openai_compatible_model_quality_tier("minimax-m2.7"), 1);
    assert_eq!(openai_compatible_model_quality_tier("kimi-k2.5"), 1);
    assert_eq!(openai_compatible_model_quality_tier("glm-4.6"), 1);

    // Cheap/small/fast-marked ids -> tier 0.
    assert_eq!(openai_compatible_model_quality_tier("gpt-5.5-mini"), 0);
    assert_eq!(openai_compatible_model_quality_tier("deepseek-v4-flash"), 0);
    assert_eq!(openai_compatible_model_quality_tier("glm-4.6-air"), 0);
    assert_eq!(
        openai_compatible_model_quality_tier("llama-3.1-8b-instant"),
        0
    );
    assert_eq!(openai_compatible_model_quality_tier("claude-haiku-4-5"), 0);

    // Brand names that merely *contain* a marker substring must NOT trip the
    // whole-token matcher: `minimax` is not `mini`/`max`.
    assert_eq!(openai_compatible_model_quality_tier("minimax-m2.7"), 1);

    // Flagship marker beats a co-occurring size token.
    assert_eq!(
        openai_compatible_model_quality_tier("qwen3-coder-30b-a3b"),
        2
    );
}

#[test]
fn newest_release_picker_prefers_strongest_tier_over_newest_cheap() {
    use jcode_provider_openrouter::ModelInfo;
    let _env = EnvGuard::save(&["JCODE_HOME"]);
    let temp = tempfile::tempdir().expect("tempdir");
    crate::env::set_var("JCODE_HOME", temp.path());

    let mk = |id: &str, created: u64| ModelInfo {
        id: id.to_string(),
        name: String::new(),
        context_length: None,
        pricing: Default::default(),
        created: Some(created),
    };

    // A heterogeneous proxy catalog (like OpenCode Zen): the NEWEST model is a
    // cheap `*-flash`, but a slightly older flagship-marked model exists. The
    // picker must choose the flagship, not the newest-cheap.
    jcode_provider_openrouter::save_disk_cache_with_source_for_namespace(
        "deepseek",
        &[
            mk("deepseek-v4-flash", 1_900_000_000), // newest, but cheap tier
            mk("deepseek-v4", 1_850_000_000),       // bare frontier
            mk("deepseek-v4-coder", 1_800_000_000), // flagship tier, oldest
        ],
        Some("https://api.deepseek.com"),
    );

    assert_eq!(
        newest_released_model_for_openai_compatible_profile("deepseek").as_deref(),
        Some("deepseek-v4-coder"),
        "a flagship-marked model must win over a newer cheap/flash sibling"
    );
}

#[test]
fn newest_release_picker_uses_recency_within_a_tier() {
    use jcode_provider_openrouter::ModelInfo;
    let _env = EnvGuard::save(&["JCODE_HOME"]);
    let temp = tempfile::tempdir().expect("tempdir");
    crate::env::set_var("JCODE_HOME", temp.path());

    let mk = |id: &str, created: u64| ModelInfo {
        id: id.to_string(),
        name: String::new(),
        context_length: None,
        pricing: Default::default(),
        created: Some(created),
    };

    // All same (bare frontier) tier: recency decides.
    jcode_provider_openrouter::save_disk_cache_with_source_for_namespace(
        "deepseek",
        &[
            mk("deepseek-v3", 1_700_000_000),
            mk("deepseek-v4", 1_900_000_000), // newest within the same tier
            mk("deepseek-v3.1", 1_800_000_000),
        ],
        Some("https://api.deepseek.com"),
    );

    assert_eq!(
        newest_released_model_for_openai_compatible_profile("deepseek").as_deref(),
        Some("deepseek-v4"),
        "within one quality tier the newest release should win"
    );
}

/// Exhaustiveness guard: every model shipped in a profile's static catalog must
/// resolve to a concrete context window. Open-weight gateways frequently omit
/// `context_length` from `/v1/models`, so a missing entry here means that model
/// would silently fall back to the generic 200K default. First-party
/// OpenAI/Claude/Gemini ids are resolved by their own providers (not this static
/// table) and are exempted.
#[test]
fn every_static_profile_model_has_a_known_context_limit() {
    use jcode_provider_core::models::context_limit_for_model_with_provider;

    // Ids handled by dedicated first-party providers rather than the
    // OpenAI-compatible static table.
    fn is_first_party(model: &str) -> bool {
        let m = model.to_ascii_lowercase();
        m.starts_with("claude-")
            || m.starts_with("gpt-")
            || m.starts_with("gemini-")
            || m.starts_with("o3")
            || m.starts_with("o4")
    }

    let mut missing: Vec<(String, String)> = Vec::new();
    for profile in jcode_provider_metadata::openai_compatible_profiles()
        .iter()
        .copied()
    {
        for model in openai_compatible_profile_static_models(profile) {
            if is_first_party(&model) {
                continue;
            }

            let via_profile = openai_compatible_profile_context_limit(profile.id, &model);
            let via_global = context_limit_for_model_with_provider(&model, Some("openrouter"));

            if via_profile.is_none() && via_global.is_none() {
                missing.push((profile.id.to_string(), model));
            }
        }
    }

    assert!(
        missing.is_empty(),
        "static profile models without a known context limit (would fall back to the \
         generic default); add them to open_weight_family_context_limit: {missing:?}"
    );
}

#[test]
fn open_weight_family_context_limits_match_published_windows() {
    use jcode_provider_core::models::open_weight_family_context_limit as f;

    // GLM family spelling variants across gateways.
    assert_eq!(f("glm-4.5"), Some(128_000));
    assert_eq!(f("glm-4.7"), Some(200_000));
    assert_eq!(f("zai-org/glm-4.7"), Some(200_000));
    assert_eq!(f("accounts/fireworks/models/glm-4p7"), Some(200_000));
    assert_eq!(f("glm-5"), Some(200_000));
    assert_eq!(f("glm-5.1"), Some(200_000));
    assert_eq!(f("zai-glm-5-1"), Some(200_000));
    assert_eq!(f("glm-5.2"), Some(1_000_000));

    // Other open-weight families.
    assert_eq!(f("kimi-k2.5"), Some(262_144));
    assert_eq!(f("minimax-m2.7"), Some(204_800));
    assert_eq!(f("mimo-v2.5"), Some(262_144));
    assert_eq!(f("deepseek-v3.2"), Some(163_840));
    assert_eq!(f("deepseek-v4-pro"), Some(1_000_000));
    assert_eq!(f("qwen3-235b-a22b-instruct-2507"), Some(262_144));
    assert_eq!(f("gpt-oss-120b"), Some(131_072));
    assert_eq!(f("llama-3.3-70b-instruct"), Some(131_072));
    assert_eq!(f("sonar-pro"), Some(128_000));

    // Unknown families stay unresolved so the dynamic cache/default can act.
    assert_eq!(f("some-unknown-model"), None);
}

#[test]
fn minimax_default_provider_applies_openai_api_key_env_not_openrouter() {
    // Regression for #407: `default_provider = "minimax"` (the built-in MiniMax
    // profile) must resolve credentials from the profile's documented
    // OPENAI_API_KEY / minimax.env, not the generic OPENROUTER_API_KEY /
    // openrouter.env. The earlier bug surfaced as
    // "OPENROUTER_API_KEY not found ..." when applying the configured
    // default_model.
    let _lock = crate::storage::lock_test_env();
    let _guard = EnvGuard::save(&[
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_PROVIDER_PROFILE_ACTIVE",
        "JCODE_NAMED_PROVIDER_PROFILE",
    ]);
    for v in [
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_PROVIDER_PROFILE_ACTIVE",
        "JCODE_NAMED_PROVIDER_PROFILE",
    ] {
        crate::env::remove_var(v);
    }

    let selection = resolve_openai_compatible_profile_selection("minimax");
    assert_eq!(
        selection.map(|profile| profile.id),
        Some("minimax"),
        "default_provider=minimax must resolve the built-in MiniMax profile"
    );

    apply_openai_compatible_profile_env(selection);

    assert_eq!(
        std::env::var("JCODE_OPENROUTER_API_KEY_NAME")
            .ok()
            .as_deref(),
        Some("OPENAI_API_KEY"),
        "MiniMax profile must use OPENAI_API_KEY, not OPENROUTER_API_KEY"
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_ENV_FILE").ok().as_deref(),
        Some("minimax.env"),
        "MiniMax profile must use minimax.env, not openrouter.env"
    );
}
