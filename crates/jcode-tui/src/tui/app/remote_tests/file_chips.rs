use super::*;

#[test]
fn queue_for_reconnect_expands_and_consumes_file_chips() {
    // Regression (PR #1268 review): a prompt submitted while the remote
    // connection is down must carry the accepted file chip's contents into
    // the queued message, and the chip must be consumed so it cannot leak
    // into a later prompt.
    let mut app = create_test_app();
    app.is_remote = true;
    app.runtime_mode = crate::tui::app::AppRuntimeMode::RemoteClient;

    let dir = tempfile::TempDir::new().expect("temp dir");
    let file = dir.path().join("notes.md");
    std::fs::write(&file, "queued file contents").expect("write file");

    app.set_input_for_test("review @notes.md".to_string());
    app.file_chips.push(file.clone());
    super::super::queue_message_for_reconnect(&mut app);

    assert_eq!(
        app.queued_messages.len(),
        1,
        "prompt must be queued exactly once"
    );
    let queued = &app.queued_messages[0];
    assert!(
        queued.contains("queued file contents"),
        "queued message must embed the file contents, got: {queued}"
    );
    assert!(
        queued.contains("notes.md"),
        "queued message must reference the attached file, got: {queued}"
    );
    assert!(
        app.file_chips.is_empty(),
        "chips must be consumed by the queue path, not left to affect a later prompt"
    );
    assert!(
        app.input.is_empty(),
        "composer input must be consumed by the queue path"
    );
}
