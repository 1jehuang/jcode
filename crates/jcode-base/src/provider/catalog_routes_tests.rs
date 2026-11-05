use super::*;
use crate::auth::{AuthState, ProviderAuth};

#[test]
fn saved_account_catalogs_keep_work_personal_and_named_accounts_separate() {
    let _guard = EnvGuard::new();
    let home = crate::storage::jcode_dir().unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let accounts: Vec<_> = ["work", "personal", "research"].into_iter().map(|label| {
        serde_json::json!({"label": label, "access": "test-access", "refresh": "test-refresh",
            "expires": i64::MAX, "email": format!("{label}@example.test"), "subscription_type": "max"})
    }).collect();
    std::fs::write(
        home.join("auth.json"),
        serde_json::to_vec(&serde_json::json!({
            "anthropic_accounts": accounts, "active_anthropic_account": "work"
        }))
        .unwrap(),
    )
    .unwrap();
    crate::auth::codex::save_auth_file(&crate::auth::codex::JcodeOpenAiAuthFile {
        openai_accounts: ["work", "personal", "research"]
            .into_iter()
            .map(|label| crate::auth::codex::OpenAiAccount {
                label: label.into(),
                access_token: "test-access".into(),
                refresh_token: "test-refresh".into(),
                id_token: None,
                account_id: None,
                expires_at: Some(i64::MAX),
                email: Some(format!("{label}@example.test")),
            })
            .collect(),
        active_openai_account: Some("work".into()),
    })
    .unwrap();
    let routes = expand_saved_account_routes(vec![
        build_anthropic_oauth_route("claude-sonnet-4-6", true, ""),
        ModelRoute {
            model: "claude-sonnet-4-6".into(),
            provider: "Anthropic".into(),
            api_method: "claude-api".into(),
            available: true,
            detail: String::new(),
            usage: None,
            cheapness: None,
        },
    ]);
    assert!(routes.iter().any(|r| r.api_method == "claude-api"));
    assert!(!routes.iter().any(|r| r.api_method == "claude-oauth"));
    for label in ["work", "personal", "research"] {
        let route = routes
            .iter()
            .find(|r| {
                r.model == "claude-sonnet-4-6" && r.api_method == format!("claude-oauth@{label}")
            })
            .unwrap();
        assert!(route.detail.contains(&format!("account {label}")));
        assert_eq!(
            super::super::RouteSelection::from_model_route(route).account_label(),
            Some(label)
        );
    }
    for label in ["work", "personal", "research"] {
        assert!(
            routes.iter().any(|route| route.model == "gpt-5.5"
                && route.api_method == format!("openai-oauth@{label}"))
        );
        assert!(
            !routes
                .iter()
                .any(|route| route.api_method == format!("openai-oauth@{label}")
                    && jcode_provider_core::is_openai_api_only_pro_model(&route.model))
        );
    }
    let fast = simplified_model_routes_for_picker(
        "Anthropic",
        "claude-sonnet-4-6",
        ["claude-sonnet-4-6".into()],
    );
    for label in ["work", "personal", "research"] {
        assert!(
            fast.iter()
                .any(|r| r.api_method == format!("claude-oauth@{label}"))
        );
    }
    super::super::models::populate_anthropic_models_for_scope(
        "oauth::work",
        vec!["claude-work-only".into()],
    );
    super::super::models::populate_anthropic_models_for_scope(
        "oauth::personal",
        vec!["claude-personal-only".into()],
    );
    super::super::models::populate_account_models_for_scope("work", vec!["gpt-work-only".into()]);
    super::super::models::populate_account_models_for_scope(
        "personal",
        vec!["gpt-personal-only".into()],
    );
    let scoped = expand_saved_account_routes(Vec::new());
    for (method, own, other) in [
        (
            "claude-oauth@work",
            "claude-work-only",
            "claude-personal-only",
        ),
        (
            "claude-oauth@personal",
            "claude-personal-only",
            "claude-work-only",
        ),
        ("openai-oauth@work", "gpt-work-only", "gpt-personal-only"),
        (
            "openai-oauth@personal",
            "gpt-personal-only",
            "gpt-work-only",
        ),
    ] {
        assert!(
            scoped
                .iter()
                .any(|r| r.api_method == method && r.model == own)
        );
        assert!(
            !scoped
                .iter()
                .any(|r| r.api_method == method && r.model == other)
        );
    }
    super::super::models::reset_model_catalog_services_for_tests();
}

