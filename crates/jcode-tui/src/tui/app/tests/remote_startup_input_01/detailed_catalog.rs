/// A names-only catalog (what the server sends when the fully-routed frame is
/// oversized) leaves placeholder routes in place. The detailed catalog that
/// follows carries the same model names, so the no-op fast path must still
/// recognize it as a change and let the real routes land.
#[test]
fn test_detailed_catalog_replaces_placeholder_routes_after_names_only_update() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.is_remote = true;
        app.remote_provider_name = Some("Copilot".to_string());

        // Names-only frame: same model list, no route expansion.
        app.handle_server_event(
            crate::protocol::ServerEvent::AvailableModelsUpdated {
                provider_name: Some("Copilot".to_string()),
                provider_model: Some("claude-opus-4.6".to_string()),
                available_models: vec!["claude-opus-4.6".to_string()],
                available_model_routes: Vec::new(),
            },
            &mut remote,
        );

        // Detailed frame with identical model names but real routes.
        let detailed_redraw = app.handle_server_event(
            crate::protocol::ServerEvent::AvailableModelsUpdated {
                provider_name: Some("Copilot".to_string()),
                provider_model: Some("claude-opus-4.6".to_string()),
                available_models: vec!["claude-opus-4.6".to_string()],
                available_model_routes: vec![crate::provider::ModelRoute {
                    model: "claude-opus-4.6".to_string(),
                    provider: "Copilot".to_string(),
                    api_method: "copilot".to_string(),
                    available: true,
                    detail: String::new(),
                    usage: None,
                    cheapness: None,
                }],
            },
            &mut remote,
        );

        assert!(
            detailed_redraw,
            "detailed routes arriving after a names-only frame must repaint"
        );
        assert!(
            app.remote_model_options
                .iter()
                .any(|route| route.api_method == "copilot"),
            "detailed routes must replace the names-only placeholder state"
        );
    });
}
