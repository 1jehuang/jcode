#[test]
fn test_favorite_hotkey_does_not_confirm_remote_placeholder_without_matching_favorite() {
    with_temp_jcode_home(|| {
        let model = "placeholder-favorite-hotkey-model";
        let mut app = create_test_app();
        app.is_remote = true;
        app.remote_provider_name = Some("Some Server".to_string());
        app.remote_provider_model = Some(model.to_string());
        app.remote_available_entries = vec![model.to_string()];
        app.remote_model_options = vec![crate::provider::ModelRoute {
            model: model.to_string(),
            provider: "Some Server".to_string(),
            api_method: "remote-catalog".to_string(),
            available: true,
            detail: "refreshing route details…".to_string(),
            usage: None,
            cheapness: None,
        }];

        app.cycle_model_favorite_hotkey();

        assert!(app.inline_interactive_state.is_some());
        assert!(app.pending_model_switch.is_none());
    });
}

#[test]
fn test_catalog_update_rebuilds_open_model_picker_with_real_routes() {
    // A picker opened while the catalog is still names-only shows placeholder
    // rows. When the detailed catalog lands, the open picker must be rebuilt
    // rather than left stale until the user reopens it.
    with_temp_jcode_home(|| {
        let model = "gpt-5.5";
        let mut app = create_test_app();
        app.is_remote = true;
        app.remote_provider_name = Some("OpenAI".to_string());
        app.remote_provider_model = Some(model.to_string());
        app.remote_available_entries = vec![model.to_string()];
        app.remote_model_options = vec![crate::provider::ModelRoute {
            model: model.to_string(),
            provider: "OpenAI".to_string(),
            api_method: "remote-catalog".to_string(),
            available: true,
            detail: "refreshing route details…".to_string(),
            usage: None,
            cheapness: None,
        }];

        app.open_model_picker();
        assert!(app.inline_interactive_state.is_some());

        app.remote_model_options = vec![crate::provider::ModelRoute {
            model: model.to_string(),
            provider: "OpenAI".to_string(),
            api_method: "openai-api".to_string(),
            available: true,
            detail: String::new(),
            usage: None,
            cheapness: None,
        }];
        app.invalidate_model_picker_cache();
        app.refresh_open_model_picker_after_catalog_update();

        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("picker should still be open after the catalog update");
        assert!(
            picker
                .entries
                .iter()
                .any(|entry| entry.name.starts_with(model)),
            "rebuilt picker should still list the model"
        );
        assert!(
            picker.entries.iter().all(|entry| {
                entry
                    .options
                    .iter()
                    .all(|route| route.api_method != "remote-catalog")
            }),
            "rebuilt picker should not keep placeholder rows"
        );
        assert!(
            picker.entries.iter().any(|entry| {
                entry
                    .options
                    .iter()
                    .any(|route| route.api_method == "openai-api")
            }),
            "rebuilt picker should expose the real route"
        );
    });
}
