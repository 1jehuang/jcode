use super::openrouter_sse_stream::OpenRouterStream;
use super::*;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;
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

fn isolate_openrouter_autodetect_env() -> Vec<EnvVarGuard> {
    let mut guards = vec![
        EnvVarGuard::remove("JCODE_OPENROUTER_API_BASE"),
        EnvVarGuard::remove("JCODE_OPENROUTER_API_KEY_NAME"),
        EnvVarGuard::remove("JCODE_OPENROUTER_ENV_FILE"),
        EnvVarGuard::remove("JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER"),
        EnvVarGuard::remove("JCODE_OPENROUTER_MODEL"),
        EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE"),
        EnvVarGuard::remove("JCODE_OPENROUTER_ALLOW_NO_AUTH"),
        EnvVarGuard::remove("JCODE_OPENAI_COMPAT_API_BASE"),
        EnvVarGuard::remove("JCODE_OPENAI_COMPAT_API_KEY_NAME"),
        EnvVarGuard::remove("JCODE_OPENAI_COMPAT_ENV_FILE"),
        EnvVarGuard::remove("JCODE_OPENAI_COMPAT_SETUP_URL"),
        EnvVarGuard::remove("JCODE_OPENAI_COMPAT_DEFAULT_MODEL"),
        EnvVarGuard::remove("JCODE_OPENAI_COMPAT_LOCAL_ENABLED"),
    ];
    guards.extend(
        crate::provider_catalog::openai_compatible_profiles()
            .iter()
            .map(|profile| EnvVarGuard::remove(profile.api_key_env)),
    );
    guards
}

#[test]
fn test_has_credentials() {
    let _has_creds = OpenRouterProvider::has_credentials();
}

#[test]
fn openai_compatible_models_endpoint_allows_minimal_model_objects() {
    let parsed = parse_openai_compatible_models_response(
        r#"{
            "object": "list",
            "data": [
                {"id": "glm-51-nvfp4", "object": "model", "created": null, "owned_by": null},
                {"id": "gte-qwen2-7b", "object": "model"}
            ]
        }"#,
    )
    .expect("minimal OpenAI-compatible /models response should parse");

    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].id, "glm-51-nvfp4");
    assert_eq!(parsed[0].name, "");
}

#[test]
fn openai_compatible_models_endpoint_allows_chutes_numeric_pricing() {
    let parsed = parse_openai_compatible_models_response(
        r#"{
            "object": "list",
            "data": [{
                "id": "Qwen/Qwen3-32B-TEE",
                "root": "Qwen/Qwen3-32B-FP8",
                "price": {
                    "input": {"tao": 0.0002439746644509701, "usd": 0.08},
                    "output": {"tao": 0.0007319239933529102, "usd": 0.24}
                },
                "object": "model",
                "parent": null,
                "created": 1778439139,
                "pricing": {
                    "prompt": 0.08,
                    "completion": 0.24,
                    "input_cache_read": 0.04
                },
                "owned_by": "sglang",
                "context_length": 40960,
                "supported_features": ["json_mode", "tools"]
            }]
        }"#,
    )
    .expect("Chutes /models response with numeric pricing should parse");

    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].id, "Qwen/Qwen3-32B-TEE");
    assert_eq!(parsed[0].pricing.prompt.as_deref(), Some("0.08"));
    assert_eq!(parsed[0].pricing.completion.as_deref(), Some("0.24"));
    assert_eq!(parsed[0].pricing.input_cache_read.as_deref(), Some("0.04"));
}

#[test]
fn openai_compatible_models_endpoint_allows_together_top_level_array() {
    let parsed = parse_openai_compatible_models_response(
        r#"[
            {
                "id": "Austism/chronos-hermes-13b",
                "object": "model",
                "created": 1692896905,
                "type": "chat",
                "display_name": "Chronos Hermes (13B)",
                "context_length": 2048,
                "pricing": {
                    "input": 0.3,
                    "output": 0.3,
                    "cached_input": 0.2
                }
            }
        ]"#,
    )
    .expect("Together /models top-level array should parse");

    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].id, "Austism/chronos-hermes-13b");
    assert_eq!(parsed[0].name, "Chronos Hermes (13B)");
    assert_eq!(parsed[0].context_length, Some(2048));
    assert_eq!(parsed[0].pricing.prompt.as_deref(), Some("0.3"));
    assert_eq!(parsed[0].pricing.completion.as_deref(), Some("0.3"));
    assert_eq!(parsed[0].pricing.input_cache_read.as_deref(), Some("0.2"));
}

