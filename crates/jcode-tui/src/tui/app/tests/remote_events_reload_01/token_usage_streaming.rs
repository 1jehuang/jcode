#[test]
fn test_handle_server_event_token_usage_uses_per_call_deltas() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.streaming.streaming_tps_collect_output = true;

    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 100,
            output: 10,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 100,
            output: 30,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 100,
            output: 30,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );

    assert_eq!(app.streaming.streaming_output_tokens, 30);
    assert_eq!(app.streaming.streaming_total_output_tokens, 30);
    assert_eq!(app.token_accounting.total_input_tokens, 100);
    assert_eq!(app.token_accounting.total_output_tokens, 30);
}

#[test]
fn test_handle_server_event_tool_exec_pauses_tps_but_collects_final_tool_usage() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.streaming.streaming_tps_elapsed = Duration::from_secs(2);

    app.handle_server_event(
        crate::protocol::ServerEvent::ToolStart {
            id: "tool-1".to_string(),
            name: "read".to_string(),
        },
        &mut remote,
    );

    assert!(app.streaming.streaming_tps_collect_output);
    assert!(app.streaming.streaming_tps_start.is_some());

    app.streaming.streaming_tps_start = Some(Instant::now() - Duration::from_secs(3));

    app.handle_server_event(
        crate::protocol::ServerEvent::ToolExec {
            id: "tool-1".to_string(),
            name: "read".to_string(),
        },
        &mut remote,
    );

    assert!(app.streaming.streaming_tps_collect_output);
    assert!(app.streaming.streaming_tps_start.is_none());
    assert!(app.streaming.streaming_tps_elapsed >= Duration::from_secs(5));

    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 100,
            output: 25,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );

    assert_eq!(app.streaming.streaming_total_output_tokens, 25);
    assert_eq!(app.streaming.streaming_tps_observed_output_tokens, 25);

    app.handle_server_event(
        crate::protocol::ServerEvent::TextDelta {
            text: "hello".to_string(),
        },
        &mut remote,
    );

    assert!(app.streaming.streaming_tps_collect_output);
    assert!(app.streaming.streaming_tps_start.is_some());
}

#[test]
fn test_handle_server_event_kv_cache_request_resets_tps_output_watermark_for_next_api_call() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.streaming.streaming_tps_collect_output = true;

    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 100,
            output: 40,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );

    app.handle_server_event(
        crate::protocol::ServerEvent::KvCacheRequest {
            system_static_hash: 1,
            tools_hash: 2,
            messages_hash: 3,
            message_hashes: vec![11, 22],
            message_count: 2,
            tool_count: 1,
            system_static_chars: 10,
            tools_json_chars: 20,
            messages_json_chars: 30,
            ephemeral_hash: None,
            ephemeral_chars: 0,
            ephemeral_message_count: 0,
        },
        &mut remote,
    );

    assert!(!app.streaming.streaming_tps_collect_output);

    app.handle_server_event(
        crate::protocol::ServerEvent::ConnectionPhase {
            phase: "streaming".to_string(),
        },
        &mut remote,
    );

    assert!(app.streaming.streaming_tps_collect_output);

    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 120,
            output: 15,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );

    assert_eq!(app.streaming.streaming_total_output_tokens, 55);
    assert_eq!(app.streaming.streaming_tps_observed_output_tokens, 55);
}

#[test]
fn test_handle_server_event_message_end_marks_stream_as_finalizing_without_stall_mode() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.streaming.streaming_tps_collect_output = true;

    let needs_redraw = app.handle_server_event(
        crate::protocol::ServerEvent::MessageEnd { stop_reason: None },
        &mut remote,
    );

    assert!(needs_redraw);
    assert!(app.stream_message_ended);
    assert!(matches!(app.status, ProcessingStatus::Streaming));
    assert!(app.streaming.streaming_tps_collect_output);
}

#[test]
fn test_remote_done_waits_for_paced_backlog_and_one_live_frame() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.current_message_id = Some(42);
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    let response = "paced text";
    let ops = app.stream_buffer.push_text(response);
    app.apply_stream_ops(ops);
    assert!(!app.stream_buffer.is_empty());

    app.handle_server_event(
        crate::protocol::ServerEvent::MessageEnd { stop_reason: None },
        &mut remote,
    );
    app.handle_server_event(crate::protocol::ServerEvent::Done { id: 42 }, &mut remote);

    assert!(app.is_processing, "Done must not force-flush the backlog");
    assert_eq!(app.deferred_stream_done_id, Some(42));
    assert!(
        app.display_messages
            .iter()
            .all(|message| { message.role != "assistant" || !message.content.contains(response) })
    );

    // The first tick drains the short backlog, but deliberately leaves the live
    // streaming representation visible for one frame before committing it.
    std::thread::sleep(Duration::from_millis(60));
    rt.block_on(crate::tui::app::remote::handle_tick(&mut app, &mut remote));
    assert!(app.stream_buffer.is_empty());
    assert!(app.is_processing);
    assert_eq!(app.deferred_stream_done_id, Some(42));
    assert_eq!(app.streaming.streaming_text, response);

    // The following tick replays Done now that the preceding live frame was
    // eligible to render, committing exactly the text that was paced out.
    rt.block_on(crate::tui::app::remote::handle_tick(&mut app, &mut remote));
    assert!(!app.is_processing);
    assert_eq!(app.deferred_stream_done_id, None);
    assert!(
        app.display_messages
            .iter()
            .any(|message| { message.role == "assistant" && message.content == response })
    );
}

