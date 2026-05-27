use super::*;
use crate::provider_catalog::{self, resolve_login_selection, resolve_openai_compatible_profile};
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use tempfile::TempDir;

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    let mutex = ENV_LOCK.get_or_init(|| Mutex::new(()));
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[test]
#[allow(deprecated)]
fn test_provider_choice_arg_values() {
    assert_eq!(ProviderChoice::Jcode.as_arg_value(), "jcode");
    assert_eq!(ProviderChoice::Claude.as_arg_value(), "claude");
    assert_eq!(ProviderChoice::AnthropicApi.as_arg_value(), "anthropic-api");
    assert_eq!(
        ProviderChoice::ClaudeSubprocess.as_arg_value(),
        "claude-subprocess"
    );
    assert_eq!(ProviderChoice::Openai.as_arg_value(), "openai");
    assert_eq!(ProviderChoice::OpenaiApi.as_arg_value(), "openai-api");
    assert_eq!(ProviderChoice::Openrouter.as_arg_value(), "openrouter");
    assert_eq!(ProviderChoice::Bedrock.as_arg_value(), "bedrock");
    assert_eq!(ProviderChoice::Azure.as_arg_value(), "azure");
    assert_eq!(ProviderChoice::Opencode.as_arg_value(), "opencode");
    assert_eq!(ProviderChoice::OpencodeGo.as_arg_value(), "opencode-go");
    assert_eq!(ProviderChoice::Zai.as_arg_value(), "zai");
    assert_eq!(ProviderChoice::Groq.as_arg_value(), "groq");
    assert_eq!(ProviderChoice::Mistral.as_arg_value(), "mistral");
    assert_eq!(ProviderChoice::Perplexity.as_arg_value(), "perplexity");
    assert_eq!(ProviderChoice::TogetherAi.as_arg_value(), "togetherai");
    assert_eq!(ProviderChoice::Deepinfra.as_arg_value(), "deepinfra");
    assert_eq!(ProviderChoice::Fireworks.as_arg_value(), "fireworks");
    assert_eq!(ProviderChoice::Minimax.as_arg_value(), "minimax");
    assert_eq!(ProviderChoice::Xai.as_arg_value(), "xai");
    assert_eq!(ProviderChoice::XiaomiMimo.as_arg_value(), "xiaomi-mimo");
    assert_eq!(ProviderChoice::Lmstudio.as_arg_value(), "lmstudio");
    assert_eq!(ProviderChoice::Ollama.as_arg_value(), "ollama");
    assert_eq!(ProviderChoice::Chutes.as_arg_value(), "chutes");
    assert_eq!(ProviderChoice::Cerebras.as_arg_value(), "cerebras");
    assert_eq!(
        ProviderChoice::AlibabaCodingPlan.as_arg_value(),
        "alibaba-coding-plan"
    );
    assert_eq!(
        ProviderChoice::OpenaiCompatible.as_arg_value(),
        "openai-compatible"
    );
    assert_eq!(ProviderChoice::Cursor.as_arg_value(), "cursor");
    assert_eq!(ProviderChoice::Copilot.as_arg_value(), "copilot");
    assert_eq!(ProviderChoice::Gemini.as_arg_value(), "gemini");
    assert_eq!(ProviderChoice::Antigravity.as_arg_value(), "antigravity");
    assert_eq!(ProviderChoice::Google.as_arg_value(), "google");
    assert_eq!(ProviderChoice::Auto.as_arg_value(), "auto");
}

#[test]
fn test_server_bootstrap_login_selection_preserves_order() {
    let providers = provider_catalog::server_bootstrap_login_providers();
    assert_eq!(
        resolve_login_selection("1", &providers).map(|provider| provider.id),
        Some("claude")
    );
    assert_eq!(
        resolve_login_selection("2", &providers).map(|provider| provider.id),
        Some("anthropic-api")
    );
    assert_eq!(
        resolve_login_selection("4", &providers).map(|provider| provider.id),
        Some("jcode")
    );
    assert_eq!(
        resolve_login_selection("5", &providers).map(|provider| provider.id),
        Some("copilot")
    );
}

