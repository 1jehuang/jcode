// Pins how `[image N]` placeholders in the input buffer interact with
// slash-command parsing in submit_input (crates/jcode-tui/src/tui/app/input.rs).
//
// Placeholders are plain text: expand_paste_placeholders only expands
// `[pasted N lines]` markers, never `[image N]`. Command routing therefore
// sees the placeholder literally.

fn attach_test_image(app: &mut App) {
    // Mirrors attach_image() in input.rs: push pending image, insert
    // "[image N]" placeholder at the cursor.
    app.pending_images
        .push(("image/png".to_string(), "aGVsbG8=".to_string()));
    let placeholder = format!("[image {}]", app.pending_images.len());
    let mut input = app.input().to_string();
    let pos = input.len();
    input.insert_str(pos, &placeholder);
    app.set_input_for_test(input);
}

#[test]
fn test_image_attachment_deletion_reports_detach_and_keeps_other_image() {
    let mut app = create_test_app();
    attach_test_image(&mut app);
    attach_test_image(&mut app);
    app.set_input_for_test("[image 2]");
    app.sync_model_picker_preview_from_input();
    assert_eq!(app.status_notice(), Some("Detached 1 image · 1 attached".to_string()));
    assert!(!crate::tui::TuiState::input_image_attached(&app, 1));
    assert!(crate::tui::TuiState::input_image_attached(&app, 2));
    assert!(!crate::tui::TuiState::input_image_attached(&app, 0));
    let prepared = crate::tui::app::input::take_prepared_input(&mut app);
    assert_eq!(prepared.images.len(), 1);
    assert_eq!(prepared.raw_input, "[image 2]");
}

#[test]
fn test_image_attachment_backspace_detaches_payload_and_reports_status() {
    let mut app = create_test_app();
    attach_test_image(&mut app);
    app.handle_key(KeyCode::Backspace, KeyModifiers::NONE).unwrap();
    assert_eq!(app.status_notice(), Some("Detached 1 image · 0 attached".to_string()));
    assert!(!crate::tui::TuiState::input_image_attached(&app, 1));
    // Undoing the text cannot falsely claim that the released payload exists.
    app.undo_input_change();
    assert_eq!(app.input(), "[image 1]");
    assert!(!crate::tui::TuiState::input_image_attached(&app, 1));
    let prepared = crate::tui::app::input::take_prepared_input(&mut app);
    assert!(prepared.images.is_empty());
}

#[test]
fn test_image_placeholder_before_text_submits_as_user_turn_with_image() {
    let mut app = create_test_app();
    attach_test_image(&mut app);
    let prompt = format!("{} describe this", app.input());
    app.set_input_for_test(prompt.clone());

    app.submit_input();

    assert!(app.is_processing, "placeholder + text should start a turn");
    assert!(
        app.pending_images.is_empty(),
        "submitting must consume pending images"
    );
    let submitted = app.session.messages.last().expect("submitted message");
    assert!(
        matches!(submitted.content.first(), Some(ContentBlock::Image { .. })),
        "image block must be attached"
    );
    assert!(matches!(
        submitted.content.last(),
        Some(ContentBlock::Text { text, .. }) if text == &prompt
    ));
}

#[test]
fn test_image_placeholder_prefix_prevents_slash_command_routing() {
    let mut app = create_test_app();
    attach_test_image(&mut app);
    let input = format!("{}/help", app.input());
    app.set_input_for_test(input);

    app.submit_input();

    // Input does not start with '/', so it is a normal user turn (with the
    // image attached), not a /help invocation.
    assert!(app.is_processing);
    assert!(app.help_scroll.is_none(), "help must not open");
    assert!(app.pending_images.is_empty());
}

#[test]
fn test_slash_command_with_trailing_image_placeholder_routes_as_command() {
    let mut app = create_test_app();
    app.set_input_for_test("/help ");
    attach_test_image(&mut app);
    assert_eq!(app.input(), "/help [image 1]");

    app.submit_input();

    // "/help [image 1]" is parsed as `/help <topic>` with the literal
    // placeholder as topic, so it reports an unknown command and the
    // pending image stays attached in the app (not sent, not dropped).
    assert!(!app.is_processing, "command routing must not start a turn");
    let last = app.display_messages().last().expect("display message");
    assert_eq!(last.role, "error");
    assert!(
        last.content.contains("Unknown command"),
        "unexpected message: {}",
        last.content
    );
    assert_eq!(
        app.pending_images.len(),
        1,
        "handled command leaves the pending image queued"
    );
}

#[test]
fn test_unknown_skill_with_image_placeholder_reports_error_and_keeps_image() {
    let mut app = create_test_app();
    app.set_input_for_test("/definitely-not-a-skill ");
    attach_test_image(&mut app);

    app.submit_input();

    assert!(!app.is_processing);
    let last = app.display_messages().last().expect("display message");
    assert_eq!(last.role, "error");
    assert!(
        last.content.contains("Unknown skill"),
        "unexpected message: {}",
        last.content
    );
    assert_eq!(app.pending_images.len(), 1);
}
