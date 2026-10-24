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
            }],
            action: crate::tui::PickerAction::Model,
            selected_option: 0,
            is_current: false,
            is_default: false,
            is_favorite: false,
            is_memory_model: false,
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

        rt.block_on(app.handle_remote_key(
            KeyCode::Char('n'),
            KeyModifiers::CONTROL,
            &mut remote,
        ))
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
        rt.block_on(app.handle_remote_key(
            KeyCode::Char('n'),
            KeyModifiers::CONTROL,
            &mut remote,
        ))
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

        rt.block_on(app.handle_remote_key(
            KeyCode::Char('o'),
            KeyModifiers::CONTROL,
            &mut remote,
        ))
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

#[test]
fn test_local_model_picker_alt_m_marks_memory_model() {
    // Alt+M (not Ctrl+M) marks the highlighted model as the memory model in
    // the /model picker. In legacy terminal encoding Ctrl+M is delivered as
    // the same byte as Enter (\r), so it can never reach this handler; the
    // chord must be Alt+M for the marking to be reachable in practice.
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();

        app.inline_interactive_state = Some(remote_model_picker_preview_state());

        app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT)
            .expect("Alt+M should be handled in the local path");

        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("picker preview should stay open after Alt+M");
        assert!(picker.preview, "picker should remain in preview mode");
        assert!(
            picker.entries[0].is_memory_model,
            "Alt+M must mark the selected model as the memory model"
        );
        assert!(
            app.status_notice()
                .is_some_and(|notice| notice.contains("Marked as memory model")),
            "memory marking should surface a status notice, got: {:?}",
            app.status_notice()
        );

        // The mark must persist to disk. Regression guard: the save used to
        // fail silently when the app config dir did not exist yet (fresh
        // installs), because the memory-model writer skipped
        // `create_dir_all` on the parent directory.
        {
            let store_path = crate::storage::app_config_dir()
                .expect("app config dir")
                .join("memory_model.json");
            let raw = std::fs::read_to_string(&store_path)
                .expect("memory model store must persist after Alt+M");
            assert!(
                raw.contains("gpt-5.5"),
                "persisted memory model store should name the marked model: {}",
                raw
            );
        }

        // Toggling again must unmark, proving the chord is consumed by the
        // picker on every press.
        app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT)
            .expect("second Alt+M should be handled");

        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("picker should still be open");
        assert!(
            !picker.entries[0].is_memory_model,
            "second Alt+M must unmark the memory model"
        );

        // The old chord must be dead: Ctrl+M (delivered as Enter \r by most
        // terminals, but simulated directly here) must not toggle marking.
        // Enter confirms the selection instead, closing the preview state
        // route for memory marking. We assert the marking flag never flips
        // back on from the legacy chord.
        app.inline_interactive_state = Some(remote_model_picker_preview_state());
        app.handle_key(KeyCode::Char('m'), KeyModifiers::CONTROL)
            .expect("Ctrl+M key event should still be consumed without panic");
        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("picker state should still be inspectable");
        assert!(
            !picker.entries[0].is_memory_model,
            "Ctrl+M must no longer mark the memory model"
        );

        // macOS terminals with the default Option behavior insert Option+M as
        // `µ` with no ALT modifier. The picker must map it back to Alt+M so the
        // advertised chord works there too, mirroring the global side-panel
        // toggle's fallback. On non-macOS platforms this arm is a no-op.
        app.inline_interactive_state = Some(remote_model_picker_preview_state());
        if cfg!(target_os = "macos") {
            app.handle_key(KeyCode::Char('µ'), KeyModifiers::NONE)
                .expect("µ (macOS Option+M) should be consumed");
            let picker = app
                .inline_interactive_state
                .as_ref()
                .expect("picker should stay open after µ");
            assert!(
                picker.entries[0].is_memory_model,
                "macOS Option+M (µ) must mark the memory model"
            );
        }
    });
}