#[test]
fn openai_compatible_models_endpoint_allows_models_array_with_name_ids() {
    let parsed = parse_openai_compatible_models_response(
        r#"{
            "models": [{
                "name": "accounts/fireworks/models/example",
                "displayName": "Example Fireworks Model",
                "contextLength": 8192
            }]
        }"#,
    )
    .expect("models array with name-based identifiers should parse");

    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].id, "accounts/fireworks/models/example");
    assert_eq!(parsed[0].name, "accounts/fireworks/models/example");
    assert_eq!(parsed[0].context_length, Some(8192));
}

#[test]
fn named_openai_compatible_provider_sets_catalog_cache_namespace() {
    let _lock = ENV_LOCK.lock().unwrap();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let _key = EnvVarGuard::set("TEST_NAMED_COMPAT_KEY", "test-key");

    let profile = crate::config::NamedProviderConfig {
        base_url: "https://llm.example.com/v1".to_string(),
        api_key_env: Some("TEST_NAMED_COMPAT_KEY".to_string()),
        model_catalog: true,
        default_model: Some("example-model".to_string()),
        ..Default::default()
    };

    let _provider = OpenRouterProvider::new_named_openai_compatible("example-compat", &profile)
        .expect("named profile should initialize");

    assert_eq!(
        std::env::var("JCODE_OPENROUTER_CACHE_NAMESPACE").as_deref(),
        Ok("example-compat")
    );
}

