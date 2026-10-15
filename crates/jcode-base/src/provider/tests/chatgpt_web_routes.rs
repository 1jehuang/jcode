use super::*;

#[test]
fn openai_model_routes_cover_oauth_api_and_no_auth_state_space() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _runtime_guard = rt.enter();
        crate::provider::chatgpt_web::seed_observed_model_for_tests("GPT-5.6 Pro");
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
        let account_label =
            crate::auth::codex::active_account_label().expect("saved OpenAI account");
        assert!(
            methods.iter().any(|(method, available)| {
                ModelRouteApiMethod::parse(method) == ModelRouteApiMethod::OpenAIOAuth
                    && jcode_provider_core::oauth_account_label(method)
                        == Some(account_label.as_str())
                    && *available
            }),
            "routes: {methods:?}"
        );
        assert!(
            methods.contains(&("openai-api-key", true)),
            "routes: {methods:?}"
        );
        let web_routes = routes
            .iter()
            .filter(|route| route.model == CHATGPT_WEB_MODEL)
            .collect::<Vec<_>>();
        assert_eq!(web_routes.len(), 1, "routes: {web_routes:?}");
        assert_eq!(web_routes[0].api_method, "chatgpt-web");
        assert!(web_routes[0].available);

        crate::env::remove_var("OPENAI_API_KEY");
        crate::auth::AuthStatus::invalidate_cache();
        let oauth_only = provider.model_routes();
        let oauth_only_methods = oauth_only
            .iter()
            .filter(|route| route.provider == "OpenAI" && route.model == model)
            .map(|route| route.api_method.as_str())
            .collect::<Vec<_>>();
        assert_eq!(oauth_only_methods.len(), 1);
        assert_eq!(
            ModelRouteApiMethod::parse(oauth_only_methods[0]),
            ModelRouteApiMethod::OpenAIOAuth
        );
        assert_eq!(
            jcode_provider_core::oauth_account_label(oauth_only_methods[0]),
            Some(account_label.as_str())
        );

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
