use super::*;

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
