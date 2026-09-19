#[test]
fn test_redraw_interval_uses_low_frequency_during_remote_startup_phase() {
    let idle = TestState {
        anim_elapsed: 10.0,
        display_messages: vec![DisplayMessage::system("seed".to_string())],
        time_since_activity: Some(crate::tui::REDRAW_DEEP_IDLE_AFTER + Duration::from_secs(1)),
        ..Default::default()
    };
    let startup = TestState {
        time_since_activity: idle.time_since_activity,
        remote_startup_phase_active: true,
        ..Default::default()
    };

    let idle_interval = crate::tui::redraw_interval(&idle);
    let startup_interval = crate::tui::redraw_interval(&startup);

    assert_eq!(idle_interval, crate::tui::REDRAW_DEEP_IDLE);
    assert_eq!(startup_interval, crate::tui::REDRAW_REMOTE_STARTUP);
}

#[test]
fn test_active_overscroll_keeps_redrawing_at_deep_idle() {
    // Regression: once a conversation has messages and is no longer processing,
    // `time_since_activity()` reports deep-idle forever. The overscroll dwell
    // line shows a live `(overscroll x.x)` countdown that must keep ticking, so
    // `periodic_redraw_required` must not short-circuit to `false` via the
    // deep-idle guard while the overscroll line is revealed.
    let deep_idle = crate::tui::REDRAW_DEEP_IDLE_AFTER + Duration::from_secs(1);

    let idle = TestState {
        display_messages: vec![DisplayMessage::system("seed".to_string())],
        time_since_activity: Some(deep_idle),
        ..Default::default()
    };
    assert!(
        !crate::tui::periodic_redraw_required(&idle),
        "a quiet deep-idle session should not require periodic redraws"
    );

    let overscrolling = TestState {
        display_messages: vec![DisplayMessage::system("seed".to_string())],
        time_since_activity: Some(deep_idle),
        chat_overscroll_active: true,
        ..Default::default()
    };
    assert!(
        crate::tui::periodic_redraw_required(&overscrolling),
        "an active overscroll countdown must keep driving redraws even at deep idle"
    );

    // The redraw cadence should also be the smooth animation interval, not the
    // coarse deep-idle one, so the countdown reads as continuous.
    assert_eq!(
        crate::tui::redraw_interval(&idle),
        crate::tui::REDRAW_DEEP_IDLE
    );
    assert_ne!(
        crate::tui::redraw_interval(&overscrolling),
        crate::tui::REDRAW_DEEP_IDLE,
        "overscroll should bump the redraw interval above the deep-idle cadence"
    );
}

#[test]
fn test_cold_cache_warning_keeps_redrawing_at_deep_idle() {
    // Regression: the notification line renders a `🧊 cache cold` warning once
    // the prompt cache TTL expires, but the cache only goes cold long after the
    // 30s deep-idle cutoff. Without a dedicated wakeup the idle loop short-
    // circuits to `false` and never repaints to reveal the warning before the
    // user submits their next prompt.
    let deep_idle = crate::tui::REDRAW_DEEP_IDLE_AFTER + Duration::from_secs(1);

    let idle = TestState {
        display_messages: vec![DisplayMessage::system("seed".to_string())],
        time_since_activity: Some(deep_idle),
        ..Default::default()
    };
    assert!(
        !crate::tui::periodic_redraw_required(&idle),
        "a quiet deep-idle session should not require periodic redraws"
    );

    let cold = TestState {
        display_messages: vec![DisplayMessage::system("seed".to_string())],
        time_since_activity: Some(deep_idle),
        cache_ttl_status: Some(crate::tui::CacheTtlInfo {
            is_estimate: false,
            remaining_secs: 0,
            ttl_secs: 300,
            is_cold: true,
            cold_for_secs: 90,
            cached_tokens: Some(4000),
        }),
        ..Default::default()
    };
    assert!(
        crate::tui::periodic_redraw_required(&cold),
        "a cold prompt cache must keep driving redraws so the warning appears at deep idle"
    );
    assert_ne!(
        crate::tui::redraw_interval(&cold),
        crate::tui::REDRAW_DEEP_IDLE,
        "a cold cache should bump the redraw interval above the deep-idle cadence"
    );

    // The same applies to the final-minute `⏳ cache Ns` countdown.
    let warm_countdown = TestState {
        display_messages: vec![DisplayMessage::system("seed".to_string())],
        time_since_activity: Some(deep_idle),
        cache_ttl_status: Some(crate::tui::CacheTtlInfo {
            is_estimate: false,
            remaining_secs: 30,
            ttl_secs: 300,
            is_cold: false,
            cold_for_secs: 0,
            cached_tokens: Some(4000),
        }),
        ..Default::default()
    };
    assert!(
        crate::tui::periodic_redraw_required(&warm_countdown),
        "the last-minute cache countdown must keep driving redraws at deep idle"
    );

    // For long TTLs the countdown window scales (10% of TTL, capped at 10min):
    // a 1h cache with 5 minutes left is about to expire and must warn.
    let warm_countdown_1h = TestState {
        display_messages: vec![DisplayMessage::system("seed".to_string())],
        time_since_activity: Some(deep_idle),
        cache_ttl_status: Some(crate::tui::CacheTtlInfo {
            is_estimate: false,
            remaining_secs: 300,
            ttl_secs: 3600,
            is_cold: false,
            cold_for_secs: 0,
            cached_tokens: Some(4000),
        }),
        ..Default::default()
    };
    assert!(
        crate::tui::periodic_redraw_required(&warm_countdown_1h),
        "a 1h cache within its scaled countdown window must keep driving redraws"
    );

    // A comfortably warm cache should not defeat the deep-idle short-circuit.
    let warm = TestState {
        display_messages: vec![DisplayMessage::system("seed".to_string())],
        time_since_activity: Some(deep_idle),
        cache_ttl_status: Some(crate::tui::CacheTtlInfo {
            is_estimate: false,
            remaining_secs: 200,
            ttl_secs: 300,
            is_cold: false,
            cold_for_secs: 0,
            cached_tokens: Some(4000),
        }),
        ..Default::default()
    };
    assert!(
        !crate::tui::periodic_redraw_required(&warm),
        "a warm cache far from expiry should stay deep-idle"
    );
}

