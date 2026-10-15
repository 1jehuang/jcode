// Real clipboard images must track their visible selection through edits and undo.
fn paste_selection_test_png(app: &mut App, value: u8) -> String {
    use base64::Engine;
    let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        1,
        1,
        image::Rgba([value, 0, 0, 255]),
    ));
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).unwrap();
    let data = base64::engine::general_purpose::STANDARD.encode(png.into_inner());
    let result = clipboard_paste_result(
        app,
        crate::bus::ClipboardPasteContent::Image {
            media_type: "image/png".to_string(),
            base64_data: data.clone(),
        },
    );
    assert!(app.handle_clipboard_paste_completed(result));
    data
}

#[test]
fn selected_image_delete_and_replace_do_not_submit_png() {
    for key in [KeyCode::Backspace, KeyCode::Delete, KeyCode::Char('x')] {
        let mut app = create_test_app();
        app.input = "explain ".to_string();
        app.cursor_pos = app.input.len();
        paste_selection_test_png(&mut app, 12);
        app.set_input_selection(8, app.input.len());
        app.handle_key(key, KeyModifiers::empty()).unwrap();
        assert!(app.pending_images.is_empty(), "{key:?} left a hidden PNG");
        let prepared = super::input::take_prepared_input(&mut app);
        assert!(prepared.images.is_empty(), "{key:?} submitted a hidden PNG");
    }
}

#[test]
fn selected_image_edit_undo_restores_png_with_placeholder() {
    for key in [KeyCode::Backspace, KeyCode::Delete, KeyCode::Char('x')] {
        let mut app = create_test_app();
        let png = paste_selection_test_png(&mut app, 24);
        app.set_input_selection(0, app.input.len());
        app.handle_key(key, KeyModifiers::empty()).unwrap();
        app.handle_key(KeyCode::Char('z'), KeyModifiers::CONTROL)
            .unwrap();
        assert_eq!(app.input, "[image 1]");
        assert_eq!(
            app.pending_images,
            vec![("image/png".to_string(), png.clone())]
        );
        let prepared = super::input::take_prepared_input(&mut app);
        assert_eq!(prepared.images, vec![("image/png".to_string(), png)]);
    }
}

#[test]
fn selected_first_image_removal_renumbers_remaining_image_and_undo() {
    let mut app = create_test_app();
    let first = paste_selection_test_png(&mut app, 1);
    super::input::insert_input_text(&mut app, " ");
    let second = paste_selection_test_png(&mut app, 2);
    app.set_input_selection(0, "[image 1]".len());
    app.handle_key(KeyCode::Delete, KeyModifiers::empty())
        .unwrap();
    assert_eq!(app.input, " [image 1]");
    assert_eq!(
        app.pending_images,
        vec![("image/png".to_string(), second.clone())]
    );
    app.undo_input_change();
    assert_eq!(app.input, "[image 1] [image 2]");
    assert_eq!(
        app.pending_images,
        vec![
            ("image/png".to_string(), first),
            ("image/png".to_string(), second),
        ]
    );
}

#[test]
fn selected_image_paste_replaces_payload_and_undo_restores_original() {
    let mut app = create_test_app();
    let old = paste_selection_test_png(&mut app, 3);
    app.set_input_selection(0, app.input.len());
    let new = paste_selection_test_png(&mut app, 4);
    assert_eq!(app.input, "[image 1]");
    assert_eq!(app.pending_images, vec![("image/png".to_string(), new)]);
    app.undo_input_change();
    assert_eq!(app.input, "[image 1]");
    assert_eq!(app.pending_images, vec![("image/png".to_string(), old)]);
    app.undo_input_change();
    assert!(app.input.is_empty());
    assert!(
        app.pending_images.is_empty(),
        "undoing the initial paste must remove its PNG"
    );
}

#[test]
fn selected_image_text_paste_does_not_submit_png() {
    let mut app = create_test_app();
    paste_selection_test_png(&mut app, 5);
    app.set_input_selection(0, app.input.len());
    super::input::handle_text_paste(&mut app, "replacement".to_string());
    assert_eq!(app.input, "replacement");
    app.submit_input();
    assert!(
        app.session.messages.iter().all(|message| message
            .content
            .iter()
            .all(|block| !matches!(block, crate::message::ContentBlock::Image { .. }))),
        "local submission leaked the removed PNG"
    );
}

