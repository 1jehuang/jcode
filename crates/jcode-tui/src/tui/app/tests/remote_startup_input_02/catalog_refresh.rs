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

#[test]
fn test_catalog_rebuild_keeps_selected_model_identity() {
    // Rebuilding the open picker restores the selection by entry identity
    // (name + saved spec), not by row number. A catalog that inserts a new
    // first row shifts every row index; a positional restore would silently
    // move the user's selection to a different model (greptile F6, PR 1879:
    // refresh changed the selected model).
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.is_remote = true;
        app.remote_provider_name = Some("OpenAI".to_string());
        app.remote_provider_model = Some("gpt-5.5".to_string());
        app.remote_available_entries = vec![
            "gpt-5.5".to_string(),
            "gpt-5.4".to_string(),
            "gpt-5.3-codex".to_string(),
        ];
        app.remote_model_options = app
            .remote_available_entries
            .iter()
            .map(|model| crate::provider::ModelRoute {
                model: model.clone(),
                provider: "OpenAI".to_string(),
                api_method: "openai-api".to_string(),
                available: true,
                detail: String::new(),
                usage: None,
                cheapness: None,
            })
            .collect();

        app.open_model_picker();
        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("picker should be open");
        // Highlight the second entry explicitly.
        let second_name = picker.entries[1].name.clone();
        drop(picker);
        app.handle_inline_interactive_key(KeyCode::Down, KeyModifiers::empty())
            .unwrap();
        let selected_name = {
            let picker = app.inline_interactive_state.as_ref().unwrap();
            let idx = picker.filtered[picker.selected];
            picker.entries[idx].name.clone()
        };
        assert_eq!(selected_name, second_name);

        // The refreshed catalog inserts a new model at the top; every row
        // index shifts down by one.
        app.remote_available_entries.insert(
            0,
            "gpt-6-luna".to_string(),
        );
        app.remote_model_options.insert(
            0,
            crate::provider::ModelRoute {
                model: "gpt-6-luna".to_string(),
                provider: "OpenAI".to_string(),
                api_method: "openai-api".to_string(),
                available: true,
                detail: String::new(),
                usage: None,
                cheapness: None,
            },
        );
        app.invalidate_model_picker_cache();
        app.refresh_open_model_picker_after_catalog_update();

        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("picker should remain open across the rebuild");
        let idx = picker.filtered[picker.selected];
        assert_eq!(
            picker.entries[idx].name, second_name,
            "the same model must stay selected after rows shift"
        );
    });
}
