use super::*;
use std::sync::MutexGuard;

mod ranking;

struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    saved: Vec<(&'static str, Option<String>)>,
}

impl EnvGuard {
    fn new(keys: &[&'static str]) -> Self {
        let lock = crate::storage::lock_test_env();
        let saved = keys
            .iter()
            .map(|key| (*key, std::env::var(key).ok()))
            .collect();
        for key in keys {
            crate::env::remove_var(key);
        }
        Self { _lock: lock, saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..) {
            if let Some(value) = value {
                crate::env::set_var(key, value);
            } else {
                crate::env::remove_var(key);
            }
        }
    }
}

fn route(model: &str, provider: &str, api_method: &str, available: bool) -> ModelRoute {
    ModelRoute {
        model: model.to_string(),
        provider: provider.to_string(),
        api_method: api_method.to_string(),
        available,
        detail: String::new(),
        usage: None,
        cheapness: None,
    }
}

#[test]
fn onboarding_default_selection_follows_global_preference() {
    assert_eq!(onboarding_default_selection(&[]), None);
    assert_eq!(onboarding_default_selection(&["openrouter"]), None);
    assert_eq!(
        onboarding_default_selection(&["gemini", "claude", "openai"]),
        Some((
            "openai".to_string(),
            jcode_provider_core::DEFAULT_OPENAI_MODEL.to_string()
        ))
    );
    assert_eq!(
        onboarding_default_selection(&["gemini", "claude-api"]),
        Some((
            "claude-api".to_string(),
            jcode_provider_core::DEFAULT_CLAUDE_MODEL.to_string()
        ))
    );
    assert_eq!(
        onboarding_default_selection(&["cursor", "gemini"]),
        Some((
            "cursor".to_string(),
            crate::provider::cursor::DEFAULT_MODEL.to_string()
        ))
    );
}

#[test]
fn api_key_login_replaces_stale_process_env_with_saved_file_key() {
    // Issue #453: a server process that inherited a stale ANTHROPIC_API_KEY
    // must start using the key that /login just wrote to anthropic.env.
    let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");
    crate::env::set_var("ANTHROPIC_API_KEY", "stale-inherited-key");
    sandbox
        .write_env_file("anthropic.env", "ANTHROPIC_API_KEY", "fresh-login-key")
        .expect("write env file");

    let mut auth = AuthChanged::new("claude-api");
    auth.credential_source = Some(crate::protocol::AuthCredentialSource::ApiKeyFile);
    auth.auth_method = Some(crate::protocol::AuthMethod::TuiPasteApiKey);
    let _ = activate_auth_change(&AuthActivationRequest::new(None, Some(auth)));

    assert_eq!(
        std::env::var("ANTHROPIC_API_KEY").as_deref(),
        Ok("fresh-login-key")
    );
    assert_eq!(
        crate::provider_catalog::load_api_key_from_env_or_config(
            "ANTHROPIC_API_KEY",
            "anthropic.env"
        )
        .as_deref(),
        Some("fresh-login-key"),
        "credential resolution must use the freshly saved key"
    );
}

#[test]
fn api_key_login_invalidates_auth_status_cached_before_activation() {
    let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");
    assert_eq!(
        crate::auth::AuthStatus::check_fast().openrouter,
        crate::auth::AuthState::NotConfigured
    );
    sandbox
        .write_env_file("openrouter.env", "OPENROUTER_API_KEY", "fresh-login-key")
        .expect("write env file");

    let mut auth = AuthChanged::new("openrouter");
    auth.credential_source = Some(crate::protocol::AuthCredentialSource::ApiKeyFile);
    auth.auth_method = Some(crate::protocol::AuthMethod::TuiPasteApiKey);
    let _ = activate_auth_change(&AuthActivationRequest::new(None, Some(auth)));

    assert_eq!(
        crate::auth::AuthStatus::check_fast().openrouter,
        crate::auth::AuthState::Available,
        "catalog refresh must not reuse the pre-activation auth snapshot"
    );
}

#[test]
fn legacy_hint_only_auth_change_still_syncs_saved_file_key() {
    let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");
    crate::env::set_var("ANTHROPIC_API_KEY", "stale-inherited-key");
    sandbox
        .write_env_file("anthropic.env", "ANTHROPIC_API_KEY", "fresh-login-key")
        .expect("write env file");

    let _ = activate_auth_change(&AuthActivationRequest::new(
        Some("anthropic-api".to_string()),
        None,
    ));

    assert_eq!(
        std::env::var("ANTHROPIC_API_KEY").as_deref(),
        Ok("fresh-login-key")
    );
}

#[test]
fn oauth_auth_change_does_not_touch_api_key_process_env() {
    let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");
    crate::env::set_var("ANTHROPIC_API_KEY", "env-key-left-alone");
    sandbox
        .write_env_file("anthropic.env", "ANTHROPIC_API_KEY", "file-key")
        .expect("write env file");

    let mut auth = AuthChanged::new("claude-api");
    auth.auth_method = Some(crate::protocol::AuthMethod::OAuthBrowser);
    auth.credential_source = Some(crate::protocol::AuthCredentialSource::OAuthTokenStore);
    let _ = activate_auth_change(&AuthActivationRequest::new(None, Some(auth)));

    assert_eq!(
        std::env::var("ANTHROPIC_API_KEY").as_deref(),
        Ok("env-key-left-alone")
    );
}

#[test]
fn direct_auth_catalog_matching_preserves_oauth_vs_api_key_route_identity() {
    for (provider_id, provider_label, matching_provider, matching_method, stale_method) in [
        (
            "claude",
            "Anthropic/Claude",
            "Anthropic",
            "claude-oauth",
            "claude-api",
        ),
        (
            "claude-api",
            "Anthropic API",
            "Anthropic",
            "claude-api",
            "claude-oauth",
        ),
        (
            "openai",
            "OpenAI",
            "OpenAI",
            "openai-oauth",
            "openai-api-key",
        ),
        (
            "openai-api",
            "OpenAI API",
            "OpenAI",
            "openai-api-key",
            "openai-oauth",
        ),
    ] {
        let activation = AuthActivationResult {
            provider_id: Some(provider_id.to_string()),
            provider_label: Some(provider_label.to_string()),
            activated_model: Some("shared-model".to_string()),
            expected_runtime: None,
            expected_catalog_namespace: None,
        };
        let routes = vec![
            route("shared-model", matching_provider, stale_method, true),
            route("shared-model", matching_provider, matching_method, true),
        ];

        let report = validate_catalog_invariants(&activation, Some("shared-model"), &routes);
        assert!(
            report.ok(),
            "{provider_id} should match {matching_method}: {report:?}"
        );
        assert_eq!(report.selectable_provider_routes, 1);
        assert_eq!(
            report.route_sample,
            vec![format!("`shared-model` via {matching_method}")]
        );
        assert_eq!(
            provider_model_to_select_after_auth(&activation, Some("shared-model"), &routes),
            Some("shared-model".to_string()),
            "duplicate model IDs must force a provider-explicit model switch for {provider_id}"
        );
    }
}

#[test]
fn typed_auth_request_provider_id_wins_over_legacy_hint() {
    let request = AuthActivationRequest::new(
        Some("openai".to_string()),
        Some(AuthChanged::new("cerebras")),
    );

    assert_eq!(request.provider_id().as_deref(), Some("cerebras"));
    assert_eq!(
        provider_display_label(request.provider_id().as_deref()).as_deref(),
        Some("Cerebras")
    );
}

#[test]
fn direct_login_provider_ids_are_normalized_with_display_labels() {
    for (hint, normalized, label) in [
        ("claude", "claude", "Anthropic/Claude"),
        ("anthropic", "claude", "Anthropic/Claude"),
        ("anthropic-api", "claude-api", "Anthropic API"),
        ("claude-api", "claude-api", "Anthropic API"),
        ("openai", "openai", "OpenAI"),
        ("openai-key", "openai-api", "OpenAI API"),
        ("openrouter", "openrouter", "OpenRouter"),
        ("subscription", "jcode", "Jcode Subscription"),
        ("grok-build", "grok-build", "Grok Build"),
        ("bedrock", "bedrock", "AWS Bedrock"),
        ("cursor", "cursor", "Cursor"),
        ("copilot", "copilot", "GitHub Copilot"),
        ("gemini", "gemini", "Gemini Code Assist (Cloud)"),
        ("antigravity", "antigravity", "Google (Antigravity)"),
    ] {
        assert_eq!(normalized_auth_provider_id(Some(hint)), Some(normalized));
        assert_eq!(provider_display_label(Some(hint)).as_deref(), Some(label));
    }
}

#[test]
fn every_model_login_provider_has_explicit_lifecycle_normalization() {
    let mut missing = Vec::new();
    for provider in crate::provider_catalog::login_providers() {
        let is_non_model_auth_surface = matches!(
            provider.target,
            crate::provider_catalog::LoginProviderTarget::AutoImport
                | crate::provider_catalog::LoginProviderTarget::Google
        );
        let normalized = normalized_auth_provider_id(Some(provider.id));
        if is_non_model_auth_surface {
            assert!(
                normalized.is_none(),
                "non-model auth provider {} should stay out of model lifecycle normalization",
                provider.id
            );
        } else if normalized.is_none() {
            missing.push(provider.id);
        }
    }

    assert!(
        missing.is_empty(),
        "model login providers missing lifecycle normalization: {:?}",
        missing
    );
}

#[test]
fn grok_build_login_selects_subscription_route_and_preserves_prefix() {
    let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");
    let activation = activate_auth_change(&AuthActivationRequest::new(
        None,
        Some(AuthChanged::new("grok-build")),
    ));
    assert_eq!(activation.provider_id.as_deref(), Some("grok-build"));
    assert_eq!(activation.provider_label.as_deref(), Some("Grok Build"));
    let routes = vec![
        route("grok-4.6", "xAI", "openrouter", true),
        route("grok-build:grok-4.6", "Grok Build", "grok-build-acp", true),
    ];
    let selected = provider_model_to_select_after_auth(&activation, Some("grok-4.6"), &routes);
    assert_eq!(selected.as_deref(), Some("grok-build:grok-4.6"));
    assert!(validate_catalog_invariants(&activation, selected.as_deref(), &routes).ok());
    for model in ["grok-4.6", "grok-build:grok-4.6"] {
        assert_eq!(
            activation.model_switch_request("OpenRouter", model),
            "grok-build:grok-4.6"
        );
    }
}

#[test]
fn login_does_not_override_configured_default_provider_for_new_sessions() {
    let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");
    std::fs::write(
        sandbox.root().join("config.toml"),
        "[provider]\ndefault_model = \"claude-oauth:claude-opus-5-5\"\ndefault_provider = \"claude-oauth\"\n",
    )
    .expect("write config");
    crate::config::invalidate_config_cache();
    crate::env::set_var("JCODE_RUNTIME_PROVIDER", "claude");
    crate::env::set_var("JCODE_ACTIVE_PROVIDER", "claude");
    crate::env::remove_var("JCODE_INITIAL_PROVIDER_EXPLICIT");

    let activation = activate_auth_change(&AuthActivationRequest::new(
        None,
        Some(AuthChanged::new("openai")),
    ));

    assert_eq!(activation.provider_id.as_deref(), Some("openai"));
    assert_eq!(
        std::env::var("JCODE_ACTIVE_PROVIDER").as_deref(),
        Ok("claude"),
        "logging into OpenAI must not retarget the daemon-wide initial provider"
    );
    assert!(std::env::var("JCODE_INITIAL_PROVIDER_EXPLICIT").is_err());

    std::fs::remove_file(sandbox.root().join("config.toml")).ok();
    crate::config::invalidate_config_cache();
}

#[test]
fn direct_login_provider_activation_sets_runtime_identity_and_active_provider() {
    // Sandbox JCODE_HOME so activation's env-file credential sync (#453)
    // cannot read the developer's real ~/.config/jcode/*.env files and
    // leak keys into this process during the matrix run.
    let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");

    for (provider, runtime, active) in [
        ("claude", "claude", "claude"),
        ("claude-api", "claude-api", "claude"),
        ("openai", "openai", "openai"),
        ("openai-api", "openai-api", "openai"),
        ("openrouter", "openrouter", "openrouter"),
        ("jcode", "jcode", "openrouter"),
        ("grok-build", "grok-build", "openrouter"),
        ("bedrock", "bedrock", "bedrock"),
        ("cursor", "cursor", "cursor"),
        ("copilot", "copilot", "copilot"),
        ("gemini", "gemini", "gemini"),
        ("antigravity", "antigravity", "antigravity"),
    ] {
        crate::env::remove_var("JCODE_RUNTIME_PROVIDER");
        crate::env::remove_var("JCODE_ACTIVE_PROVIDER");
        crate::env::remove_var("JCODE_INITIAL_PROVIDER_EXPLICIT");

        let activation = activate_auth_change(&AuthActivationRequest::new(
            None,
            Some(AuthChanged::new(provider)),
        ));

        assert_eq!(activation.provider_id.as_deref(), Some(provider));
        assert_eq!(
            std::env::var("JCODE_RUNTIME_PROVIDER").as_deref(),
            Ok(runtime)
        );
        assert_eq!(
            std::env::var("JCODE_ACTIVE_PROVIDER").as_deref(),
            Ok(active)
        );
        assert_eq!(
            std::env::var("JCODE_INITIAL_PROVIDER_EXPLICIT").as_deref(),
            Ok("1")
        );
    }
}

#[test]
fn direct_login_provider_descriptor_matrix_has_full_lifecycle_parity() {
    // Sandbox JCODE_HOME for the same reason as the activation matrix
    // above: keep the #453 credential sync away from real env files.
    let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");

    let mut covered = Vec::new();
    for provider in crate::provider_catalog::login_providers() {
        let Some((normalized, runtime, active, switch_prefix)) = (match provider.target {
            crate::provider_catalog::LoginProviderTarget::Jcode => {
                Some(("jcode", "jcode", "openrouter", ""))
            }
            crate::provider_catalog::LoginProviderTarget::GrokBuild => {
                Some(("grok-build", "grok-build", "openrouter", "grok-build"))
            }
            crate::provider_catalog::LoginProviderTarget::Claude => {
                Some(("claude", "claude", "claude", "claude-oauth"))
            }
            crate::provider_catalog::LoginProviderTarget::ClaudeApiKey => {
                Some(("claude-api", "claude-api", "claude", "claude-api"))
            }
            crate::provider_catalog::LoginProviderTarget::OpenAi => {
                Some(("openai", "openai", "openai", "openai-oauth"))
            }
            crate::provider_catalog::LoginProviderTarget::OpenAiApiKey => {
                Some(("openai-api", "openai-api", "openai", "openai-api"))
            }
            crate::provider_catalog::LoginProviderTarget::OpenRouter => {
                Some(("openrouter", "openrouter", "openrouter", "openrouter"))
            }
            crate::provider_catalog::LoginProviderTarget::Bedrock => {
                Some(("bedrock", "bedrock", "bedrock", "bedrock"))
            }
            crate::provider_catalog::LoginProviderTarget::Cursor => {
                Some(("cursor", "cursor", "cursor", "cursor"))
            }
            crate::provider_catalog::LoginProviderTarget::Copilot => {
                Some(("copilot", "copilot", "copilot", "copilot"))
            }
            crate::provider_catalog::LoginProviderTarget::Gemini => {
                Some(("gemini", "gemini", "gemini", "gemini"))
            }
            crate::provider_catalog::LoginProviderTarget::Antigravity => {
                Some(("antigravity", "antigravity", "antigravity", "antigravity"))
            }
            _ => None,
        }) else {
            continue;
        };

        covered.push(provider.id);
        assert_eq!(
            normalized_auth_provider_id(Some(provider.id)),
            Some(normalized),
            "{} descriptor id must normalize into the auth lifecycle",
            provider.id
        );
        for alias in provider.aliases {
            assert_eq!(
                normalized_auth_provider_id(Some(alias)),
                Some(normalized),
                "{} alias `{}` must normalize into the same auth lifecycle provider",
                provider.id,
                alias
            );
        }
        assert_eq!(
            provider_display_label(Some(provider.id)).as_deref(),
            Some(provider.display_name),
            "{} descriptor display label must be user-visible auth label",
            provider.id
        );

        crate::env::remove_var("JCODE_RUNTIME_PROVIDER");
        crate::env::remove_var("JCODE_ACTIVE_PROVIDER");
        crate::env::remove_var("JCODE_INITIAL_PROVIDER_EXPLICIT");

        let activation = activate_auth_change(&AuthActivationRequest::new(
            None,
            Some(AuthChanged::new(provider.id)),
        ));
        assert_eq!(activation.provider_id.as_deref(), Some(normalized));
        assert_eq!(
            activation.provider_label.as_deref(),
            Some(provider.display_name)
        );
        assert_eq!(
            std::env::var("JCODE_RUNTIME_PROVIDER").as_deref(),
            Ok(runtime)
        );
        assert_eq!(
            std::env::var("JCODE_ACTIVE_PROVIDER").as_deref(),
            Ok(active)
        );
        assert_eq!(
            std::env::var("JCODE_INITIAL_PROVIDER_EXPLICIT").as_deref(),
            Ok("1")
        );
        let expected_switch = if switch_prefix.is_empty() {
            "shared-model".to_string()
        } else {
            format!("{switch_prefix}:shared-model")
        };
        assert_eq!(
            activation.model_switch_request("ignored-runtime", "shared-model"),
            expected_switch,
            "{} direct auth model switch must preserve its canonical route identity",
            provider.id
        );
    }

    for expected in [
        "claude",
        "anthropic-api",
        "openai",
        "openai-api",
        "openrouter",
        "jcode",
        "grok-build",
        "bedrock",
        "cursor",
        "copilot",
        "gemini",
        "antigravity",
    ] {
        assert!(
            covered.contains(&expected),
            "direct provider parity matrix did not cover {expected}: {covered:?}"
        );
    }
}

#[test]
fn model_switch_request_prefixes_openai_compatible_profiles_with_profile_id() {
    assert_eq!(
        model_switch_request_for_provider_id(Some("cerebras"), "mock-auth", "llama3.1-8b"),
        "cerebras:llama3.1-8b"
    );
    assert_eq!(
        model_switch_request_for_provider_id(Some("cerebras"), "openrouter", "llama3.1-8b"),
        "cerebras:llama3.1-8b"
    );
}

#[test]
fn model_switch_request_is_provider_explicit_for_all_auth_providers() {
    for (provider, expected) in [
        ("claude", "claude-oauth:shared-model"),
        ("anthropic", "claude-oauth:shared-model"),
        ("anthropic-api", "claude-api:shared-model"),
        ("openai", "openai-oauth:shared-model"),
        ("openai-api", "openai-api:shared-model"),
        ("openrouter", "openrouter:shared-model"),
        ("jcode", "shared-model"),
        ("azure-openai", "openrouter:shared-model"),
        ("bedrock", "bedrock:shared-model"),
        ("cursor", "cursor:shared-model"),
        ("copilot", "copilot:shared-model"),
        ("gemini", "gemini:shared-model"),
        ("antigravity", "antigravity:shared-model"),
        ("cerebras", "cerebras:shared-model"),
    ] {
        assert_eq!(
            model_switch_request_for_provider_id(Some(provider), "mock-auth", "shared-model"),
            expected,
            "{provider} auth switch request must route explicitly so duplicate model IDs cannot select the wrong provider"
        );
    }
}

#[test]
fn jcode_auth_lifecycle_matches_only_managed_subscription_routes() {
    let activation = AuthActivationResult {
        provider_id: Some("jcode".to_string()),
        provider_label: Some("Jcode Subscription".to_string()),
        activated_model: Some("gpt-5.5".to_string()),
        expected_runtime: Some("jcode-subscription".to_string()),
        expected_catalog_namespace: Some("jcode-subscription".to_string()),
    };
    let routes = vec![
        route("gpt-5.5", "OpenRouter", "openrouter", true),
        route(
            "gpt-5.5",
            "Jcode Subscription",
            crate::subscription_catalog::JCODE_ROUTE_API_METHOD,
            true,
        ),
    ];

    let report = validate_catalog_invariants(&activation, Some("gpt-5.5"), &routes);
    assert!(
        report.ok(),
        "canonical Jcode route should match: {report:?}"
    );
    assert_eq!(report.selectable_provider_routes, 1);
    assert_eq!(
        report.route_sample,
        vec![format!(
            "`gpt-5.5` via {}",
            crate::subscription_catalog::JCODE_ROUTE_API_METHOD
        )]
    );
    assert_eq!(
        activation.model_switch_request("Jcode Subscription", "gpt-5.5"),
        "gpt-5.5"
    );
}

#[test]
fn post_auth_model_selection_reselects_duplicate_model_name_from_matching_provider_route() {
    let activation = AuthActivationResult {
        provider_id: Some("cerebras".to_string()),
        provider_label: Some("Cerebras".to_string()),
        activated_model: Some("llama3.1-8b".to_string()),
        expected_runtime: Some("openai-compatible".to_string()),
        expected_catalog_namespace: Some("cerebras".to_string()),
    };
    let routes = vec![
        route(
            "llama3.1-8b",
            "Other Gateway",
            "openai-compatible:other",
            true,
        ),
        route(
            "llama3.1-8b",
            "Cerebras",
            "openai-compatible:cerebras",
            true,
        ),
    ];

    assert_eq!(
        provider_model_to_select_after_auth(&activation, Some("llama3.1-8b"), &routes),
        Some("llama3.1-8b".to_string()),
        "duplicate model IDs must force an explicit provider-profile model switch"
    );
}

#[test]
fn catalog_invariants_pass_when_selected_model_matches_provider_route() {
    let activation = AuthActivationResult {
        provider_id: Some("cerebras".to_string()),
        provider_label: Some("Cerebras".to_string()),
        activated_model: Some("llama3.1-8b".to_string()),
        expected_runtime: Some("openai-compatible".to_string()),
        expected_catalog_namespace: Some("cerebras".to_string()),
    };
    let routes = vec![
        route("gpt-5.5", "OpenAI", "openai", true),
        route(
            "llama3.1-8b",
            "Cerebras",
            "openai-compatible:cerebras",
            true,
        ),
    ];

    let report = validate_catalog_invariants(&activation, Some("llama3.1-8b"), &routes);

    assert!(
        report.ok(),
        "unexpected warning: {:?}",
        report.warning_message()
    );
    assert_eq!(report.selectable_provider_routes, 1);
}

#[test]
fn catalog_invariants_reject_generic_openai_compatible_route_for_namespaced_auth() {
    let activation = AuthActivationResult {
        provider_id: Some("cerebras".to_string()),
        provider_label: Some("Cerebras".to_string()),
        activated_model: Some("llama3.1-8b".to_string()),
        expected_runtime: Some("openai-compatible".to_string()),
        expected_catalog_namespace: Some("cerebras".to_string()),
    };
    let routes = vec![route("llama3.1-8b", "Cerebras", "openai-compatible", true)];

    let report = validate_catalog_invariants(&activation, Some("llama3.1-8b"), &routes);

    assert!(
        !report.ok(),
        "generic openai-compatible route should not satisfy namespaced auth: {report:?}"
    );
    assert_eq!(report.selectable_provider_routes, 0);
    assert!(
        report
            .warning_message()
            .expect("warning")
            .contains("Expected selectable Cerebras model routes")
    );
}

#[test]
fn catalog_invariants_warn_when_selected_model_is_from_stale_provider() {
    let activation = AuthActivationResult {
        provider_id: Some("cerebras".to_string()),
        provider_label: Some("Cerebras".to_string()),
        activated_model: Some("llama3.1-8b".to_string()),
        expected_runtime: Some("openai-compatible".to_string()),
        expected_catalog_namespace: Some("cerebras".to_string()),
    };
    let routes = vec![route("gpt-5.5", "OpenAI", "openai", true)];

    let report = validate_catalog_invariants(&activation, Some("gpt-5.5"), &routes);

    assert!(!report.ok());
    let warning = report.warning_message().expect("warning expected");
    assert!(warning.contains("Expected selectable Cerebras model routes"));
    assert!(warning.contains("Selected model: `gpt-5.5`"));
}

#[test]
fn post_auth_model_selection_prefers_matching_provider_route_over_stale_model() {
    let activation = AuthActivationResult {
        provider_id: Some("cerebras".to_string()),
        provider_label: Some("Cerebras".to_string()),
        activated_model: Some("qwen-3-235b-a22b-instruct-2507".to_string()),
        expected_runtime: Some("openai-compatible".to_string()),
        expected_catalog_namespace: Some("cerebras".to_string()),
    };
    let routes = vec![
        route("gpt-5.5", "OpenAI", "openai", true),
        route(
            "qwen-3-235b-a22b-instruct-2507",
            "Cerebras",
            "openai-compatible:cerebras",
            true,
        ),
        route(
            "llama3.1-8b",
            "Cerebras",
            "openai-compatible:cerebras",
            true,
        ),
    ];

    assert_eq!(
        provider_model_to_select_after_auth(&activation, Some("gpt-5.5"), &routes).as_deref(),
        Some("qwen-3-235b-a22b-instruct-2507")
    );
    assert_eq!(
        provider_model_to_select_after_auth(
            &activation,
            Some("qwen-3-235b-a22b-instruct-2507"),
            &routes
        ),
        None
    );
}

#[test]
fn post_auth_model_selection_prefers_anthropic_flagship_over_catalog_order() {
    // Live Anthropic catalogs list `claude-haiku-4-5-...` before the
    // flagship, and an API-key login supplies no activated model. Plain
    // catalog order would auto-select Haiku; the flagship-first fallback
    // must land on the curated quality-first default (Opus 5) instead.
    let activation = AuthActivationResult {
        provider_id: Some("claude-api".to_string()),
        provider_label: Some("Anthropic".to_string()),
        activated_model: None,
        expected_runtime: None,
        expected_catalog_namespace: None,
    };
    let routes = vec![
        route("claude-haiku-4-5-20251001", "Anthropic", "claude-api", true),
        route("claude-opus-4-6", "Anthropic", "claude-api", true),
        route("claude-opus-4-8", "Anthropic", "claude-api", true),
        route("claude-opus-5", "Anthropic", "claude-api", true),
        route("claude-fable-5", "Anthropic", "claude-api", true),
        route("claude-sonnet-4-6", "Anthropic", "claude-api", true),
    ];

    assert_eq!(
        provider_model_to_select_after_auth(&activation, None, &routes).as_deref(),
        Some("claude-opus-5"),
        "API-key login should auto-select the Anthropic flagship, not the first catalog route"
    );
}

#[test]
fn post_auth_model_selection_preserves_configured_claude_model() {
    let activation = AuthActivationResult {
        provider_id: Some("claude-api".to_string()),
        provider_label: Some("Anthropic".to_string()),
        activated_model: None,
        expected_runtime: None,
        expected_catalog_namespace: None,
    };
    let routes = vec![
        route(
            jcode_provider_core::DEFAULT_CLAUDE_MODEL,
            "Anthropic",
            "claude-api",
            true,
        ),
        route("claude-opus-4-6", "Anthropic", "claude-api", true),
    ];

    assert_eq!(
        provider_model_to_select_after_auth_with_configured_default(
            &activation,
            Some("claude-opus-4-6"),
            Some(jcode_provider_core::DEFAULT_CLAUDE_MODEL),
            &routes,
        )
        .as_deref(),
        Some("claude-opus-4-6")
    );
}

#[test]
fn post_auth_model_selection_preserves_provider_prefixed_configured_default() {
    // Regression: config.provider.default_model is persisted by the model
    // picker as a full spec with an explicit provider prefix (e.g.
    // `claude-oauth:claude-sonnet-5`). Comparing that raw string against
    // route.model (always bare) must not silently miss and fall through
    // to the flagship-first pick (Opus) on every /login or
    // /refresh-model-list.
    let activation = AuthActivationResult {
        provider_id: Some("claude".to_string()),
        provider_label: Some("Anthropic".to_string()),
        activated_model: None,
        expected_runtime: None,
        expected_catalog_namespace: None,
    };
    let routes = vec![
        route(
            jcode_provider_core::DEFAULT_CLAUDE_MODEL,
            "Anthropic",
            "claude-oauth",
            true,
        ),
        route("claude-sonnet-5", "Anthropic", "claude-oauth", true),
    ];

    assert_eq!(
        provider_model_to_select_after_auth_with_configured_default(
            &activation,
            Some("claude-oauth:claude-sonnet-5"),
            Some(jcode_provider_core::DEFAULT_CLAUDE_MODEL),
            &routes,
        )
        .as_deref(),
        Some("claude-sonnet-5")
    );
}

#[test]
fn post_auth_model_selection_prefers_claude_oauth_flagship() {
    let activation = AuthActivationResult {
        provider_id: Some("claude".to_string()),
        provider_label: Some("Anthropic".to_string()),
        activated_model: None,
        expected_runtime: None,
        expected_catalog_namespace: None,
    };
    let routes = vec![
        route("claude-haiku-4-5", "Anthropic", "claude-oauth", true),
        route("claude-opus-4-8", "Anthropic", "claude-oauth", true),
        route("claude-opus-5", "Anthropic", "claude-oauth", true),
        route("claude-fable-5", "Anthropic", "claude-oauth", true),
    ];

    assert_eq!(
        provider_model_to_select_after_auth(&activation, None, &routes).as_deref(),
        Some("claude-opus-5")
    );
}

#[test]
fn post_auth_model_selection_prefers_openai_flagship_over_catalog_order() {
    let activation = AuthActivationResult {
        provider_id: Some("openai-api".to_string()),
        provider_label: Some("OpenAI".to_string()),
        activated_model: None,
        expected_runtime: None,
        expected_catalog_namespace: None,
    };
    let routes = vec![
        route("gpt-5.1", "OpenAI", "openai-api", true),
        route("gpt-5.5", "OpenAI", "openai-api", true),
        route("gpt-5.6-sol", "OpenAI", "openai-api", true),
    ];

    assert_eq!(
        provider_model_to_select_after_auth(&activation, None, &routes).as_deref(),
        Some("gpt-5.6-sol")
    );
}
