use super::*;

#[test]
fn compaction_notice_preserves_known_usage_and_handles_missing_metadata() {
    let mut event = crate::compaction::CompactionEvent {
        trigger: "auto_recovery".into(),
        pre_tokens: None,
        post_tokens: None,
        tokens_saved: None,
        duration_ms: None,
        messages_dropped: None,
        messages_compacted: None,
        summary_chars: None,
        active_messages: None,
    };
    let notice = |event: &crate::compaction::CompactionEvent| {
        App::format_compaction_complete_message(event, 200_000)
    };
    assert_eq!(notice(&event), "📦 Context compacted (automatic recovery)");

    event.messages_dropped = Some(1);
    assert_eq!(
        notice(&event),
        "📦 Context compacted (automatic recovery): 1 message dropped"
    );
    event.messages_dropped = Some(1_234);
    assert_eq!(
        notice(&event),
        "📦 Context compacted (automatic recovery): 1,234 messages dropped"
    );
    event.messages_compacted = Some(1);
    assert_eq!(
        notice(&event),
        "📦 Compacted 1 message (automatic recovery)"
    );
    event.messages_compacted = Some(0);
    assert_eq!(
        notice(&event),
        "📦 Compacted 0 messages (automatic recovery)"
    );

    event.pre_tokens = Some(123_456);
    assert_eq!(
        notice(&event),
        "📦 Compacted (automatic recovery): before ~123,456 tokens (61.7%); after unavailable"
    );
    assert_eq!(
        App::format_compaction_complete_message(&event, 0),
        "📦 Compacted (automatic recovery): before ~123,456 tokens; after unavailable"
    );
    event.pre_tokens = None;
    event.post_tokens = Some(0);
    assert_eq!(
        notice(&event),
        "📦 Compacted (automatic recovery): after ~0 tokens (0.0%); before unavailable"
    );
    event.post_tokens = Some(4_321);
    assert_eq!(
        App::format_compaction_complete_message(&event, 0),
        "📦 Compacted (automatic recovery): after ~4,321 tokens; before unavailable"
    );
    event.pre_tokens = Some(123_456);
    assert_eq!(
        notice(&event),
        "📦 Compacted (automatic recovery): ~123,456 tokens (61.7%) → ~4,321 tokens (2.2%)"
    );
}

/// Exercise the actual OpenAI SSE parser, the native completion wire shape,
/// remote event handling, and the terminal renderer. The provider reports no
/// before/after token count in this native automatic compaction item.
#[test]
fn native_auto_compaction_notice_reaches_terminal_without_invented_usage() {
    let mut app = super::super::tests::create_test_app();
    let _render_guard = crate::tui::ui::render_state_test_lock();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    app.is_remote = true;
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    let raw = r#"{"type":"response.output_item.done","item":{"id":"cmp_123","type":"compaction","encrypted_content":"enc_abc"}}"#;
    let event = jcode_provider_openai::stream::parse_openai_response_event(
        raw,
        &mut false,
        &mut false,
        &mut Default::default(),
        &mut Default::default(),
        &mut Default::default(),
    )
    .expect("native compaction event");
    let StreamEvent::Compaction {
        trigger,
        pre_tokens,
        openai_encrypted_content,
    } = event
    else {
        panic!("expected native compaction event");
    };
    assert_eq!(trigger, "openai_native_auto");
    assert_eq!(pre_tokens, None);
    assert_eq!(openai_encrypted_content.as_deref(), Some("enc_abc"));

    // Same wire event as Agent::run_turn_streaming_mpsc after native state is
    // applied. Its input usage must not be presented as pre-compaction usage.
    let wire = crate::protocol::ServerEvent::Compaction {
        trigger: "openai_native".into(),
        pre_tokens,
        post_tokens: None,
        tokens_saved: None,
        duration_ms: None,
        messages_dropped: None,
        messages_compacted: Some(24),
        summary_chars: None,
        active_messages: None,
    };
    let wire = serde_json::from_str(&serde_json::to_string(&wire).unwrap()).unwrap();
    app.handle_server_event(wire, &mut remote);

    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 30)).expect("test terminal");
    terminal
        .draw(|frame| crate::tui::ui::draw(frame, &app))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let screen = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    println!("NATIVE AUTO COMPACTION TERMINAL\n{screen}");
    assert!(
        screen.contains("Compacted 24 messages (automatic)"),
        "{screen}"
    );
    assert!(!screen.contains("unknown"), "{screen}");
    assert!(!screen.contains("~0 tokens"), "{screen}");
    let notice = &app.display_messages().last().unwrap().content;
    assert_eq!(notice, "📦 Compacted 24 messages (automatic)");
}