#[test]
fn saved_account_expiry_requires_login_only_without_refresh_credentials() {
    let _guard = EnvGuard::new();
    let mut expired = build_openai_oauth_route("gpt-5.5", true, "");
    apply_saved_account_status(&mut expired, "openai", "expired-test", true, false, true);
    assert!(!expired.available);
    assert!(expired.detail.contains("expired"));
    let mut refreshable = build_openai_oauth_route("gpt-5.5", true, "");
    apply_saved_account_status(&mut refreshable, "openai", "refresh-test", true, true, true);
    assert!(refreshable.available);
    assert!(refreshable.detail.contains("refresh on selection"));
    let mut missing = build_anthropic_oauth_route("claude-sonnet-4-6", true, "");
    apply_saved_account_status(&mut missing, "claude", "missing-test", false, false, false);
    assert!(!missing.available);
}

struct EnvGuard {
    vars: Vec<(&'static str, Option<std::ffi::OsString>)>,
    _temp: tempfile::TempDir,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl EnvGuard {
    fn new() -> Self {
        let lock = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let vars = vec![
            ("JCODE_HOME", std::env::var_os("JCODE_HOME")),
            ("OPENCODE_API_KEY", std::env::var_os("OPENCODE_API_KEY")),
        ];
        crate::env::set_var("JCODE_HOME", temp.path());
        crate::env::set_var("OPENCODE_API_KEY", "sk-test-opencode");
        Self {
            vars,
            _temp: temp,
            _lock: lock,
        }
    }

    fn save_opencode_cache(&self, source_api_base: &str, model_ids: &[&str]) {
        let jcode_home = std::env::var_os("JCODE_HOME").expect("JCODE_HOME set");
        let cache_dir = std::path::PathBuf::from(jcode_home).join("cache");
        std::fs::create_dir_all(&cache_dir).expect("create cache dir");
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
                    ..Default::default()
                })
                .collect(),
        };
        std::fs::write(
            cache_dir.join("opencode_models.json"),
            serde_json::to_string(&cache).expect("serialize cache"),
        )
        .expect("write cache");
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.vars.drain(..) {
            if let Some(value) = value {
                crate::env::set_var(key, value);
            } else {
                crate::env::remove_var(key);
            }
        }
    }
}

/// Issue #694: a bare model id from a user-defined `[providers.<name>]`
/// profile must resolve to that profile, not fall through to the Copilot
/// heuristic (which mislabels it and builds an unresolvable `copilot:` id).
#[test]
fn named_provider_profile_model_routes_to_its_own_profile() {
    let mut providers = std::collections::BTreeMap::new();
    providers.insert(
        "omlx".to_string(),
        crate::config::NamedProviderConfig {
            base_url: "http://127.0.0.1:18000/v1".to_string(),
            default_model: Some("KAT-Coder-V2.5-Dev-OptiQ-4bit".to_string()),
            ..Default::default()
        },
    );

    let route =
        named_provider_profile_route_for_model_in("KAT-Coder-V2.5-Dev-OptiQ-4bit", &providers)
            .expect("custom profile model must resolve to its profile");
    assert_eq!(route.provider, "omlx");
    assert_eq!(route.api_method, "openai-compatible:omlx");
    assert_eq!(route.detail, "http://127.0.0.1:18000/v1");
    assert!(!route.api_method.starts_with("copilot"));
    assert!(matches!(
        route.api_method_kind(),
        jcode_provider_core::ModelRouteApiMethod::OpenAiCompatible { .. }
    ));
}