#[test]
fn test_auto_init_login_selection_preserves_order() {
    let providers = provider_catalog::auto_init_login_providers();
    assert_eq!(
        resolve_login_selection("1", &providers).map(|provider| provider.id),
        Some("claude")
    );
    assert_eq!(
        resolve_login_selection("2", &providers).map(|provider| provider.id),
        Some("anthropic-api")
    );
    assert_eq!(
        resolve_login_selection("11", &providers).map(|provider| provider.id),
        Some("alibaba-coding-plan")
    );
    assert_eq!(
        resolve_login_selection("12", &providers).map(|provider| provider.id),
        Some("cursor")
    );
    assert_eq!(
        resolve_login_selection("13", &providers).map(|provider| provider.id),
        Some("copilot")
    );
    assert_eq!(
        resolve_login_selection("14", &providers).map(|provider| provider.id),
        Some("gemini")
    );
    assert_eq!(
        resolve_login_selection("15", &providers).map(|provider| provider.id),
        Some("antigravity")
    );
}

#[test]
fn test_init_provider_jcode_delegates_runtime_profile_to_wrapper() {
    let _guard = lock_env();
    let _env_guard = crate::storage::lock_test_env();
    crate::subscription_catalog::clear_runtime_env();
    crate::env::remove_var("JCODE_OPENROUTER_MODEL");
    crate::env::remove_var("JCODE_RUNTIME_PROVIDER");
    crate::env::remove_var("JCODE_ACTIVE_PROVIDER");
    crate::env::remove_var("JCODE_FORCE_PROVIDER");

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let provider = runtime
        .block_on(init_provider(&ProviderChoice::Jcode, None))
        .expect("init jcode provider");

    assert_eq!(provider.name(), "Jcode Subscription");
    assert!(crate::subscription_catalog::is_runtime_mode_enabled());
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_MODEL").ok().as_deref(),
        Some(crate::subscription_catalog::default_model().id)
    );
    assert_eq!(
        std::env::var("JCODE_ACTIVE_PROVIDER").ok().as_deref(),
        Some("openrouter")
    );
    assert_eq!(
        std::env::var("JCODE_RUNTIME_PROVIDER").ok().as_deref(),
        Some("jcode")
    );
    assert_eq!(
        std::env::var("JCODE_FORCE_PROVIDER").ok().as_deref(),
        Some("1")
    );

    crate::subscription_catalog::clear_runtime_env();
    crate::env::remove_var("JCODE_OPENROUTER_MODEL");
    crate::env::remove_var("JCODE_RUNTIME_PROVIDER");
    crate::env::remove_var("JCODE_ACTIVE_PROVIDER");
    crate::env::remove_var("JCODE_FORCE_PROVIDER");
}

#[test]
fn test_openai_compatible_profile_overrides() {
    let _guard = lock_env();
    let keys = [
        "JCODE_OPENAI_COMPAT_API_BASE",
        "JCODE_OPENAI_COMPAT_API_KEY_NAME",
        "JCODE_OPENAI_COMPAT_ENV_FILE",
        "JCODE_OPENAI_COMPAT_DEFAULT_MODEL",
    ];
    let saved: Vec<(String, Option<String>)> = keys
        .iter()
        .map(|k| (k.to_string(), std::env::var(k).ok()))
        .collect();

    crate::env::set_var(
        "JCODE_OPENAI_COMPAT_API_BASE",
        "https://api.groq.com/openai/v1/",
    );
    crate::env::set_var("JCODE_OPENAI_COMPAT_API_KEY_NAME", "GROQ_API_KEY");
    crate::env::set_var("JCODE_OPENAI_COMPAT_ENV_FILE", "groq.env");
    crate::env::set_var("JCODE_OPENAI_COMPAT_DEFAULT_MODEL", "openai/gpt-oss-120b");

    let resolved = resolve_openai_compatible_profile(provider_catalog::OPENAI_COMPAT_PROFILE);
    assert_eq!(resolved.api_base, "https://api.groq.com/openai/v1");
    assert_eq!(resolved.api_key_env, "GROQ_API_KEY");
    assert_eq!(resolved.env_file, "groq.env");
    assert_eq!(
        resolved.default_model.as_deref(),
        Some("openai/gpt-oss-120b")
    );

    for (key, value) in saved {
        if let Some(value) = value {
            crate::env::set_var(&key, value);
        } else {
            crate::env::remove_var(&key);
        }
    }
}

