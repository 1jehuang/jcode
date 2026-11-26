#[test]
fn jcode_subscription_model_prefix_parses_explicit_specs() {
    assert_eq!(
        super::jcode_subscription_model_prefix("jcode:gpt-5.5"),
        Some("gpt-5.5")
    );
    assert_eq!(
        super::jcode_subscription_model_prefix("jcode-subscription: gpt-5.5 "),
        Some("gpt-5.5")
    );
    assert_eq!(super::jcode_subscription_model_prefix("jcode:"), None);
    assert_eq!(super::jcode_subscription_model_prefix("gpt-5.5"), None);
    assert_eq!(
        super::jcode_subscription_model_prefix("openai:gpt-5.5"),
        None
    );
}

/// A session persisted on one credential (e.g. `claude-api:`) must not stay
/// pinned to it after the deployment moved to the other credential. The
/// runtime pin wins; with no pin, the bare provider prefix lets auto mode pick.
#[test]
fn stale_credential_route_fallback_follows_runtime_pin_or_drops_to_auto() {
    with_clean_provider_test_env(|| {
        // Runtime pinned to Claude OAuth, session persisted on the API key.
        crate::env::set_var("JCODE_RUNTIME_PROVIDER", "claude");
        let (request, healed) = stale_credential_route_fallback("claude-api:claude-sonnet-5")
            .expect("stale API-key pin must fall back to the runtime's OAuth route");
        assert_eq!(request, "claude-oauth:claude-sonnet-5");
        assert_eq!(healed.provider_key.as_deref(), Some("claude-oauth"));
        assert_eq!(healed.route_api_method.as_deref(), Some("claude-oauth"));
        assert_eq!(healed.stale_request, "claude-api:claude-sonnet-5");

        // The reverse move: runtime on the API key, session persisted on OAuth.
        crate::env::set_var("JCODE_RUNTIME_PROVIDER", "claude-api");
        let (request, healed) = stale_credential_route_fallback("claude-oauth:claude-sonnet-5")
            .expect("stale OAuth pin must fall back to the runtime's API-key route");
        assert_eq!(request, "claude-api:claude-sonnet-5");
        assert_eq!(healed.provider_key.as_deref(), Some("claude-api"));
        assert_eq!(
            healed.route_api_method.as_deref(),
            Some("anthropic-api-key")
        );

        // The runtime pins the very credential that failed: that is a real
        // auth failure, not a stale session, so there is nothing to fall back to.
        assert!(stale_credential_route_fallback("claude-api:claude-sonnet-5").is_none());

        // A runtime pin for the *other* provider says nothing about this one:
        // drop to auto mode.
        crate::env::set_var("JCODE_RUNTIME_PROVIDER", "openai-api");
        let (request, healed) = stale_credential_route_fallback("claude-api:claude-sonnet-5")
            .expect("unpinned provider falls back to auto mode");
        assert_eq!(request, "claude:claude-sonnet-5");
        assert_eq!(healed.provider_key.as_deref(), Some("claude"));
        assert_eq!(healed.route_api_method, None);

        crate::env::remove_var("JCODE_RUNTIME_PROVIDER");
        let (request, _) = stale_credential_route_fallback("openai-oauth:gpt-5.5")
            .expect("no runtime pin falls back to auto mode");
        assert_eq!(request, "openai:gpt-5.5");

        // Requests that pin no credential have nothing to recover.
        assert!(stale_credential_route_fallback("claude:claude-sonnet-5").is_none());
        assert!(stale_credential_route_fallback("claude-sonnet-5").is_none());
        assert!(stale_credential_route_fallback("openrouter:z-ai/glm-5.2@Novita").is_none());
    });
}