#[test]
fn selected_image_disconnected_delete_and_connected_replace_clear_payload() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    for key in [KeyCode::Backspace, KeyCode::Delete, KeyCode::Char('x')] {
        let mut app = create_test_app();
        paste_selection_test_png(&mut app, 6);
        app.set_input_selection(0, app.input.len());
        super::remote::handle_disconnected_key(&mut app, key, KeyModifiers::empty()).unwrap();
        assert!(app.pending_images.is_empty(), "disconnected {key:?}");
        app.undo_input_change();
        app.set_input_selection(0, app.input.len());
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        rt.block_on(app.handle_remote_key(key, KeyModifiers::empty(), &mut remote))
            .unwrap();
        assert!(app.pending_images.is_empty(), "connected {key:?}");
    }
}

#[test]
fn image_partial_selection_and_ordinary_deletes_remove_attachment_atomically() {
    for selected in [false, true] {
        for key in [KeyCode::Backspace, KeyCode::Delete] {
            let mut app = create_test_app();
            let png = paste_selection_test_png(&mut app, 7);
            if selected {
                app.set_input_selection(2, 5);
            } else {
                app.cursor_pos = 4;
            }
            app.handle_key(key, KeyModifiers::empty()).unwrap();
            assert!(app.input.is_empty(), "{selected} {key:?}");
            assert!(app.pending_images.is_empty());
            assert_eq!(app.cursor_pos, 0);
            app.undo_input_change();
            assert_eq!(app.input, "[image 1]");
            assert_eq!(app.pending_images, vec![("image/png".to_string(), png)]);
        }
    }
}

#[test]
fn image_typing_inside_marker_drops_payload_and_undo_restores_it() {
    let mut app = create_test_app();
    let png = paste_selection_test_png(&mut app, 8);
    app.cursor_pos = 4;
    app.handle_key(KeyCode::Char('x'), KeyModifiers::empty())
        .unwrap();
    assert!(app.pending_images.is_empty());
    app.undo_input_change();
    assert_eq!(app.input, "[image 1]");
    assert_eq!(app.pending_images, vec![("image/png".to_string(), png)]);
}

#[test]
fn image_renumbering_before_caret_preserves_unicode_and_payload_order() {
    let mut app = create_test_app();
    let mut images = Vec::new();
    for value in 1..=11 {
        images.push((
            "image/png".to_string(),
            paste_selection_test_png(&mut app, value),
        ));
        super::input::insert_input_text(&mut app, " ");
    }
    // Move image 11 before image 1 to exercise renumbering before the caret.
    app.input = format!(
        "[image 11] é {}",
        (1..=10).map(|n| format!("[image {n}]")).collect::<String>()
    );
    let at = app.input.find("[image 1]").unwrap();
    app.set_input_selection(at, at + "[image 1]".len());
    app.handle_key(KeyCode::Delete, KeyModifiers::empty())
        .unwrap();
    assert!(app.input.starts_with("[image 10] é "));
    assert_eq!(app.cursor_pos, "[image 10] é ".len());
    assert_eq!(app.pending_images, images[1..]);
    app.handle_key(KeyCode::Char('!'), KeyModifiers::empty())
        .unwrap();
    assert!(app.input.starts_with("[image 10] é !"));
}

#[test]
fn image_text_only_undo_snapshots_share_large_payloads() {
    let mut app = create_test_app();
    paste_selection_test_png(&mut app, 9);
    app.pending_images[0]
        .1
        .push_str(&"A".repeat(2 * 1024 * 1024));
    super::input::insert_input_text(&mut app, " first");
    super::input::insert_input_text(&mut app, " second");
    let snapshots = &app.input_undo_stack;
    let len = snapshots.len();
    assert!(
        std::sync::Arc::ptr_eq(&snapshots[len - 1].2, &snapshots[len - 2].2),
        "text-only undo entries must share, not clone multi-MB base64 payloads"
    );
}