#[test]
fn test_openai_compatible_profile_rejects_invalid_overrides() {
    let _guard = lock_env();
    let keys = [
        "JCODE_OPENAI_COMPAT_API_BASE",
        "JCODE_OPENAI_COMPAT_API_KEY_NAME",
        "JCODE_OPENAI_COMPAT_ENV_FILE",
    ];
    let saved: Vec<(String, Option<String>)> = keys
        .iter()
        .map(|k| (k.to_string(), std::env::var(k).ok()))
        .collect();

    crate::env::set_var("JCODE_OPENAI_COMPAT_API_BASE", "http://example.com/v1");
    crate::env::set_var("JCODE_OPENAI_COMPAT_API_KEY_NAME", "bad-key-name");
    crate::env::set_var("JCODE_OPENAI_COMPAT_ENV_FILE", "../bad.env");

    let resolved = resolve_openai_compatible_profile(provider_catalog::OPENAI_COMPAT_PROFILE);
    assert_eq!(
        resolved.api_base,
        provider_catalog::OPENAI_COMPAT_PROFILE.api_base
    );
    assert_eq!(
        resolved.api_key_env,
        provider_catalog::OPENAI_COMPAT_PROFILE.api_key_env
    );
    assert_eq!(
        resolved.env_file,
        provider_catalog::OPENAI_COMPAT_PROFILE.env_file
    );

    for (key, value) in saved {
        if let Some(value) = value {
            crate::env::set_var(&key, value);
        } else {
            crate::env::remove_var(&key);
        }
    }
}

#[test]
fn parse_external_auth_review_selection_supports_all_and_deduped_indices() {
    assert_eq!(
        parse_external_auth_review_selection("", 3).unwrap(),
        Vec::<usize>::new()
    );
    assert_eq!(
        parse_external_auth_review_selection("a", 3).unwrap(),
        vec![0, 1, 2]
    );
    assert_eq!(
        parse_external_auth_review_selection("2,1,2", 3).unwrap(),
        vec![1, 0]
    );
    assert!(parse_external_auth_review_selection("4", 3).is_err());
    assert!(parse_external_auth_review_selection("nope", 3).is_err());
}

#[test]
fn parse_login_provider_selection_supports_skip_and_names() {
    let providers = provider_catalog::cli_login_providers();

    assert!(
        parse_login_provider_selection_input("", &providers)
            .unwrap()
            .is_none()
    );
    assert!(
        parse_login_provider_selection_input("skip", &providers)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        parse_login_provider_selection_input("claude", &providers)
            .unwrap()
            .map(|provider| provider.id),
        Some("claude")
    );
    let first_provider = providers[0].id;
    assert_eq!(
        parse_login_provider_selection_input("1", &providers)
            .unwrap()
            .map(|provider| provider.id),
        Some(first_provider)
    );
    assert!(parse_login_provider_selection_input("not-a-provider", &providers).is_err());
}

#[test]
fn login_provider_menu_shows_autodetected_auth_and_skip() {
    let providers = vec![
        provider_catalog::CLAUDE_LOGIN_PROVIDER,
        provider_catalog::OPENAI_LOGIN_PROVIDER,
    ];
    let status = auth::AuthStatus {
        anthropic: auth::ProviderAuth {
            state: auth::AuthState::Available,
            has_oauth: true,
            has_api_key: false,
        },
        ..Default::default()
    };

    let menu = render_login_provider_selection_menu("Choose a provider:", &providers, &status);
    assert!(menu.contains("Autodetected auth:"));
    assert!(menu.contains("Anthropic/Claude: configured: OAuth"));
    assert!(menu.contains("[configured"));
    assert!(menu.contains("[not configured"));
    assert!(menu.contains("Skip: press Enter"));
}

