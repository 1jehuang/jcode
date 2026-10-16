#[test]
fn test_save_and_restore_reload_state_preserves_observe_mode() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        let session_id = format!("test-reload-observe-{}", std::process::id());

        app.set_observe_mode_enabled(true, true);
        app.observe_page_markdown = "# Observe\n\nPersist me through reload.".to_string();
        app.observe_page_updated_at_ms = 42;
        app.save_input_for_reload(&session_id);

        let restored =
            App::restore_input_for_reload(&session_id).expect("reload state should exist");
        assert!(restored.observe_mode_enabled);
        assert_eq!(
            restored.observe_page_markdown,
            "# Observe\n\nPersist me through reload."
        );
        assert_eq!(restored.observe_page_updated_at_ms, 42);
    });
}

#[test]
fn test_save_and_restore_reload_state_preserves_split_view_mode() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        let session_id = format!("test-reload-splitview-{}", std::process::id());

        app.set_split_view_enabled(true, true);
        app.save_input_for_reload(&session_id);

        let restored =
            App::restore_input_for_reload(&session_id).expect("reload state should exist");
        assert!(restored.split_view_enabled);
    });
}