#[test]
fn test_active_swarm_spinner_keeps_redrawing_at_deep_idle() {
    // Regression: the swarm strip/dock animates a 12.5 fps status spinner for
    // active agents off the wall clock. A coordinator that is just watching
    // its agents goes deep-idle itself, and without a dedicated wakeup the
    // idle loop stops repainting, freezing every agent's spinner mid-frame.
    fn swarm_member(status: &str) -> crate::protocol::SwarmMemberStatus {
        crate::protocol::SwarmMemberStatus {
            session_id: format!("session-{status}"),
            friendly_name: Some("worker".to_string()),
            status: status.to_string(),
            detail: None,
            task_label: None,
            role: None,
            is_headless: Some(true),
            live_attachments: None,
            status_age_secs: Some(3),
            output_tail: None,
            report_back_to_session_id: None,
            todo_progress: None,
            todo_items: Vec::new(),
            runtime: crate::protocol::SwarmMemberRuntime::default(),
        }
    }

    let deep_idle = crate::tui::REDRAW_DEEP_IDLE_AFTER + Duration::from_secs(1);

    let quiet = TestState {
        display_messages: vec![DisplayMessage::system("seed".to_string())],
        time_since_activity: Some(deep_idle),
        ..Default::default()
    };
    assert!(
        !crate::tui::periodic_redraw_required(&quiet),
        "a quiet deep-idle session should not require periodic redraws"
    );

    // Every active status must keep the spinner animating.
    for status in ["running", "streaming", "thinking"] {
        let animating = TestState {
            display_messages: vec![DisplayMessage::system("seed".to_string())],
            time_since_activity: Some(deep_idle),
            swarm_members: vec![swarm_member(status)],
            ..Default::default()
        };
        assert!(
            crate::tui::periodic_redraw_required(&animating),
            "an {status} swarm agent must keep driving redraws at deep idle"
        );
        assert_eq!(
            crate::tui::redraw_interval(&animating),
            crate::tui::REDRAW_SWARM_SPINNER,
            "an {status} swarm agent should repaint at the spinner cadence"
        );
    }

    // Terminal statuses render fixed glyphs: no animation frames needed, the
    // deep-idle short-circuit must stay in effect.
    for status in ["completed", "failed", "stopped", "ready", "blocked"] {
        let settled = TestState {
            display_messages: vec![DisplayMessage::system("seed".to_string())],
            time_since_activity: Some(deep_idle),
            swarm_members: vec![swarm_member(status)],
            ..Default::default()
        };
        assert!(
            !crate::tui::periodic_redraw_required(&settled),
            "a {status} swarm agent renders a static glyph and should stay deep-idle"
        );
        assert_eq!(
            crate::tui::redraw_interval(&settled),
            crate::tui::REDRAW_DEEP_IDLE,
            "a {status} swarm agent should not bump the redraw cadence"
        );
    }
}

