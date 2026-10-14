#[test]
fn native_citation_suffix_correction_flushes_pacing_and_preserves_message_boundaries() {
    use crate::protocol::ServerEvent;
    let mut app = create_test_app();
    app.is_remote = true;
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    for event in [
        ServerEvent::TextDelta {
            text: "Earlier commentary.".into(),
        },
        ServerEvent::TextDone,
        ServerEvent::ToolStart {
            id: "ws_citation".into(),
            name: "web_search".into(),
        },
        ServerEvent::ToolDone {
            id: "ws_citation".into(),
            name: "web_search".into(),
            output: "Search completed".into(),
            error: None,
        },
    ] {
        app.handle_server_event(event, &mut remote);
    }
    assert!(
        app.display_messages
            .iter()
            .any(|message| message.role == "assistant" && message.content == "Earlier commentary.")
    );
    let marker = "\u{e200}cite\u{e202}turn0search12\u{e201}";
    for label in ["B", "C"] {
        let old_text = format!("{label}. {marker}");
        let new_text = format!("{label}. [{label}](<https://source.example/{label}>)");
        // Leave deltas in the pacer. The correction must flush before rewriting.
        app.handle_server_event(
            ServerEvent::TextDelta {
                text: format!("{label}. "),
            },
            &mut remote,
        );
        app.handle_server_event(
            ServerEvent::TextDelta {
                text: marker.into(),
            },
            &mut remote,
        );
        app.handle_server_event(
            ServerEvent::TextSuffixReplace { old_text, new_text },
            &mut remote,
        );
        app.handle_server_event(ServerEvent::TextDone, &mut remote);
        assert!(app.stream_buffer.is_empty());
        assert!(!app.streaming.streaming_text.contains("turn0search12"));
    }
    assert_eq!(
        app.streaming.streaming_text,
        "B. [B](<https://source.example/B>)C. [C](<https://source.example/C>)"
    );
    app.handle_server_event(ServerEvent::MessageEnd { stop_reason: None }, &mut remote);
    app.handle_server_event(ServerEvent::Done { id: 0 }, &mut remote);
    let assistant: Vec<_> = app
        .display_messages
        .iter()
        .filter(|message| message.role == "assistant")
        .map(|message| message.content.as_str())
        .collect();
    assert_eq!(
        assistant,
        vec![
            "Earlier commentary.",
            "B. [B](<https://source.example/B>)C. [C](<https://source.example/C>)"
        ]
    );
}

#[test]
fn native_citation_suffix_correction_keeps_reasoning_prefix_and_ignores_stale_text() {
    use crate::protocol::ServerEvent;
    let mut app = create_test_app();
    app.is_remote = true;
    app.is_processing = true;
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();
    let marker = "\u{e200}cite\u{e202}turn0search12\u{e201}";
    let prefix = "Retained reasoning 🦀\nEarlier commentary. ";
    app.streaming.streaming_text = format!("{prefix}{marker}");
    app.handle_server_event(
        ServerEvent::TextSuffixReplace {
            old_text: marker.into(),
            new_text: "[Source](<https://source.example>)".into(),
        },
        &mut remote,
    );
    assert_eq!(
        app.streaming.streaming_text,
        format!("{prefix}[Source](<https://source.example>)")
    );
    let before = app.streaming.streaming_text.clone();
    app.handle_server_event(
        ServerEvent::TextSuffixReplace {
            old_text: marker.into(),
            new_text: "wrong".into(),
        },
        &mut remote,
    );
    assert_eq!(app.streaming.streaming_text, before);
    app.handle_server_event(
        ServerEvent::RetryRollback { attempt: 1, max: 2 },
        &mut remote,
    );
    assert!(app.streaming.streaming_text.is_empty());
}
