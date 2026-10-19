#[test]
fn test_model_picker_preview_arrow_keys_navigate() {
    let mut app = create_test_app();
    configure_test_remote_models(&mut app);

    // Type /model to open preview
    for c in "/model".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker preview should be open");
    assert!(picker.preview);
    let initial_selected = picker.selected;

    // Down arrow should navigate in preview mode
    app.handle_key(KeyCode::Down, KeyModifiers::empty())
        .unwrap();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("picker should still be open");
    assert!(picker.preview, "should remain in preview mode");
    assert_eq!(picker.selected, initial_selected + 1);

    // Up arrow should navigate back
    app.handle_key(KeyCode::Up, KeyModifiers::empty()).unwrap();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("picker should still be open");
    assert!(picker.preview, "should remain in preview mode");
    assert_eq!(picker.selected, initial_selected);

    // Opening the preview should place the cursor in the model filter argument.
    assert_eq!(app.input(), "/model ");
}

#[test]
fn test_open_model_picker_without_routes_shows_actionable_guidance() {
    let mut app = create_test_app();

    app.open_model_picker();
    wait_for_model_picker_load(&mut app);

    assert!(app.inline_interactive_state.is_none());
    assert_eq!(app.status_notice(), Some("No models available".to_string()));

    let last = app.display_messages.last().expect("display message");
    assert_eq!(last.role, "system");
    assert!(last.content.contains("/login"));
    assert!(last.content.contains("/account"));
    assert!(last.content.contains("/model"));
}

#[test]
fn test_remote_model_picker_during_startup_waits_for_session_catalog() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.set_remote_startup_phase(crate::tui::app::RemoteStartupPhase::LoadingSession);
    app.remote_provider_model = Some("gpt-5.6-sol".to_string());
    app.remote_available_entries.clear();
    app.remote_model_options.clear();

    app.open_model_picker();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("loading model picker should be open");
    assert_eq!(picker.entries.len(), 1);
    assert_eq!(picker.entries[0].name, "gpt-5.6-sol");
    assert_eq!(picker.entries[0].options[0].detail, "updating model list…");
}

/// A large remote catalog must not be expanded on the UI thread: `/model`
/// shows a loading row immediately, the real rows arrive via polling, and the
/// next open is a cache hit with no pending background load.
#[test]
fn test_large_remote_model_picker_builds_off_ui_thread_then_caches() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_provider_name = Some("openrouter".to_string());
    app.remote_provider_model = Some("model-0".to_string());
    app.remote_available_entries = (0..600).map(|i| format!("model-{i}")).collect();
    app.remote_model_options = app
        .remote_available_entries
        .iter()
        .cloned()
        .map(|model| crate::provider::ModelRoute {
            model,
            provider: "OpenRouter".to_string(),
            api_method: "openrouter".to_string(),
            available: true,
            detail: String::new(),
            usage: None,
            cheapness: None,
        })
        .collect();

    app.open_model_picker();
    assert!(
        app.pending_model_picker_load.is_some(),
        "large catalog should build in the background"
    );
    assert_eq!(
        app.inline_interactive_state.as_ref().map(|p| p.entries.len()),
        Some(1),
        "picker should open immediately with a loading row"
    );

    let started = std::time::Instant::now();
    while app.pending_model_picker_load.is_some()
        && started.elapsed() < std::time::Duration::from_secs(10)
    {
        app.poll_model_picker_load();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let entries = app
        .inline_interactive_state
        .as_ref()
        .map(|p| p.entries.len())
        .unwrap_or(0);
    assert!(entries >= 600, "background build should fill the picker, got {entries}");

    app.inline_interactive_state = None;
    app.open_model_picker();
    assert!(
        app.pending_model_picker_load.is_none(),
        "second open should hit the cache"
    );
    assert!(
        app.inline_interactive_state
            .as_ref()
            .is_some_and(|p| p.entries.len() >= 600)
    );
}
