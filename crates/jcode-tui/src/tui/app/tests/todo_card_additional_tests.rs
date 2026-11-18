use super::*;

#[test]
fn resumed_local_snapshot_keeps_polling_so_card_does_not_go_stale() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_session_id = Some("resumed-local-snapshot".into());
    let payload = |status: &str| {
        serde_json::json!({
            "todos": [{"id":"lane", "content":"Integrate lane", "priority":"high", "status":status}],
            "plan": {}, "goals": []
        })
        .to_string()
    };
    // Resume seeds an authoritative snapshot, after which disk is ignored.
    assert!(app.receive_remote_todos("resumed-local-snapshot".into(), payload("in_progress")));
    assert!(
        !app.receive_remote_todos("resumed-local-snapshot".into(), payload("in_progress")),
        "an identical poll answer must not ask for a repaint"
    );
    app.toggle_todo_card();
    assert!(
        app.remote_todos_refresh_due(),
        "a snapshot-backed card must request fresh todos, not freeze"
    );
    app.receive_remote_todos("resumed-local-snapshot".into(), payload("completed"));
    let card = app
        .display_messages
        .iter()
        .find(|m| m.role == "todos")
        .unwrap();
    assert!(card.content.contains("completed"), "{}", card.content);
}

#[test]
fn background_turn_polls_todos_slowly_and_catches_up_on_focus() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_session_id = Some("background-turn".into());
    let payload = serde_json::json!({
        "todos": [{"id":"lane", "content":"Lane", "priority":"high", "status":"pending"}],
        "plan": {}, "goals": []
    })
    .to_string();
    app.receive_remote_todos("background-turn".into(), payload);
    app.toggle_todo_card();
    app.is_processing = true;
    let five_seconds_ago = std::time::Instant::now() - std::time::Duration::from_secs(5);

    app.remote_todos_requested_at = Some(five_seconds_ago);
    assert!(
        app.remote_todos_refresh_due(),
        "a focused turn keeps the 2s poll"
    );

    app.set_client_focused(false);
    app.remote_todos_requested_at = Some(five_seconds_ago);
    assert!(
        !app.remote_todos_refresh_due(),
        "an unfocused turn must not poll every 2s"
    );

    app.set_client_focused(true);
    assert!(
        app.remote_todos_refresh_due(),
        "gaining focus refreshes at once"
    );
}

#[test]
fn a_pushing_server_ends_the_periodic_todo_poll() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_session_id = Some("pushed-turn".into());
    let payload = serde_json::json!({
        "todos": [{"id":"lane", "content":"Lane", "priority":"high", "status":"pending"}],
        "plan": {}, "goals": []
    })
    .to_string();
    app.receive_remote_todos("pushed-turn".into(), payload);
    app.toggle_todo_card();
    app.is_processing = true;
    let a_minute_ago = std::time::Instant::now() - std::time::Duration::from_secs(60);

    app.remote_todos_requested_at = Some(a_minute_ago);
    assert!(
        app.remote_todos_refresh_due(),
        "a server that does not push is still polled"
    );

    app.remote_todos_pushed = true;
    app.remote_todos_requested_at = Some(a_minute_ago);
    assert!(
        !app.remote_todos_refresh_due(),
        "a focused turn must not poll a server that pushes"
    );

    app.request_remote_todos_refresh_soon();
    assert!(
        app.remote_todos_refresh_due(),
        "an explicit refresh still asks once"
    );
}
