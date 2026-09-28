// Regression tests for issue #438: in remote sessions, the runtime model
// picker preview advertises Ctrl+N (toggle favorite) and Ctrl+O (set default),
// but the remote key path never routed those chords to
// `model_picker_preview_hotkey`. They fell through to the remote global
// Ctrl-key handling and were swallowed as unrecognized hotkeys.

fn remote_model_picker_preview_state() -> crate::tui::InlineInteractiveState {
    crate::tui::InlineInteractiveState {
        kind: crate::tui::PickerKind::Model,
        filtered: vec![0],
        entries: vec![crate::tui::PickerEntry {
            name: "gpt-5.5".to_string(),
            options: vec![crate::tui::PickerOption {
                provider: "OpenAI".to_string(),
                api_method: "openai-api".to_string(),
                available: true,
                detail: String::new(),
                estimated_reference_cost_micros: None,
                comparable_reference_cost_micros: None,
            }],
            action: crate::tui::PickerAction::Model,
            selected_option: 0,
            is_current: false,
            is_default: false,
            is_favorite: false,
            recommended: false,
            recommendation_rank: usize::MAX,
            usage_score: 0,
            old: false,
            created_date: None,
            effort: None,
        }],
        selected: 0,
        column: 0,
        filter: String::new(),
        preview: true,
    }
}

#[test]
fn test_remote_model_picker_preview_ctrl_n_toggles_favorite() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.is_remote = true;
        app.inline_interactive_state = Some(remote_model_picker_preview_state());

        rt.block_on(app.handle_remote_key(KeyCode::Char('n'), KeyModifiers::CONTROL, &mut remote))
            .expect("Ctrl+N should be handled in the remote path");

        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("picker preview should stay open after Ctrl+N");
        assert!(picker.preview, "picker should remain in preview mode");
        assert!(
            picker.entries[0].is_favorite,
            "Ctrl+N must toggle the selected model as a favorite in remote sessions"
        );
        assert!(
            app.status_notice()
                .is_some_and(|notice| notice.contains("Favorited")),
            "favorite toggle should surface a status notice, got: {:?}",
            app.status_notice()
        );

        // Toggling again must unfavorite, proving the chord is consumed by the
        // picker on every press instead of falling through once.
        rt.block_on(app.handle_remote_key(KeyCode::Char('n'), KeyModifiers::CONTROL, &mut remote))
            .expect("second Ctrl+N should be handled in the remote path");
        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("picker preview should stay open");
        assert!(!picker.entries[0].is_favorite);
    });
}

#[test]
fn test_remote_model_picker_preview_ctrl_o_sets_default() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.is_remote = true;
        app.inline_interactive_state = Some(remote_model_picker_preview_state());

        rt.block_on(app.handle_remote_key(KeyCode::Char('o'), KeyModifiers::CONTROL, &mut remote))
            .expect("Ctrl+O should be handled in the remote path");

        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("picker preview should stay open after Ctrl+O");
        assert!(picker.preview, "picker should remain in preview mode");
        assert!(
            picker.entries[0].is_default,
            "Ctrl+O must mark the selected model as the default in remote sessions"
        );
        assert!(
            app.display_messages()
                .iter()
                .any(|msg| msg.content.contains("Saved default model")),
            "setting the default should confirm with a system message"
        );
    });
}
