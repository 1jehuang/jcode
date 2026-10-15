use super::*;

#[test]
fn account_switch_preserves_same_provider_model_and_replaces_account_pin() {
    for (id, name, model, method) in [
        ("claude", "Claude", "claude-custom-model", "claude-oauth"),
        ("openai", "OpenAI", "gpt-custom-model", "openai-oauth"),
    ] {
        let selection = saved_account_route_selection(
            id,
            "work",
            name,
            model,
            Some("openai-oauth:configured-other-model"),
        )
        .unwrap();
        assert_eq!(selection.model, model);
        assert_eq!(selection.account_label(), Some("work"));
        assert_eq!(
            selection.routed_model_spec(),
            format!("{method}@work:{model}")
        );
    }
}

#[test]
fn account_switch_uses_target_provider_default_for_cross_provider_selection() {
    for (id, other) in [("claude", "OpenAI"), ("openai", "Claude")] {
        let selection =
            saved_account_route_selection(id, "personal", other, "foreign-model", None).unwrap();
        assert_eq!(
            selection.model,
            crate::auth::lifecycle::default_model_for_provider(id).unwrap()
        );
        assert_eq!(selection.account_label(), Some("personal"));
        assert_ne!(selection.model, "foreign-model");
    }
}

#[test]
fn cross_provider_account_switch_honors_matching_configured_default() {
    let mut config = crate::config::Config::default();
    config.provider.default_model = Some("openai-oauth:gpt-6.1-sol".to_string());
    let configured = config.provider.default_model.as_deref();
    let selection =
        saved_account_route_selection("openai", "work", "Claude", "claude-current", configured)
            .unwrap();
    assert_eq!(selection.model, "gpt-6.1-sol");
    assert_eq!(
        selection.routed_model_spec(),
        "openai-oauth@work:gpt-6.1-sol"
    );
    let selection =
        saved_account_route_selection("openai", "work", "OpenAI", "gpt-current", configured)
            .unwrap();
    assert_eq!(selection.model, "gpt-current");
    let selection =
        saved_account_route_selection("claude", "work", "OpenAI", "gpt-current", configured)
            .unwrap();
    assert_eq!(
        selection.model,
        crate::auth::lifecycle::default_model_for_provider("claude").unwrap()
    );
    let selection = saved_account_route_selection(
        "claude",
        "work",
        "OpenAI",
        "gpt-current",
        Some("claude-api:claude-configured"),
    )
    .unwrap();
    assert_eq!(
        selection.routed_model_spec(),
        "claude-oauth@work:claude-configured"
    );
}
#[test]
fn unsupported_saved_account_provider_returns_error_without_panicking() {
    let error = saved_account_route_selection("unsupported", "work", "OpenAI", "gpt-current", None)
        .unwrap_err();
    assert!(error.to_string().contains("not supported"));
}