#[test]
fn choice_for_login_provider_round_trips_core_targets() {
    assert_eq!(
        choice_for_login_provider(provider_catalog::JCODE_LOGIN_PROVIDER),
        Some(ProviderChoice::Jcode)
    );
    assert_eq!(
        choice_for_login_provider(provider_catalog::OPENROUTER_LOGIN_PROVIDER),
        Some(ProviderChoice::Openrouter)
    );
    assert_eq!(
        choice_for_login_provider(provider_catalog::ANTHROPIC_API_LOGIN_PROVIDER),
        Some(ProviderChoice::AnthropicApi)
    );
    assert_eq!(
        choice_for_login_provider(provider_catalog::AZURE_LOGIN_PROVIDER),
        Some(ProviderChoice::Azure)
    );
    assert_eq!(
        choice_for_login_provider(provider_catalog::CURSOR_LOGIN_PROVIDER),
        Some(ProviderChoice::Cursor)
    );
    assert_eq!(
        choice_for_login_provider(provider_catalog::AUTO_IMPORT_LOGIN_PROVIDER),
        None
    );
}

#[test]
fn choice_for_login_provider_round_trips_openai_compatible_profiles() {
    assert_eq!(
        choice_for_login_provider(provider_catalog::OPENCODE_LOGIN_PROVIDER),
        Some(ProviderChoice::Opencode)
    );
    assert_eq!(
        choice_for_login_provider(provider_catalog::LMSTUDIO_LOGIN_PROVIDER),
        Some(ProviderChoice::Lmstudio)
    );
    assert_eq!(
        choice_for_login_provider(provider_catalog::OPENAI_COMPAT_LOGIN_PROVIDER),
        Some(ProviderChoice::OpenaiCompatible)
    );
}

#[test]
fn login_provider_choice_table_round_trips_catalog_providers() {
    let mut seen_choices = HashSet::new();
    let mut reverse_mapped_provider_ids = HashSet::new();

    for (choice, provider) in login_provider_choice_mappings() {
        assert!(
            seen_choices.insert(choice.as_arg_value()),
            "duplicate provider choice mapping for {}",
            choice.as_arg_value()
        );
        assert_eq!(
            login_provider_for_choice(choice).map(|candidate| candidate.id),
            Some(provider.id),
            "choice {} should resolve to {}",
            choice.as_arg_value(),
            provider.id
        );

        if reverse_mapped_provider_ids.insert(provider.id) {
            assert_eq!(
                choice_for_login_provider(*provider),
                Some(*choice),
                "provider {} should reverse-map to {}",
                provider.id,
                choice.as_arg_value()
            );
        }
    }

    for provider in provider_catalog::login_providers() {
        if matches!(
            provider.target,
            provider_catalog::LoginProviderTarget::AutoImport
        ) {
            assert_eq!(choice_for_login_provider(*provider), None);
        } else {
            assert!(
                reverse_mapped_provider_ids.contains(provider.id),
                "provider {} is in the catalog but not the CLI choice table",
                provider.id
            );
        }
    }
}

#[test]
fn auth_integration_registry_matches_cli_choice_runtime_wiring() {
    for provider in provider_catalog::login_providers() {
        let integration = crate::auth::integration::auth_provider_integration(provider.id)
            .expect("catalog provider should have integration metadata");
        assert_eq!(integration.descriptor, *provider);

        if !matches!(
            provider.target,
            provider_catalog::LoginProviderTarget::AutoImport
        ) {
            assert!(
                choice_for_login_provider(*provider).is_some(),
                "provider {} is missing a CLI choice mapping",
                provider.id
            );
        }

        let status = auth::AuthStatus::default();
        let assessment = status.assessment_for_provider(*provider);
        assert!(
            !assessment.method_detail.is_empty(),
            "provider {} should have non-empty auth status method detail",
            provider.id
        );
    }
}

