// Ctrl+Enter on a slash or shell command while a turn runs queues the command
// and runs it as a real command once the turn ends, instead of running it now
// or sending its text to the model.

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }
}

#[test]
fn test_ctrl_enter_queues_slash_command_while_processing() {
    let mut app = create_test_app();
    app.is_processing = true;

    type_text(&mut app, "/help");
    app.handle_key(KeyCode::Enter, KeyModifiers::CONTROL)
        .unwrap();

    assert_eq!(app.queued_commands, vec!["/help"]);
    assert!(app.queued_messages().is_empty(), "never sent as a prompt");
    assert!(app.input().is_empty());
    assert!(app.help_scroll.is_none(), "not run yet");
    assert_eq!(crate::tui::TuiState::queued_commands(&app), &["/help"]);
}

#[test]
fn test_ctrl_enter_queues_shell_command_while_processing() {
    let mut app = create_test_app();
    app.is_processing = true;

    type_text(&mut app, "!echo hi");
    app.handle_key(KeyCode::Enter, KeyModifiers::CONTROL)
        .unwrap();

    assert_eq!(app.queued_commands, vec!["!echo hi"]);
    assert!(app.queued_messages().is_empty());
}

#[test]
fn test_plain_enter_still_runs_slash_command_immediately_while_processing() {
    let mut app = create_test_app();
    app.is_processing = true;

    type_text(&mut app, "/help");
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    assert!(app.queued_commands.is_empty());
    assert!(app.help_scroll.is_some(), "plain Enter runs the command now");
}

#[test]
fn test_ctrl_enter_on_idle_slash_command_runs_it_now() {
    let mut app = create_test_app();

    type_text(&mut app, "/help");
    app.handle_key(KeyCode::Enter, KeyModifiers::CONTROL)
        .unwrap();

    assert!(app.queued_commands.is_empty());
    assert!(app.help_scroll.is_some());
}

#[test]
fn test_local_queued_command_runs_on_idle_tick_and_restores_draft() {
    let mut app = create_test_app();
    app.is_processing = true;
    type_text(&mut app, "/help");
    app.handle_key(KeyCode::Enter, KeyModifiers::CONTROL)
        .unwrap();

    // The user starts drafting the next prompt before the turn ends.
    type_text(&mut app, "next idea");
    app.is_processing = false;
    super::local::handle_tick(&mut app);

    assert!(app.queued_commands.is_empty());
    assert!(app.help_scroll.is_some(), "the queued command ran");
    assert_eq!(app.input(), "next idea", "the draft survives");
}

#[test]
fn test_ctrl_up_recalls_queued_command_for_editing() {
    let mut app = create_test_app();
    app.is_processing = true;
    type_text(&mut app, "/compact");
    app.handle_key(KeyCode::Enter, KeyModifiers::CONTROL)
        .unwrap();

    app.handle_key(KeyCode::Up, KeyModifiers::CONTROL).unwrap();

    assert_eq!(app.input(), "/compact");
    assert!(app.queued_commands.is_empty());
}

#[test]
fn test_reload_round_trip_preserves_queued_commands() {
    let mut app = create_test_app();
    let session_id = format!("test-queued-commands-reload-{}", std::process::id());
    app.queued_commands.push("/compact".to_string());
    app.save_input_for_reload(&session_id);

    let restored = App::new_for_remote(Some(session_id));
    assert_eq!(restored.queued_commands, vec!["/compact"]);
    assert!(!restored.is_processing);
}

#[test]
fn test_remote_ctrl_enter_queues_command_and_runs_it_after_turn() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.current_message_id = Some(7);
    app.input = "/help".to_string();
    app.cursor_pos = app.input.len();
    rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::CONTROL, &mut remote))
        .unwrap();

    assert_eq!(app.queued_commands, vec!["/help"]);
    assert!(app.help_scroll.is_none(), "not run while the turn is live");

    // A follow-up check mid-turn must not run it.
    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));
    assert_eq!(app.queued_commands, vec!["/help"]);

    app.input = "half-typed".to_string();
    app.cursor_pos = app.input.len();
    app.handle_server_event(crate::protocol::ServerEvent::Done { id: 7 }, &mut remote);
    assert!(!app.is_processing);
    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));

    assert!(app.queued_commands.is_empty());
    assert!(app.help_scroll.is_some(), "ran as a real command");
    assert!(app.queued_messages().is_empty(), "never became a prompt");
    assert_eq!(app.input(), "half-typed");
    assert!(
        !app.display_messages()
            .iter()
            .any(|m| m.role == "user" && m.content.contains("/help")),
        "the command text was not sent to the model"
    );
}

#[test]
fn test_remote_queued_command_still_runs_after_interrupt() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.current_message_id = Some(9);
    app.queued_commands.push("/help".to_string());

    app.handle_server_event(crate::protocol::ServerEvent::Interrupted, &mut remote);
    assert!(!app.is_processing);
    app.pending_queued_dispatch = false;
    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));

    assert!(app.queued_commands.is_empty());
    assert!(app.help_scroll.is_some());
}