#[test]
fn named_anthropic_profile_preserves_profile_identity_in_picker_switch() {
    let mut providers = std::collections::BTreeMap::new();
    providers.insert(
        "corp-claude".to_string(),
        crate::config::NamedProviderConfig {
            provider_type: crate::config::NamedProviderType::AnthropicCompatible,
            base_url: "https://gateway.example/anthropic/v1".to_string(),
            default_model: Some("claude-custom".to_string()),
            ..Default::default()
        },
    );

    let route = named_provider_profile_route_for_model_in("claude-custom", &providers)
        .expect("Anthropic-compatible model must resolve to its profile");
    assert_eq!(route.provider, "corp-claude");
    assert_eq!(route.api_method, "openai-compatible:corp-claude");
    assert_eq!(
        MultiProvider::model_switch_request_for_session_route(
            &route.model,
            Some(&route.provider),
            Some(&route.api_method),
        ),
        "corp-claude:claude-custom"
    );
}

#[test]
fn unknown_model_does_not_match_named_provider_profiles() {
    let mut providers = std::collections::BTreeMap::new();
    providers.insert(
        "omlx".to_string(),
        crate::config::NamedProviderConfig {
            base_url: "http://127.0.0.1:18000/v1".to_string(),
            default_model: Some("KAT-Coder-V2.5-Dev-OptiQ-4bit".to_string()),
            ..Default::default()
        },
    );

    assert!(named_provider_profile_route_for_model_in("some-other-model", &providers).is_none());
    assert!(named_provider_profile_route_for_model_in("", &providers).is_none());
}

#[test]
fn simplified_anthropic_routes_preserve_oauth_vs_api_key_state_space() {
    let mut guard = EnvGuard::new();
    for key in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "JCODE_ANTHROPIC_AUTH",
        "JCODE_ANTHROPIC_API_KEY_NAME",
        "JCODE_ANTHROPIC_ENV_FILE",
    ] {
        guard.vars.push((key, std::env::var_os(key)));
        crate::env::remove_var(key);
    }
    super::super::models::reset_model_catalog_services_for_tests();
    for (has_oauth, has_api_key) in [(true, false), (false, true), (true, true), (false, false)] {
        let auth = AuthStatus {
            anthropic: ProviderAuth {
                state: if has_oauth || has_api_key {
                    AuthState::Available
                } else {
                    AuthState::NotConfigured
                },
                has_oauth,
                oauth_state: if has_oauth {
                    AuthState::Available
                } else {
                    AuthState::NotConfigured
                },
                has_api_key,
            },
            ..AuthStatus::default()
        };
        let mut routes = Vec::new();

        append_simplified_anthropic_model_routes(
            &mut routes,
            // Test credential state independently of Opus subscription
            // policy and optional long-context extra-usage requirements.
            "claude-sonnet-4-6".to_string(),
            &auth,
        );

        let methods = routes
            .iter()
            .map(|route| route.api_method.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            methods,
            vec!["claude-api", "claude-oauth"],
            "oauth={has_oauth} api={has_api_key}"
        );
        assert!(routes.iter().all(|route| route.provider == "Anthropic"));
        assert_eq!(routes[0].available, has_api_key);
        assert_eq!(routes[1].available, has_oauth);
        assert_eq!(
            routes[0].detail,
            if has_api_key { "" } else { "no API key" }
        );
        assert_eq!(
            routes[1].detail,
            if has_oauth { "" } else { "no Claude login" }
        );
    }
    super::super::models::reset_model_catalog_services_for_tests();
}

