async fn clear_draft_key(
    mode: u8,
    app: &mut App,
    remote: &mut crate::tui::backend::RemoteConnection,
    key: KeyCode,
) {
    match mode {
        0 => app.handle_key(key, KeyModifiers::CONTROL).unwrap(),
        1 => app
            .handle_remote_key(key, KeyModifiers::CONTROL, remote)
            .await
            .unwrap(),
        _ => super::remote::handle_disconnected_key(app, key, KeyModifiers::CONTROL).unwrap(),
    }
}

#[test]
fn clear_attachment_draft_all_clients_clear_image_only_then_quit() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    for mode in 0..3 {
        let mut app = create_test_app();
        app.is_remote = mode != 0;
        app.pending_images = vec![("image/png".into(), "first-payload".into())];
        rt.block_on(async {
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            clear_draft_key(mode, &mut app, &mut remote, KeyCode::Char('c')).await;
            assert!(app.input.is_empty());
            assert!(
                app.pending_images.is_empty(),
                "mode {mode} left an image in the composer"
            );
            assert!(!app.should_quit);
            assert!(app.quit_pending.is_some());
            clear_draft_key(mode, &mut app, &mut remote, KeyCode::Char('c')).await;
            assert!(app.should_quit, "mode {mode} required a third press");
        });
    }
}

#[test]
fn clear_attachment_draft_each_clear_restores_its_own_images_and_pastes() {
    let mut app = create_test_app();
    super::input::handle_text_paste(&mut app, "a\nb\nc\nd\ne\nf".into());
    super::input_images::attach_image(&mut app, "image/png".into(), "first-payload".into());
    let original = app.input.clone();
    app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL)
        .unwrap();
    super::input_images::attach_image(&mut app, "image/png".into(), "second-payload".into());
    app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL)
        .unwrap();
    app.undo_input_change();
    assert_eq!(app.pending_images[0].1, "second-payload");
    app.undo_input_change();
    assert!(app.pending_images.is_empty());
    app.undo_input_change();
    assert_eq!(app.input, original);
    assert_eq!(app.pending_images[0].1, "first-payload");
    assert_eq!(app.pasted_contents, vec!["a\nb\nc\nd\ne\nf"]);
}

#[test]
fn clear_attachment_draft_disconnected_ctrl_d_forward_deletes_unicode() {
    let mut app = create_test_app();
    app.input = "héllo".into();
    app.cursor_pos = 1;
    super::remote::handle_disconnected_key(&mut app, KeyCode::Char('d'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.input, "hllo");
    assert_eq!(app.cursor_pos, 1);
    assert!(app.quit_pending.is_none());
    app.cursor_pos = app.input.len();
    super::remote::handle_disconnected_key(&mut app, KeyCode::Char('d'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.input, "hllo");
    assert!(app.quit_pending.is_none());
}

#[test]
fn clear_attachment_draft_remote_text_clear_resets_completion_and_keeps_undo() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.input = "/mo".into();
    app.cursor_pos = 3;
    app.tab_completion_state = Some(("/mo".into(), 1));
    app.command_suggestion_selected = 2;
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        clear_draft_key(1, &mut app, &mut remote, KeyCode::Char('c')).await;
    });
    assert!(app.input.is_empty());
    assert!(app.tab_completion_state.is_none());
    assert_eq!(app.command_suggestion_selected, 0);
    app.undo_input_change();
    assert_eq!(app.input, "/mo");
}