fn record_test_chat_snapshot(text: &str) {
    clear_copy_viewport_snapshot();
    let width = line_display_width(text);
    record_copy_viewport_snapshot(
        Arc::new(vec![text.to_string()]),
        Arc::new(vec![0]),
        Arc::new(vec![text.to_string()]),
        Arc::new(vec![WrappedLineMap {
            raw_line: 0,
            start_col: 0,
            end_col: width,
        }]),
        0,
        1,
        Rect::new(0, 0, 80, 5),
        &[0],
    );
}

fn make_prepared_messages_with_content_bytes(bytes: usize, marker: &str) -> Arc<PreparedMessages> {
    let content = format!(
        "{}{}",
        marker,
        "x".repeat(bytes.saturating_sub(marker.len()))
    );
    Arc::new(PreparedMessages {
        wrapped_lines: vec![Line::from(content.clone())],
        wrapped_plain_lines: Arc::new(vec![content.clone()]),
        wrapped_copy_offsets: Arc::new(vec![0]),
        raw_plain_lines: Arc::new(vec![content]),
        wrapped_line_map: Arc::new(Vec::new()),
        wrapped_user_indices: Vec::new(),
        wrapped_user_prompt_starts: Vec::new(),
        wrapped_user_prompt_ends: Vec::new(),
        user_prompt_texts: Vec::new(),
        image_regions: Vec::new(),
        edit_tool_ranges: Vec::new(),
        copy_targets: Vec::new(),
        message_boundaries: Vec::new(),
        mermaid_pending_epoch: None,
    })
}

fn make_oversized_prepared_messages(marker: &str) -> Arc<PreparedMessages> {
    make_prepared_messages_with_content_bytes(12 * 1024 * 1024, marker)
}

fn make_prepared_chat_frame(prepared: Arc<PreparedMessages>) -> Arc<PreparedChatFrame> {
    Arc::new(PreparedChatFrame::from_single(prepared))
}

fn make_prepared_chat_frame_with_content_bytes(
    bytes: usize,
    marker: &str,
) -> Arc<PreparedChatFrame> {
    make_prepared_chat_frame(make_prepared_messages_with_content_bytes(bytes, marker))
}

fn make_oversized_prepared_chat_frame(marker: &str) -> Arc<PreparedChatFrame> {
    make_prepared_chat_frame(make_oversized_prepared_messages(marker))
}

#[test]
fn test_calculate_input_lines_empty() {
    assert_eq!(calculate_input_lines("", 80), 1);
}

#[test]
fn test_inline_ui_gap_height_only_when_inline_ui_visible() {
    let state = TestState::default();
    assert_eq!(inline_ui_gap_height(&state), 0);

    let inline_interactive_state = crate::tui::InlineInteractiveState {
        kind: crate::tui::PickerKind::Model,
        entries: vec![],
        filtered: vec![],
        selected: 0,
        column: 0,
        filter: String::new(),
        preview: false,
    };
    let state_with_picker = TestState {
        inline_interactive_state: Some(inline_interactive_state),
        ..Default::default()
    };
    assert_eq!(inline_ui_gap_height(&state_with_picker), 1);

    let state_with_inline_view = TestState {
        inline_view_state: Some(crate::tui::InlineViewState {
            title: "USAGE".to_string(),
            status: Some("refreshing".to_string()),
            lines: vec!["Refreshing usage".to_string()],
        }),
        ..Default::default()
    };
    assert_eq!(inline_ui_gap_height(&state_with_inline_view), 1);
}