#[test]
fn resolved_profile_default_model_uses_openai_compatible_override() {
    let _guard = lock_env();
    let _env_guard = crate::storage::lock_test_env();
    let saved: Vec<(String, Option<String>)> = [
        "JCODE_OPENAI_COMPAT_API_BASE",
        "JCODE_OPENAI_COMPAT_API_KEY_NAME",
        "JCODE_OPENAI_COMPAT_ENV_FILE",
        "JCODE_OPENAI_COMPAT_DEFAULT_MODEL",
    ]
    .iter()
    .map(|k| (k.to_string(), std::env::var(k).ok()))
    .collect();

    crate::env::set_var("JCODE_OPENAI_COMPAT_API_BASE", "http://localhost:11434/v1");
    crate::env::set_var("JCODE_OPENAI_COMPAT_DEFAULT_MODEL", "llama3.2");

    assert_eq!(
        resolved_profile_default_model(provider_catalog::OPENAI_COMPAT_PROFILE).as_deref(),
        Some("llama3.2")
    );

    for (key, value) in saved {
        if let Some(value) = value {
            crate::env::set_var(&key, value);
        } else {
            crate::env::remove_var(&key);
        }
    }
}

#[test]
fn apply_login_provider_profile_env_locks_compatible_profile_for_auto_spawn() {
    let _guard = lock_env();
    let _env_guard = crate::storage::lock_test_env();
    let saved: Vec<(String, Option<String>)> = [
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_OPENROUTER_PROVIDER_FEATURES",
        "JCODE_OPENROUTER_ALLOW_NO_AUTH",
        "JCODE_OPENROUTER_STATIC_MODELS",
        "JCODE_PROVIDER_PROFILE_ACTIVE",
        "JCODE_PROVIDER_PROFILE_NAME",
        "JCODE_NAMED_PROVIDER_PROFILE",
    ]
    .iter()
    .map(|k| (k.to_string(), std::env::var(k).ok()))
    .collect();

    for (key, _) in &saved {
        crate::env::remove_var(key);
    }

    apply_login_provider_profile_env(provider_catalog::OPENCODE_GO_LOGIN_PROVIDER);

    assert_eq!(
        std::env::var("JCODE_OPENROUTER_API_BASE").ok().as_deref(),
        Some("https://opencode.ai/zen/go/v1")
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_API_KEY_NAME")
            .ok()
            .as_deref(),
        Some("OPENCODE_GO_API_KEY")
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_ENV_FILE").ok().as_deref(),
        Some("opencode-go.env")
    );
    assert_eq!(
        std::env::var("JCODE_PROVIDER_PROFILE_ACTIVE")
            .ok()
            .as_deref(),
        Some("1")
    );

    // Mirrors the daemon child process starting with `--provider auto`: with the
    // active marker present, auto init must not erase the selected profile env.
    provider_catalog::apply_openai_compatible_profile_env(None);
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_API_KEY_NAME")
            .ok()
            .as_deref(),
        Some("OPENCODE_GO_API_KEY")
    );

    // A later explicit compatible-provider selection in the same process must
    // still replace the active profile instead of being blocked by the marker.
    apply_login_provider_profile_env(provider_catalog::OPENCODE_LOGIN_PROVIDER);
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_API_KEY_NAME")
            .ok()
            .as_deref(),
        Some("OPENCODE_API_KEY")
    );
    assert_eq!(
        std::env::var("JCODE_PROVIDER_PROFILE_ACTIVE")
            .ok()
            .as_deref(),
        Some("1")
    );

    for (key, value) in saved {
        if let Some(value) = value {
            crate::env::set_var(&key, value);
        } else {
            crate::env::remove_var(&key);
        }
    }
}

