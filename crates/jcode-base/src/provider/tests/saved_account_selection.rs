use super::*;

#[test]
fn saved_account_routes_preserve_identity_across_restore_and_model_changes() {
    for (method, model, provider) in [
        ("openai-oauth@work", "gpt-5.5", "openai"),
        ("claude-oauth@personal", "claude-sonnet-5", "Anthropic"),
    ] {
        let spec = format!("{method}:{model}");
        assert_eq!(
            MultiProvider::model_switch_request_for_session_route(
                model,
                Some(provider),
                Some(method)
            ),
            spec
        );
        assert_eq!(
            MultiProvider::model_switch_request_for_session_model(model, Some(method)),
            spec
        );
        assert_eq!(
            MultiProvider::model_switch_request_for_session_route(
                &spec,
                Some("openrouter"),
                Some("openrouter")
            ),
            spec
        );
        assert_eq!(
            MultiProvider::session_provider_key_for_model_request(&spec, provider).as_deref(),
            Some(method)
        );
        assert_eq!(
            MultiProvider::session_provider_key_after_model_switch(model, provider, Some(method))
                .as_deref(),
            Some(method)
        );
        assert!(
            stale_credential_route_fallback(&spec).is_none(),
            "explicit account must never heal onto another credential"
        );
    }
}

#[test]
fn saved_account_model_selection_is_transactional_and_forks_preserve_account() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _guard = rt.enter();
        let provider = test_multi_provider_with_openai();
        let work = crate::auth::codex::next_account_label().unwrap();
        crate::auth::codex::upsert_account_from_tokens(
            &work,
            "work-token",
            "work-refresh",
            None,
            Some(chrono::Utc::now().timestamp_millis() + 86_400_000),
        )
        .unwrap();
        let global_before = crate::auth::codex::active_account_label();
        let model = ALL_OPENAI_MODELS[0];
        provider
            .set_model(&format!("openai-oauth@{work}:{model}"))
            .unwrap();
        assert_eq!(
            provider.oauth_account_label().as_deref(),
            Some(work.as_str())
        );
        assert_eq!(
            provider
                .openai_provider()
                .unwrap()
                .oauth_account_label()
                .as_deref(),
            Some(work.as_str())
        );
        assert_eq!(
            crate::auth::codex::active_account_label(),
            global_before,
            "session choice must not change another session's preference"
        );
        assert_eq!(
            provider.fork().oauth_account_label().as_deref(),
            Some(work.as_str())
        );
        provider.set_model(model).unwrap();
        assert_eq!(
            provider.oauth_account_label().as_deref(),
            Some(work.as_str())
        );
        assert!(
            provider
                .set_model(&format!("openai-oauth@missing:{model}"))
                .is_err()
        );
        assert!(
            provider
                .set_model(&format!("openai-oauth@{work}:not-a-model"))
                .is_err()
        );
        assert_eq!(
            provider.oauth_account_label().as_deref(),
            Some(work.as_str())
        );
        assert_eq!(provider.model(), model);
        provider
            .set_model(&format!("openai-oauth:{model}"))
            .unwrap();
        assert_eq!(provider.oauth_account_label(), None);
        assert_eq!(
            provider.openai_provider().unwrap().oauth_account_label(),
            None
        );
    });
}
