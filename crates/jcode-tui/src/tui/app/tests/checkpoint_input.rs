use super::*;

#[test]
fn checkpoint_local_automatic_queue_keeps_human_boundaries() {
    let mut app = create_test_app();
    app.append_queued_user_payload(&crate::todo::build_auto_poke_message(2));
    app.append_queued_user_payload("<system-reminder>Continue.</system-reminder>");
    assert!(app.session.rewind_checkpoints().unwrap().is_empty());
    assert!(
        !app.display_messages
            .iter()
            .any(|message| message.role == "error")
    );
    let boundary = app.session.messages.len();
    app.queued_message_images = vec![("image/png".into(), "AA==".into())];
    app.append_queued_user_payload("");
    let targets = app.session.rewind_checkpoints().unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].message_index, boundary);
    assert!(targets[0].can_restore_files);
}

#[test]
fn checkpoint_local_submission_ignores_synthetic_envelope_without_bind_error() {
    let mut app = create_test_app();
    app.input = crate::todo::build_auto_poke_message(2);
    app.cursor_pos = app.input.len();
    app.submit_input();
    assert!(app.session.rewind_checkpoints().unwrap().is_empty());
    assert!(
        !app.display_messages
            .iter()
            .any(|message| message.role == "error")
    );
}
