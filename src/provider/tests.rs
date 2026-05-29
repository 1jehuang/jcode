use super::*;
use crate::provider::models::{ensure_model_allowed_for_subscription, filtered_display_models};

fn with_clean_provider_test_env<T>(f: impl FnOnce() -> T) -> T {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    let prev_subscription =
        std::env::var_os(crate::subscription_catalog::JCODE_SUBSCRIPTION_ACTIVE_ENV);
    let mut profile_env_keys = vec![
        "OPENROUTER_API_KEY",
        "DEEPSEEK_API_KEY",
        "KIMI_API_KEY",
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
        "JCODE_OPENAI_COMPAT_API_BASE",
        "JCODE_OPENAI_COMPAT_API_KEY_NAME",
        "JCODE_OPENAI_COMPAT_ENV_FILE",
        "JCODE_OPENAI_COMPAT_DEFAULT_MODEL",
        "JCODE_OPENAI_COMPAT_LOCAL_ENABLED",
        "OPENAI_COMPAT_API_KEY",
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "JCODE_RUNTIME_PROVIDER",
        "JCODE_ACTIVE_PROVIDER",
        "JCODE_FORCE_PROVIDER",
        "JCODE_OPENAI_MODEL",
        "JCODE_NAMED_PROVIDER_PROFILE",
        "JCODE_PROVIDER_PROFILE_ACTIVE",
        "JCODE_PROVIDER_PROFILE_NAME",
    ];
    for profile in crate::provider_catalog::openai_compatible_profiles() {
        if !profile_env_keys.contains(&profile.api_key_env) {
            profile_env_keys.push(profile.api_key_env);
        }
    }
    let saved_profile_env = profile_env_keys
        .into_iter()
        .map(|key| (key, std::env::var_os(key)))
        .collect::<Vec<_>>();
    crate::env::set_var("JCODE_HOME", temp.path());
    for (key, _) in &saved_profile_env {
        crate::env::remove_var(key);
    }
    crate::subscription_catalog::clear_runtime_env();
    crate::auth::claude::set_active_account_override(None);
    crate::auth::codex::set_active_account_override(None);

    let result = f();

    crate::auth::claude::set_active_account_override(None);
    crate::auth::codex::set_active_account_override(None);
    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
    if let Some(prev_subscription) = prev_subscription {
        crate::env::set_var(
            crate::subscription_catalog::JCODE_SUBSCRIPTION_ACTIVE_ENV,
            prev_subscription,
        );
    } else {
        crate::env::remove_var(crate::subscription_catalog::JCODE_SUBSCRIPTION_ACTIVE_ENV);
    }
    for (key, value) in saved_profile_env {
        if let Some(value) = value {
            crate::env::set_var(key, value);
        } else {
            crate::env::remove_var(key);
        }
    }
    crate::subscription_catalog::clear_runtime_env();
    result
}

fn enter_test_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
}

fn with_env_var<T>(key: &str, value: &str, f: impl FnOnce() -> T) -> T {
    let prev = std::env::var_os(key);
    crate::env::set_var(key, value);
    let result = f();
    if let Some(prev) = prev {
        crate::env::set_var(key, prev);
    } else {
        crate::env::remove_var(key);
    }
    result
}

fn save_test_openai_compatible_login_config(default_model: &str) {
    let env_file = crate::provider_catalog::OPENAI_COMPAT_PROFILE.env_file;
    crate::provider_catalog::save_env_value_to_env_file(
        "JCODE_OPENAI_COMPAT_API_BASE",
        env_file,
        Some("https://example-openai-compatible.test/v1"),
    )
    .expect("save api base");
    crate::provider_catalog::save_env_value_to_env_file(
        "OPENAI_COMPAT_API_KEY",
        env_file,
        Some("sk-test-openai-compatible"),
    )
    .expect("save api key");
    crate::provider_catalog::save_env_value_to_env_file(
        "JCODE_OPENAI_COMPAT_DEFAULT_MODEL",
        env_file,
        Some(default_model),
    )
    .expect("save default model");
}

