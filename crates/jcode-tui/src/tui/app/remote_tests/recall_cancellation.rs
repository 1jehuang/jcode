#[test]
fn remote_ctrl_up_preserves_recalled_draft_when_server_writer_is_broken() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.is_processing = true;
    app.pending_soft_interrupts.push("already sent followup".to_string());
    app.queued_messages.push("local queued followup".to_string());
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        drop(remote.take_dummy_peer().unwrap());
        let result = app.handle_remote_key(
            crossterm::event::KeyCode::Up,
            crossterm::event::KeyModifiers::CONTROL,
            &mut remote,
        ).await;
        assert!(result.is_ok(), "cancellation failure must not close the TUI: {result:?}");
        assert!(app.input.contains("already sent followup"));
        assert!(app.input.contains("local queued followup"));
        assert!(app.queued_messages.is_empty());
        assert!(app.display_messages().iter().any(|message| message.content.contains("server interrupt cancellation failed")));
    });
}

#[test]
fn remote_ctrl_up_does_not_cancel_server_interrupts_for_local_only_queue() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.is_processing = true;
    app.queued_messages.push("local only".to_string());
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        drop(remote.take_dummy_peer().unwrap());
        let request_id = remote.next_request_id_for_test();
        app.handle_remote_key(
            crossterm::event::KeyCode::Up,
            crossterm::event::KeyModifiers::CONTROL,
            &mut remote,
        ).await.unwrap();
        assert_eq!(app.input, "local only");
        assert_eq!(remote.next_request_id_for_test(), request_id);
        assert!(!app.display_messages().iter().any(|message| message.content.contains("server interrupt cancellation failed")));
    });
}
