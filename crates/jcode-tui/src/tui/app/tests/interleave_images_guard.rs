/// Every place that consumes a staged interleave message must also carry its
/// staged images.
///
/// This is a source-level guard because the bug it prevents is invisible to
/// ordinary tests: one send path passed `vec![]` instead of the staged
/// attachments, which compiled cleanly, type-checked, and silently dropped every
/// image on an interleaved send. No panic, no error, no failing assertion.
/// `test_interleave_submission_preserves_pending_images` only proves the images
/// reach `app.interleave_images`; it cannot see a consumer that then ignores
/// them, and exercising the real consumer needs a live remote connection.
///
/// So assert the pairing directly: wherever `interleave_message.take()` happens,
/// a matching take of `interleave_images` must happen too, and the taken value
/// must be forwarded rather than replaced by an empty vector.
///
/// Verified by reintroducing the original bug: this fails with a message naming
/// the mismatch (2 paths consume the message, only 1 takes the images).
#[test]
fn every_interleave_send_path_carries_the_staged_images() {
    let source = include_str!("../remote.rs");

    let takes = source.matches("interleave_message.take()").count();
    assert!(
        takes > 0,
        "expected at least one interleave send path in remote.rs; if this moved, \
         move this guard with it rather than deleting it"
    );

    let image_takes = source
        .matches("std::mem::take(&mut app.interleave_images)")
        .count();
    assert_eq!(
        image_takes, takes,
        "every interleave send path must take the staged images ({takes} paths \
         consume interleave_message but only {image_takes} take \
         interleave_images). A path that omits this silently drops the user's \
         attachments while still compiling."
    );

    assert!(
        !source.contains("interleave_msg, vec![], false"),
        "an interleave send is passing an empty image vector instead of the \
         staged attachments, which silently drops them"
    );
}

#[test]
fn remote_image_queue_keeps_complete_draft_for_both_enter_shortcuts() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    for (queue_mode, modifiers) in [(true, KeyModifiers::NONE), (false, KeyModifiers::CONTROL)] {
        let mut app = create_test_app();
        app.is_remote = true;
        app.is_processing = true;
        app.queue_mode = queue_mode;
        app.input = "Compare [image 1] [pasted 2 lines]".to_string();
        app.pasted_contents = vec!["detail one\ndetail two".to_string()];
        app.cursor_pos = app.input.len();
        let expected = input::expand_paste_placeholders(&app, &app.input);
        let image = ("image/png".to_string(), "queued-image".to_string());
        app.pending_images = vec![image.clone()];
        let mut connection = crate::tui::backend::RemoteConnection::dummy();
        let next_id = connection.next_request_id_for_test();
        rt.block_on(remote::handle_remote_key(
            &mut app,
            KeyCode::Enter,
            modifiers,
            &mut connection,
        ))
        .unwrap();
        assert!(
            app.queued_messages.is_empty(),
            "no image-less prompt may be queued"
        );
        assert_eq!(app.input, expected);
        assert_eq!(app.cursor_pos, expected.len());
        app.sync_model_picker_preview_from_input();
        assert_eq!(app.pending_images, vec![image]);
        assert_eq!(connection.next_request_id_for_test(), next_id);
        assert!(
            app.status_notice
                .as_ref()
                .is_some_and(|(notice, _)| { notice.contains("Images cannot be queued") })
        );
        assert!(app.display_messages().iter().any(|message| {
            message.content.contains("Message not queued")
                && message
                    .content
                    .contains("text and images are still in the input")
        }));
    }
}

#[test]
fn remote_text_only_queue_still_accepts_both_enter_shortcuts() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    for (queue_mode, modifiers) in [(true, KeyModifiers::NONE), (false, KeyModifiers::CONTROL)] {
        let mut app = create_test_app();
        app.is_remote = true;
        app.is_processing = true;
        app.queue_mode = queue_mode;
        app.input = "Queue this text".to_string();
        app.cursor_pos = app.input.len();
        let mut connection = crate::tui::backend::RemoteConnection::dummy();
        rt.block_on(remote::handle_remote_key(
            &mut app,
            KeyCode::Enter,
            modifiers,
            &mut connection,
        ))
        .unwrap();
        assert_eq!(app.queued_messages, vec!["Queue this text".to_string()]);
        assert!(app.input.is_empty());
    }
}
