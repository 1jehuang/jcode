//! Catalog refresh fanout and read-only discovery regression tests.
use super::*;

#[test]
fn catalog_refresh_reaches_every_runtime_slot_and_compatible_profile() {
    #[derive(Clone, Default)]
    struct CatalogProbe {
        prefetches: Arc<std::sync::atomic::AtomicUsize>,
        forced: Arc<std::sync::atomic::AtomicUsize>,
        fail: Arc<std::sync::atomic::AtomicBool>,
    }
    #[async_trait::async_trait]
    impl Provider for CatalogProbe {
        async fn complete(
            &self,
            _: &[Message],
            _: &[ToolDefinition],
            _: &str,
            _: Option<&str>,
        ) -> Result<EventStream> {
            unreachable!("catalog refresh must not complete a prompt")
        }
        fn name(&self) -> &str {
            "catalog-probe"
        }
        fn model(&self) -> String {
            "static-fallback".into()
        }
        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(self.clone())
        }
        fn available_models_display(&self) -> Vec<String> {
            vec![self.model()]
        }
        async fn prefetch_models(&self) -> Result<()> {
            self.prefetches
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        async fn refresh_model_catalog(&self) -> Result<ModelCatalogRefreshSummary> {
            self.forced
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
                anyhow::bail!("probe catalog unavailable");
            }
            Ok(ModelCatalogRefreshSummary::default())
        }
    }

    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _enter = runtime.enter();
        let provider = test_multi_provider_with_openai();
        let slots = [
            ("anthropic", &provider.anthropic),
            ("openai", &provider.openai),
            ("openrouter", &provider.openrouter),
            ("copilot", &provider.copilot_api),
            ("antigravity", &provider.antigravity),
            ("gemini", &provider.gemini),
            ("cursor", &provider.cursor),
        ];
        let mut probes = Vec::new();
        for (name, slot) in slots {
            let probe = Arc::new(CatalogProbe::default());
            *slot.write().unwrap() = Some(probe.clone());
            probes.push((name, probe));
        }
        for id in [GROK_BUILD_PROFILE_ID, "custom-endpoint"] {
            let probe = Arc::new(CatalogProbe::default());
            ProviderRegistry::new(&provider).install_compatible_profile(id, probe.clone());
            probes.push((id, probe));
        }
        runtime.block_on(provider.prefetch_models()).unwrap();
        runtime.block_on(provider.refresh_model_catalog()).unwrap();
        for (name, probe) in &probes {
            assert_eq!(
                probe.prefetches.load(std::sync::atomic::Ordering::SeqCst),
                1,
                "startup must prefetch {name} exactly once"
            );
            assert_eq!(
                probe.forced.load(std::sync::atomic::Ordering::SeqCst),
                1,
                "manual refresh must honor {name}'s force-refresh override"
            );
        }
        // A compatible endpoint is not the real OpenRouter runtime. An
        // optional OpenRouter failure must not fail the active profile refresh.
        *provider.active.write().unwrap() = ActiveProvider::OpenRouter;
        ProviderRegistry::new(&provider).set_active_compatible_profile(GROK_BUILD_PROFILE_ID);
        let real_openrouter = &probes
            .iter()
            .find(|(name, _)| *name == "openrouter")
            .unwrap()
            .1;
        real_openrouter
            .fail
            .store(true, std::sync::atomic::Ordering::SeqCst);
        runtime.block_on(provider.refresh_model_catalog()).unwrap();
        let active = &probes
            .iter()
            .find(|(name, _)| *name == GROK_BUILD_PROFILE_ID)
            .unwrap()
            .1;
        active.fail.store(true, std::sync::atomic::Ordering::SeqCst);
        let generation_before = catalog_generation();
        let error = runtime
            .block_on(provider.refresh_model_catalog())
            .unwrap_err();
        assert!(error.to_string().contains(GROK_BUILD_PROFILE_ID));
        assert!(
            catalog_generation() > generation_before,
            "partial success must invalidate route memos even when the active catalog fails"
        );
    });
}

/// Rendering the route catalog must never schedule network work.
///
/// Regression guard for the "spawning a session refetches every provider
/// catalog" bug: route building used to schedule a background `/models` fetch
/// for each stale or missing profile cache, so every session attach and picker
/// open fanned out dozens of HTTP requests. Refresh cadence now belongs solely
/// to the background catalog scheduler.
#[test]
fn building_direct_profile_routes_does_not_schedule_catalog_refreshes() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        runtime.block_on(async {
            crate::provider_catalog::save_env_value_to_env_file(
                "OPENROUTER_API_KEY",
                "openrouter.env",
                Some("sk-test-openrouter"),
            )
            .expect("save openrouter key");

            // Deliberately leave every profile cache missing/stale: under the
            // old behavior this was the worst case that scheduled a refresh
            // for each configured profile.
            jcode_provider_openrouter_runtime::reset_profile_catalog_refresh_tracker_for_tests();

            for profile in crate::provider_catalog::openai_compatible_profiles()
                .iter()
                .copied()
            {
                let _ = super::direct_openai_compatible_profile_routes(profile);
            }

            // If route building had scheduled refreshes, the profile tracker
            // would have recorded attempts, and this direct call for the
            // standard OpenRouter namespace would be throttled/in-flight.
            assert!(
                openrouter::maybe_schedule_standard_openrouter_catalog_refresh(
                    "unit test post-render scheduling"
                ),
                "route building must leave the refresh tracker untouched"
            );
        });
    });
}

/// The scheduler's staleness predicate must treat a missing or mismatched
/// cache as needing a refresh, so the sweeper actually populates cold caches.
#[test]
fn profile_catalog_cache_needs_refresh_for_missing_cache() {
    with_clean_provider_test_env(|| {
        let profile = crate::provider_catalog::openai_compatible_profiles()
            .first()
            .copied()
            .expect("at least one OpenAI-compatible profile is defined");
        assert!(
            crate::provider::catalog_scheduler::profile_catalog_cache_needs_refresh(profile),
            "a missing catalog cache must be reported as needing a refresh"
        );
    });
}
