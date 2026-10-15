#[test]
fn test_info_widget_remote_openai_uses_remote_provider_for_usage_and_context() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.is_remote = true;
        app.remote_provider_name = Some("OpenAI".to_string());
        app.remote_provider_model = Some("gpt-5.4".to_string());
        app.remote_resolved_credential = Some(jcode_provider_core::ResolvedCredential::Oauth);
        app.update_context_limit_for_model("gpt-5.4", None);

        let data = crate::tui::TuiState::info_widget_data(&app);

        assert_eq!(data.provider_name.as_deref(), Some("OpenAI"));
        assert_eq!(data.model.as_deref(), Some("gpt-5.4"));
        assert_eq!(data.context_limit, Some(1_000_000));
        assert_eq!(
            data.auth_method,
            crate::tui::info_widget::AuthMethod::OpenAIOAuth
        );
        assert_eq!(
            data.usage_info.as_ref().map(|info| info.provider),
            Some(crate::tui::info_widget::UsageProvider::OpenAI)
        );
    });
}

#[test]
fn test_info_widget_remote_model_falls_back_to_model_provider_detection() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.is_remote = true;
        app.remote_provider_model = Some("gpt-5.4".to_string());
        app.update_context_limit_for_model("gpt-5.4", None);

        let data = crate::tui::TuiState::info_widget_data(&app);

        assert_eq!(data.context_limit, Some(1_000_000));
        assert_eq!(
            data.auth_method,
            crate::tui::info_widget::AuthMethod::Unknown
        );
        assert!(
            data.usage_info.is_none(),
            "provider/model detection alone must not guess subscription billing"
        );
    });
}
