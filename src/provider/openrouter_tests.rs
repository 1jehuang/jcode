use super::openrouter_sse_stream::OpenRouterStream;
use super::*;
use std::ffi::OsString;
use std::sync::Mutex;
use tempfile::TempDir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, previous }
    }

    fn remove(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        crate::env::remove_var(key);
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            crate::env::set_var(self.key, previous);
        } else {
            crate::env::remove_var(self.key);
        }
    }
}

fn test_config_dir(temp: &TempDir) -> std::path::PathBuf {
    #[cfg(target_os = "macos")]
    {
        temp.path().join("Library").join("Application Support")
    }
    #[cfg(target_os = "windows")]
    {
        temp.path().join("AppData").join("Roaming")
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        temp.path().to_path_buf()
    }
}

fn write_test_api_key(temp: &TempDir, env_file: &str, env_key: &str, value: &str) {
    let config_dir = test_config_dir(temp).join("jcode");
    std::fs::create_dir_all(&config_dir).expect("create test config dir");
    std::fs::write(config_dir.join(env_file), format!("{env_key}={value}\n"))
        .expect("write test api key");
}

#[test]
fn test_has_credentials() {
    let _has_creds = OpenRouterProvider::has_credentials();
}

#[test]
fn test_configured_api_base_accepts_https() {
    let _lock = ENV_LOCK.lock().unwrap();
    let prev = std::env::var("JCODE_OPENROUTER_API_BASE").ok();
    crate::env::set_var(
        "JCODE_OPENROUTER_API_BASE",
        "https://api.groq.com/openai/v1/",
    );
    assert_eq!(configured_api_base(), "https://api.groq.com/openai/v1");
    if let Some(value) = prev {
        crate::env::set_var("JCODE_OPENROUTER_API_BASE", value);
    } else {
        crate::env::remove_var("JCODE_OPENROUTER_API_BASE");
    }
}

#[test]
fn test_configured_api_base_rejects_insecure_http_remote() {
    let _lock = ENV_LOCK.lock().unwrap();
    let prev = std::env::var("JCODE_OPENROUTER_API_BASE").ok();
    crate::env::set_var("JCODE_OPENROUTER_API_BASE", "http://example.com/v1");
    assert_eq!(configured_api_base(), DEFAULT_API_BASE);
    if let Some(value) = prev {
        crate::env::set_var("JCODE_OPENROUTER_API_BASE", value);
    } else {
        crate::env::remove_var("JCODE_OPENROUTER_API_BASE");
    }
}

#[test]
fn autodetects_single_saved_openai_compatible_profile() {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp = TempDir::new().expect("create temp dir");
    let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path());
    let _home = EnvVarGuard::set("HOME", temp.path());
    let _appdata = EnvVarGuard::set("APPDATA", temp.path().join("AppData").join("Roaming"));
    let _openrouter_base = EnvVarGuard::remove("JCODE_OPENROUTER_API_BASE");
    let _openrouter_key = EnvVarGuard::remove("JCODE_OPENROUTER_API_KEY_NAME");
    let _openrouter_file = EnvVarGuard::remove("JCODE_OPENROUTER_ENV_FILE");
    let _openrouter_dynamic = EnvVarGuard::remove("JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER");
    let _openrouter_api_key = EnvVarGuard::remove("OPENROUTER_API_KEY");
    let _opencode_api_key = EnvVarGuard::remove("OPENCODE_API_KEY");

    let opencode = crate::provider_catalog::resolve_openai_compatible_profile(
        crate::provider_catalog::OPENCODE_PROFILE,
    );
    write_test_api_key(
        &temp,
        &opencode.env_file,
        &opencode.api_key_env,
        "test-opencode-key",
    );

    assert_eq!(configured_api_base(), opencode.api_base);
    assert_eq!(configured_api_key_name(), opencode.api_key_env);
    assert_eq!(configured_env_file_name(), opencode.env_file);
    assert!(OpenRouterProvider::has_credentials());
}