#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "test env locks intentionally stay held across provider init to isolate process-global runtime env"
)]
async fn init_provider_for_ollama_reapplies_local_compat_runtime_env_after_disabling_subscription_mode()
 {
    let _guard = lock_env();
    let _env_guard = crate::storage::lock_test_env();
    let dir = TempDir::new().expect("temp dir");
    let saved: Vec<(String, Option<String>)> = [
        "JCODE_HOME",
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_OPENROUTER_PROVIDER_FEATURES",
        "JCODE_OPENROUTER_ALLOW_NO_AUTH",
        "JCODE_RUNTIME_PROVIDER",
        "JCODE_FORCE_PROVIDER",
        "JCODE_ACTIVE_PROVIDER",
    ]
    .iter()
    .map(|k| (k.to_string(), std::env::var(k).ok()))
    .collect();

    crate::env::set_var("JCODE_HOME", dir.path());
    crate::subscription_catalog::apply_runtime_env();

    let provider = init_provider_for_validation(&ProviderChoice::Ollama, Some("llama3.2"))
        .await
        .expect("init ollama provider");

    assert_eq!(
        std::env::var("JCODE_OPENROUTER_API_BASE").ok().as_deref(),
        Some("http://localhost:11434/v1")
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_API_KEY_NAME")
            .ok()
            .as_deref(),
        Some("OLLAMA_API_KEY")
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_ENV_FILE").ok().as_deref(),
        Some("ollama.env")
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_ALLOW_NO_AUTH")
            .ok()
            .as_deref(),
        Some("1")
    );
    assert_eq!(
        std::env::var("JCODE_FORCE_PROVIDER").ok().as_deref(),
        Some("1")
    );
    assert_eq!(
        std::env::var("JCODE_ACTIVE_PROVIDER").ok().as_deref(),
        Some("openrouter")
    );
    assert_eq!(
        std::env::var("JCODE_RUNTIME_PROVIDER").ok().as_deref(),
        Some("openai-compatible")
    );
    assert_eq!(provider.name(), "openrouter");
    assert_eq!(provider.model(), "llama3.2");

    for (key, value) in saved {
        if let Some(value) = value {
            crate::env::set_var(&key, value);
        } else {
            crate::env::remove_var(&key);
        }
    }
}

#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "test env locks intentionally stay held across provider init to isolate process-global runtime env"
)]
async fn auto_provider_uses_config_default_named_no_auth_provider() {
    let _guard = lock_env();
    let _env_guard = crate::storage::lock_test_env();
    let dir = TempDir::new().expect("temp dir");
    let saved: Vec<(String, Option<String>)> = [
        "JCODE_HOME",
        "JCODE_NON_INTERACTIVE",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "GITHUB_TOKEN",
        "GEMINI_API_KEY",
        "CURSOR_API_KEY",
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ALLOW_NO_AUTH",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_DEFAULT_MODEL",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_PROVIDER_PROFILE_ACTIVE",
        "JCODE_PROVIDER_PROFILE_NAME",
        "JCODE_NAMED_PROVIDER_PROFILE",
        "JCODE_RUNTIME_PROVIDER",
        "JCODE_ACTIVE_PROVIDER",
        "JCODE_FORCE_PROVIDER",
    ]
    .iter()
    .map(|k| (k.to_string(), std::env::var(k).ok()))
    .collect();

    crate::env::set_var("JCODE_HOME", dir.path());
    crate::env::set_var("JCODE_NON_INTERACTIVE", "1");
    for key in [
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "GITHUB_TOKEN",
        "GEMINI_API_KEY",
        "CURSOR_API_KEY",
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ALLOW_NO_AUTH",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_DEFAULT_MODEL",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_PROVIDER_PROFILE_ACTIVE",
        "JCODE_PROVIDER_PROFILE_NAME",
        "JCODE_NAMED_PROVIDER_PROFILE",
        "JCODE_RUNTIME_PROVIDER",
        "JCODE_ACTIVE_PROVIDER",
        "JCODE_FORCE_PROVIDER",
    ] {
        crate::env::remove_var(key);
    }
    std::fs::write(
        dir.path().join("config.toml"),
        r#"
[provider]
default_provider = "ollama-local"
default_model = "llama3.1:8b"

[providers.ollama-local]
type = "openai-compatible"
base_url = "http://localhost:11434/v1"
auth = "none"
default_model = "llama3.1:8b"
requires_api_key = false

[[providers.ollama-local.models]]
id = "llama3.1:8b"
"#,
    )
    .expect("write config");
    crate::config::invalidate_config_cache();

    let provider = init_provider_for_validation(&ProviderChoice::Auto, None)
        .await
        .expect("auto provider should honor config default_provider named profile");

    assert_eq!(provider.model(), "llama3.1:8b");
    assert!(provider.model_routes().iter().any(|route| {
        route.provider == "ollama-local" && route.model == "llama3.1:8b" && route.available
    }));

    for (key, value) in saved {
        if let Some(value) = value {
            crate::env::set_var(&key, value);
        } else {
            crate::env::remove_var(&key);
        }
    }
    crate::config::invalidate_config_cache();
}