#[test]
fn named_openai_compatible_provider_exposes_static_models_as_routes() {
    let _lock = ENV_LOCK.lock().unwrap();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let _key = EnvVarGuard::set("TEST_NAMED_COMPAT_KEY", "test-key");

    let profile = crate::config::NamedProviderConfig {
        base_url: "https://llm.example.com/v1".to_string(),
        api_key_env: Some("TEST_NAMED_COMPAT_KEY".to_string()),
        model_catalog: true,
        default_model: Some("glm-51-nvfp4".to_string()),
        models: vec![crate::config::NamedProviderModelConfig {
            id: "glm-51-nvfp4".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    };

    let provider = OpenRouterProvider::new_named_openai_compatible("comtegra-test", &profile)
        .expect("named profile should initialize");
    let routes = provider.model_routes();

    assert!(routes.iter().any(|route| {
        route.model == "glm-51-nvfp4"
            && route.api_method == "openai-compatible:comtegra-test"
            && route.available
    }));
}

#[test]
fn minimax_profile_exposes_static_models_before_catalog_refresh() {
    let models = crate::provider_catalog::openai_compatible_profile_static_models(
        jcode_provider_metadata::MINIMAX_PROFILE,
    );

    assert!(models.iter().any(|model| model == "MiniMax-M2.7"));
    assert!(models.iter().any(|model| model == "MiniMax-M2.7-highspeed"));
    assert!(models.iter().any(|model| model == "MiniMax-M2"));
}

#[test]
fn cerebras_profile_exposes_static_models_before_catalog_refresh() {
    assert_eq!(
        jcode_provider_metadata::CEREBRAS_PROFILE.default_model,
        Some("qwen-3-235b-a22b-instruct-2507")
    );

    let models = crate::provider_catalog::openai_compatible_profile_static_models(
        jcode_provider_metadata::CEREBRAS_PROFILE,
    );

    assert!(
        !models.iter().any(|model| model == "qwen-3-coder-480b"),
        "old Cerebras default is no longer returned by the live /models catalog"
    );
    assert!(
        models
            .iter()
            .any(|model| model == "qwen-3-235b-a22b-instruct-2507")
    );
    assert!(models.iter().any(|model| model == "llama3.1-8b"));
    assert!(
        !models.iter().any(|model| model == "zai-glm-4.7"),
        "Cerebras exposes zai-glm-4.7 from /models for some keys, but chat/completions returns model_not_found"
    );
    assert!(
        !models.iter().any(|model| model == "gpt-oss-120b"),
        "Cerebras exposes gpt-oss-120b from /models for some keys, but chat/completions returns model_not_found"
    );
}

#[test]
fn openai_compatible_profiles_with_unverified_live_catalogs_have_static_fallbacks() {
    let cases = [
        (jcode_provider_metadata::OPENCODE_PROFILE, "minimax-m2.7"),
        (jcode_provider_metadata::OPENCODE_GO_PROFILE, "kimi-k2.5"),
        (jcode_provider_metadata::ZAI_PROFILE, "glm-4.7"),
        (
            jcode_provider_metadata::AI302_PROFILE,
            "qwen3-235b-a22b-instruct-2507",
        ),
        (jcode_provider_metadata::BASETEN_PROFILE, "zai-org/GLM-4.7"),
        (jcode_provider_metadata::CORTECS_PROFILE, "kimi-k2.5"),
        (jcode_provider_metadata::KIMI_PROFILE, "kimi-for-coding"),
        (jcode_provider_metadata::FIRMWARE_PROFILE, "kimi-k2.5"),
        (
            jcode_provider_metadata::HUGGING_FACE_PROFILE,
            "Qwen/Qwen3-Coder-480B-A35B-Instruct",
        ),
        (jcode_provider_metadata::MOONSHOT_PROFILE, "kimi-k2.5"),
        (
            jcode_provider_metadata::NEBIUS_PROFILE,
            "openai/gpt-oss-120b",
        ),
        (
            jcode_provider_metadata::SCALEWAY_PROFILE,
            "qwen3-coder-30b-a3b-instruct",
        ),
        (
            jcode_provider_metadata::STACKIT_PROFILE,
            "openai/gpt-oss-120b",
        ),
        (jcode_provider_metadata::PERPLEXITY_PROFILE, "sonar"),
        (
            jcode_provider_metadata::DEEPINFRA_PROFILE,
            "moonshotai/Kimi-K2-Instruct",
        ),
        (
            jcode_provider_metadata::FIREWORKS_PROFILE,
            "accounts/fireworks/routers/kimi-k2p5-turbo",
        ),
        (
            jcode_provider_metadata::ALIBABA_CODING_PLAN_PROFILE,
            "qwen3-coder-plus",
        ),
    ];

    for (profile, expected_model) in cases {
        let models = crate::provider_catalog::openai_compatible_profile_static_models(profile);
        assert!(
            models.iter().any(|model| model == expected_model),
            "{} should expose static fallback model {expected_model}; got {models:?}",
            profile.id
        );
    }
}

#[test]
fn comtegra_profile_uses_endpoint_default_max_tokens() {
    let _lock = ENV_LOCK.lock().unwrap();
    let _override = EnvVarGuard::remove("JCODE_OPENROUTER_MAX_TOKENS");

    assert_eq!(
        OpenRouterProvider::configured_max_tokens(Some("comtegra")),
        None
    );
    assert_eq!(
        OpenRouterProvider::configured_max_tokens(Some("deepseek")),
        None
    );
}

#[test]
fn max_tokens_env_overrides_profile_default() {
    let _lock = ENV_LOCK.lock().unwrap();
    let _override = EnvVarGuard::set("JCODE_OPENROUTER_MAX_TOKENS", "4096");

    assert_eq!(
        OpenRouterProvider::configured_max_tokens(Some("comtegra")),
        Some(4096)
    );
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
    let _env = isolate_openrouter_autodetect_env();

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
    let _env = isolate_openrouter_autodetect_env();

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
    let _env = isolate_openrouter_autodetect_env();

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
    let _env = isolate_openrouter_autodetect_env();

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
        reasoning_effort: Arc::new(RwLock::new(None)),
        api_base: DEFAULT_API_BASE.to_string(),
        auth: ProviderAuth::AuthorizationBearer {
            token: "test".to_string(),
            label: DEFAULT_API_KEY_NAME.to_string(),
        },
        supports_provider_features: true,
        supports_model_catalog: true,
        profile_id: None,
        max_tokens: None,
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
        reasoning_effort: Arc::new(RwLock::new(None)),
        api_base: "https://compat.example.test/v1".to_string(),
        auth: ProviderAuth::AuthorizationBearer {
            token: "test".to_string(),
            label: "OPENAI_COMPAT_API_KEY".to_string(),
        },
        supports_provider_features: false,
        supports_model_catalog: true,
        profile_id: None,
        max_tokens: None,
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

fn spawn_single_response_models_server(body: &'static str) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake provider server");
    let addr = listener.local_addr().expect("fake provider addr");
    let (request_tx, request_rx) = mpsc::channel();

    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept fake provider request");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set read timeout");
        let mut request = vec![0u8; 8192];
        let n = stream.read(&mut request).unwrap_or(0);
        let request = String::from_utf8_lossy(&request[..n]).into_owned();
        let _ = request_tx.send(request);

        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .expect("write fake provider response");
    });

    (format!("http://{addr}/v1"), request_rx)
}