#[test]
fn test_handle_server_event_tps_connection_phase_streaming_starts_collection_only_for_streaming() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::ConnectionPhase {
            phase: "waiting for response".to_string(),
        },
        &mut remote,
    );

    assert!(!app.streaming.streaming_tps_collect_output);
    assert!(app.streaming.streaming_tps_start.is_none());

    app.handle_server_event(
        crate::protocol::ServerEvent::ConnectionPhase {
            phase: "streaming".to_string(),
        },
        &mut remote,
    );

    assert!(app.streaming.streaming_tps_collect_output);
    assert!(app.streaming.streaming_tps_start.is_some());
    assert!(matches!(app.status, ProcessingStatus::Streaming));
}

#[test]
fn test_connection_phase_elapsed_resets_per_attempt_not_per_turn() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    // Simulate a long-running turn: the whole-turn timer has been ticking for
    // well over the "suspiciously long" yellow threshold.
    app.is_processing = true;
    app.processing_started = Some(Instant::now() - Duration::from_secs(120));
    assert!(crate::tui::TuiState::elapsed(&app).unwrap() > Duration::from_secs(60));

    // A later round-trip enters the connecting phase. The per-attempt timer
    // must start fresh, so it reads as a brief connect (well under 10s) instead
    // of inheriting the 120s whole-turn elapsed and rendering yellow.
    app.handle_server_event(
        crate::protocol::ServerEvent::ConnectionPhase {
            phase: "connecting".to_string(),
        },
        &mut remote,
    );

    assert!(matches!(
        app.status,
        ProcessingStatus::Connecting(crate::message::ConnectionPhase::Connecting)
    ));
    let phase_elapsed = crate::tui::TuiState::connection_phase_elapsed(&app)
        .expect("connection phase elapsed should be tracked");
    assert!(
        phase_elapsed < Duration::from_secs(5),
        "per-attempt connection elapsed should be fresh, got {:?}",
        phase_elapsed
    );

    // Sub-phase transitions within the same attempt must not restart the timer.
    let started = app.connection_phase_started;
    app.handle_server_event(
        crate::protocol::ServerEvent::ConnectionPhase {
            phase: "waiting for response".to_string(),
        },
        &mut remote,
    );
    assert_eq!(
        app.connection_phase_started, started,
        "sub-phase transitions should keep the same per-attempt start"
    );

    // Streaming clears the per-attempt timer.
    app.handle_server_event(
        crate::protocol::ServerEvent::ConnectionPhase {
            phase: "streaming".to_string(),
        },
        &mut remote,
    );
    assert!(app.connection_phase_started.is_none());
}

#[test]
fn test_handle_server_event_tps_message_end_counts_late_usage_without_timer_running() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::ConnectionPhase {
            phase: "streaming".to_string(),
        },
        &mut remote,
    );
    app.streaming.streaming_tps_start = Some(Instant::now() - Duration::from_secs(4));

    app.handle_server_event(
        crate::protocol::ServerEvent::MessageEnd { stop_reason: None },
        &mut remote,
    );

    assert!(app.streaming.streaming_tps_collect_output);
    assert!(app.streaming.streaming_tps_start.is_none());
    assert!(app.streaming.streaming_tps_elapsed >= Duration::from_secs(4));

    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 100,
            output: 20,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );

    assert_eq!(app.streaming.streaming_total_output_tokens, 20);
    assert_eq!(app.streaming.streaming_tps_observed_output_tokens, 20);
    assert!(app.streaming.streaming_tps_observed_elapsed >= Duration::from_secs(4));
    assert!(app.streaming.streaming_tps_start.is_none());
}

#[test]
fn test_handle_server_event_tps_redundant_late_usage_after_message_end_does_not_double_count() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::ConnectionPhase {
            phase: "streaming".to_string(),
        },
        &mut remote,
    );
    app.streaming.streaming_tps_start = Some(Instant::now() - Duration::from_secs(5));

    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 100,
            output: 10,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::MessageEnd { stop_reason: None },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 100,
            output: 30,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 100,
            output: 30,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );

    assert_eq!(app.streaming.streaming_total_output_tokens, 30);
    assert_eq!(app.streaming.streaming_tps_observed_output_tokens, 30);
    assert_eq!(*remote.call_output_tokens_seen(), 30);
}
