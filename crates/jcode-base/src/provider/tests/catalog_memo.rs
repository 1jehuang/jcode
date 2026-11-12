use super::*;

/// Run explicitly with `--ignored --nocapture`. Both paths execute the real
/// key construction, generation reads, TTL checks and instance memo lock. The
/// baseline recreates the former two deep-cloned entries used by a snapshot,
/// rather than timing an isolated Arc clone against an isolated Vec clone.
#[test]
#[ignore = "representative route memo hit-path benchmark"]
fn benchmark_route_memo_snapshot_hit_path() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _runtime_guard = rt.enter();
        let provider = test_multi_provider_with_openai();
        let routes: Vec<_> = (0..4_000)
            .map(|index| {
                build_openai_api_key_route(
                    &format!("gpt-future-benchmark-{index}"),
                    true,
                    "representative cached catalog route",
                )
            })
            .collect();
        *provider.routes_memo.lock().unwrap() = Some(Arc::new(RoutesMemoEntry {
            built_at: std::time::Instant::now(),
            scope_key: provider.routes_memo_key(),
            auth_generation: pricing::auth_pricing_generation(),
            catalog_generation: catalog_generation(),
            listable_models: listable_model_names_from_routes(&routes),
            routes,
        }));
        let baseline = || {
            let models_entry = (*provider.fresh_routes_memo_entry()).clone();
            let routes_entry = (*provider.fresh_routes_memo_entry()).clone();
            jcode_provider_core::ModelCatalogSnapshot::new(
                Some(provider.display_name()),
                Some(provider.model()),
                models_entry.listable_models,
                routes_entry.routes,
            )
        };
        assert_eq!(baseline(), provider.model_catalog_snapshot());
        let measure = |old: bool| {
            let start = std::time::Instant::now();
            for _ in 0..250 {
                std::hint::black_box(if old {
                    baseline()
                } else {
                    provider.model_catalog_snapshot()
                });
            }
            start.elapsed()
        };
        for round in 0..4 {
            let (old, shared) = if round % 2 == 0 {
                (measure(true), measure(false))
            } else {
                let shared = measure(false);
                (measure(true), shared)
            };
            eprintln!(
                "route memo snapshot round {round}: 4000 routes x 250 hits, old deep-clone {old:?}, shared {shared:?}, ratio {:.2}x",
                old.as_secs_f64() / shared.as_secs_f64()
            );
        }
    });
}

#[test]
fn route_memo_arc_shares_local_and_fork_hits_and_invalidates_generations() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _runtime_guard = rt.enter();
        let provider = test_multi_provider_with_openai();
        let mut fork = test_multi_provider_with_openai();
        fork.catalog_scope = provider.catalog_scope;
        assert_eq!(provider.routes_memo_key(), fork.routes_memo_key());
        let first = provider.fresh_routes_memo_entry();
        assert!(Arc::ptr_eq(&first, &provider.fresh_routes_memo_entry()));
        let shared = fork.fresh_routes_memo_entry();
        assert!(Arc::ptr_eq(&first, &shared));
        assert!(Arc::ptr_eq(&shared, &fork.fresh_routes_memo_entry()));

        crate::auth::AuthStatus::invalidate_cache();
        let after_auth = provider.fresh_routes_memo_entry();
        assert!(!Arc::ptr_eq(&first, &after_auth));
        assert!(Arc::ptr_eq(&after_auth, &fork.fresh_routes_memo_entry()));
        bump_catalog_generation();
        let after_catalog = fork.fresh_routes_memo_entry();
        assert!(!Arc::ptr_eq(&after_auth, &after_catalog));
        assert!(Arc::ptr_eq(
            &after_catalog,
            &provider.fresh_routes_memo_entry()
        ));
    });
}

#[test]
fn route_memo_snapshot_preserves_future_catalog_models_and_output_isolation() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _runtime_guard = rt.enter();
        let provider = test_multi_provider_with_openai();
        let future = "gpt-arbitrary-future-catalog-model-2099";
        let scope = openai_catalog_scope_for_credential(false, "sk-test-openai-api-key");
        persist_openai_model_catalog_for_scope(
            &scope,
            &models::OpenAIModelCatalog {
                available_models: vec![future.to_string()],
                ..Default::default()
            },
        );
        bump_catalog_generation();
        let entry = provider.fresh_routes_memo_entry();
        let expected = provider.model_catalog_snapshot();
        assert_eq!(expected.provider_name, Some(provider.display_name()));
        assert_eq!(expected.provider_model, Some(provider.model()));
        assert!(
            expected
                .available_models
                .iter()
                .any(|model| model == future)
        );
        assert!(
            expected
                .model_routes
                .iter()
                .any(|route| route.model == future)
        );
        assert_eq!(expected.model_routes, entry.routes);
        assert_eq!(expected.available_models, entry.listable_models);

        let mut changed = provider.model_catalog_snapshot();
        changed.available_models.clear();
        changed.model_routes[0].model = "mutated-output".to_string();
        let mut routes = provider.model_routes();
        routes.clear();
        let mut models = provider.available_models_display();
        models.clear();
        assert!(Arc::ptr_eq(&entry, &provider.fresh_routes_memo_entry()));
        assert_eq!(expected, provider.model_catalog_snapshot());
    });
}

