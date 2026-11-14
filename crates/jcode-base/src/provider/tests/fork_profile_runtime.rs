use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Issue #1225: forking a provider must carry the active direct
/// OpenAI-compatible runtime. Rebuilding the slot from
/// `OpenRouterRuntimeSpec::Default` with an empty profile map dropped the
/// profile identity behind the current model, so a forked provider lost the
/// per-model protocol hint and could not tell an OpenCode Go `/responses`
/// model from a `/messages` one until the next model switch.
#[test]
fn forked_provider_reuses_the_active_compatible_profile_runtime() {
    with_clean_provider_test_env(|| {
        let built = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&built);
        external::register_openrouter_factory(move |spec| {
            use external::OpenRouterRuntimeSpec;
            use jcode_provider_openrouter_runtime::OpenRouterProvider;
            if matches!(spec, OpenRouterRuntimeSpec::CompatibleProfile(_)) {
                counter.fetch_add(1, Ordering::SeqCst);
            }
            let provider: Arc<dyn Provider> = match spec {
                OpenRouterRuntimeSpec::Default => Arc::new(OpenRouterProvider::new()?),
                OpenRouterRuntimeSpec::OpenRouterApiKey => {
                    Arc::new(OpenRouterProvider::new_openrouter_api_key_runtime()?)
                }
                OpenRouterRuntimeSpec::CompatibleProfile(profile) => {
                    Arc::new(OpenRouterProvider::new_openai_compatible_profile_runtime(
                        profile,
                    )?)
                }
                OpenRouterRuntimeSpec::NamedProfile { name, config } => {
                    Arc::new(OpenRouterProvider::new_named_openai_compatible(
                        &name, &config,
                    )?)
                }
            };
            Ok(provider)
        });

        crate::env::set_var("OPENCODE_GO_API_KEY", "test-opencode-go-key");

        let provider = MultiProvider {
            anthropic: RwLock::new(None),
            openai: RwLock::new(None),
            copilot_api: RwLock::new(None),
            antigravity: RwLock::new(None),
            gemini: RwLock::new(None),
            cursor: RwLock::new(None),
            bedrock: RwLock::new(None),
            openrouter: RwLock::new(None),
            openai_compatible_profiles: RwLock::new(std::collections::HashMap::new()),
            active_openai_compatible_profile: RwLock::new(None),
            session_id: RwLock::new(None),
            active: RwLock::new(ActiveProvider::OpenRouter),
            startup_notices: RwLock::new(Vec::new()),
            initial_provider: None,
            catalog_scope: next_catalog_scope(),
            routes_memo: std::sync::Mutex::new(None),
            post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };

        provider
            .set_model("opencode-go:kimi-k2.5")
            .expect("opencode-go model should be selectable with the key configured");
        assert_eq!(
            ProviderRegistry::new(&provider).active_compatible_profile_id(),
            Some("opencode-go".to_string())
        );
        assert_eq!(
            built.load(Ordering::SeqCst),
            1,
            "selecting the profile should build exactly one compatible-profile runtime"
        );

        let forked = provider.fork();
        assert_eq!(
            built.load(Ordering::SeqCst),
            1,
            "fork must reuse the active profile runtime instead of rebuilding the slot"
        );

        // `display_name` reports the runtime that actually executes requests,
        // so it only reads "OpenCode Go" while the profile runtime is carried.
        assert_eq!(forked.display_name(), "OpenCode Go");
        assert_eq!(forked.model(), "kimi-k2.5");

        use jcode_provider_openrouter_runtime::openai_compatible_route_path;
        let api_base = crate::provider_catalog::OPENCODE_GO_PROFILE.api_base;
        // The gateway is multi-protocol: the fork must keep routing each model
        // family to the endpoint its gateway actually serves.
        assert_eq!(
            openai_compatible_route_path(api_base, Some("opencode-go"), "grok-4.6"),
            "responses"
        );
        assert_eq!(
            openai_compatible_route_path(
                api_base,
                Some("opencode-go"),
                "muse-spark-1.3-contributor"
            ),
            "responses"
        );
        assert_eq!(
            openai_compatible_route_path(api_base, Some("opencode-go"), "minimax-m2.7"),
            "messages"
        );
        assert_eq!(
            openai_compatible_route_path(api_base, Some("opencode-go"), "kimi-k2.5"),
            "chat/completions"
        );
    });
}