fn save_test_openrouter_model_cache(namespace: &str, source_api_base: &str, model_ids: &[&str]) {
    let jcode_home = std::env::var_os("JCODE_HOME").expect("test JCODE_HOME should be set");
    let cache_dir = std::path::PathBuf::from(jcode_home).join("cache");
    std::fs::create_dir_all(&cache_dir).expect("create model cache dir");
    let cache = jcode_provider_openrouter::DiskCache {
        cached_at: jcode_provider_openrouter::current_unix_secs().expect("current unix time"),
        source_api_base: Some(source_api_base.to_string()),
        models: model_ids
            .iter()
            .map(|id| jcode_provider_openrouter::ModelInfo {
                id: (*id).to_string(),
                name: String::new(),
                context_length: None,
                pricing: jcode_provider_openrouter::ModelPricing::default(),
                created: None,
            })
            .collect(),
    };
    let path = cache_dir.join(format!("{namespace}_models.json"));
    std::fs::write(
        path,
        serde_json::to_string(&cache).expect("serialize model cache"),
    )
    .expect("write model cache");
}

fn clear_openai_compatible_runtime_env() {
    for key in [
        "JCODE_OPENAI_COMPAT_API_BASE",
        "JCODE_OPENAI_COMPAT_API_KEY_NAME",
        "JCODE_OPENAI_COMPAT_ENV_FILE",
        "JCODE_OPENAI_COMPAT_DEFAULT_MODEL",
        "JCODE_OPENAI_COMPAT_LOCAL_ENABLED",
        "OPENAI_COMPAT_API_KEY",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
    ] {
        crate::env::remove_var(key);
    }
}

fn save_test_openai_oauth_credentials() {
    crate::auth::codex::upsert_account_from_tokens(
        &crate::auth::codex::primary_account_label(),
        "test-oauth-access-token",
        "test-oauth-refresh-token",
        None,
        Some(chrono::Utc::now().timestamp_millis() + 86_400_000),
    )
    .expect("save test OpenAI OAuth credentials");
}

fn test_multi_provider_with_openai() -> MultiProvider {
    save_test_openai_oauth_credentials();
    crate::env::set_var("OPENAI_API_KEY", "sk-test-openai-api-key");
    let credentials = crate::auth::codex::load_credentials().expect("OpenAI credentials");
    MultiProvider {
        claude: RwLock::new(None),
        anthropic: RwLock::new(None),
        openai: RwLock::new(Some(Arc::new(openai::OpenAIProvider::new(credentials)))),
        copilot_api: RwLock::new(None),
        antigravity: RwLock::new(None),
        gemini: RwLock::new(None),
        cursor: RwLock::new(None),
        bedrock: RwLock::new(None),
        openrouter: RwLock::new(None),
        active: RwLock::new(ActiveProvider::OpenAI),
        use_claude_cli: false,
        startup_notices: RwLock::new(Vec::new()),
        forced_provider: None,
    }
}

#[test]
fn openai_model_switch_prefixes_preserve_oauth_vs_api_state_space() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _runtime_guard = rt.enter();
        let provider = test_multi_provider_with_openai();
        let models = known_openai_model_ids();
        let primary = models.first().expect("at least one OpenAI model").as_str();
        let alternate = models.get(1).map(String::as_str).unwrap_or(primary);

        let cases = [
            vec![
                (
                    format!("openai-api:{primary}"),
                    openai::OpenAICredentialMode::ApiKey,
                    primary,
                ),
                (
                    format!("openai-oauth:{alternate}"),
                    openai::OpenAICredentialMode::OAuth,
                    alternate,
                ),
            ],
            vec![
                (
                    format!("openai-oauth:{primary}"),
                    openai::OpenAICredentialMode::OAuth,
                    primary,
                ),
                (
                    format!("openai-api:{alternate}"),
                    openai::OpenAICredentialMode::ApiKey,
                    alternate,
                ),
                (
                    format!("openai-oauth:{primary}"),
                    openai::OpenAICredentialMode::OAuth,
                    primary,
                ),
            ],
        ];

        for sequence in cases {
            for (request, expected_mode, expected_model) in sequence {
                provider
                    .set_model(&request)
                    .unwrap_or_else(|err| panic!("switch {request} should succeed: {err}"));
                assert_eq!(
                    provider.active_provider(),
                    ActiveProvider::OpenAI,
                    "{request}"
                );
                assert_eq!(provider.model(), expected_model, "{request}");
                assert_eq!(
                    provider
                        .openai_provider()
                        .expect("OpenAI provider")
                        .credential_mode_snapshot(),
                    expected_mode,
                    "{request}"
                );
            }
        }
    });
}