/// Issue #694 through the real path a user hits: a custom
/// `[providers.<name>]` profile in config.toml. The picker must route the
/// model to that profile, and must not offer it a Copilot route.
#[test]
fn custom_config_profile_model_is_routed_and_not_offered_a_copilot_route() {
    let _guard = EnvGuard::new();
    let jcode_home = std::env::var_os("JCODE_HOME").expect("JCODE_HOME set");
    std::fs::write(
        std::path::PathBuf::from(jcode_home).join("config.toml"),
        "[providers.omlx]\ntype = \"openai-compatible\"\nbase_url = \"http://127.0.0.1:18000/v1\"\ndefault_model = \"KAT-Coder-V2.5-Dev-OptiQ-4bit\"\n",
    )
    .expect("write config.toml");
    crate::config::invalidate_config_cache();

    let model = "KAT-Coder-V2.5-Dev-OptiQ-4bit";
    let route = remote_openai_compatible_route_for_model(model)
        .expect("custom config profile model must be routed to its profile");
    assert_eq!(route.provider, "omlx");
    assert_eq!(route.api_method, "openai-compatible:omlx");
    assert!(
        !remote_model_should_offer_copilot_route(model),
        "a custom profile's model must never be offered a Copilot route"
    );

    // The full fallback builder (what the picker renders) agrees.
    let routes = remote_model_routes_fallback(Some("omlx"), &[model.to_string()]);
    assert!(
        routes
            .iter()
            .all(|route| !route.api_method.contains("copilot")),
        "picker routes must not contain a copilot route: {routes:?}"
    );
    assert!(
        routes
            .iter()
            .any(|route| route.api_method == "openai-compatible:omlx"),
        "picker routes must include the profile route: {routes:?}"
    );
}

/// Issue #694 across both route sources. The picker is fed either by the
/// server-built catalog (named profile routes) or, before that frame
/// arrives, by the client-side fallback. Neither may attach a Copilot
/// route to a custom profile model, otherwise the label flickers to
/// Copilot and the selected id becomes a copilot-prefixed id.
#[test]
fn custom_config_profile_model_never_gets_a_copilot_route_from_either_source() {
    let _guard = EnvGuard::new();
    let jcode_home = std::env::var_os("JCODE_HOME").expect("JCODE_HOME set");
    std::fs::write(
        std::path::PathBuf::from(jcode_home).join("config.toml"),
        "[providers.omlx]\ntype = \"openai-compatible\"\nbase_url = \"http://127.0.0.1:18000/v1\"\ndefault_model = \"KAT-Coder-V2.5-Dev-OptiQ-4bit\"\n",
    )
    .expect("write config.toml");
    crate::config::invalidate_config_cache();

    let model = "KAT-Coder-V2.5-Dev-OptiQ-4bit";

    // Source 1: the named-profile routes the server contributes.
    let named = named_provider_profile_routes(
        "omlx",
        crate::config::config()
            .providers
            .get("omlx")
            .expect("omlx profile"),
    );
    assert!(
        named
            .iter()
            .any(|route| route.model == model && route.api_method == "openai-compatible:omlx"),
        "server catalog must offer the profile route: {named:?}"
    );

    // Source 2: the client-side fallback, including the lightweight
    // variant used while route details are still refreshing.
    for routes in [
        remote_model_routes_fallback(Some("omlx"), &[model.to_string()]),
        remote_model_routes_lightweight_fallback(Some("omlx"), &[model.to_string()], model),
    ] {
        assert!(!routes.is_empty(), "fallback must offer the model");
        assert!(
            routes.iter().all(|route| {
                !route.api_method.contains("copilot") && route.provider != "Copilot"
            }),
            "no source may attach a Copilot route: {routes:?}"
        );
    }
}

#[test]
fn remote_compatible_route_uses_live_cache_and_does_not_mark_fallback() {
    let guard = EnvGuard::new();
    guard.save_opencode_cache("https://opencode.ai/zen/v1", &["qwen3.6-plus"]);

    let route = remote_openai_compatible_route_for_model("qwen3.6-plus")
        .expect("live-cache-only OpenCode model should be routed");

    assert_eq!(route.provider, "OpenCode Zen");
    assert_eq!(route.api_method, "openai-compatible:opencode");
    assert_eq!(route.detail, "https://opencode.ai/zen/v1");
    assert!(!route.detail.contains("fallback"));
}

#[test]
fn slash_model_fallback_prefers_matching_compatible_profile() {
    let guard = EnvGuard::new();
    let model = "vendouple/gpt-5.6-sol";
    guard.save_opencode_cache("https://opencode.ai/zen/v1", &[model]);

    let routes = remote_model_routes_fallback(Some("OpenCode Zen"), &[model.to_string()]);

    assert_eq!(routes.len(), 1, "unexpected fallback routes: {routes:?}");
    assert_eq!(routes[0].provider, "OpenCode Zen");
    assert_eq!(routes[0].api_method, "openai-compatible:opencode");
    assert!(routes[0].available);
}

