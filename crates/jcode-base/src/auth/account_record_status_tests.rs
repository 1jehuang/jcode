#[test]
fn claude_rejected_refresh_readiness_matches_full_fast_and_model_routes() {
    let _sandbox = super::test_sandbox::AuthTestSandbox::new().unwrap();
    crate::provider::models::reset_model_catalog_services_for_tests();
    let previous_override = claude::get_active_account_override();
    claude::set_active_account_override(Some("claude-otter".into()));
    crate::env::set_var("ANTHROPIC_API_KEY", "test-independent-api-key");
    let now = chrono::Utc::now().timestamp_millis();
    let rejected = "readiness-rejected-refresh";
    refresh_state::record_permanent_rejection("claude", rejected, "invalid_grant").unwrap();
    // Another account's refresh must not overwrite the active rejection.
    refresh_state::record_permanent_rejection("claude", "other-rejected-refresh", "invalid_grant")
        .unwrap();
    refresh_state::record_success("claude").unwrap();
    for (case, expires, refresh, expected) in [
        (
            "expired rejected",
            now - 60_000,
            rejected,
            AuthState::Expired,
        ),
        (
            "expired refreshable",
            now - 60_000,
            "new-refresh",
            AuthState::Available,
        ),
        (
            "expired missing refresh",
            now - 60_000,
            "",
            AuthState::Expired,
        ),
        (
            "live rejected proactive refresh",
            now + 120_000,
            rejected,
            AuthState::Available,
        ),
    ] {
        claude::upsert_account(claude::AnthropicAccount {
            record_id: None,
            label: "claude-otter".into(),
            access: "readiness-access".into(),
            refresh: refresh.into(),
            expires,
            email: None,
            subscription_type: Some("max".into()),
            scopes: vec!["user:inference".into()],
        })
        .unwrap();
        AuthStatus::invalidate_cache();
        for status in [AuthStatus::check_fast(), AuthStatus::check()] {
            assert!(status.anthropic.has_oauth, "{case}");
            assert_eq!(status.anthropic.oauth_state, expected, "{case}");
            assert_eq!(
                status.anthropic.state,
                AuthState::Available,
                "API key: {case}"
            );
            assert_eq!(
                status
                    .assessment_for_provider(crate::provider_catalog::CLAUDE_LOGIN_PROVIDER)
                    .state,
                expected,
                "{case}"
            );
            assert_eq!(
                status
                    .assessment_for_provider(crate::provider_catalog::ANTHROPIC_API_LOGIN_PROVIDER)
                    .state,
                AuthState::Available,
                "{case}"
            );
            let mut routes = Vec::new();
            crate::provider::append_simplified_anthropic_model_routes(
                &mut routes,
                "claude-sonnet-4-6",
                &status,
            );
            assert_eq!(
                routes
                    .iter()
                    .find(|r| r.api_method == "claude-oauth")
                    .unwrap()
                    .available,
                expected == AuthState::Available,
                "implicit route: {case}"
            );
            assert!(
                routes
                    .iter()
                    .find(|r| r.api_method == "claude-api")
                    .unwrap()
                    .available,
                "API route: {case}"
            );
        }
        let routes = crate::provider::simplified_model_routes_for_picker(
            "Anthropic",
            "claude-sonnet-4-6",
            vec!["claude-sonnet-4-6".into()],
        );
        let saved = routes
            .iter()
            .find(|r| r.api_method == "claude-oauth@claude-otter" && r.model == "claude-sonnet-4-6")
            .unwrap();
        assert_eq!(
            saved.available,
            expected == AuthState::Available,
            "saved route: {case}"
        );
    }
    claude::set_active_account_override(previous_override);
    crate::provider::models::reset_model_catalog_services_for_tests();
}