#[test]
fn autodetects_single_saved_local_openai_compatible_profile() {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp = TempDir::new().expect("create temp dir");
    let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path());
    let _home = EnvVarGuard::set("HOME", temp.path());
    let _appdata = EnvVarGuard::set("APPDATA", temp.path().join("AppData").join("Roaming"));
    let _openrouter_base = EnvVarGuard::remove("JCODE_OPENROUTER_API_BASE");
    let _openrouter_key = EnvVarGuard::remove("JCODE_OPENROUTER_API_KEY_NAME");
    let _openrouter_file = EnvVarGuard::remove("JCODE_OPENROUTER_ENV_FILE");
    let _openrouter_dynamic = EnvVarGuard::remove("JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER");
    let _openrouter_no_auth = EnvVarGuard::remove("JCODE_OPENROUTER_ALLOW_NO_AUTH");
    let _openrouter_api_key = EnvVarGuard::remove("OPENROUTER_API_KEY");
    let _lmstudio_api_key = EnvVarGuard::remove("LMSTUDIO_API_KEY");

    let lmstudio = crate::provider_catalog::resolve_openai_compatible_profile(
        crate::provider_catalog::LMSTUDIO_PROFILE,
    );
    let config_dir = test_config_dir(&temp).join("jcode");
    std::fs::create_dir_all(&config_dir).expect("create test config dir");
    std::fs::write(
        config_dir.join(&lmstudio.env_file),
        format!(
            "{}=1\n",
            crate::provider_catalog::OPENAI_COMPAT_LOCAL_ENABLED_ENV
        ),
    )
    .expect("write local config");

    assert_eq!(configured_api_base(), lmstudio.api_base);
    assert_eq!(configured_api_key_name(), lmstudio.api_key_env);
    assert_eq!(configured_env_file_name(), lmstudio.env_file);
    assert!(configured_allow_no_auth());
    assert!(OpenRouterProvider::has_credentials());
}

#[test]
fn does_not_guess_when_multiple_saved_openai_compatible_profiles_exist() {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp = TempDir::new().expect("create temp dir");
    let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path());
    let _home = EnvVarGuard::set("HOME", temp.path());
    let _appdata = EnvVarGuard::set("APPDATA", temp.path().join("AppData").join("Roaming"));
    let _openrouter_base = EnvVarGuard::remove("JCODE_OPENROUTER_API_BASE");
    let _openrouter_key = EnvVarGuard::remove("JCODE_OPENROUTER_API_KEY_NAME");
    let _openrouter_file = EnvVarGuard::remove("JCODE_OPENROUTER_ENV_FILE");
    let _openrouter_dynamic = EnvVarGuard::remove("JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER");
    let _openrouter_api_key = EnvVarGuard::remove("OPENROUTER_API_KEY");
    let _opencode_api_key = EnvVarGuard::remove("OPENCODE_API_KEY");
    let _chutes_api_key = EnvVarGuard::remove("CHUTES_API_KEY");

    let opencode = crate::provider_catalog::resolve_openai_compatible_profile(
        crate::provider_catalog::OPENCODE_PROFILE,
    );
    let chutes = crate::provider_catalog::resolve_openai_compatible_profile(
        crate::provider_catalog::CHUTES_PROFILE,
    );
    write_test_api_key(
        &temp,
        &opencode.env_file,
        &opencode.api_key_env,
        "test-opencode-key",
    );
    write_test_api_key(
        &temp,
        &chutes.env_file,
        &chutes.api_key_env,
        "test-chutes-key",
    );

    assert_eq!(configured_api_base(), DEFAULT_API_BASE);
    assert_eq!(configured_api_key_name(), DEFAULT_API_KEY_NAME);
    assert_eq!(configured_env_file_name(), DEFAULT_ENV_FILE);
    assert!(!OpenRouterProvider::has_credentials());
}