#[test]
fn openai_model_route_roundtrip_preserves_auth_method_for_model_switches() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _runtime_guard = rt.enter();
        let provider = test_multi_provider_with_openai();
        let models = known_openai_model_ids();
        let primary = models.first().expect("at least one OpenAI model").as_str();
        let alternate = models.get(1).map(String::as_str).unwrap_or(primary);

        // This mirrors the /model picker path: the selected ModelRoute becomes a
        // default/session model + provider key, then a future /model switch uses
        // that persisted provider key to reconstruct the provider-prefixed
        // request. The important invariant is that OpenAI OAuth and API key are
        // distinct states even though both execute in ActiveProvider::OpenAI.
        let route_cases = [
            (
                primary,
                "openai-oauth",
                "openai",
                "openai-oauth",
                openai::OpenAICredentialMode::OAuth,
            ),
            (
                alternate,
                "openai-api-key",
                "openai-api",
                "openai-api",
                openai::OpenAICredentialMode::ApiKey,
            ),
            (
                primary,
                "openai-api",
                "openai-api",
                "openai-api",
                openai::OpenAICredentialMode::ApiKey,
            ),
        ];

        for (bare_model, api_method, expected_provider_key, expected_prefix, expected_mode) in
            route_cases
        {
            let selection =
                MultiProvider::default_model_selection_from_route(bare_model, api_method, "OpenAI");
            assert_eq!(
                selection.model_spec,
                format!("{expected_prefix}:{bare_model}")
            );
            assert_eq!(
                selection.provider_key.as_deref(),
                Some(expected_provider_key)
            );

            let request = MultiProvider::model_switch_request_for_session_model(
                bare_model,
                selection.provider_key.as_deref(),
            );
            assert_eq!(request, format!("{expected_prefix}:{bare_model}"));

            provider
                .set_model(&request)
                .unwrap_or_else(|err| panic!("/model switch {request} should succeed: {err}"));
            assert_eq!(
                provider.active_provider(),
                ActiveProvider::OpenAI,
                "{request}"
            );
            assert_eq!(provider.model(), bare_model, "{request}");
            assert_eq!(
                provider
                    .openai_provider()
                    .expect("OpenAI provider")
                    .credential_mode_snapshot(),
                expected_mode,
                "{request}"
            );
        }
    });
}

#[test]
fn openai_model_routes_cover_oauth_api_and_no_auth_state_space() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _runtime_guard = rt.enter();
        let model = known_openai_model_ids()
            .first()
            .expect("at least one OpenAI model")
            .clone();

        let provider = test_multi_provider_with_openai();
        let routes = provider.model_routes();
        let methods = routes
            .iter()
            .filter(|route| route.provider == "OpenAI" && route.model == model)
            .map(|route| (route.api_method.as_str(), route.available))
            .collect::<Vec<_>>();
        assert!(
            methods.contains(&("openai-oauth", true)),
            "routes: {methods:?}"
        );
        assert!(
            methods.contains(&("openai-api-key", true)),
            "routes: {methods:?}"
        );

        crate::env::remove_var("OPENAI_API_KEY");
        crate::auth::AuthStatus::invalidate_cache();
        let oauth_only = provider.model_routes();
        let oauth_only_methods = oauth_only
            .iter()
            .filter(|route| route.provider == "OpenAI" && route.model == model)
            .map(|route| route.api_method.as_str())
            .collect::<Vec<_>>();
        assert_eq!(oauth_only_methods, vec!["openai-oauth"]);

        crate::env::set_var("OPENAI_API_KEY", "sk-test-openai-api-key");
        std::fs::remove_file(
            crate::storage::jcode_dir()
                .unwrap()
                .join("openai-auth.json"),
        )
        .expect("remove oauth credentials");
        crate::auth::AuthStatus::invalidate_cache();
        let api_only = provider.model_routes();
        let api_only_methods = api_only
            .iter()
            .filter(|route| route.provider == "OpenAI" && route.model == model)
            .map(|route| route.api_method.as_str())
            .collect::<Vec<_>>();
        assert_eq!(api_only_methods, vec!["openai-api-key"]);
    });
}

fn assert_openai_compatible_route_available(provider: &MultiProvider, model: &str) {
    let routes = provider.model_routes();
    assert!(
        routes.iter().any(|route| {
            route.provider == "OpenAI-compatible"
                && matches!(
                    route.api_method.as_str(),
                    "openai-compatible" | "openai-compatible:openai-compatible"
                )
                && route.model == model
                && route.available
        }),
        "configured OpenAI-compatible model should be immediately visible after API-key setup; routes: {routes:?}"
    );
}