#[test]
fn current_compatible_profile_accepts_only_cataloged_slash_models() {
    let guard = EnvGuard::new();
    let model = "vendouple/gpt-5.6-sol";
    guard.save_opencode_cache("https://opencode.ai/zen/v1", &[model]);

    let route = remote_current_openai_compatible_route_for_model(Some("OpenCode Zen"), model)
        .expect("cataloged slash model should use the current compatible profile");
    assert_eq!(route.api_method, "openai-compatible:opencode");
    assert!(
        remote_current_openai_compatible_route_for_model(Some("OpenCode Zen"), "unknown/model")
            .is_none()
    );
}

#[test]
fn remote_compatible_route_marks_static_model_list_fallback() {
    let _guard = EnvGuard::new();

    let route = remote_openai_compatible_route_for_model("glm-4.7")
        .expect("static OpenCode fallback model should be routed");

    assert_eq!(route.provider, "OpenCode Zen");
    assert!(
        route
            .detail
            .contains("fallback: static provider model list")
    );
}

#[test]
fn remote_compatible_route_ignores_live_cache_from_wrong_api_base() {
    let guard = EnvGuard::new();
    guard.save_opencode_cache("https://wrong.example.test/v1", &["qwen3.6-plus"]);

    assert!(remote_openai_compatible_route_for_model("qwen3.6-plus").is_none());
}

fn save_openrouter_catalog_cache(model_ids: &[&str]) {
    let jcode_home = std::env::var_os("JCODE_HOME").expect("JCODE_HOME set");
    let cache_dir = std::path::PathBuf::from(jcode_home).join("cache");
    std::fs::create_dir_all(&cache_dir).expect("create cache dir");
    let cache = jcode_provider_openrouter::DiskCache {
        cached_at: jcode_provider_openrouter::current_unix_secs().expect("current unix time"),
        source_api_base: None,
        models: model_ids
            .iter()
            .map(|id| jcode_provider_openrouter::ModelInfo {
                id: (*id).to_string(),
                name: String::new(),
                context_length: None,
                pricing: jcode_provider_openrouter::ModelPricing::default(),
                created: None,
                ..Default::default()
            })
            .collect(),
    };
    std::fs::write(
        cache_dir.join("openrouter_models.json"),
        serde_json::to_string(&cache).expect("serialize cache"),
    )
    .expect("write cache");
}

/// OpenRouter alternative routes must not be fabricated for models the
/// OpenRouter catalog definitively does not list (e.g. the
/// ChatGPT-exclusive `gpt-5.3-codex-spark`), while staying optimistic
/// when no catalog cache exists yet.
#[test]
fn openrouter_alternative_routes_skip_models_absent_from_catalog() {
    let _guard = EnvGuard::new();

    // No catalog cache: optimistic, spark gets a fallback route.
    let mut routes = Vec::new();
    let mut stats = OpenRouterRouteStats::default();
    append_openrouter_alternative_routes(&mut routes, &mut stats);
    assert!(
        routes
            .iter()
            .any(|r| r.model == "gpt-5.3-codex-spark" && r.api_method == "openrouter"),
        "without a catalog cache the fallback route stays optimistic"
    );

    // Fresh catalog listing codex but not spark: spark route is dropped.
    save_openrouter_catalog_cache(&["openai/gpt-5.3-codex", "openai/gpt-5.5"]);
    let mut routes = Vec::new();
    let mut stats = OpenRouterRouteStats::default();
    append_openrouter_alternative_routes(&mut routes, &mut stats);
    assert!(
        !routes
            .iter()
            .any(|r| r.model == "gpt-5.3-codex-spark" && r.api_method == "openrouter"),
        "catalog-confirmed-absent model must not get an OpenRouter fallback route"
    );
    assert!(
        routes
            .iter()
            .any(|r| r.model == "gpt-5.3-codex" && r.api_method == "openrouter"),
        "catalog-listed model keeps its OpenRouter fallback route"
    );
}