#[test]
fn autodetected_profile_seeds_default_model_and_cache_namespace() {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp = TempDir::new().expect("create temp dir");
    let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path());
    let _home = EnvVarGuard::set("HOME", temp.path());
    let _appdata = EnvVarGuard::set("APPDATA", temp.path().join("AppData").join("Roaming"));
    let _openrouter_base = EnvVarGuard::remove("JCODE_OPENROUTER_API_BASE");
    let _openrouter_key = EnvVarGuard::remove("JCODE_OPENROUTER_API_KEY_NAME");
    let _openrouter_file = EnvVarGuard::remove("JCODE_OPENROUTER_ENV_FILE");
    let _openrouter_dynamic = EnvVarGuard::remove("JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER");
    let _openrouter_model = EnvVarGuard::remove("JCODE_OPENROUTER_MODEL");
    let _openrouter_cache_ns = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let _zhipu = EnvVarGuard::remove("ZHIPU_API_KEY");

    let zai = crate::provider_catalog::resolve_openai_compatible_profile(
        crate::provider_catalog::ZAI_PROFILE,
    );
    write_test_api_key(&temp, &zai.env_file, &zai.api_key_env, "test-zai-key");

    let provider = OpenRouterProvider::new().expect("provider");
    assert_eq!(provider.model.blocking_read().clone(), "glm-4.5");
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_CACHE_NAMESPACE")
            .ok()
            .as_deref(),
        Some("zai")
    );
}

#[test]
fn test_parse_model_spec() {
    let (model, provider) = parse_model_spec("anthropic/claude-sonnet-4@Fireworks");
    assert_eq!(model, "anthropic/claude-sonnet-4");
    let provider = provider.expect("provider");
    assert_eq!(provider.name, "Fireworks");
    assert!(provider.allow_fallbacks);

    let (model, provider) = parse_model_spec("anthropic/claude-sonnet-4@Fireworks!");
    assert_eq!(model, "anthropic/claude-sonnet-4");
    let provider = provider.expect("provider");
    assert_eq!(provider.name, "Fireworks");
    assert!(!provider.allow_fallbacks);

    let (model, provider) = parse_model_spec("moonshotai/kimi-k2.5@moonshot");
    assert_eq!(model, "moonshotai/kimi-k2.5");
    let provider = provider.expect("provider");
    assert_eq!(provider.name, "Moonshot AI");

    let (model, provider) = parse_model_spec("anthropic/claude-sonnet-4@auto");
    assert_eq!(model, "anthropic/claude-sonnet-4");
    assert!(provider.is_none());
}

fn make_endpoint(name: &str, throughput: f64, uptime: f64, cache: bool, cost: f64) -> EndpointInfo {
    EndpointInfo {
        provider_name: name.to_string(),
        tag: None,
        pricing: ModelPricing {
            prompt: Some(format!("{:.10}", cost)),
            completion: None,
            input_cache_read: if cache {
                Some("0.00000007".to_string())
            } else {
                None
            },
            input_cache_write: None,
        },
        context_length: None,
        max_completion_tokens: None,
        quantization: None,
        uptime_last_30m: Some(uptime),
        latency_last_30m: None,
        throughput_last_30m: Some(serde_json::json!({"p50": throughput})),
        supports_implicit_caching: Some(cache),
        status: Some(0),
    }
}

fn make_provider() -> OpenRouterProvider {
    OpenRouterProvider {
        client: crate::provider::shared_http_client(),
        model: Arc::new(RwLock::new(DEFAULT_MODEL.to_string())),
        api_base: DEFAULT_API_BASE.to_string(),
        auth: ProviderAuth::AuthorizationBearer {
            token: "test".to_string(),
            label: DEFAULT_API_KEY_NAME.to_string(),
        },
        supports_provider_features: true,
        supports_model_catalog: true,
        profile_id: None,
        static_models: Vec::new(),
        static_context_limits: HashMap::new(),
        send_openrouter_headers: true,
        models_cache: Arc::new(RwLock::new(ModelsCache::default())),
        model_catalog_refresh: Arc::new(Mutex::new(ModelCatalogRefreshState::default())),
        endpoint_refresh: Arc::new(Mutex::new(EndpointRefreshTracker::default())),
        provider_routing: Arc::new(RwLock::new(ProviderRouting::default())),
        provider_pin: Arc::new(Mutex::new(None)),
        endpoints_cache: Arc::new(RwLock::new(HashMap::new())),
    }
}