#[test]
fn test_slow_frame_history_retains_recent_samples() {
    clear_slow_frame_history_for_tests();
    record_slow_frame_sample(SlowFrameSample {
        timestamp_ms: 1,
        threshold_ms: 40.0,
        session_id: Some("session_test".to_string()),
        session_name: Some("test".to_string()),
        status: "Idle".to_string(),
        diff_mode: "Off".to_string(),
        centered: false,
        is_processing: false,
        auto_scroll_paused: false,
        display_messages: 10,
        display_messages_version: 3,
        user_messages: 5,
        queued_messages: 0,
        streaming_text_len: 0,
        prepare_ms: 12.0,
        draw_ms: 9.0,
        total_ms: 41.0,
        messages_ms: Some(7.0),
        input_event: None,
        scroll_delta: None,
        model_picker_open: false,
        resources: Default::default(),
        perf: FramePerfStats {
            viewport_total_wrapped_lines: 200,
            body_misses: 1,
            ..Default::default()
        },
    });
    record_slow_frame_sample(SlowFrameSample {
        timestamp_ms: 2,
        threshold_ms: 40.0,
        session_id: Some("session_test".to_string()),
        session_name: Some("test".to_string()),
        status: "Streaming".to_string(),
        diff_mode: "Off".to_string(),
        centered: false,
        is_processing: true,
        auto_scroll_paused: true,
        display_messages: 11,
        display_messages_version: 4,
        user_messages: 5,
        queued_messages: 1,
        streaming_text_len: 120,
        prepare_ms: 20.0,
        draw_ms: 15.0,
        total_ms: 55.0,
        messages_ms: Some(14.0),
        input_event: None,
        scroll_delta: None,
        model_picker_open: false,
        resources: Default::default(),
        perf: FramePerfStats {
            viewport_total_wrapped_lines: 240,
            body_hits: 1,
            ..Default::default()
        },
    });

    let payload = debug_slow_frame_history(8);
    assert_eq!(payload["buffered_samples"], 2);
    assert_eq!(payload["returned_samples"], 2);
    assert_eq!(payload["summary"]["max_total_ms"], 55.0);
    assert_eq!(payload["samples"][1]["status"], "Streaming");
    assert_eq!(payload["samples"][0]["perf"]["body_misses"], 1);
}

