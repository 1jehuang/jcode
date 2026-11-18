// Built-in voice input (Ctrl+Space / /voice), matching Jcode Desktop.

#[test]
fn voice_transcript_remote_honors_armed_new_session() {
    for busy in [false, true] {
        let mut app = create_test_app();
        app.is_remote = true;
        app.is_processing = busy;
        app.set_input_for_test("keep my draft");
        app.cursor_pos = 4;
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            app.handle_remote_key(KeyCode::Char(' '), KeyModifiers::SUPER, &mut remote)
                .await
                .unwrap();
            super::remote::submit_remote_voice_transcript(&mut app, &mut remote, "fix the test")
                .await
                .unwrap();
        });
        assert!(!app.route_next_prompt_to_new_session);
        assert_eq!(
            app.pending_split_prompt.as_ref().unwrap().content,
            "<transcription>\nfix the test\n</transcription>"
        );
        assert_eq!(app.input(), "keep my draft");
        assert_eq!(app.cursor_pos(), 4);
        assert!(app.interleave_message.is_none());
        assert!(app.queued_messages.is_empty());
        assert!(!app.pending_turn);
        assert!(app.current_message_id.is_none());
    }
}

#[test]
fn external_transcript_remote_honors_armed_new_session() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.is_processing = true;
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        app.handle_remote_key(KeyCode::Char(' '), KeyModifiers::SUPER, &mut remote)
            .await
            .unwrap();
        super::remote::apply_remote_transcript_event(
            &mut app,
            &mut remote,
            "fix the test".into(),
            crate::protocol::TranscriptMode::Send,
        )
        .await
        .unwrap();
    });
    assert!(!app.route_next_prompt_to_new_session);
    assert_eq!(
        app.pending_split_prompt.as_ref().unwrap().content,
        "[transcription] fix the test"
    );
    assert!(app.interleave_message.is_none());
    assert!(app.queued_messages.is_empty());
}

#[test]
fn voice_transcript_local_send_wraps_tags_and_keeps_typed_draft() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    app.set_input_for_test("half typed thought");
    app.cursor_pos = 4;

    super::remote::submit_voice_transcript(&mut app, "fix the flaky test");

    let last = app
        .display_messages()
        .last()
        .expect("user message displayed");
    assert_eq!(last.role, "user");
    assert_eq!(
        last.content,
        "<transcription>\nfix the flaky test\n</transcription>"
    );
    assert!(app.pending_turn, "local send uses the normal submit path");
    assert_eq!(app.input(), "half typed thought", "typed draft is kept");
    assert_eq!(app.cursor_pos(), 4, "cursor is restored");
}

#[test]
fn voice_transcript_remote_send_wraps_tags_and_keeps_typed_draft() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.set_input_for_test("draft stays");
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    rt.block_on(async {
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        super::remote::submit_remote_voice_transcript(&mut app, &mut remote, "rename the rows")
            .await
    })
    .expect("remote voice send succeeds");

    let last = app
        .display_messages()
        .last()
        .expect("user message displayed");
    assert_eq!(last.role, "user");
    assert_eq!(
        last.content,
        "<transcription>\nrename the rows\n</transcription>"
    );
    assert!(app.is_processing, "remote send enters processing");
    assert!(!app.pending_turn, "remote send never uses local pending_turn");
    assert_eq!(app.input(), "draft stays");
}

#[test]
fn voice_transcript_while_busy_steers_instead_of_waiting() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    app.is_processing = true;
    app.queue_mode = false;

    super::remote::submit_voice_transcript(&mut app, "also check the logs");

    assert_eq!(
        app.interleave_message.as_deref(),
        Some("<transcription>\nalso check the logs\n</transcription>"),
        "like Enter, a busy turn is steered right away"
    );
}

#[test]
fn voice_key_is_consumed_and_never_types_or_opens_other_actions() {
    let mut app = create_test_app();
    let key = crossterm::event::KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL);
    // Without a Nari key in the test sandbox this reports setup help rather
    // than recording, but the key itself must still be consumed.
    assert!(app.handle_voice_key_event(&key));
    assert!(app.input().is_empty());
    let plain = crossterm::event::KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
    assert!(!app.handle_voice_key_event(&plain), "plain space still types");
}

#[test]
fn voice_key_auto_repeat_is_not_a_second_toggle() {
    let mut app = create_test_app();
    let press = crossterm::event::KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL);
    let before = app.display_messages().len();
    assert!(app.handle_voice_key_event(&press));
    let after_first = app.display_messages().len();
    // A second press within the debounce window is the same physical hold.
    assert!(app.handle_voice_key_event(&press));
    assert_eq!(app.display_messages().len(), after_first);
    assert!(after_first >= before);
}

#[test]
fn voice_esc_passes_through_when_not_recording() {
    let mut app = create_test_app();
    let esc = crossterm::event::KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    assert!(
        !app.handle_voice_key_event(&esc),
        "Esc keeps its normal meaning when voice input is idle"
    );
}

#[test]
fn voice_input_help_and_command_are_registered() {
    assert!(super::commands_dispatch::contains_registered_slash_command("/voice"));
    let mut app = create_test_app();
    app.set_input_for_test("/voice extra");
    app.submit_input();
    let last = app.display_messages().last().expect("usage shown");
    assert!(last.content.contains("Usage: /voice"), "{}", last.content);
}

#[test]
fn korean_jamo_voice_chord_is_normalized_before_voice_interception() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    // Every terminal read site (local, remote, disconnected, turn loops) normalizes first,
    // so a Korean-layout Ctrl+ㅠ reaches the voice and shortcut handlers as Ctrl+b.
    let key = jcode_tui_core::korean_input::normalize_key_event(KeyEvent::new(
        KeyCode::Char('ㅠ'),
        KeyModifiers::CONTROL,
    ));
    assert_eq!(key.code, KeyCode::Char('b'));
    assert_eq!(key.modifiers, KeyModifiers::CONTROL);

    let plain = jcode_tui_core::korean_input::normalize_key_event(KeyEvent::new(
        KeyCode::Char('ㅠ'),
        KeyModifiers::NONE,
    ));
    assert_eq!(plain.code, KeyCode::Char('ㅠ'), "plain Korean typing is untouched");
}