fn make_custom_compatible_provider() -> OpenRouterProvider {
    OpenRouterProvider {
        client: crate::provider::shared_http_client(),
        model: Arc::new(RwLock::new(DEFAULT_MODEL.to_string())),
        api_base: "https://compat.example.test/v1".to_string(),
        auth: ProviderAuth::AuthorizationBearer {
            token: "test".to_string(),
            label: "OPENAI_COMPAT_API_KEY".to_string(),
        },
        supports_provider_features: false,
        supports_model_catalog: true,
        profile_id: None,
        static_models: Vec::new(),
        static_context_limits: HashMap::new(),
        send_openrouter_headers: false,
        models_cache: Arc::new(RwLock::new(ModelsCache::default())),
        model_catalog_refresh: Arc::new(Mutex::new(ModelCatalogRefreshState::default())),
        endpoint_refresh: Arc::new(Mutex::new(EndpointRefreshTracker::default())),
        provider_routing: Arc::new(RwLock::new(ProviderRouting::default())),
        provider_pin: Arc::new(Mutex::new(None)),
        endpoints_cache: Arc::new(RwLock::new(HashMap::new())),
    }
}

#[test]
fn named_openai_compatible_loads_api_key_from_env_file() {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp = TempDir::new().expect("create temp dir");
    let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path());
    let _home = EnvVarGuard::set("HOME", temp.path());
    let _appdata = EnvVarGuard::set("APPDATA", temp.path().join("AppData").join("Roaming"));
    let _api_key = EnvVarGuard::remove("CUSTOM_API_KEY");
    write_test_api_key(&temp, "custom.env", "CUSTOM_API_KEY", "from-env-file");

    let config = crate::config::NamedProviderConfig {
        base_url: "https://compat.example.test/v1".to_string(),
        api_key_env: Some("CUSTOM_API_KEY".to_string()),
        env_file: Some("custom.env".to_string()),
        default_model: Some("custom-model".to_string()),
        ..Default::default()
    };

    OpenRouterProvider::new_named_openai_compatible("custom", &config)
        .expect("provider should load key from env file");
}

#[test]
fn custom_compatible_provider_preserves_claude_like_model_ids() {
    let provider = make_custom_compatible_provider();

    provider.set_model("claude-opus4.6-thinking").unwrap();

    assert_eq!(provider.model(), "claude-opus4.6-thinking");
}

#[test]
fn custom_compatible_provider_preserves_at_sign_model_ids() {
    let provider = make_custom_compatible_provider();

    provider.set_model("gpt-5.4@OpenAI").unwrap();

    assert_eq!(provider.model(), "gpt-5.4@OpenAI");
}

#[test]
fn openrouter_provider_normalizes_bare_pinned_model_ids() {
    let provider = make_provider();

    provider.set_model("gpt-5.4@OpenAI").unwrap();

    assert_eq!(provider.model(), "openai/gpt-5.4");
}

#[test]
fn test_rank_providers_cache_priority() {
    let endpoints = vec![
        make_endpoint("FastCache", 50.0, 99.0, true, 0.0000002),
        make_endpoint("FasterNoCache", 60.0, 99.0, false, 0.0000001),
    ];

    let ranked = OpenRouterProvider::rank_providers_from_endpoints(&endpoints);
    assert_eq!(ranked.first().map(|s| s.as_str()), Some("FastCache"));
}

#[test]
fn test_rank_providers_speed_priority_among_cache_capable() {
    let endpoints = vec![
        make_endpoint("Fireworks", 120.0, 99.0, true, 0.0000013),
        make_endpoint("Moonshot AI", 80.0, 99.0, true, 0.0000010),
    ];

    let ranked = OpenRouterProvider::rank_providers_from_endpoints(&endpoints);
    assert_eq!(ranked.first().map(|s| s.as_str()), Some("Fireworks"));
}

#[test]
fn test_rank_providers_filters_down_providers() {
    let mut down_ep = make_endpoint("DownProvider", 200.0, 100.0, true, 0.0000001);
    down_ep.status = Some(1); // down
    let endpoints = vec![
        down_ep,
        make_endpoint("UpProvider", 50.0, 99.0, true, 0.0000002),
    ];

    let ranked = OpenRouterProvider::rank_providers_from_endpoints(&endpoints);
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0], "UpProvider");
}

