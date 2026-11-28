/// Plain model rows (no effort suffix) must not stage a reasoning effort.
/// Routes whose runtime cannot apply a reasoning effort (e.g. Copilot) get
/// plain rows even for models that have an effort ladder elsewhere.
#[test]
fn test_model_picker_plain_selection_stages_no_effort_in_remote_mode() {
    let mut app = create_test_app();
    configure_test_remote_models_with_openai_recommendations(&mut app);
    // A Copilot-backed route cannot apply per-request reasoning effort, so it
    // must render as a plain row (issue #458 route gating).
    app.remote_model_options.push(crate::provider::ModelRoute {
        model: "claude-opus-4-8".to_string(),
        provider: "Copilot".to_string(),
        api_method: "copilot".to_string(),
        available: true,
        detail: String::new(),
        usage: None,
        cheapness: None,
    });

    app.open_model_picker();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");

    let entry_idx = picker
        .entries
        .iter()
        .position(|m| m.name == "claude-opus-4-8" && m.effort.is_none())
        .expect("claude-opus-4-8 should be in picker without an effort variant");

    let filtered_pos = picker
        .filtered
        .iter()
        .position(|&i| i == entry_idx)
        .expect("claude-opus-4-8 should be in filtered list");
    app.inline_interactive_state.as_mut().unwrap().selected = filtered_pos;

    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    assert!(
        app.inline_interactive_state.is_none(),
        "picker should close"
    );
    assert!(
        app.pending_reasoning_effort.is_none(),
        "plain rows must not override the server's effort"
    );
}

#[test]
fn test_model_switch_notice_omits_placeholder_route_details() {
    // Selecting a model whose chosen row is a placeholder ("remote-catalog")
    // must not advertise a bogus provider/method or the "refreshing route
    // details…" text; those describe a catalog still being refreshed.
    with_temp_jcode_home(|| {
        let model = "placeholder-only-model";
        let mut app = create_test_app();
        app.is_remote = true;
        app.remote_provider_name = Some("Some Server".to_string());
        app.remote_provider_model = Some("other-model".to_string());
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

        app.open_model_picker();
        // Placeholder-only entries also get real routes synthesized, so locate
        // the entry/option pair that is still the placeholder and pick it.
        let (entry_idx, option_idx) = {
            let picker = app
                .inline_interactive_state
                .as_ref()
                .expect("model picker should be open");
            picker
                .entries
                .iter()
                .enumerate()
                .find_map(|(entry_idx, entry)| {
                    entry
                        .options
                        .iter()
                        .position(|route| route.api_method == "remote-catalog")
                        .map(|option_idx| (entry_idx, option_idx))
                })
                .expect("a placeholder route option should be present")
        };
        let filtered_pos = app
            .inline_interactive_state
            .as_ref()
            .unwrap()
            .filtered
            .iter()
            .position(|&i| i == entry_idx)
            .expect("placeholder entry should be in the filtered list");
        {
            let picker = app.inline_interactive_state.as_mut().unwrap();
            picker.selected = filtered_pos;
            picker.entries[entry_idx].selected_option = option_idx;
        }

        app.handle_key(KeyCode::Enter, KeyModifiers::empty())
            .unwrap();

        let notice = app
            .status_notice
            .as_ref()
            .map(|(text, _)| text.clone())
            .expect("a model switch notice should be set");
        assert!(!notice.contains("remote-catalog"), "got {notice}");
        assert!(!notice.contains("refreshing route details"), "got {notice}");
        assert!(notice.starts_with("Model → "), "got {notice}");
        assert!(!notice.contains(" via "), "got {notice}");
    });
}
