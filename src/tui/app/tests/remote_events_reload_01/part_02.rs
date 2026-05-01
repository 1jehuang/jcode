#[test]
fn test_remote_done_shows_footer_after_final_tool_result_without_trailing_text() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_processing = true;
    app.auto_poke_incomplete_todos = false;
    app.status = ProcessingStatus::Streaming;
    app.current_message_id = Some(42);
    app.processing_started = Some(Instant::now());
    app.visible_turn_started = Some(Instant::now());

    app.handle_server_event(
        crate::protocol::ServerEvent::ToolStart {
            id: "tool_read".to_string(),
            name: "read".to_string(),
        },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::ToolInput {
            delta: r#"{"file_path":"src/main.rs","start_line":1,"end_line":2}"#.to_string(),
        },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::ToolExec {
            id: "tool_read".to_string(),
            name: "read".to_string(),
        },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 123,
            output: 45,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::ToolDone {
            id: "tool_read".to_string(),
            name: "read".to_string(),
            output: "1 fn main() {}".to_string(),
            error: None,
        },
        &mut remote,
    );

    let needs_redraw =
        app.handle_server_event(crate::protocol::ServerEvent::Done { id: 42 }, &mut remote);

    assert!(
        needs_redraw,
        "remote Done must redraw after finalizing the response"
    );

    let footers: Vec<&DisplayMessage> = app
        .display_messages()
        .iter()
        .filter(|msg| msg.role == "meta")
        .collect();
    assert!(
        footers.iter().any(|msg| msg.content.contains("↑123 ↓45")),
        "footer not found"
    );
}

#[test]
fn test_remote_auto_poke_followup_preserves_visible_timer_and_stays_hidden() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();

        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "todo-1".to_string(),
                content: "Continue working".to_string(),
                status: "pending".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
            }],
        )
        .expect("save todos");

        let started = Instant::now() - Duration::from_secs(90);
        app.is_remote = true;
        app.auto_poke_incomplete_todos = true;
        app.is_processing = true;
        app.status = ProcessingStatus::Streaming;
        app.current_message_id = Some(42);
        app.visible_turn_started = Some(started);

        let needs_redraw =
            app.handle_server_event(crate::protocol::ServerEvent::Done { id: 42 }, &mut remote);

        assert!(needs_redraw);
        assert!(app.pending_queued_dispatch);

        app.pending_queued_dispatch = false;
        rt.block_on(remote::process_remote_followups(&mut app, &mut remote));

        assert_eq!(app.visible_turn_started, Some(started));
        assert!(app.is_processing);
        assert!(app.current_message_id.is_some());
        assert!(!app.display_messages().iter().any(|msg| {
            msg.role == "user"
                && msg
                    .content
                    .contains("Continue working, or update the todo tool.")
        }));
    });
}

#[test]
fn test_remote_poke_status_and_off_update_state() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "todo-1".to_string(),
                content: "Continue working".to_string(),
                status: "pending".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
            }],
        )
        .expect("save todos");

        app.is_remote = true;
        app.auto_poke_incomplete_todos = true;
        app.is_processing = true;
        app.status = ProcessingStatus::Streaming;
        app.current_message_id = Some(42);
        app.pending_queued_dispatch = true;
        app.queued_messages
            .push(super::commands::build_poke_message(
                &super::commands::incomplete_poke_todos(&app),
            ));

        app.input = "/poke status".to_string();
        app.cursor_pos = app.input.len();
        rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
            .expect("/poke status should succeed remotely");
        assert!(app.display_messages().iter().any(|msg| {
            msg.content
                .contains("Auto-poke: **ON**. 1 incomplete todo.")
                && msg.content.contains("A follow-up poke is queued.")
                && msg.content.contains("A turn is currently running.")
        }));

        app.input = "/poke off".to_string();
        app.cursor_pos = app.input.len();
        rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
            .expect("/poke off should succeed remotely");

        assert!(!app.auto_poke_incomplete_todos);
        assert!(!app.pending_queued_dispatch);
        assert!(app.queued_messages().is_empty());
        assert_eq!(app.status_notice(), Some("Poke: OFF".to_string()));
        assert!(app.display_messages().iter().any(|msg| {
            msg.content.contains("Auto-poke disabled.")
                && msg.content.contains("Cleared 1 queued poke follow-up")
        }));
    });
}

#[test]
fn test_remote_rewind_lists_display_history_when_session_transcript_is_empty() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_remote = true;
    app.session.messages.clear();
    app.push_display_message(DisplayMessage::user("hello"));
    app.push_display_message(DisplayMessage::assistant("hi there"));

    app.input = "/rewind".to_string();
    app.cursor_pos = app.input.len();
    rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
        .expect("/rewind should be handled remotely");

    let last = app.display_messages().last().expect("history message");
    assert!(last.content.contains("**Conversation history:**"));
    assert!(last.content.contains("`1` 👤 User - hello"));
    assert!(last.content.contains("`2` 🤖 Assistant - hi there"));
    assert!(!last.content.contains("No messages in conversation"));
}