#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "test env locks intentionally stay held across provider init to isolate process-global auth env"
)]
async fn auto_provider_noninteractive_skips_untrusted_external_auth_instead_of_blocking() {
    let _guard = lock_env();
    let _env_guard = crate::storage::lock_test_env();
    let dir = TempDir::new().expect("temp dir");
    let saved: Vec<(String, Option<String>)> = [
        "JCODE_HOME",
        "JCODE_NON_INTERACTIVE",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "GITHUB_TOKEN",
        "GEMINI_API_KEY",
        "CURSOR_API_KEY",
        "JCODE_RUNTIME_PROVIDER",
        "JCODE_ACTIVE_PROVIDER",
        "JCODE_FORCE_PROVIDER",
    ]
    .iter()
    .map(|k| (k.to_string(), std::env::var(k).ok()))
    .collect();

    crate::env::set_var("JCODE_HOME", dir.path());
    crate::env::set_var("JCODE_NON_INTERACTIVE", "1");
    for key in [
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "GITHUB_TOKEN",
        "GEMINI_API_KEY",
        "CURSOR_API_KEY",
        "JCODE_ACTIVE_PROVIDER",
        "JCODE_FORCE_PROVIDER",
    ] {
        crate::env::remove_var(key);
    }

    let opencode_path = crate::auth::claude::ExternalClaudeAuthSource::OpenCode
        .path()
        .expect("opencode path");
    std::fs::create_dir_all(opencode_path.parent().expect("opencode parent"))
        .expect("create opencode dir");
    std::fs::write(
        &opencode_path,
        serde_json::json!({
            "anthropic": {
                "access": "oc_acc",
                "refresh": "oc_ref",
                "expires": chrono::Utc::now().timestamp_millis() + 60_000
            }
        })
        .to_string(),
    )
    .expect("write opencode auth");

    let result = init_provider_for_validation(&ProviderChoice::Auto, None).await;
    let err = match result {
        Ok(provider) => panic!(
            "auto init should still fail without trusted/direct credentials, got provider {}",
            provider.name()
        ),
        Err(err) => err,
    };
    let message = err.to_string();
    assert!(
        message.contains("No credentials configured"),
        "unexpected error: {message}"
    );
    assert!(
        !message.contains("will not read them without confirmation"),
        "auto mode should skip untrusted external auth, not fail with the consent prompt error: {message}"
    );

    for (key, value) in saved {
        if let Some(value) = value {
            crate::env::set_var(&key, value);
        } else {
            crate::env::remove_var(&key);
        }
    }
}

#[test]
fn pending_external_auth_review_candidates_include_shared_and_legacy_sources() {
    let _guard = lock_env();
    let _env_guard = crate::storage::lock_test_env();
    let dir = TempDir::new().expect("temp dir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", dir.path());

    let opencode_path = crate::auth::external::ExternalAuthSource::OpenCode
        .path()
        .expect("opencode path");
    std::fs::create_dir_all(opencode_path.parent().expect("opencode parent"))
        .expect("create opencode dir");
    std::fs::write(
        &opencode_path,
        serde_json::json!({
            "openai": {
                "type": "oauth",
                "access": "sk-openai",
                "refresh": "refresh",
                "expires": chrono::Utc::now().timestamp_millis() + 60_000
            }
        })
        .to_string(),
    )
    .expect("write opencode auth");

    let codex_path = crate::auth::codex::legacy_auth_file_path().expect("codex path");
    std::fs::create_dir_all(codex_path.parent().expect("codex parent")).expect("create codex dir");
    std::fs::write(
        &codex_path,
        serde_json::json!({
            "tokens": {
                "access_token": "sk-codex",
                "refresh_token": "refresh",
                "expires_at": chrono::Utc::now().timestamp_millis() + 60_000
            }
        })
        .to_string(),
    )
    .expect("write codex auth");

    let candidates = pending_external_auth_review_candidates().expect("candidates");
    assert!(candidates.iter().any(|candidate| {
        candidate.source_name == "OpenCode auth.json"
            && candidate.provider_summary.contains("OpenAI/Codex")
    }));
    assert!(candidates.iter().any(|candidate| {
        candidate.source_name == "Codex auth.json" && candidate.provider_summary == "OpenAI/Codex"
    }));

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}