fn spawn_single_response_chat_server() -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake provider server");
    let addr = listener.local_addr().expect("fake provider addr");
    let (request_tx, request_rx) = mpsc::channel();

    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept fake provider request");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set read timeout");
        let mut request = vec![0u8; 16384];
        let n = stream.read(&mut request).unwrap_or(0);
        let request = String::from_utf8_lossy(&request[..n]).into_owned();
        let _ = request_tx.send(request);

        let body = "data: [DONE]\n\n";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .expect("write fake provider response");
    });

    (format!("http://{addr}/v1"), request_rx)
}

#[test]
fn direct_deepseek_profile_exposes_max_reasoning_effort() {
    let provider = OpenRouterProvider {
        profile_id: Some("deepseek".to_string()),
        supports_provider_features: false,
        ..make_custom_compatible_provider()
    };

    assert_eq!(
        provider.available_efforts(),
        vec!["none", "low", "medium", "high", "max"]
    );
    provider
        .set_reasoning_effort("max")
        .expect("DeepSeek direct profile should accept max effort");
    assert_eq!(provider.reasoning_effort().as_deref(), Some("max"));
}

#[test]
fn non_deepseek_compatible_profile_does_not_expose_reasoning_effort() {
    let provider = make_custom_compatible_provider();

    assert!(provider.available_efforts().is_empty());
    let error = provider
        .set_reasoning_effort("max")
        .expect_err("generic compatible profile should not expose DeepSeek effort UX");
    assert!(
        error.to_string().contains("DeepSeek direct profiles"),
        "unexpected error: {error:?}"
    );
}

#[test]
fn direct_deepseek_chat_request_sends_reasoning_effort() {
    let (api_base, request_rx) = spawn_single_response_chat_server();
    let provider = OpenRouterProvider {
        api_base,
        model: Arc::new(RwLock::new("deepseek-v4-pro".to_string())),
        profile_id: Some("deepseek".to_string()),
        supports_provider_features: false,
        supports_model_catalog: false,
        send_openrouter_headers: false,
        ..make_custom_compatible_provider()
    };
    provider
        .set_reasoning_effort("max")
        .expect("DeepSeek direct profile should accept max effort");

    let messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "hello".to_string(),
            cache_control: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }];

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(async {
        let mut stream = provider
            .complete(&messages, &[], "", None)
            .await
            .expect("fake chat request should start");
        while let Some(event) = stream.next().await {
            event.expect("stream event should parse");
        }
    });

    let request = request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("capture fake provider request");
    assert!(
        request.starts_with("POST /v1/chat/completions "),
        "unexpected chat request: {request}"
    );
    assert!(
        request.contains(r#""model":"deepseek-v4-pro""#),
        "request should contain model: {request}"
    );
    assert!(
        request.contains(r#""reasoning_effort":"max""#),
        "DeepSeek request should include max reasoning effort: {request}"
    );
}

#[test]
fn openai_compatible_model_catalog_refresh_calls_models_endpoint_and_updates_display() {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp = TempDir::new().expect("create temp home");
    let _home = EnvVarGuard::set("HOME", temp.path());
    let _appdata = EnvVarGuard::set("APPDATA", temp.path().join("AppData").join("Roaming"));
    let _namespace = EnvVarGuard::set(
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "test-openai-compatible-flow",
    );
    let (api_base, request_rx) = spawn_single_response_models_server(
        r#"{
            "object": "list",
            "data": [
                {"id": "live-login-flow-model", "object": "model"}
            ]
        }"#,
    );
    let provider = OpenRouterProvider {
        api_base,
        auth: ProviderAuth::AuthorizationBearer {
            token: "sk-live-catalog".to_string(),
            label: "OPENAI_COMPAT_API_KEY".to_string(),
        },
        supports_provider_features: false,
        supports_model_catalog: true,
        profile_id: None,
        static_models: vec!["static-login-flow-fallback".to_string()],
        send_openrouter_headers: false,
        ..make_custom_compatible_provider()
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let fetched = rt
        .block_on(provider.refresh_models())
        .expect("refresh fake model catalog");
    assert_eq!(fetched[0].id, "live-login-flow-model");

    let request = request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("capture fake provider request");
    assert!(
        request.starts_with("GET /v1/models "),
        "unexpected catalog request: {request}"
    );
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer sk-live-catalog"),
        "catalog request should include saved API key auth header: {request}"
    );
    assert!(
        request.to_ascii_lowercase().contains("user-agent: jcode/"),
        "catalog requests must include a User-Agent because providers like Cerebras reject bare HTTP clients: {request}"
    );

    let display = provider.available_models_display();
    assert!(display.iter().any(|model| model == "live-login-flow-model"));
    assert!(
        display
            .iter()
            .any(|model| model == "static-login-flow-fallback"),
        "static fallback/default models should remain visible alongside live catalog models: {display:?}"
    );
}