#[test]
fn catalog_sharing_identity_fresh_sessions_share_equivalent_configuration() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _runtime_guard = rt.enter();
        let provider = MultiProvider::new_fast();
        let fork = provider.fork_for_new_session();
        assert_eq!(
            provider.catalog_sharing_identity(),
            fork.catalog_sharing_identity()
        );
    });
}

#[cfg(unix)]
#[test]
fn catalog_sharing_identity_accepts_non_utf8_home() {
    with_clean_provider_test_env(|| {
        use std::os::unix::ffi::OsStringExt;
        let rt = enter_test_runtime();
        let _runtime_guard = rt.enter();
        let provider = test_multi_provider_with_openai();
        let old_home = std::env::var_os("JCODE_HOME").unwrap();
        let mut bytes = old_home.clone().into_vec();
        bytes.extend_from_slice(b"/non-utf8-\xff");
        crate::env::set_var("JCODE_HOME", std::ffi::OsString::from_vec(bytes));
        assert!(provider.catalog_sharing_identity().is_some());
        crate::env::set_var("JCODE_HOME", old_home);
    });
}

/// The route-catalog memo must serve repeated `model_routes()` calls without
/// rebuilding (a shared server fans one ModelsUpdated event out to every
/// connection, each of which snapshots the catalog), while auth invalidation
/// and model switches must bypass it immediately.
#[test]
fn model_routes_memo_serves_repeats_and_invalidates_on_auth_and_model_changes() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _runtime_guard = rt.enter();
        let provider = test_multi_provider_with_openai();

        let first = provider.model_routes();
        let second = provider.model_routes();
        assert_eq!(
            first, second,
            "memoized catalog must be identical across back-to-back reads"
        );
        assert!(
            provider
                .routes_memo
                .lock()
                .expect("routes memo lock")
                .is_some(),
            "memo should be populated after a build"
        );

        // Auth invalidation bumps the pricing generation, which must make the
        // memo stale even within the TTL window (verified behaviorally by the
        // oauth/api state-space test above; here we check the memo mechanism).
        let generation_before = crate::provider::pricing::auth_pricing_generation();
        crate::auth::AuthStatus::invalidate_cache();
        assert!(
            crate::provider::pricing::auth_pricing_generation() > generation_before,
            "auth invalidation must advance the pricing generation"
        );

        // A model switch drops the memo outright.
        let model = known_openai_model_ids()
            .first()
            .expect("at least one OpenAI model")
            .clone();
        provider.model_routes();
        let _ = provider.set_model(&format!("openai-api:{model}"));
        assert!(
            provider
                .routes_memo
                .lock()
                .expect("routes memo lock")
                .is_none(),
            "set_model must invalidate the routes memo"
        );

        // A second instance with identical catalog-relevant state must reuse
        // the shared process-wide memo (this is what collapses shared-server
        // connect bursts down to one build), and instances with different
        // state must not share a key.
        let first_instance = test_multi_provider_with_openai();
        let mut second_instance = test_multi_provider_with_openai();
        assert_ne!(
            first_instance.catalog_sharing_identity(),
            second_instance.catalog_sharing_identity(),
            "independently configured runtimes must not share catalogs"
        );
        second_instance.catalog_scope = first_instance.catalog_scope;
        assert_eq!(
            first_instance.routes_memo_key(),
            second_instance.routes_memo_key(),
            "identical forks must share a memo key"
        );
        let baseline = first_instance.model_routes();
        assert!(
            second_instance
                .routes_memo
                .lock()
                .expect("routes memo lock")
                .is_none(),
            "second instance has not built anything yet"
        );
        let shared = second_instance.model_routes();
        assert_eq!(baseline, shared, "shared memo must serve identical routes");
        assert!(
            second_instance
                .routes_memo
                .lock()
                .expect("routes memo lock")
                .is_some(),
            "shared hit should hydrate the instance memo"
        );
        let _ = second_instance.set_model(&format!("openai-oauth:{model}"));
        // Credential-mode switches don't change catalog content (routes come
        // from global auth status), so the key may legitimately stay equal.
        // A different *model* must change it: the active model gets special
        // treatment (endpoint refresh priority) during the build.
        if let Some(alternate) = known_openai_model_ids().get(1) {
            let _ = second_instance.set_model(&format!("openai-oauth:{alternate}"));
            assert_ne!(
                first_instance.routes_memo_key(),
                second_instance.routes_memo_key(),
                "a different active model must change the memo key"
            );
        }
    });
}

#[test]
fn catalog_sharing_identity_tracks_auth_catalog_home_and_fork_scope() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _runtime_guard = rt.enter();
        let provider = test_multi_provider_with_openai();
        let fork = provider.fork();
        assert_eq!(
            provider.catalog_sharing_identity(),
            fork.catalog_sharing_identity()
        );
        let original = provider.catalog_sharing_identity();
        crate::auth::AuthStatus::invalidate_cache();
        let auth = provider.catalog_sharing_identity();
        assert_ne!(original, auth);
        bump_catalog_generation();
        let catalog = provider.catalog_sharing_identity();
        assert_ne!(auth, catalog);
        let old_home = std::env::var_os("JCODE_HOME").unwrap();
        let alternate = tempfile::tempdir().unwrap();
        crate::env::set_var("JCODE_HOME", alternate.path());
        assert_ne!(catalog, provider.catalog_sharing_identity());
        crate::env::set_var("JCODE_HOME", old_home);
    });
}