#[test]
fn test_remote_tick_arms_dispatch_for_idle_queued_command() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    app.queued_commands.push("/help".to_string());
    rt.block_on(remote::handle_tick(&mut app, &mut remote));
    assert!(app.pending_queued_dispatch, "an idle client must not strand it");
}

#[test]
fn test_remote_clear_drops_queued_commands() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    app.queued_commands.push("/compact".to_string());
    app.input = "/clear".to_string();
    app.cursor_pos = app.input.len();
    let _ = rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote));

    assert!(app.queued_commands.is_empty());
}

#[test]
fn test_remote_queued_commands_run_before_queued_prompts() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    app.queued_commands.push("/help".to_string());
    app.queued_messages.push("then do this".to_string());
    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));

    assert!(app.queued_commands.is_empty());
    assert!(app.help_scroll.is_some());
    assert_eq!(app.queued_messages(), &["then do this"], "prompt waits its turn");
}

fn queued_image_command_draft(app: &mut App, command: &str) {
    type_text(app, &format!("{command} literal [image 1] "));
    super::input::handle_text_paste(app, "one\ntwo\nthree\nfour\nfive\nsix".into());
    super::input::insert_input_text(app, " ");
    super::input_images::attach_image(app, "image/png".into(), "queue-fixture-image".into());
}

#[test]
fn queued_command_image_refusal_local_preserves_complete_draft() {
    for command in ["/help", "!echo fixture"] {
        let mut app = create_test_app();
        app.is_processing = true;
        app.queued_commands.push("/compact".into());
        queued_image_command_draft(&mut app, command);
        app.cursor_pos = 2;
        let original = app.input_draft_snapshot();
        let undo = app.input_undo_stack.clone();
        let typing_undo = app.input_typing_undo;

        app.handle_key(KeyCode::Enter, KeyModifiers::CONTROL)
            .unwrap();

        assert_eq!(
            app.input_draft_snapshot(),
            original,
            "refusal must retain the whole draft"
        );
        assert_eq!(app.input_undo_stack, undo);
        assert_eq!(app.input_typing_undo, typing_undo);
        assert_eq!(app.queued_commands, ["/compact"]);
        assert!(app.queued_messages.is_empty());
        assert!(app.interleave_message.is_none());
        assert!(app.status_notice().unwrap().contains("Press Enter"));

        // Undo still removes the attachment, and its paste payload remains available.
        app.undo_input_change();
        assert!(app.pending_images.is_empty());
        assert_eq!(app.pasted_contents, ["one\ntwo\nthree\nfour\nfive\nsix"]);
        app.restore_input_draft(original.clone());
        let prepared = super::input::take_prepared_input(&mut app);
        assert_eq!(prepared.raw_input, original.0);
        assert_eq!(
            prepared.expanded,
            format!("{command} literal [image 1] one\ntwo\nthree\nfour\nfive\nsix ")
        );
        assert_eq!(
            prepared.images,
            [("image/png".into(), "queue-fixture-image".into())]
        );
        assert!(prepared.routing_input().starts_with(command));

        type_text(&mut app, "newer composer");
        let newer = super::input::take_prepared_input(&mut app);
        assert_eq!(newer.expanded, "newer composer");
        assert!(
            newer.images.is_empty(),
            "the next draft must not inherit an image"
        );
        assert!(newer.pasted_contents.is_empty());
    }
}

#[test]
fn queued_command_image_refusal_remote_keeps_normal_enter_payload_and_newer_composer() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.current_message_id = Some(7);
    queued_image_command_draft(&mut app, "/help");
    let original = app.input_draft_snapshot();
    let undo = app.input_undo_stack.clone();

    rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::CONTROL, &mut remote))
        .unwrap();

    assert_eq!(
        app.input_draft_snapshot(),
        original,
        "refusal must retain the whole draft"
    );
    assert_eq!(app.input_undo_stack, undo);
    assert!(app.queued_commands.is_empty());
    assert!(app.queued_messages.is_empty());
    assert!(app.interleave_message.is_none());
    assert!(
        app.rate_limit_pending_message.is_none(),
        "refused shortcut cannot fall through to a send"
    );
    assert_eq!(app.current_message_id, Some(7));
    assert!(app.status_notice().unwrap().contains("Press Enter"));

    // The user explicitly turns the preserved slash draft into a prompt, then sends it.
    app.is_processing = false;
    app.cursor_pos = 0;
    super::input::insert_input_text(&mut app, "Explain ");
    rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::NONE, &mut remote))
        .unwrap();
    let sent = app
        .rate_limit_pending_message
        .as_ref()
        .expect("normal Enter sends");
    assert_eq!(
        sent.content,
        "Explain /help literal [image 1] one\ntwo\nthree\nfour\nfive\nsix "
    );
    assert_eq!(
        sent.images,
        [("image/png".into(), "queue-fixture-image".into())]
    );
    assert!(app.input.is_empty());
    assert!(app.pending_images.is_empty());
    assert!(app.pasted_contents.is_empty());

    type_text(&mut app, "newer composer");
    rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::CONTROL, &mut remote))
        .unwrap();
    assert_eq!(app.queued_messages, ["newer composer"]);
    assert!(
        app.queued_images.is_empty(),
        "later queued prompt cannot inherit the old image"
    );
    assert!(app.queued_commands.is_empty());
}