#[test]
fn built_in_openai_compatible_static_models_drop_out_after_live_catalog() {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp = TempDir::new().expect("create temp home");
    let _home = EnvVarGuard::set("HOME", temp.path());
    let _appdata = EnvVarGuard::set("APPDATA", temp.path().join("AppData").join("Roaming"));
    let _namespace = EnvVarGuard::set(
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "test-cerebras-live-catalog-filters-static-fallback",
    );
    let (api_base, _request_rx) = spawn_single_response_models_server(
        r#"{
            "object": "list",
            "data": [
                {"id": "qwen-3-235b-a22b-instruct-2507", "object": "model"},
                {"id": "zai-glm-4.7", "object": "model"},
                {"id": "gpt-oss-120b", "object": "model"}
            ]
        }"#,
    );
    let provider = OpenRouterProvider {
        api_base,
        auth: ProviderAuth::AuthorizationBearer {
            token: "sk-live-catalog".to_string(),
            label: "CEREBRAS_API_KEY".to_string(),
        },
        supports_provider_features: false,
        supports_model_catalog: true,
        profile_id: Some("cerebras".to_string()),
        static_models: vec![
            "zai-glm-4.7".to_string(),
            "qwen-3-235b-a22b-instruct-2507".to_string(),
        ],
        send_openrouter_headers: false,
        ..make_custom_compatible_provider()
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(provider.refresh_models())
        .expect("refresh fake model catalog");

    let display = provider.available_models_display();
    assert!(
        display
            .iter()
            .any(|model| model == "qwen-3-235b-a22b-instruct-2507")
    );
    assert!(
        !display.iter().any(|model| model == "zai-glm-4.7"),
        "Cerebras models that 404 on chat/completions should not be advertised after a live catalog refresh: {display:?}"
    );
    assert!(
        !display.iter().any(|model| model == "gpt-oss-120b"),
        "Cerebras models that 404 on chat/completions should not be advertised after a live catalog refresh: {display:?}"
    );
}

#[test]
fn cerebras_chat_unavailable_catalog_models_are_rejected_on_explicit_switch() {
    let provider = OpenRouterProvider {
        supports_provider_features: false,
        supports_model_catalog: true,
        profile_id: Some("cerebras".to_string()),
        static_models: vec!["qwen-3-235b-a22b-instruct-2507".to_string()],
        send_openrouter_headers: false,
        ..make_custom_compatible_provider()
    };

    let error = provider
        .set_model("zai-glm-4.7")
        .expect_err("known Cerebras chat-unavailable model should be rejected before request time");
    assert!(
        error
            .to_string()
            .contains("not currently usable for chat completions"),
        "unexpected error: {error:?}"
    );
    provider
        .set_model("qwen-3-235b-a22b-instruct-2507")
        .expect("chat-supported Cerebras model should remain selectable");
}

#[test]
fn direct_deepseek_profile_uses_static_1m_context_when_catalog_is_absent() {
    let _lock = ENV_LOCK.lock().unwrap();
    let _base = EnvVarGuard::set("JCODE_OPENROUTER_API_BASE", "https://api.deepseek.com");
    let _key_name = EnvVarGuard::set("JCODE_OPENROUTER_API_KEY_NAME", "DEEPSEEK_API_KEY");
    let _api_key = EnvVarGuard::set("DEEPSEEK_API_KEY", "test");
    let _namespace = EnvVarGuard::set("JCODE_OPENROUTER_CACHE_NAMESPACE", "deepseek");
    let _model = EnvVarGuard::set("JCODE_OPENROUTER_MODEL", "deepseek-v4-flash");
    let _catalog = EnvVarGuard::set("JCODE_OPENROUTER_MODEL_CATALOG", "0");

    let provider = OpenRouterProvider::new().expect("provider");

    assert_eq!(provider.context_window(), 1_000_000);
}

