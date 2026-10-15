use super::*;

#[test]
fn saved_account_routes_survive_serialization_and_restore_specs() {
    for (base, runtime, model) in [
        ("claude-oauth", RuntimeKey::ClaudeOAuth, "claude-sonnet-4-6"),
        ("openai-oauth", RuntimeKey::OpenAIOAuth, "gpt-5.5"),
    ] {
        for label in ["work", "personal", "Named-Account"] {
            let route = ModelRoute {
                model: model.to_string(),
                provider: "provider".to_string(),
                api_method: format!("{base}@{label}"),
                available: true,
                detail: String::new(),
                cheapness: None,
                usage: None,
            };
            let selection = RouteSelection::from_model_route(&route);
            assert_eq!(selection.runtime_key, runtime);
            assert_eq!(selection.account_label(), Some(label));
            let json = serde_json::to_string(&selection).unwrap();
            let restored: RouteSelection = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, selection);
            assert_eq!(
                restored.routed_model_spec(),
                format!("{base}@{label}:{model}")
            );
        }
    }
}

#[test]
fn invalid_or_non_oauth_account_methods_are_not_accepted() {
    for method in [
        "claude-oauth@",
        "openai-oauth@work:bad",
        "openai-oauth@a@b",
        "openai-api-key@work",
        "claude-oauth@work\nlabel",
    ] {
        assert_eq!(oauth_account_label(method), None, "{method:?}");
    }
}
