use super::*;

struct MixedCatalog;

#[async_trait::async_trait]
impl Provider for MixedCatalog {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> anyhow::Result<EventStream> {
        anyhow::bail!("catalog-only fixture")
    }
    fn name(&self) -> &str {
        "Cursor ACP"
    }
    fn available_models_display(&self) -> Vec<String> {
        vec!["future-acp-2099".into(), "local-provider-only".into()]
    }
    fn available_models_for_switching(&self) -> Vec<String> {
        vec!["future-acp-2099".into()]
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self)
    }
}

#[test]
fn fast_cursor_acp_picker_does_not_relabel_union_catalog_models() {
    let routes =
        catalog_routes::simplified_model_routes_for_provider(&MixedCatalog, "future-acp-2099");
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].model, "future-acp-2099");
    assert_eq!(routes[0].api_method, "cursor-acp");
}

#[test]
fn explicit_cursor_acp_spec_selects_from_another_runtime() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _enter = runtime.enter();
        let provider = test_multi_provider_with_cursor_acp();
        provider.set_model("cursor-acp:gpt-5.3-codex[]").unwrap();
        assert_eq!(
            provider.active_external_provider().as_deref(),
            Some("cursor-acp")
        );
        assert_eq!(provider.model(), "gpt-5.3-codex[]");
        assert_eq!(provider.display_name(), "Cursor ACP");
    });
}

#[test]
fn cursor_acp_restore_spec_keeps_transport_and_explicit_override() {
    for model in ["future-model-2099", "gpt-future[reasoning=high]"] {
        let spec = MultiProvider::model_switch_request_for_session_route(
            model,
            Some("openai"),
            Some("cursor-acp"),
        );
        assert_eq!(spec, format!("cursor-acp:{model}"));
        assert_eq!(
            MultiProvider::model_switch_request_for_session_route(
                &spec,
                Some("openai"),
                Some("openai-oauth"),
            ),
            spec
        );
        assert_eq!(
            MultiProvider::model_switch_request_for_session_model(&spec, Some("cursor-acp"),),
            spec
        );
    }
}

#[test]
fn lazy_cursor_acp_selection_constructs_the_registered_runtime() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _enter = runtime.enter();
        external::register_external_provider(CURSOR_ACP_RUNTIME, test_cursor_acp_runtime);
        let provider = test_multi_provider_with_cursor();
        assert!(provider.cursor_acp_provider().is_none());
        provider.set_model("cursor-acp:gpt-5.3-codex[]").unwrap();
        assert!(provider.cursor_acp_provider().is_some());
        assert_eq!(provider.model(), "gpt-5.3-codex[]");
        assert_eq!(provider.display_name(), "Cursor ACP");
    });
}

#[test]
fn rejected_cursor_acp_selection_keeps_the_current_route() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _enter = runtime.enter();
        let provider = test_multi_provider_with_cursor_acp();
        let before = provider.model();
        assert!(provider.set_model("cursor-acp:not-advertised").is_err());
        assert_eq!(provider.active_external_provider(), None);
        assert_eq!(provider.model(), before);
        assert!(provider.set_model("cursor-acp: ").is_err());
        assert_eq!(provider.active_external_provider(), None);
    });
}

#[test]
fn structured_gemini_selection_leaves_cursor_acp() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _enter = runtime.enter();
        let provider = test_multi_provider_with_cursor_acp();
        *provider.gemini.write().unwrap() = Some(Arc::new(StubExternalRuntime::new(
            "gemini",
            "Gemini",
            "code-assist-oauth",
            &["future-vendor-model-2099"],
        )));
        provider.set_active_provider_external(CURSOR_ACP_RUNTIME);
        let selection = RouteSelection {
            model: "future-vendor-model-2099".into(),
            runtime_key: RuntimeKey::Gemini,
            api_method: "code-assist-oauth".into(),
            provider_label: "Gemini".into(),
            detail: String::new(),
        };
        provider.set_route_selection(&selection).unwrap();
        assert_eq!(provider.active_external_provider(), None);
        assert_eq!(provider.active_provider(), ActiveProvider::Gemini);
        assert_eq!(provider.model(), "future-vendor-model-2099");
    });
}

#[test]
fn configured_cursor_acp_pair_routes_opaque_models_to_acp() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _enter = runtime.enter();
        let provider = test_multi_provider_with_cursor_acp();
        provider
            .set_config_default_model("gpt-5.3-codex[]", Some("cursor-acp"))
            .unwrap();
        assert_eq!(
            provider.active_external_provider().as_deref(),
            Some("cursor-acp")
        );
        assert_eq!(provider.model(), "gpt-5.3-codex[]");
        provider
            .set_config_default_model("cursor-acp:claude-opus-5[thinking=true]", Some("openai"))
            .unwrap();
        assert_eq!(provider.model(), "claude-opus-5[thinking=true]");
    });
}

#[test]
fn cursor_acp_display_name_wins_over_inactive_openrouter_slot() {
    let provider = test_multi_provider_with_cursor_acp();
    *provider.openrouter.write().unwrap() = Some(Arc::new(StubExternalRuntime::new(
        "openrouter",
        "Inactive gateway",
        "openrouter",
        &["other-model"],
    )));
    *provider.active.write().unwrap() = ActiveProvider::OpenRouter;
    provider.set_active_provider_external(CURSOR_ACP_RUNTIME);
    assert_eq!(provider.display_name(), "Cursor ACP");
}

#[test]
fn direct_cursor_display_label_does_not_match_cursor_acp() {
    for label in ["Cursor", "Cursor (API)"] {
        assert!(model_route_provider_labels_match(label, "Cursor (API)"));
        assert!(model_route_provider_labels_match("Cursor (API)", label));
        assert!(!model_route_provider_labels_match(label, "Cursor ACP"));
    }
}