#[test]
fn test_background_refresh_waits_for_soft_ttl() {
    let provider = make_provider();

    assert!(!provider.should_background_refresh_model_catalog(
        MODEL_CATALOG_SOFT_REFRESH_SECS.saturating_sub(1)
    ));
    assert!(provider.should_background_refresh_model_catalog(MODEL_CATALOG_SOFT_REFRESH_SECS));
}

#[test]
fn test_background_refresh_is_throttled_between_attempts() {
    let provider = make_provider();
    assert!(provider.begin_background_model_catalog_refresh());
    assert!(!provider.should_background_refresh_model_catalog(MODEL_CATALOG_SOFT_REFRESH_SECS));

    OpenRouterProvider::finish_background_model_catalog_refresh(&provider.model_catalog_refresh);

    assert!(!provider.should_background_refresh_model_catalog(MODEL_CATALOG_SOFT_REFRESH_SECS));
}

#[test]
fn test_kimi_routing_uses_endpoints_or_fallback() {
    let provider = OpenRouterProvider {
        model: Arc::new(RwLock::new("moonshotai/kimi-k2.5".to_string())),
        ..make_provider()
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let routing = rt.block_on(provider.effective_routing("moonshotai/kimi-k2.5"));
    let order = routing.order.expect("provider order should be set");
    // Should have providers - either from endpoint API or Kimi fallback
    assert!(
        !order.is_empty(),
        "Kimi routing should always produce a provider order"
    );
}

#[test]
fn test_kimi_coding_header_detection_matches_endpoint_and_model() {
    assert!(should_send_kimi_coding_agent_headers(
        "https://api.kimi.com/coding/v1",
        None,
    ));
    assert!(should_send_kimi_coding_agent_headers(
        "https://coding.dashscope.aliyuncs.com/v1",
        None,
    ));
    assert!(should_send_kimi_coding_agent_headers(
        "https://coding-intl.dashscope.aliyuncs.com/v1",
        None,
    ));
    assert!(should_send_kimi_coding_agent_headers(
        "https://api.z.ai/api/coding/paas/v4",
        None,
    ));
    assert!(should_send_kimi_coding_agent_headers(
        "https://example.com/v1",
        Some("kimi-for-coding"),
    ));
    assert!(!should_send_kimi_coding_agent_headers(
        "https://api.openrouter.ai/api/v1",
        Some("anthropic/claude-sonnet-4"),
    ));
}

#[test]
fn test_parse_next_event_accepts_compact_sse_data_and_reasoning_content() {
    let mut stream = OpenRouterStream::new(
        futures::stream::empty::<Result<Bytes, reqwest::Error>>(),
        "kimi-for-coding".to_string(),
        Arc::new(Mutex::new(None)),
    );
    stream.buffer =
        "data:{\"choices\":[{\"delta\":{\"reasoning_content\":\"thinking\"}}]}\n\n".to_string();

    match stream.parse_next_event() {
        Some(StreamEvent::ThinkingDelta(text)) => assert_eq!(text, "thinking"),
        other => panic!("expected ThinkingDelta, got {:?}", other),
    }
}

#[test]
fn test_endpoint_detail_string() {
    let ep = EndpointInfo {
        provider_name: "TestProvider".to_string(),
        tag: None,
        pricing: ModelPricing {
            prompt: Some("0.00000045".to_string()),
            completion: Some("0.00000225".to_string()),
            input_cache_read: Some("0.00000007".to_string()),
            input_cache_write: None,
        },
        context_length: Some(131072),
        max_completion_tokens: Some(8192),
        quantization: Some("fp8".to_string()),
        uptime_last_30m: Some(99.5),
        latency_last_30m: Some(serde_json::json!({"p50": 500, "p75": 800})),
        throughput_last_30m: Some(serde_json::json!({"p50": 42, "p75": 55})),
        supports_implicit_caching: Some(true),
        status: Some(0),
    };
    let detail = ep.detail_string();
    assert!(
        detail.contains("$0.45/M"),
        "should contain price: {}",
        detail
    );
    assert!(detail.contains("100%"), "should contain uptime: {}", detail);
    assert!(
        detail.contains("42tps"),
        "should contain throughput: {}",
        detail
    );
    assert!(detail.contains("cache"), "should contain cache: {}", detail);
    assert!(
        detail.contains("fp8"),
        "should contain quantization: {}",
        detail
    );
}