#[test]
fn openai_compatible_api_key_setup_makes_configured_model_route_available() {
    with_clean_provider_test_env(|| {
        save_test_openai_compatible_login_config("glm-test-login-flow");

        assert!(
            crate::provider_catalog::openai_compatible_profile_is_configured(
                crate::provider_catalog::OPENAI_COMPAT_PROFILE,
            )
        );

        let provider = MultiProvider::new();
        assert_openai_compatible_route_available(&provider, "glm-test-login-flow");

        provider
            .set_model_on_openai_compatible_profile(
                crate::provider_catalog::OPENAI_COMPAT_PROFILE,
                "glm-test-login-flow",
            )
            .expect("configured OpenAI-compatible model should select without requiring another provider login");

        assert_eq!(provider.model(), "glm-test-login-flow");
    });
}

#[test]
fn openai_compatible_api_key_setup_survives_process_restart_without_relogin() {
    with_clean_provider_test_env(|| {
        save_test_openai_compatible_login_config("restart-visible-model");

        // Simulate a fresh process: the login command wrote the config file, but
        // none of the runtime env vars from the login process remain populated.
        clear_openai_compatible_runtime_env();

        let resolved = crate::provider_catalog::resolve_openai_compatible_profile(
            crate::provider_catalog::OPENAI_COMPAT_PROFILE,
        );
        assert_eq!(
            resolved.api_base,
            "https://example-openai-compatible.test/v1"
        );
        assert_eq!(
            resolved.default_model.as_deref(),
            Some("restart-visible-model")
        );
        assert!(
            crate::provider_catalog::openai_compatible_profile_is_configured(
                crate::provider_catalog::OPENAI_COMPAT_PROFILE,
            )
        );

        let provider = MultiProvider::new();
        assert_openai_compatible_route_available(&provider, "restart-visible-model");
        provider
            .set_model_on_openai_compatible_profile(
                crate::provider_catalog::OPENAI_COMPAT_PROFILE,
                "restart-visible-model",
            )
            .expect("saved credentials should be selectable after a fresh process restart");
        assert_eq!(provider.model(), "restart-visible-model");
    });
}

#[test]
fn configured_openai_compatible_profile_routes_use_live_cache_when_not_active_provider() {
    with_clean_provider_test_env(|| {
        crate::provider_catalog::save_env_value_to_env_file(
            "OPENROUTER_API_KEY",
            "openrouter.env",
            Some("sk-test-openrouter"),
        )
        .expect("save openrouter key");
        crate::provider_catalog::save_env_value_to_env_file(
            "OPENCODE_API_KEY",
            "opencode.env",
            Some("oc-test-opencode"),
        )
        .expect("save opencode key");
        save_test_openrouter_model_cache(
            "opencode",
            "https://opencode.ai/zen/v1",
            &["kimi-k2.6", "zen-live-only-model"],
        );

        let provider = MultiProvider::new();
        let routes = provider.model_routes();
        let opencode_routes = routes
            .iter()
            .filter(|route| route.provider == "OpenCode Zen")
            .collect::<Vec<_>>();

        assert!(
            opencode_routes
                .iter()
                .any(|route| route.model == "zen-live-only-model"
                    && route.api_method == "openai-compatible:opencode"
                    && !route
                        .detail
                        .contains("fallback: static provider model list")),
            "non-active configured direct profile should expose its live /models cache, routes: {opencode_routes:?}"
        );
        assert!(
            !opencode_routes.iter().any(|route| route.model == "glm-4.7"),
            "static fallback models should drop out once a live profile catalog is available, routes: {opencode_routes:?}"
        );
    });
}

fn test_multi_provider_with_cursor() -> MultiProvider {
    MultiProvider {
        claude: RwLock::new(None),
        anthropic: RwLock::new(None),
        openai: RwLock::new(None),
        copilot_api: RwLock::new(None),
        antigravity: RwLock::new(None),
        gemini: RwLock::new(None),
        cursor: RwLock::new(Some(Arc::new(cursor::CursorCliProvider::new()))),
        bedrock: RwLock::new(None),
        openrouter: RwLock::new(None),
        active: RwLock::new(ActiveProvider::Cursor),
        use_claude_cli: false,
        startup_notices: RwLock::new(Vec::new()),
        forced_provider: None,
    }
}

include!("tests/auth_refresh.rs");
include!("tests/model_resolution.rs");
include!("tests/fallback_failover.rs");
include!("tests/catalog_subscription.rs");
