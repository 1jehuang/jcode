use super::*;

#[test]
fn test_model_picker_copilot_selection_prefixes_model() {
    // The daemon shell exports a named provider profile whose runtime vars
    // short-circuit every built-in route availability check, so grok-code-fast-1
    // resolves to that profile's route (e.g. firmware:) instead of the copilot
    // fallback under test. `with_temp_jcode_home` captures and clears those
    // profile vars for the duration of the test (restoring them even on
    // panic) and also gives the selection a private home: Enter records the
    // choice in the picker usage store even in remote mode, and that write
    // must not land in a concurrently running locked test's tempdir.
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        configure_test_remote_models_with_copilot(&mut app);

        app.open_model_picker();

        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("model picker should be open");

        // Find grok-code-fast-1 (which should only be a copilot route)
        let grok_idx = picker
            .entries
            .iter()
            .position(|m| m.name == "grok-code-fast-1")
            .expect("grok-code-fast-1 should be in picker");

        // Navigate to it and select
        let filtered_pos = picker
            .filtered
            .iter()
            .position(|&i| i == grok_idx)
            .expect("grok-code-fast-1 should be in filtered list");

        // Set the selected position to grok's position
        app.inline_interactive_state.as_mut().unwrap().selected = filtered_pos;

        // Press Enter to select
        app.handle_key(KeyCode::Enter, KeyModifiers::empty())
            .unwrap();

        // In remote mode, selection should produce a pending_model_switch with copilot: prefix
        let spec = app
            .pending_model_switch
            .as_ref()
            .expect("copilot selection must produce a pending model switch");
        assert!(
            spec.starts_with("copilot:"),
            "copilot model should be prefixed with 'copilot:', got: {}",
            spec
        );
        // Picker should be closed
        assert!(app.inline_interactive_state.is_none());
    })
}
