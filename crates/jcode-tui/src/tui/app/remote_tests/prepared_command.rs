// PreparedInput carries composer provenance even when dispatch chooses a command boundary.
#[test]
fn remote_multiple_known_slash_commands_use_the_local_command_boundary() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.runtime_mode = crate::tui::app::AppRuntimeMode::RemoteClient;

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(crate::tui::app::remote::submit_remote_slash_input(
        &mut app,
        &mut remote,
        crate::tui::app::input::PreparedInput {
            image_markers: Default::default(),
            routing_input: None,
            pasted_contents: Vec::new(),
            raw_input: "/help compact /test help".to_string(),
            expanded: "/help compact /test help".to_string(),
            images: vec![],
        },
    ))
    .expect("known built-in slash commands should stay local");

    let messages = app.display_messages();
    assert_eq!(messages.len(), 2);
    assert!(messages[0].content.contains("/compact"));
    assert!(messages[1].content.contains("Usage: /test"));
    assert!(!app.pending_turn);
    assert!(!app.is_processing);
}