fn buffer_to_text(terminal: &ratatui::Terminal<ratatui::backend::TestBackend>) -> String {
    let buf = terminal.backend().buffer();
    let width = buf.area.width as usize;
    let height = buf.area.height as usize;
    let mut lines = Vec::with_capacity(height);
    for y in 0..height {
        let mut line = String::with_capacity(width);
        for x in 0..width {
            line.push_str(buf[(x as u16, y as u16)].symbol());
        }
        lines.push(line.trim_end().to_string());
    }
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

#[test]
fn test_changelog_overlay_repeated_renders_are_stable() {
    let _lock = viewport_snapshot_test_lock();
    let state = TestState {
        changelog_scroll: Some(0),
        chat_native_scrollbar: true,
        ..Default::default()
    };
    let sizes = [
        (24_u16, 10_u16),
        (28, 12),
        (32, 14),
        (36, 16),
        (40, 18),
        (48, 20),
        (60, 20),
        (72, 24),
    ];

    for (width, height) in sizes {
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
        let mut frames = Vec::new();
        clear_flicker_frame_history_for_tests();
        for _ in 0..3 {
            terminal
                .draw(|frame| crate::tui::ui::draw(frame, &state))
                .expect("overlay draw should succeed");
            frames.push(buffer_to_text(&terminal));
        }
        assert!(
            frames.windows(2).all(|pair| pair[0] == pair[1]),
            "expected stable changelog overlay renders at {width}x{height}, got differing frames: {frames:#?}"
        );

        let payload = debug_flicker_frame_history(8);
        assert_eq!(
            payload["buffered_samples"], 3,
            "expected overlay frames to be recorded for flicker diagnostics at {width}x{height}"
        );
    }
}

#[test]
fn test_updates_header_repeated_renders_stay_stable_near_scrollbar_threshold() {
    let _lock = viewport_snapshot_test_lock();
    super::header::set_unseen_changelog_entries_override_for_tests(Some(vec![
        "Update one".to_string(),
        "Update two".to_string(),
        "Update three".to_string(),
        "Update four".to_string(),
        "Update five".to_string(),
    ]));

    let state = TestState {
        display_messages: vec![DisplayMessage::assistant("ok")],
        messages_version: 1,
        chat_native_scrollbar: true,
        ..Default::default()
    };

    let mut unstable = Vec::new();
    for width in 22_u16..=28 {
        for height in 10_u16..=18 {
            let backend = ratatui::backend::TestBackend::new(width, height);
            let mut terminal =
                ratatui::Terminal::new(backend).expect("failed to create test terminal");
            let mut frames = Vec::new();
            clear_flicker_frame_history_for_tests();
            for _ in 0..4 {
                terminal
                    .draw(|frame| crate::tui::ui::draw(frame, &state))
                    .expect("header draw should succeed");
                frames.push(buffer_to_text(&terminal));
            }
            if frames.windows(2).any(|pair| pair[0] != pair[1]) {
                unstable.push((width, height, frames));
            }
        }
    }

    super::header::set_unseen_changelog_entries_override_for_tests(None);

    assert!(
        unstable.is_empty(),
        "expected updates header to render stably near scrollbar threshold, found unstable sizes: {unstable:#?}"
    );
}

fn test_flicker_sample(timestamp_ms: u64, visible_hash: u64) -> FlickerFrameSample {
    FlickerFrameSample {
        timestamp_ms,
        session_id: Some("session_test".to_string()),
        session_name: Some("test".to_string()),
        display_messages_version: 9,
        diff_mode: "Off".to_string(),
        centered: false,
        is_processing: false,
        auto_scroll_paused: false,
        scroll: 100,
        visible_end: 120,
        visible_lines: 20,
        total_wrapped_lines: 1000,
        prompt_preview_lines: 0,
        messages_area_width: 90,
        messages_area_height: 24,
        content_width: 89,
        chat_scrollbar_visible: true,
        visible_hash,
        visible_streaming_hash: 0,
        visible_batch_progress_hash: 0,
        total_ms: 5.0,
        prepare_ms: 2.0,
        draw_ms: 1.5,
    }
}

#[test]
fn test_flicker_frame_history_detects_same_state_hash_change() {
    let _lock = viewport_snapshot_test_lock();
    clear_flicker_frame_history_for_tests();
    record_flicker_frame_sample(FlickerFrameSample {
        timestamp_ms: 10,
        session_id: Some("session_test".to_string()),
        session_name: Some("test".to_string()),
        display_messages_version: 9,
        diff_mode: "Off".to_string(),
        centered: false,
        is_processing: false,
        auto_scroll_paused: false,
        scroll: 100,
        visible_end: 120,
        visible_lines: 20,
        total_wrapped_lines: 1000,
        prompt_preview_lines: 0,
        messages_area_width: 90,
        messages_area_height: 24,
        content_width: 89,
        chat_scrollbar_visible: true,
        visible_hash: 111,
        visible_streaming_hash: 0,
        visible_batch_progress_hash: 0,
        total_ms: 5.0,
        prepare_ms: 2.0,
        draw_ms: 1.5,
    });
    record_flicker_frame_sample(FlickerFrameSample {
        timestamp_ms: 11,
        session_id: Some("session_test".to_string()),
        session_name: Some("test".to_string()),
        display_messages_version: 9,
        diff_mode: "Off".to_string(),
        centered: false,
        is_processing: false,
        auto_scroll_paused: false,
        scroll: 100,
        visible_end: 120,
        visible_lines: 20,
        total_wrapped_lines: 1000,
        prompt_preview_lines: 0,
        messages_area_width: 90,
        messages_area_height: 24,
        content_width: 89,
        chat_scrollbar_visible: true,
        visible_hash: 222,
        visible_streaming_hash: 0,
        visible_batch_progress_hash: 0,
        total_ms: 5.5,
        prepare_ms: 2.2,
        draw_ms: 1.6,
    });

    let payload = debug_flicker_frame_history(8);
    assert_eq!(payload["buffered_samples"], 2);
    assert_eq!(payload["buffered_events"], 1);
    assert_eq!(payload["summary"]["visible_hash_change_events"], 1);
    assert_eq!(
        payload["events"][0]["kind"],
        "visible_hash_changed_same_state"
    );
}

#[test]
fn test_flicker_frame_history_detects_layout_oscillation() {
    let _lock = viewport_snapshot_test_lock();
    clear_flicker_frame_history_for_tests();
    for (timestamp_ms, content_width, visible_hash) in
        [(20, 89, 333_u64), (21, 88, 444), (22, 89, 333)]
    {
        record_flicker_frame_sample(FlickerFrameSample {
            timestamp_ms,
            session_id: Some("session_test".to_string()),
            session_name: Some("test".to_string()),
            display_messages_version: 10,
            diff_mode: "Off".to_string(),
            centered: false,
            is_processing: false,
            auto_scroll_paused: false,
            scroll: 250,
            visible_end: 270,
            visible_lines: 20,
            total_wrapped_lines: 1200,
            prompt_preview_lines: 0,
            messages_area_width: 90,
            messages_area_height: 24,
            content_width,
            chat_scrollbar_visible: true,
            visible_hash,
            visible_streaming_hash: 0,
            visible_batch_progress_hash: 0,
            total_ms: 6.0,
            prepare_ms: 2.0,
            draw_ms: 1.0,
        });
    }

    let payload = debug_flicker_frame_history(8);
    assert_eq!(payload["buffered_samples"], 3);
    assert_eq!(payload["summary"]["layout_oscillation_events"], 1);
    let events = payload["events"]
        .as_array()
        .expect("events should be an array");
    assert!(
        events
            .iter()
            .any(|event| event["kind"] == "layout_oscillation"),
        "expected at least one layout_oscillation event"
    );
}

#[test]
fn test_flicker_frame_history_detects_layout_feedback_oscillation() {
    let _lock = viewport_snapshot_test_lock();
    clear_flicker_frame_history_for_tests();
    for sample in [
        FlickerFrameSample {
            timestamp_ms: 30,
            session_id: Some("session_test".to_string()),
            session_name: Some("test".to_string()),
            display_messages_version: 11,
            diff_mode: "Off".to_string(),
            centered: false,
            is_processing: false,
            auto_scroll_paused: false,
            scroll: 0,
            visible_end: 10,
            visible_lines: 10,
            total_wrapped_lines: 10,
            prompt_preview_lines: 0,
            messages_area_width: 22,
            messages_area_height: 12,
            content_width: 21,
            chat_scrollbar_visible: false,
            visible_hash: 111,
            visible_streaming_hash: 0,
            visible_batch_progress_hash: 0,
            total_ms: 4.0,
            prepare_ms: 1.0,
            draw_ms: 1.0,
        },
        FlickerFrameSample {
            timestamp_ms: 31,
            session_id: Some("session_test".to_string()),
            session_name: Some("test".to_string()),
            display_messages_version: 11,
            diff_mode: "Off".to_string(),
            centered: false,
            is_processing: false,
            auto_scroll_paused: false,
            scroll: 7,
            visible_end: 17,
            visible_lines: 10,
            total_wrapped_lines: 17,
            prompt_preview_lines: 1,
            messages_area_width: 22,
            messages_area_height: 12,
            content_width: 20,
            chat_scrollbar_visible: true,
            visible_hash: 222,
            visible_streaming_hash: 0,
            visible_batch_progress_hash: 0,
            total_ms: 4.2,
            prepare_ms: 1.1,
            draw_ms: 1.0,
        },
        FlickerFrameSample {
            timestamp_ms: 32,
            session_id: Some("session_test".to_string()),
            session_name: Some("test".to_string()),
            display_messages_version: 11,
            diff_mode: "Off".to_string(),
            centered: false,
            is_processing: false,
            auto_scroll_paused: false,
            scroll: 0,
            visible_end: 10,
            visible_lines: 10,
            total_wrapped_lines: 10,
            prompt_preview_lines: 0,
            messages_area_width: 22,
            messages_area_height: 12,
            content_width: 21,
            chat_scrollbar_visible: false,
            visible_hash: 111,
            visible_streaming_hash: 0,
            visible_batch_progress_hash: 0,
            total_ms: 4.1,
            prepare_ms: 1.0,
            draw_ms: 1.0,
        },
    ] {
        record_flicker_frame_sample(sample);
    }

    let payload = debug_flicker_frame_history(8);
    assert_eq!(payload["buffered_samples"], 3);
    assert_eq!(payload["summary"]["layout_feedback_oscillation_events"], 1);
    let events = payload["events"]
        .as_array()
        .expect("events should be an array");
    assert!(
        events
            .iter()
            .any(|event| event["kind"] == "layout_feedback_oscillation"),
        "expected at least one layout_feedback_oscillation event"
    );
}

#[test]
fn notification_spans_include_recent_flicker_warning_and_log_hint() {
    let _lock = viewport_snapshot_test_lock();
    clear_flicker_frame_history_for_tests();
    record_flicker_frame_sample(FlickerFrameSample {
        timestamp_ms: 10,
        session_id: Some("session_test".to_string()),
        session_name: Some("test".to_string()),
        display_messages_version: 9,
        diff_mode: "Off".to_string(),
        centered: false,
        is_processing: false,
        auto_scroll_paused: false,
        scroll: 100,
        visible_end: 120,
        visible_lines: 20,
        total_wrapped_lines: 1000,
        prompt_preview_lines: 0,
        messages_area_width: 90,
        messages_area_height: 24,
        content_width: 89,
        chat_scrollbar_visible: true,
        visible_hash: 111,
        visible_streaming_hash: 0,
        visible_batch_progress_hash: 0,
        total_ms: 5.0,
        prepare_ms: 2.0,
        draw_ms: 1.5,
    });
    record_flicker_frame_sample(FlickerFrameSample {
        timestamp_ms: 11,
        session_id: Some("session_test".to_string()),
        session_name: Some("test".to_string()),
        display_messages_version: 9,
        diff_mode: "Off".to_string(),
        centered: false,
        is_processing: false,
        auto_scroll_paused: false,
        scroll: 100,
        visible_end: 120,
        visible_lines: 20,
        total_wrapped_lines: 1000,
        prompt_preview_lines: 0,
        messages_area_width: 90,
        messages_area_height: 24,
        content_width: 89,
        chat_scrollbar_visible: true,
        visible_hash: 222,
        visible_streaming_hash: 0,
        visible_batch_progress_hash: 0,
        total_ms: 5.5,
        prepare_ms: 2.2,
        draw_ms: 1.6,
    });

    let state = TestState::default();
    let spans = input_ui::build_notification_spans(&state);
    let rendered = spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(
        rendered.contains("flicker detected"),
        "expected flicker warning in notification line, got: {rendered}"
    );
    assert!(
        rendered.contains("client:flicker-frames 32"),
        "expected flicker debug hint in notification line, got: {rendered}"
    );
    assert!(
        rendered.contains("logs:"),
        "expected log hint in notification line, got: {rendered}"
    );
    assert!(
        rendered.contains("[Z]"),
        "expected copy badge in notification line, got: {rendered}"
    );

    let target = recent_flicker_copy_target_for_key('z').expect("expected flicker copy target");
    assert_eq!(target.key, 'z');
    assert_eq!(target.copied_notice, "Copied flicker hint");
    assert!(target.content.contains("client:flicker-frames 32"));

    clear_flicker_frame_history_for_tests();
}

#[test]
fn test_flicker_frame_history_ignores_visible_batch_progress_updates() {
    let _lock = viewport_snapshot_test_lock();
    clear_flicker_frame_history_for_tests();
    record_flicker_frame_sample(FlickerFrameSample {
        timestamp_ms: 40,
        session_id: Some("session_test".to_string()),
        session_name: Some("test".to_string()),
        display_messages_version: 12,
        diff_mode: "Off".to_string(),
        centered: false,
        is_processing: true,
        auto_scroll_paused: false,
        scroll: 100,
        visible_end: 120,
        visible_lines: 20,
        total_wrapped_lines: 1000,
        prompt_preview_lines: 0,
        messages_area_width: 90,
        messages_area_height: 24,
        content_width: 89,
        chat_scrollbar_visible: true,
        visible_hash: 111,
        visible_streaming_hash: 0,
        visible_batch_progress_hash: 1,
        total_ms: 5.0,
        prepare_ms: 2.0,
        draw_ms: 1.5,
    });
    record_flicker_frame_sample(FlickerFrameSample {
        timestamp_ms: 41,
        session_id: Some("session_test".to_string()),
        session_name: Some("test".to_string()),
        display_messages_version: 12,
        diff_mode: "Off".to_string(),
        centered: false,
        is_processing: true,
        auto_scroll_paused: false,
        scroll: 100,
        visible_end: 120,
        visible_lines: 20,
        total_wrapped_lines: 1000,
        prompt_preview_lines: 0,
        messages_area_width: 90,
        messages_area_height: 24,
        content_width: 89,
        chat_scrollbar_visible: true,
        visible_hash: 222,
        visible_streaming_hash: 0,
        visible_batch_progress_hash: 2,
        total_ms: 5.1,
        prepare_ms: 2.0,
        draw_ms: 1.5,
    });

    let payload = debug_flicker_frame_history(8);
    assert_eq!(payload["buffered_samples"], 2);
    assert_eq!(payload["buffered_events"], 0);
}

#[test]
fn test_flicker_frame_history_ignores_visible_streaming_updates() {
    let _lock = viewport_snapshot_test_lock();
    clear_flicker_frame_history_for_tests();
    record_flicker_frame_sample(FlickerFrameSample {
        timestamp_ms: 50,
        session_id: Some("session_test".to_string()),
        session_name: Some("test".to_string()),
        display_messages_version: 13,
        diff_mode: "Off".to_string(),
        centered: false,
        is_processing: true,
        auto_scroll_paused: false,
        scroll: 100,
        visible_end: 120,
        visible_lines: 20,
        total_wrapped_lines: 1000,
        prompt_preview_lines: 0,
        messages_area_width: 90,
        messages_area_height: 24,
        content_width: 89,
        chat_scrollbar_visible: true,
        visible_hash: 111,
        visible_streaming_hash: 1,
        visible_batch_progress_hash: 0,
        total_ms: 5.0,
        prepare_ms: 2.0,
        draw_ms: 1.5,
    });
    record_flicker_frame_sample(FlickerFrameSample {
        timestamp_ms: 51,
        session_id: Some("session_test".to_string()),
        session_name: Some("test".to_string()),
        display_messages_version: 13,
        diff_mode: "Off".to_string(),
        centered: false,
        is_processing: true,
        auto_scroll_paused: false,
        scroll: 100,
        visible_end: 120,
        visible_lines: 20,
        total_wrapped_lines: 1000,
        prompt_preview_lines: 0,
        messages_area_width: 90,
        messages_area_height: 24,
        content_width: 89,
        chat_scrollbar_visible: true,
        visible_hash: 222,
        visible_streaming_hash: 2,
        visible_batch_progress_hash: 0,
        total_ms: 5.1,
        prepare_ms: 2.0,
        draw_ms: 1.5,
    });

    let payload = debug_flicker_frame_history(8);
    assert_eq!(payload["buffered_samples"], 2);
    assert_eq!(payload["buffered_events"], 0);
}

#[test]
fn test_flicker_frame_history_ignores_live_batch_hash_noise() {
    let _lock = viewport_snapshot_test_lock();
    clear_flicker_frame_history_for_tests();
    let mut first = test_flicker_sample(60, 111);
    first.is_processing = true;
    first.visible_batch_progress_hash = 77;
    let mut second = test_flicker_sample(61, 222);
    second.is_processing = true;
    second.visible_batch_progress_hash = 77;

    record_flicker_frame_sample(first);
    record_flicker_frame_sample(second);

    let payload = debug_flicker_frame_history(8);
    assert_eq!(payload["buffered_samples"], 2);
    assert_eq!(payload["buffered_events"], 0);
}

#[test]
fn test_flicker_frame_history_ignores_manual_scroll_feedback() {
    let _lock = viewport_snapshot_test_lock();
    clear_flicker_frame_history_for_tests();
    for (timestamp_ms, scroll, visible_hash) in [(70, 100, 111), (71, 101, 222), (72, 100, 111)] {
        let mut sample = test_flicker_sample(timestamp_ms, visible_hash);
        sample.auto_scroll_paused = true;
        sample.scroll = scroll;
        sample.visible_end = scroll + sample.visible_lines;
        record_flicker_frame_sample(sample);
    }

    let payload = debug_flicker_frame_history(8);
    assert_eq!(payload["buffered_samples"], 3);
    assert_eq!(payload["buffered_events"], 0);
}

#[test]
fn test_cache_retention_estimates_never_create_proactive_expiry_ui() {
    let _lock = viewport_snapshot_test_lock();
    for ttl_secs in [300, 1800, 86400] {
        for remaining_secs in [ttl_secs, 180, 30, 0] {
            let info = crate::tui::CacheTtlInfo {
                is_estimate: true,
                remaining_secs,
                ttl_secs,
                is_cold: remaining_secs == 0,
                cold_for_secs: 90,
                cached_tokens: Some(4000),
            };
            assert!(!info.expiring_soon());
            assert!(!info.expiry_notification_active());
            let state = TestState {
                display_messages: vec![DisplayMessage::system("seed")],
                time_since_activity: Some(
                    crate::tui::REDRAW_DEEP_IDLE_AFTER + Duration::from_secs(1),
                ),
                cache_ttl_status: Some(info),
                ..Default::default()
            };
            let text = crate::tui::ui::input_ui::build_notification_spans(&state)
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>();
            assert!(
                !text.contains("cache"),
                "estimated TTL {ttl_secs}, remaining {remaining_secs}: {text}"
            );
            assert!(!crate::tui::TuiState::has_notification(&state));
            assert!(!crate::tui::periodic_redraw_required(&state));
            assert_eq!(
                crate::tui::redraw_interval(&state),
                crate::tui::REDRAW_DEEP_IDLE
            );
        }
    }
}

#[test]
fn test_cache_explicit_ttls_keep_expiry_notifications() {
    let _lock = viewport_snapshot_test_lock();
    for ttl_secs in [300, 3600] {
        for (remaining_secs, expected) in [(30, "cache 30s"), (0, "cache cold")] {
            let state = TestState {
                cache_ttl_status: Some(crate::tui::CacheTtlInfo {
                    is_estimate: false,
                    remaining_secs,
                    ttl_secs,
                    is_cold: remaining_secs == 0,
                    cold_for_secs: 10,
                    cached_tokens: Some(4000),
                }),
                ..Default::default()
            };
            assert!(crate::tui::TuiState::has_notification(&state));
            let text = crate::tui::ui::input_ui::build_notification_spans(&state)
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>();
            assert!(text.contains(expected), "{text}");
        }
    }
}
