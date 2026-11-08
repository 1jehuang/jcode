fn recent_history_test_app() -> App {
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_session_id = Some("recent-test".into());
    app.replace_display_messages(vec![
        DisplayMessage::system("Earlier remote history - 872 older historical messages hidden. Showing 128 of 1000 messages. Scroll to the top to load more. Hidden user prompts: 99."),
        DisplayMessage::user("latest prompt"),
        DisplayMessage::assistant("latest answer"),
    ]);
    app
}

#[test]
fn recent_history_bootstrap_renders_latest_frame_without_older_transcript() {
    let _lock = scroll_render_test_lock();
    let mut app = recent_history_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::None;
    app.diagram_pane_enabled = false;
    app.status = ProcessingStatus::Idle;
    let messages = std::iter::once(app.display_messages[0].clone())
        .chain((0..128).map(|i| {
            DisplayMessage::assistant(format!("RECENT{i:03} {}", "bounded history ".repeat(128)))
        }))
        .map(|message| crate::protocol::HistoryMessage {
            role: message.role,
            content: message.content,
            tool_calls: None,
            tool_data: None,
            response_stats: None,
        })
        .collect();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    let start = std::time::Instant::now();
    assert!(app.handle_server_event(
        delta_history_event("recent-test", messages, None),
        &mut remote
    ));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    render_and_snap(&app, &mut terminal);
    let elapsed = start.elapsed();
    assert_eq!(app.display_messages.len(), 129);
    assert!(buffer_to_text(&terminal).contains("RECENT127"));
    assert_eq!(app.compacted_hidden_user_prompts(), 99);
    eprintln!(
        "recent_history initial event + 128-message frame: {:.2}ms",
        elapsed.as_secs_f64() * 1000.0
    );
}

#[test]
fn recent_history_marker_preserves_absolute_prompt_numbers_and_queues_page() {
    let mut app = recent_history_test_app();
    assert!(app.recent_history_window_active());
    assert_eq!(app.compacted_hidden_user_prompts(), 99);
    assert_eq!(
        crate::tui::ui::input_ui::next_input_prompt_number(&app),
        101
    );
    app.auto_scroll_paused = true;
    app.scroll_offset = 0;
    app.maybe_queue_compacted_history_load();
    assert_eq!(app.take_pending_compacted_history_load(), Some(192));
}

#[test]
fn recent_history_page_retries_instead_of_discarding_live_append() {
    let mut app = recent_history_test_app();
    app.recent_history_paging.request = Some((42, app.display_messages_version, 192));
    app.display_messages
        .push(DisplayMessage::assistant("live response after request"));
    app.bump_display_messages_version();
    assert!(!app.accept_recent_history_response(42));
    assert_eq!(
        app.display_messages.last().unwrap().content,
        "live response after request"
    );
    assert_eq!(
        app.compacted_history_lazy.pending_request_visible,
        Some(192)
    );
    assert_eq!(app.take_pending_compacted_history_load(), None);
    app.recent_history_paging.retry_at = Some(Instant::now());
    assert_eq!(app.take_pending_compacted_history_load(), Some(192));
}

#[test]
fn recent_history_page_rejects_wrong_request_and_rewind() {
    let mut app = recent_history_test_app();
    app.recent_history_paging.request = Some((42, app.display_messages_version, 192));
    assert!(!app.accept_recent_history_response(41));
    assert!(app.recent_history_paging.request.is_some());
    app.replace_display_messages(vec![DisplayMessage::user("rewound transcript")]);
    assert!(!app.accept_recent_history_response(42));
    assert_eq!(app.take_pending_compacted_history_load(), None);
    assert_eq!(app.display_messages.len(), 1);
}

#[test]
fn recent_history_full_window_restores_archived_compaction_loading() {
    let mut app = recent_history_test_app();
    app.recent_history_paging.request = Some((42, app.display_messages_version, 1000));
    assert!(app.accept_recent_history_response(42));
    app.apply_compacted_history_window(
        vec![
            DisplayMessage::system("Earlier conversation compacted - 240 historical messages hidden from the UI. Scroll to the top to load older history. Hidden user prompts: 31."),
            DisplayMessage::user("current context prompt"),
        ], vec![], 1000, 1000, 0, 31,
    );
    assert!(!app.recent_history_window_active());
    assert_eq!(app.compacted_history_lazy_state().total_messages, 240);
    assert_eq!(app.compacted_history_lazy_state().remaining_messages, 240);
    assert_eq!(app.compacted_hidden_user_prompts(), 31);
}

#[test]
fn recent_history_page_accepts_unchanged_generation() {
    let mut app = recent_history_test_app();
    app.recent_history_paging.request = Some((42, app.display_messages_version, 192));
    assert!(app.accept_recent_history_response(42));
    assert!(app.recent_history_paging.request.is_none());
    // Unsolicited page payloads cannot replace a bounded current-history view.
    assert!(!app.accept_recent_history_response(42));
}

#[test]
fn recent_history_page_rejects_snapshot_missing_already_visible_rows() {
    let mut app = recent_history_test_app();
    let mut stale = vec![DisplayMessage::user("latest prompt")];
    assert!(!app.recent_history_page_contains_visible_tail(&mut stale, 192));
    assert_eq!(
        app.compacted_history_lazy.pending_request_visible,
        Some(192)
    );
    assert_eq!(
        app.display_messages.last().unwrap().content,
        "latest answer"
    );
    let mut newer = vec![
        DisplayMessage::assistant("older page content"),
        DisplayMessage::user("latest prompt"),
        DisplayMessage::assistant("latest answer"),
        DisplayMessage::assistant("even newer persisted response"),
    ];
    assert!(app.recent_history_page_contains_visible_tail(&mut newer, 192));
}