#[test]
fn named_openai_compatible_model_context_window_overrides_default() {
    let _lock = ENV_LOCK.lock().unwrap();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let mut config = crate::config::NamedProviderConfig {
        base_url: "https://compat.example.test/v1".to_string(),
        api_key: Some("test".to_string()),
        default_model: Some("custom-long-context".to_string()),
        models: vec![crate::config::NamedProviderModelConfig {
            id: "custom-long-context".to_string(),
            context_window: Some(512_000),
            input: Vec::new(),
        }],
        ..Default::default()
    };
    config.model_catalog = false;

    let provider =
        OpenRouterProvider::new_named_openai_compatible("custom", &config).expect("provider");

    assert_eq!(provider.context_window(), 512_000);
}

#[test]
fn named_openai_compatible_loads_api_key_from_env_file() {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp = TempDir::new().expect("create temp dir");
    let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path());
    let _home = EnvVarGuard::set("HOME", temp.path());
    let _appdata = EnvVarGuard::set("APPDATA", temp.path().join("AppData").join("Roaming"));
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
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
    assert!(should_send_kimi_coding_agent_headers(
        "https://openrouter.ai/api/v1",
        Some("moonshotai/kimi-k2.5"),
    ));
    assert!(!should_send_kimi_coding_agent_headers(
        "https://api.openrouter.ai/api/v1",
        Some("anthropic/claude-sonnet-4"),
    ));
}

#[test]
fn test_openrouter_kimi_chat_request_includes_compat_user_agent() {
    let request = apply_kimi_coding_agent_headers(
        Client::new().post("https://openrouter.ai/api/v1/chat/completions"),
        "https://openrouter.ai/api/v1",
        Some("moonshotai/kimi-k2.5"),
    )
    .build()
    .expect("build request");
    assert!(
        request
            .headers()
            .get("User-Agent")
            .and_then(|value| value.to_str().ok())
            == Some(KIMI_CODING_USER_AGENT),
        "Kimi OpenRouter chat request should include compatibility User-Agent"
    );
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
fn test_parse_next_event_emits_only_incremental_reasoning_content() {
    let mut stream = OpenRouterStream::new(
        futures::stream::empty::<Result<Bytes, reqwest::Error>>(),
        "moonshotai/kimi-k2.5".to_string(),
        Arc::new(Mutex::new(None)),
    );

    stream.buffer =
        "data:{\"choices\":[{\"delta\":{\"reasoning_content\":\"Thinking\"}}]}\n\n".to_string();
    match stream.parse_next_event() {
        Some(StreamEvent::ThinkingDelta(text)) => assert_eq!(text, "Thinking"),
        other => panic!("expected first ThinkingDelta, got {:?}", other),
    }

    stream.buffer =
        "data:{\"choices\":[{\"delta\":{\"reasoning_content\":\"Thinking more\"}}]}\n\n"
            .to_string();
    match stream.parse_next_event() {
        Some(StreamEvent::ThinkingDelta(text)) => assert_eq!(text, " more"),
        other => panic!("expected incremental ThinkingDelta, got {:?}", other),
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
            input_cache_write: Some("0.00000012".to_string()),
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
        detail.contains("out $2.25/M"),
        "should contain output price: {}",
        detail
    );
    assert!(
        detail.contains("cache write $0.12/M"),
        "should contain cache write price: {}",
        detail
    );
    assert!(
        detail.contains("cache read $0.07/M"),
        "should contain cache read price: {}",
        detail
    );
    assert!(
        detail.contains("500ms p50"),
        "should contain latency: {}",
        detail
    );
    assert!(
        detail.contains("42tps"),
        "should contain throughput: {}",
        detail
    );
    assert!(
        detail.contains("cache on"),
        "should contain cache: {}",
        detail
    );
    assert!(
        detail.contains("fp8"),
        "should contain quantization: {}",
        detail
    );
}
