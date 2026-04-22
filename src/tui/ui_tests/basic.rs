use super::*;

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

#[test]
fn test_flicker_frame_history_detects_same_state_hash_change() {
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
fn notification_spans_include_recent_flicker_warning_and_log_hint() {
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

    clear_flicker_frame_history_for_tests();
}

#[test]
fn test_link_target_from_screen_detects_chat_url() {
    let _lock = viewport_snapshot_test_lock();
    record_test_chat_snapshot("Docs: https://example.com/docs).");

    assert_eq!(
        link_target_from_screen(10, 0),
        Some("https://example.com/docs".to_string())
    );
}

#[test]
fn test_link_target_from_screen_detects_side_pane_url() {
    let _lock = viewport_snapshot_test_lock();
    clear_copy_viewport_snapshot();
    record_side_pane_snapshot(
        &[Line::from("See https://example.com/side for details")],
        0,
        1,
        Rect::new(40, 0, 40, 5),
    );

    assert_eq!(
        link_target_from_screen(45, 0),
        Some("https://example.com/side".to_string())
    );
}

#[test]
fn test_link_target_from_screen_returns_none_without_url() {
    let _lock = viewport_snapshot_test_lock();
    record_test_chat_snapshot("No links here");
    assert_eq!(link_target_from_screen(3, 0), None);
}

#[test]
fn test_prompt_entry_animation_detects_newly_visible_prompt_line() {
    reset_prompt_viewport_state_for_test();

    // First frame initializes viewport history and should not animate.
    update_prompt_entry_animation(&[5, 20], 0, 10, 1000);
    assert!(active_prompt_entry_animation(1000).is_none());

    // Scrolling down brings line 20 into view and should trigger animation.
    update_prompt_entry_animation(&[5, 20], 15, 25, 1100);
    let anim = active_prompt_entry_animation(1100).expect("expected active prompt animation");
    assert_eq!(anim.line_idx, 20);
}

#[test]
fn test_prompt_entry_animation_expires_after_window() {
    reset_prompt_viewport_state_for_test();

    update_prompt_entry_animation(&[5, 20], 0, 10, 2000);
    update_prompt_entry_animation(&[5, 20], 15, 25, 2100);

    assert!(active_prompt_entry_animation(2100).is_some());
    assert!(
        active_prompt_entry_animation(2100 + PROMPT_ENTRY_ANIMATION_MS + 1).is_none(),
        "animation should expire after configured duration"
    );
}

#[test]
fn test_prompt_entry_bg_color_pulses_then_fades() {
    let base = user_bg();
    let early = prompt_entry_bg_color(base, 0.15);
    let peak = prompt_entry_bg_color(base, 0.45);
    let late = prompt_entry_bg_color(base, 0.95);

    assert_ne!(early, base);
    assert_ne!(peak, base);
    assert_ne!(late, peak);
}

#[test]
fn test_prompt_entry_shimmer_color_moves_across_positions() {
    let base = user_text();
    let left_early = prompt_entry_shimmer_color(base, 0.1, 0.1);
    let right_early = prompt_entry_shimmer_color(base, 0.9, 0.1);
    let left_late = prompt_entry_shimmer_color(base, 0.1, 0.8);
    let right_late = prompt_entry_shimmer_color(base, 0.9, 0.8);

    assert_ne!(left_early, right_early);
    assert_ne!(left_late, right_late);
    assert_ne!(left_early, left_late);
}

#[test]
fn test_active_file_diff_context_resolves_visible_edit() {
    let prepared = PreparedMessages {
        wrapped_lines: vec![Line::from("a"); 20],
        wrapped_plain_lines: Arc::new(vec!["a".to_string(); 20]),
        wrapped_copy_offsets: Arc::new(vec![0; 20]),
        raw_plain_lines: Arc::new(Vec::new()),
        wrapped_line_map: Arc::new(Vec::new()),
        wrapped_user_indices: Vec::new(),
        wrapped_user_prompt_starts: Vec::new(),
        wrapped_user_prompt_ends: Vec::new(),
        user_prompt_texts: Vec::new(),
        image_regions: Vec::new(),
        edit_tool_ranges: vec![
            EditToolRange {
                edit_index: 0,
                msg_index: 3,
                file_path: "src/one.rs".to_string(),
                start_line: 2,
                end_line: 5,
            },
            EditToolRange {
                edit_index: 1,
                msg_index: 7,
                file_path: "src/two.rs".to_string(),
                start_line: 10,
                end_line: 14,
            },
        ],
        copy_targets: Vec::new(),
    };

    let prepared = PreparedChatFrame::from_single(Arc::new(prepared));
    let active = active_file_diff_context(&prepared, 9, 4).expect("visible edit context");
    assert_eq!(active.edit_index, 2);
    assert_eq!(active.msg_index, 7);
    assert_eq!(active.file_path, "src/two.rs");
}

#[test]
fn test_body_cache_state_keeps_multiple_width_entries() {
    let key_a = BodyCacheKey {
        width: 40,
        diff_mode: crate::config::DiffDisplayMode::Off,
        messages_version: 1,
        diagram_mode: crate::config::DiagramDisplayMode::Pinned,
        centered: false,
    };
    let key_b = BodyCacheKey {
        width: 41,
        ..key_a.clone()
    };

    let prepared_a = Arc::new(PreparedMessages {
        wrapped_lines: vec![Line::from("a")],
        wrapped_plain_lines: Arc::new(vec!["a".to_string()]),
        wrapped_copy_offsets: Arc::new(vec![0]),
        raw_plain_lines: Arc::new(Vec::new()),
        wrapped_line_map: Arc::new(Vec::new()),
        wrapped_user_indices: Vec::new(),
        wrapped_user_prompt_starts: Vec::new(),
        wrapped_user_prompt_ends: Vec::new(),
        user_prompt_texts: Vec::new(),
        image_regions: Vec::new(),
        edit_tool_ranges: Vec::new(),
        copy_targets: Vec::new(),
    });
    let prepared_b = Arc::new(PreparedMessages {
        wrapped_lines: vec![Line::from("b")],
        wrapped_plain_lines: Arc::new(vec!["b".to_string()]),
        wrapped_copy_offsets: Arc::new(vec![0]),
        raw_plain_lines: Arc::new(Vec::new()),
        wrapped_line_map: Arc::new(Vec::new()),
        wrapped_user_indices: Vec::new(),
        wrapped_user_prompt_starts: Vec::new(),
        wrapped_user_prompt_ends: Vec::new(),
        user_prompt_texts: Vec::new(),
        image_regions: Vec::new(),
        edit_tool_ranges: Vec::new(),
        copy_targets: Vec::new(),
    });

    let mut cache = BodyCacheState::default();
    cache.insert(key_a.clone(), prepared_a.clone(), 3);
    cache.insert(key_b.clone(), prepared_b.clone(), 3);

    let hit_a = cache
        .get_exact(&key_a)
        .expect("expected width 40 cache hit");
    let hit_b = cache
        .get_exact(&key_b)
        .expect("expected width 41 cache hit");

    assert!(Arc::ptr_eq(&hit_a, &prepared_a));
    assert!(Arc::ptr_eq(&hit_b, &prepared_b));
    assert_eq!(cache.entries.len(), 2);
}

#[test]
fn test_body_cache_state_evicts_oldest_entries() {
    let mut cache = BodyCacheState::default();

    for idx in 0..(BODY_CACHE_MAX_ENTRIES + 2) {
        let key = BodyCacheKey {
            width: 40 + idx as u16,
            diff_mode: crate::config::DiffDisplayMode::Off,
            messages_version: 1,
            diagram_mode: crate::config::DiagramDisplayMode::Pinned,
            centered: false,
        };
        let prepared = Arc::new(PreparedMessages {
            wrapped_lines: vec![Line::from(format!("{idx}"))],
            wrapped_plain_lines: Arc::new(vec![format!("{idx}")]),
            wrapped_copy_offsets: Arc::new(vec![0]),
            raw_plain_lines: Arc::new(Vec::new()),
            wrapped_line_map: Arc::new(Vec::new()),
            wrapped_user_indices: Vec::new(),
            wrapped_user_prompt_starts: Vec::new(),
            wrapped_user_prompt_ends: Vec::new(),
            user_prompt_texts: Vec::new(),
            image_regions: Vec::new(),
            edit_tool_ranges: Vec::new(),
            copy_targets: Vec::new(),
        });
        cache.insert(key, prepared, idx);
    }

    assert_eq!(cache.entries.len(), BODY_CACHE_MAX_ENTRIES);
    assert!(
        cache.entries.iter().all(|entry| entry.key.width >= 42),
        "oldest widths should be evicted"
    );
}

#[test]
fn test_body_cache_state_accepts_large_single_entry_within_total_budget() {
    let key = BodyCacheKey {
        width: 120,
        diff_mode: crate::config::DiffDisplayMode::Off,
        messages_version: 99,
        diagram_mode: crate::config::DiagramDisplayMode::Pinned,
        centered: false,
    };
    let prepared = make_prepared_messages_with_content_bytes(3 * 1024 * 1024, "body-large-");

    assert!(estimate_prepared_messages_bytes(&prepared) > 4 * 1024 * 1024);
    assert!(estimate_prepared_messages_bytes(&prepared) < BODY_CACHE_MAX_BYTES);

    let mut cache = BodyCacheState::default();
    cache.insert(key.clone(), prepared.clone(), 60);

    let hit = cache
        .get_exact(&key)
        .expect("expected large body cache entry to be retained");
    assert!(Arc::ptr_eq(&hit, &prepared));
}

#[test]
fn test_body_cache_state_retains_oversized_hot_entry() {
    let key = BodyCacheKey {
        width: 140,
        diff_mode: crate::config::DiffDisplayMode::Off,
        messages_version: 120,
        diagram_mode: crate::config::DiagramDisplayMode::Pinned,
        centered: false,
    };
    let prepared = make_oversized_prepared_messages("body-oversized-");

    assert!(estimate_prepared_messages_bytes(&prepared) > BODY_CACHE_MAX_BYTES);

    let mut cache = BodyCacheState::default();
    cache.insert(key.clone(), prepared.clone(), 120);

    let hit = cache
        .get_exact(&key)
        .expect("expected oversized body cache entry to be retained as hot entry");
    assert!(Arc::ptr_eq(&hit, &prepared));
    assert!(cache.entries.is_empty());
    assert_eq!(cache.oversized_entries.len(), 1);
}

#[test]
fn test_body_cache_state_keeps_two_oversized_width_entries_hot() {
    let key_a = BodyCacheKey {
        width: 140,
        diff_mode: crate::config::DiffDisplayMode::Off,
        messages_version: 120,
        diagram_mode: crate::config::DiagramDisplayMode::Pinned,
        centered: false,
    };
    let key_b = BodyCacheKey {
        width: 139,
        ..key_a.clone()
    };
    let prepared_a = make_oversized_prepared_messages("body-oversized-a-");
    let prepared_b = make_oversized_prepared_messages("body-oversized-b-");

    let mut cache = BodyCacheState::default();
    cache.insert(key_a.clone(), prepared_a.clone(), 120);
    cache.insert(key_b.clone(), prepared_b.clone(), 120);

    let hit_a = cache
        .get_exact(&key_a)
        .expect("expected first oversized body width to remain hot");
    let hit_b = cache
        .get_exact(&key_b)
        .expect("expected second oversized body width to remain hot");
    assert!(Arc::ptr_eq(&hit_a, &prepared_a));
    assert!(Arc::ptr_eq(&hit_b, &prepared_b));
    assert_eq!(cache.oversized_entries.len(), 2);
}

#[test]
fn test_body_cache_state_uses_oversized_hot_entry_as_incremental_base() {
    let key = BodyCacheKey {
        width: 140,
        diff_mode: crate::config::DiffDisplayMode::Off,
        messages_version: 120,
        diagram_mode: crate::config::DiagramDisplayMode::Pinned,
        centered: false,
    };
    let prepared = make_oversized_prepared_messages("body-oversized-base-");

    assert!(estimate_prepared_messages_bytes(&prepared) > BODY_CACHE_MAX_BYTES);

    let mut cache = BodyCacheState::default();
    cache.insert(key.clone(), prepared.clone(), 120);

    let base = cache
        .best_incremental_base(
            &BodyCacheKey {
                messages_version: 121,
                ..key.clone()
            },
            121,
        )
        .expect("expected oversized hot entry to remain eligible as incremental base");
    assert!(Arc::ptr_eq(&base.0, &prepared));
    assert_eq!(base.1, 120);
}

#[test]
fn test_prepare_body_incremental_reuses_unique_prepared_arc() {
    let width = 80;
    let base_state = TestState {
        display_messages: vec![
            DisplayMessage::user("first prompt"),
            DisplayMessage::assistant("initial answer"),
        ],
        messages_version: 1,
        ..Default::default()
    };
    let grown_state = TestState {
        display_messages: vec![
            DisplayMessage::user("first prompt"),
            DisplayMessage::assistant("initial answer"),
            DisplayMessage::user("second prompt"),
            DisplayMessage::assistant("follow-up answer"),
        ],
        messages_version: 2,
        ..Default::default()
    };

    let prepared = Arc::new(super::prepare::prepare_body(&base_state, width, false));
    let base_ptr = Arc::as_ptr(&prepared) as usize;
    let incremented = super::prepare::prepare_body_incremental(&grown_state, width, prepared, 2);

    assert_eq!(Arc::as_ptr(&incremented) as usize, base_ptr);
    assert!(
        incremented.wrapped_lines.len() >= 4,
        "expected incremental prep to append new wrapped content"
    );
}

#[test]
fn test_full_prep_cache_state_keeps_multiple_width_entries() {
    let key_a = FullPrepCacheKey {
        width: 40,
        height: 20,
        diff_mode: crate::config::DiffDisplayMode::Off,
        messages_version: 1,
        diagram_mode: crate::config::DiagramDisplayMode::Pinned,
        centered: false,
        is_processing: false,
        streaming_text_len: 0,
        streaming_text_hash: 0,
        batch_progress_hash: 0,
    };
    let key_b = FullPrepCacheKey {
        width: 39,
        ..key_a.clone()
    };

    let prepared_a = make_prepared_chat_frame(Arc::new(PreparedMessages {
        wrapped_lines: vec![Line::from("a")],
        wrapped_plain_lines: Arc::new(vec!["a".to_string()]),
        wrapped_copy_offsets: Arc::new(vec![0]),
        raw_plain_lines: Arc::new(Vec::new()),
        wrapped_line_map: Arc::new(Vec::new()),
        wrapped_user_indices: Vec::new(),
        wrapped_user_prompt_starts: Vec::new(),
        wrapped_user_prompt_ends: Vec::new(),
        user_prompt_texts: Vec::new(),
        image_regions: Vec::new(),
        edit_tool_ranges: Vec::new(),
        copy_targets: Vec::new(),
    }));
    let prepared_b = make_prepared_chat_frame(Arc::new(PreparedMessages {
        wrapped_lines: vec![Line::from("b")],
        wrapped_plain_lines: Arc::new(vec!["b".to_string()]),
        wrapped_copy_offsets: Arc::new(vec![0]),
        raw_plain_lines: Arc::new(Vec::new()),
        wrapped_line_map: Arc::new(Vec::new()),
        wrapped_user_indices: Vec::new(),
        wrapped_user_prompt_starts: Vec::new(),
        wrapped_user_prompt_ends: Vec::new(),
        user_prompt_texts: Vec::new(),
        image_regions: Vec::new(),
        edit_tool_ranges: Vec::new(),
        copy_targets: Vec::new(),
    }));

    let mut cache = FullPrepCacheState::default();
    cache.insert(key_a.clone(), prepared_a.clone());
    cache.insert(key_b.clone(), prepared_b.clone());

    let hit_a = cache
        .get_exact(&key_a)
        .expect("expected width 40 full prep cache hit");
    let hit_b = cache
        .get_exact(&key_b)
        .expect("expected width 39 full prep cache hit");

    assert!(Arc::ptr_eq(&hit_a, &prepared_a));
    assert!(Arc::ptr_eq(&hit_b, &prepared_b));
    assert_eq!(cache.entries.len(), 2);
}

#[test]
fn test_full_prep_cache_state_evicts_oldest_entries() {
    let mut cache = FullPrepCacheState::default();

    for idx in 0..(FULL_PREP_CACHE_MAX_ENTRIES + 2) {
        let key = FullPrepCacheKey {
            width: 40 + idx as u16,
            height: 20,
            diff_mode: crate::config::DiffDisplayMode::Off,
            messages_version: 1,
            diagram_mode: crate::config::DiagramDisplayMode::Pinned,
            centered: false,
            is_processing: false,
            streaming_text_len: 0,
            streaming_text_hash: 0,
            batch_progress_hash: 0,
        };
        let prepared = make_prepared_chat_frame(Arc::new(PreparedMessages {
            wrapped_lines: vec![Line::from(format!("{idx}"))],
            wrapped_plain_lines: Arc::new(vec![format!("{idx}")]),
            wrapped_copy_offsets: Arc::new(vec![0]),
            raw_plain_lines: Arc::new(Vec::new()),
            wrapped_line_map: Arc::new(Vec::new()),
            wrapped_user_indices: Vec::new(),
            wrapped_user_prompt_starts: Vec::new(),
            wrapped_user_prompt_ends: Vec::new(),
            user_prompt_texts: Vec::new(),
            image_regions: Vec::new(),
            edit_tool_ranges: Vec::new(),
            copy_targets: Vec::new(),
        }));
        cache.insert(key, prepared);
    }

    assert_eq!(cache.entries.len(), FULL_PREP_CACHE_MAX_ENTRIES);
    assert!(
        cache.entries.iter().all(|entry| entry.key.width >= 42),
        "oldest widths should be evicted"
    );
}

#[test]
fn test_full_prep_cache_state_accepts_large_single_entry_within_total_budget() {
    let key = FullPrepCacheKey {
        width: 120,
        height: 40,
        diff_mode: crate::config::DiffDisplayMode::Off,
        messages_version: 99,
        diagram_mode: crate::config::DiagramDisplayMode::Pinned,
        centered: false,
        is_processing: false,
        streaming_text_len: 0,
        streaming_text_hash: 0,
        batch_progress_hash: 0,
    };
    let prepared = make_prepared_chat_frame_with_content_bytes(3 * 1024 * 1024, "full-large-");

    assert!(estimate_prepared_chat_frame_bytes(&prepared) < FULL_PREP_CACHE_MAX_BYTES);

    let mut cache = FullPrepCacheState::default();
    cache.insert(key.clone(), prepared.clone());

    let hit = cache
        .get_exact(&key)
        .expect("expected large full prep cache entry to be retained");
    assert!(Arc::ptr_eq(&hit, &prepared));
}

#[test]
fn test_full_prep_cache_state_retains_oversized_hot_entry() {
    let key = FullPrepCacheKey {
        width: 140,
        height: 42,
        diff_mode: crate::config::DiffDisplayMode::Off,
        messages_version: 120,
        diagram_mode: crate::config::DiagramDisplayMode::Pinned,
        centered: false,
        is_processing: true,
        streaming_text_len: 4096,
        streaming_text_hash: 12345,
        batch_progress_hash: 0,
    };
    let prepared = make_oversized_prepared_chat_frame("full-oversized-");

    assert!(estimate_prepared_chat_frame_bytes(&prepared) <= FULL_PREP_CACHE_MAX_BYTES);

    let mut cache = FullPrepCacheState::default();
    cache.insert(key.clone(), prepared.clone());

    let hit = cache
        .get_exact(&key)
        .expect("expected oversized full prep entry to be retained as hot entry");
    assert!(Arc::ptr_eq(&hit, &prepared));
    assert!(cache.entries.is_empty());
    assert_eq!(cache.oversized_entries.len(), 1);
}

#[test]
fn test_full_prep_cache_state_keeps_two_oversized_width_entries_hot() {
    let key_a = FullPrepCacheKey {
        width: 140,
        height: 42,
        diff_mode: crate::config::DiffDisplayMode::Off,
        messages_version: 120,
        diagram_mode: crate::config::DiagramDisplayMode::Pinned,
        centered: false,
        is_processing: true,
        streaming_text_len: 4096,
        streaming_text_hash: 12345,
        batch_progress_hash: 0,
    };
    let key_b = FullPrepCacheKey {
        width: 139,
        ..key_a.clone()
    };
    let prepared_a = make_oversized_prepared_chat_frame("full-oversized-a-");
    let prepared_b = make_oversized_prepared_chat_frame("full-oversized-b-");

    let mut cache = FullPrepCacheState::default();
    cache.insert(key_a.clone(), prepared_a.clone());
    cache.insert(key_b.clone(), prepared_b.clone());

    let hit_a = cache
        .get_exact(&key_a)
        .expect("expected first oversized full-prep width to remain hot");
    let hit_b = cache
        .get_exact(&key_b)
        .expect("expected second oversized full-prep width to remain hot");
    assert!(Arc::ptr_eq(&hit_a, &prepared_a));
    assert!(Arc::ptr_eq(&hit_b, &prepared_b));
    assert_eq!(cache.oversized_entries.len(), 2);
}

#[test]
fn test_file_diff_cache_reuses_entry_when_signature_matches() {
    let temp = tempfile::NamedTempFile::new().expect("temp file");
    std::fs::write(temp.path(), "fn main() {}\n").expect("write file");
    let path = temp.path().to_string_lossy().to_string();

    let state = file_diff_cache();
    {
        let mut cache = state.lock().expect("cache lock");
        cache.entries.clear();
        cache.order.clear();
        let key = FileDiffCacheKey {
            file_path: path.clone(),
            msg_index: 1,
        };
        let sig = file_content_signature(&path);
        cache.insert(
            key.clone(),
            FileDiffViewCacheEntry {
                file_sig: sig.clone(),
                rows: vec![file_diff_ui::FileDiffDisplayRow {
                    prefix: String::new(),
                    text: "cached".to_string(),
                    kind: file_diff_ui::FileDiffDisplayRowKind::Placeholder,
                }],
                rendered_rows: vec![Some(Line::from("cached"))],
                first_change_line: 0,
                additions: 1,
                deletions: 0,
                file_ext: None,
            },
        );

        let cached = cache.entries.get(&key).expect("cached entry");
        assert_eq!(cached.file_sig, sig);
    }
}

#[test]
fn test_calculate_input_lines_single_line() {
    assert_eq!(calculate_input_lines("hello", 80), 1);
    assert_eq!(calculate_input_lines("hello world", 80), 1);
}

#[test]
fn test_calculate_input_lines_wrapped() {
    // 10 chars with width 5 = 2 lines
    assert_eq!(calculate_input_lines("aaaaaaaaaa", 5), 2);
    // 15 chars with width 5 = 3 lines
    assert_eq!(calculate_input_lines("aaaaaaaaaaaaaaa", 5), 3);
}

#[test]
fn test_calculate_input_lines_with_newlines() {
    // Two lines separated by newline
    assert_eq!(calculate_input_lines("hello\nworld", 80), 2);
    // Three lines
    assert_eq!(calculate_input_lines("a\nb\nc", 80), 3);
    // Trailing newline
    assert_eq!(calculate_input_lines("hello\n", 80), 2);
}

#[test]
fn test_calculate_input_lines_newlines_and_wrapping() {
    // First line wraps (10 chars / 5 = 2), second line is short (1)
    assert_eq!(calculate_input_lines("aaaaaaaaaa\nb", 5), 3);
}

#[test]
fn test_calculate_input_lines_zero_width() {
    assert_eq!(calculate_input_lines("hello", 0), 1);
}

#[test]
fn test_wrap_input_text_empty() {
    let (lines, cursor_line, cursor_col) =
        input_ui::wrap_input_text("", 0, 80, "1", "> ", user_color(), 3);
    assert_eq!(lines.len(), 1);
    assert_eq!(cursor_line, 0);
    assert_eq!(cursor_col, 0);
}

#[test]
fn test_wrap_input_text_simple() {
    let (lines, cursor_line, cursor_col) =
        input_ui::wrap_input_text("hello", 5, 80, "1", "> ", user_color(), 3);
    assert_eq!(lines.len(), 1);
    assert_eq!(cursor_line, 0);
    assert_eq!(cursor_col, 5); // cursor at end
}

#[test]
fn test_wrap_input_text_cursor_middle() {
    let (lines, cursor_line, cursor_col) =
        input_ui::wrap_input_text("hello world", 6, 80, "1", "> ", user_color(), 3);
    assert_eq!(lines.len(), 1);
    assert_eq!(cursor_line, 0);
    assert_eq!(cursor_col, 6); // cursor at 'w'
}

#[test]
fn test_wrap_input_text_wrapping() {
    // 10 chars with width 5 = 2 lines
    let (lines, cursor_line, cursor_col) =
        input_ui::wrap_input_text("aaaaaaaaaa", 7, 5, "1", "> ", user_color(), 3);
    assert_eq!(lines.len(), 2);
    assert_eq!(cursor_line, 1); // second line
    assert_eq!(cursor_col, 2); // 7 - 5 = 2
}

#[test]
fn test_wrap_input_text_with_newlines() {
    let (lines, cursor_line, cursor_col) =
        input_ui::wrap_input_text("hello\nworld", 6, 80, "1", "> ", user_color(), 3);
    assert_eq!(lines.len(), 2);
    assert_eq!(cursor_line, 1); // second line (after newline)
    assert_eq!(cursor_col, 0); // at start of 'world'
}

#[test]
fn test_wrap_input_text_cursor_at_end_of_wrapped() {
    // 10 chars with width 5, cursor at position 10 (end)
    let (lines, cursor_line, cursor_col) =
        input_ui::wrap_input_text("aaaaaaaaaa", 10, 5, "1", "> ", user_color(), 3);
    assert_eq!(lines.len(), 2);
    assert_eq!(cursor_line, 1);
    assert_eq!(cursor_col, 5);
}

#[test]
fn test_wrap_input_text_many_lines() {
    // Create text that spans 15 lines when wrapped to width 10
    let text = "a".repeat(150);
    let (lines, cursor_line, cursor_col) =
        input_ui::wrap_input_text(&text, 145, 10, "1", "> ", user_color(), 3);
    assert_eq!(lines.len(), 15);
    assert_eq!(cursor_line, 14); // last line
    assert_eq!(cursor_col, 5); // 145 % 10 = 5
}

#[test]
fn test_wrap_input_text_multiple_newlines() {
    let (lines, cursor_line, cursor_col) =
        input_ui::wrap_input_text("a\nb\nc\nd", 6, 80, "1", "> ", user_color(), 3);
    assert_eq!(lines.len(), 4);
    assert_eq!(cursor_line, 3); // on 'd' line
    assert_eq!(cursor_col, 0);
}

#[test]
fn test_wrapped_input_line_count_respects_two_digit_prompt_width() {
    let mut app = TestState {
        input: "abcdefghijk".to_string(),
        cursor_pos: "abcdefghijk".len(),
        ..Default::default()
    };
    for _ in 0..9 {
        app.display_messages.push(DisplayMessage {
            role: "user".to_string(),
            content: "previous".to_string(),
            tool_calls: Vec::new(),
            duration_secs: None,
            title: None,
            tool_data: None,
        });
    }

    // Old layout math effectively used width 11 here (14 total - hardcoded prompt width 3),
    // which incorrectly fit this input on a single line. The real prompt is "10> ", width 4,
    // so the wrapped renderer only has 10 columns and must use 2 lines.
    assert_eq!(calculate_input_lines(app.input(), 11), 1);
    assert_eq!(input_ui::wrapped_input_line_count(&app, 14, 10), 2);
}

#[test]
fn test_compute_visible_margins_centered_respects_line_alignment() {
    let lines = vec![
        ratatui::text::Line::from("centered").centered(),
        ratatui::text::Line::from("left block").left_aligned(),
        ratatui::text::Line::from("right").right_aligned(),
    ];
    let area = Rect::new(0, 0, 20, 3);
    let margins = compute_visible_margins(&lines, &[], area, true);

    // centered: used=8 => total_margin=12 => 6/6 split
    assert_eq!(margins.left_widths[0], 6);
    assert_eq!(margins.right_widths[0], 6);

    // left-aligned: used=10 => left=0, right=10
    assert_eq!(margins.left_widths[1], 0);
    assert_eq!(margins.right_widths[1], 10);

    // right-aligned: used=5 => left=15, right=0
    assert_eq!(margins.left_widths[2], 15);
    assert_eq!(margins.right_widths[2], 0);
}

#[test]
fn test_estimate_pinned_diagram_pane_width_scales_to_height() {
    let diagram = info_widget::DiagramInfo {
        hash: 1,
        width: 800,
        height: 600,
        label: None,
    };
    let width = estimate_pinned_diagram_pane_width_with_font(&diagram, 20, 24, Some((8, 16)));
    assert_eq!(width, 50);
}

#[test]
fn test_estimate_pinned_diagram_pane_width_respects_minimum() {
    let diagram = info_widget::DiagramInfo {
        hash: 2,
        width: 120,
        height: 120,
        label: None,
    };
    let width = estimate_pinned_diagram_pane_width_with_font(&diagram, 10, 24, Some((8, 16)));
    assert_eq!(width, 24);
}
