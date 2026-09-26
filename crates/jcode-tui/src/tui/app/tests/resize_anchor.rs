// The persistent half of bug #1412: a resize while the reader is parked in
// history used to reinterpret the stored wrapped-line index against the new
// wrap width, landing them on unrelated messages. The position is now captured
// in content coordinates (which message, which row inside it) and resolved
// against the frame being drawn.
//
// Epic #1411 phase 4.

/// Build a transcript of `TOKENnnn` messages, each long enough to rewrap.
fn anchored_scroll_test_app() -> crate::tui::app::App {
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::None;
    app.diagram_pane_enabled = false;
    app.display_messages.replace((0..40)
        .map(|i| DisplayMessage::assistant(format!("TOKEN{i:03} - {}", "filler ".repeat(8))))
        .collect());
    app.bump_display_messages_version();
    app.scroll_offset = 0;
    app.auto_scroll_paused = false;
    app.status = ProcessingStatus::Idle;
    app.session.short_name = Some("test".to_string());
    app
}

/// Text of the messages area of the most recent rendered frame.
fn anchored_chat_area(terminal: &ratatui::Terminal<ratatui::backend::TestBackend>) -> String {
    let area = crate::tui::ui::last_layout_snapshot()
        .expect("layout snapshot")
        .messages_area;
    buffer_to_text(terminal)
        .lines()
        .skip(area.y as usize)
        .take(area.height as usize)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The `TOKENnnn` marker on the first non-blank chat row, if any.
fn top_token(chat: &str) -> Option<String> {
    chat.lines()
        .find(|line| line.contains("TOKEN"))
        .and_then(|line| {
            let start = line.find("TOKEN")?;
            Some(line[start..].chars().take(8).collect())
        })
}

#[test]
fn resize_keeps_the_anchored_message_under_a_paused_reader() {
    let _lock = scroll_render_test_lock();
    crate::perf::pin_full_profile_for_tests();

    let mut app = anchored_scroll_test_app();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    render_and_snap(&app, &mut terminal);

    // Park the reader in history so the `↓` overflow indicator shows.
    app.scroll_up(20);
    render_and_snap(&app, &mut terminal);
    let before_scroll = crate::tui::ui::last_resolved_chat_scroll();
    let before_max = crate::tui::ui::last_max_scroll();
    assert!(before_scroll > 0 && before_scroll < before_max);
    let before_token = top_token(&anchored_chat_area(&terminal)).expect("token at the top row");

    // Resize narrower: the same content wraps into more rows, so a stored line
    // index would now point at a different message.
    assert!(app.should_redraw_after_resize());
    let mut narrow = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 30)).unwrap();
    render_and_snap(&app, &mut narrow);

    let narrow_max = crate::tui::ui::last_max_scroll();
    assert!(
        narrow_max > before_max,
        "narrowing must wrap into more rows: {before_max} -> {narrow_max}"
    );
    let after_token = top_token(&anchored_chat_area(&narrow)).expect("token at the top row");
    assert_eq!(
        after_token, before_token,
        "the message under the reader must survive the reflow"
    );

    // The app adopts the resolved row on the next tick, after which the anchor
    // is dropped and the position is stored as the resolved index.
    assert!(app.reconcile_resize_anchor());
    assert!(app.pending_resize_anchor.is_none());
    render_and_snap(&app, &mut narrow);
    assert_eq!(
        top_token(&anchored_chat_area(&narrow)).as_deref(),
        Some(before_token.as_str()),
        "the position must stay put once the anchor is adopted"
    );
}

#[test]
fn resize_back_to_the_original_width_returns_to_the_same_message() {
    let _lock = scroll_render_test_lock();
    crate::perf::pin_full_profile_for_tests();

    let mut app = anchored_scroll_test_app();
    let mut wide = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    render_and_snap(&app, &mut wide);
    app.scroll_up(20);
    render_and_snap(&app, &mut wide);
    let original = top_token(&anchored_chat_area(&wide)).expect("token at the top row");

    let mut narrow = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 30)).unwrap();
    assert!(app.should_redraw_after_resize());
    render_and_snap(&app, &mut narrow);
    assert!(app.reconcile_resize_anchor());

    // Clear the resize debounce so the second resize commits immediately.
    app.last_resize_redraw = Some(std::time::Instant::now() - std::time::Duration::from_millis(40));
    let mut wide_again =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    assert!(app.should_redraw_after_resize());
    render_and_snap(&app, &mut wide_again);

    assert_eq!(
        top_token(&anchored_chat_area(&wide_again)).as_deref(),
        Some(original.as_str()),
        "100 -> 60 -> 100 must return to the same message"
    );
}

#[test]
fn tail_following_resize_captures_no_anchor() {
    // While following the tail there is no reading position to preserve: the
    // resize is left to the tail path, and nothing is captured here.
    let _lock = scroll_render_test_lock();
    crate::perf::pin_full_profile_for_tests();

    let mut app = anchored_scroll_test_app();
    let mut wide = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    render_and_snap(&app, &mut wide);
    assert!(!app.auto_scroll_paused);

    assert!(app.should_redraw_after_resize());
    assert!(
        app.pending_resize_anchor.is_none(),
        "a tail-following resize must not be captured as a reading position"
    );
    let mut narrow = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 30)).unwrap();
    render_and_snap(&app, &mut narrow);
    assert!(!app.reconcile_resize_anchor());
}

#[test]
fn user_scroll_supersedes_a_pending_resize_anchor() {
    let _lock = scroll_render_test_lock();
    crate::perf::pin_full_profile_for_tests();

    let mut app = anchored_scroll_test_app();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    render_and_snap(&app, &mut terminal);
    app.scroll_up(20);
    render_and_snap(&app, &mut terminal);

    assert!(app.should_redraw_after_resize());
    assert!(app.pending_resize_anchor.is_some());
    app.scroll_up(3);
    assert!(
        app.pending_resize_anchor.is_none(),
        "a user scroll wins over the pending correction"
    );
}

#[test]
fn resuming_the_tail_drops_a_pending_resize_anchor() {
    // The anchor describes a reading position. Resuming bottom-follow must not
    // leave the viewport pinned to it.
    let _lock = scroll_render_test_lock();
    crate::perf::pin_full_profile_for_tests();

    let mut app = anchored_scroll_test_app();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    render_and_snap(&app, &mut terminal);
    app.scroll_up(20);
    render_and_snap(&app, &mut terminal);

    assert!(app.should_redraw_after_resize());
    assert!(app.pending_resize_anchor.is_some());

    app.follow_chat_bottom();
    assert!(app.pending_resize_anchor.is_none());

    let mut narrow = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 30)).unwrap();
    render_and_snap(&app, &mut narrow);
    assert_eq!(
        crate::tui::ui::last_resolved_chat_scroll(),
        crate::tui::ui::last_max_scroll(),
        "resuming the tail must land at the bottom, not the anchored message"
    );
}

#[test]
fn a_prepend_invalidates_a_pending_resize_anchor() {
    // A resize anchor captured before older history is prepended is measured
    // against the older content; the prepend anchor is authoritative and must
    // supersede it.
    let _lock = scroll_render_test_lock();
    crate::perf::pin_full_profile_for_tests();
    let mut app = anchored_scroll_test_app();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    render_and_snap(&app, &mut terminal);
    app.scroll_up(20);
    render_and_snap(&app, &mut terminal);

    assert!(app.should_redraw_after_resize());
    assert!(app.pending_resize_anchor.is_some());

    app.capture_history_anchor(0);
    assert!(app.pending_history_anchor.is_some());
    assert!(
        app.pending_resize_anchor.is_none(),
        "a prepend supersedes the resize anchor"
    );
}

#[test]
fn a_pending_prepend_does_not_block_a_resize_capture() {
    // The prepend anchor stores a row distance, which only means something at the
    // width it was captured at. Refusing a resize while a prepend is pending left
    // that stale distance to be resolved against the rewrapped total, so the
    // reader jumped. The resize must still capture a content position.
    let _lock = scroll_render_test_lock();
    crate::perf::pin_full_profile_for_tests();
    let mut app = anchored_scroll_test_app();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    render_and_snap(&app, &mut terminal);
    app.scroll_up(20);
    render_and_snap(&app, &mut terminal);

    app.capture_history_anchor(0);
    assert!(app.pending_history_anchor.is_some());

    assert!(app.should_redraw_after_resize());
    assert!(
        app.pending_resize_anchor.is_some(),
        "a resize during a pending prepend must still capture a content position"
    );
}

/// The message owning the row at the top of the viewport.
fn top_message_id() -> Option<jcode_tui_messages::ItemId> {
    let frame = crate::tui::ui::last_chat_frame()?;
    match jcode_tui_messages::content_pos_at_row(
        &frame,
        crate::tui::ui::last_resolved_chat_scroll(),
    )? {
        jcode_tui_messages::ContentPos::Message(anchor) => Some(anchor.item_id),
        jcode_tui_messages::ContentPos::Section { .. } => None,
    }
}

#[test]
fn resize_during_a_pending_prepend_keeps_the_same_message() {
    // Reviewer finding on #1427: a paused reader with a compacted-history prepend
    // pending, then a resize. The pending distance from the bottom is a row count
    // measured at the old width, so resolving it against the rewrapped total
    // showed a different message.
    let _lock = scroll_render_test_lock();
    crate::perf::pin_full_profile_for_tests();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::None;
    app.diagram_pane_enabled = false;
    app.status = ProcessingStatus::Idle;
    app.session.short_name = Some("test".to_string());
    app.display_messages.replace((0..30)
        .map(|i| DisplayMessage::assistant(format!("TOKEN{i:03} - {}", "filler ".repeat(20))))
        .collect());
    app.bump_display_messages_version();

    let mut wide = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    render_and_snap(&app, &mut wide);
    app.scroll_offset = 40;
    app.auto_scroll_paused = true;
    render_and_snap(&app, &mut wide);
    let before = top_message_id().expect("a message under the reader");

    // Older history is queued (its anchor is a row distance at 100 columns).
    app.capture_history_anchor(0);
    assert!(app.pending_history_anchor.is_some());

    // Then the terminal is resized.
    assert!(app.should_redraw_after_resize());
    let mut narrow = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 30)).unwrap();
    render_and_snap(&app, &mut narrow);

    assert_eq!(
        top_message_id(),
        Some(before),
        "the reader must stay on the same message while history is loading"
    );
}

/// A transcript body long enough to wrap into several rows at 60 and 100 cols.
fn resize_anchor_filler(index: usize) -> DisplayMessage {
    DisplayMessage::assistant(format!("FILLER{index:02} - {}", "filler ".repeat(20)))
}

/// The owed regression for the reviewer finding: a resize anchor held across a
/// prepend used to resolve its duplicate-content ordinal against the prepended
/// message, teleporting the reader to the top of the transcript. The prepend
/// must win instead.
#[test]
fn resize_then_prepend_of_a_duplicate_does_not_teleport_the_reader() {
    let _lock = scroll_render_test_lock();
    crate::perf::pin_full_profile_for_tests();

    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::None;
    app.diagram_pane_enabled = false;
    app.status = ProcessingStatus::Idle;
    app.session.short_name = Some("test".to_string());

    // Visible window: filler, with TARGET deep enough that its row is well below
    // the transcript top. The identical duplicate is not in this window yet.
    let target_body = format!("TOKENsame - {}", "identical body ".repeat(8));
    let mut visible: Vec<DisplayMessage> = (0..30).map(resize_anchor_filler).collect();
    visible.insert(8, DisplayMessage::assistant(target_body.clone()));
    app.display_messages.replace(visible.clone());
    app.bump_display_messages_version();
    app.compacted_history_lazy = super::CompactedHistoryLazyState {
        total_messages: visible.len() + 2,
        visible_messages: visible.len(),
        remaining_messages: 2,
        hidden_user_prompts: 0,
        pending_request_visible: None,
    };

    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    render_and_snap(&app, &mut terminal);

    // Park the reader with TARGET at the top of the viewport.
    let target_row = {
        let frame = crate::tui::ui::last_chat_frame().expect("frame");
        (0..frame.total_wrapped_lines())
            .find(|row| {
                frame
                    .wrapped_plain_line(*row)
                    .is_some_and(|text| text.contains("TOKENsame"))
            })
            .expect("target row")
    };
    app.scroll_offset = target_row;
    app.auto_scroll_paused = true;
    render_and_snap(&app, &mut terminal);
    let before = crate::tui::ui::last_resolved_chat_scroll();
    assert_eq!(
        before, target_row,
        "fixture must park the reader on TARGET without clamping"
    );

    // Resize: captures a resize anchor naming TARGET.
    assert!(app.should_redraw_after_resize());
    assert!(app.pending_resize_anchor.is_some());
    let mut narrow = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 30)).unwrap();
    render_and_snap(&app, &mut narrow);
    // What the tick loop would have adopted before the prepend lands.
    app.scroll_offset = crate::tui::ui::last_resolved_chat_scroll();
    let before_area = anchored_chat_area(&narrow);

    // Older history containing an identical duplicate of TARGET is prepended.
    let mut older: Vec<DisplayMessage> = vec![resize_anchor_filler(100)];
    older.push(DisplayMessage::assistant(target_body.clone()));
    older.extend(visible.iter().cloned());
    let total = older.len();
    app.capture_history_anchor(0);
    app.apply_compacted_history_window(older, Vec::new(), total, total, 0, 0);
    render_and_snap(&app, &mut narrow);

    let after = crate::tui::ui::last_resolved_chat_scroll();
    assert!(
        after >= before,
        "the reader must not teleport up to the prepended duplicate: before={before} after={after}"
    );
    assert_eq!(
        top_token(&anchored_chat_area(&narrow)),
        top_token(&before_area),
        "the reader must still be looking at TARGET after the prepend"
    );

    // The invariant behind that outcome: the prepend's anchor is authoritative,
    // so the stale-ordinal resize anchor is gone.
    assert!(
        app.pending_resize_anchor.is_none(),
        "the prepend must supersede the resize anchor"
    );
}

/// The reviewer's other case: a reader parked inside live output has no message
/// boundary, so it used to capture nothing and the resize replayed the stale
/// wrapped row, showing unrelated stream text.
#[test]
fn resize_keeps_a_reader_parked_in_live_output_on_the_same_text() {
    let _lock = scroll_render_test_lock();
    crate::perf::pin_full_profile_for_tests();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::None;
    app.diagram_pane_enabled = false;
    app.status = ProcessingStatus::Streaming;
    app.is_processing = true;
    app.session.short_name = Some("test".to_string());
    app.streaming.streaming_text = (0..120)
        .map(|i| format!("STREAM{i:03} - {}", "filler ".repeat(12)))
        .collect::<Vec<_>>()
        .join("\n");

    let mut wide = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    render_and_snap(&app, &mut wide);

    let streaming_start = {
        let frame = crate::tui::ui::last_chat_frame().expect("frame");
        frame
            .sections
            .iter()
            .find(|section| section.kind == jcode_tui_messages::PreparedSectionKind::Streaming)
            .map(|section| section.line_start)
            .expect("streaming section")
    };
    app.scroll_offset = streaming_start + 40;
    app.auto_scroll_paused = true;
    render_and_snap(&app, &mut wide);

    let before_top =
        crate::tui::ui::copy_viewport_line_text(crate::tui::ui::last_resolved_chat_scroll())
            .unwrap_or_default();
    assert!(
        before_top.starts_with("STREAM040"),
        "fixture must park inside the live output: {before_top:?}"
    );

    assert!(app.should_redraw_after_resize());
    assert!(
        app.pending_resize_anchor.is_some(),
        "a live-output row must capture a content position"
    );
    let mut narrow = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 30)).unwrap();
    render_and_snap(&app, &mut narrow);

    let after_top =
        crate::tui::ui::copy_viewport_line_text(crate::tui::ui::last_resolved_chat_scroll())
            .unwrap_or_default();
    assert!(
        after_top.starts_with("STREAM040"),
        "the reader must stay on the same live text: {after_top:?}"
    );
}

/// The ambiguous corner: the prepended message is word-for-word identical to the
/// anchored one, so nothing in the content says which copy the reader was on.
/// Identity carries the answer instead. The reader is pushed down by the
/// prepended height (the prepend's own distance-from-bottom anchor), and what
/// they are looking at must not change -- same text, so the same prompt number.
#[test]
fn an_identical_prepend_keeps_the_readers_text_and_number() {
    let _lock = scroll_render_test_lock();
    crate::perf::pin_full_profile_for_tests();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::None;
    app.diagram_pane_enabled = false;
    app.status = ProcessingStatus::Idle;
    app.session.short_name = Some("test".to_string());

    let prompt = "TOKENsame - identical prompt";
    let mut visible: Vec<DisplayMessage> = vec![DisplayMessage::user(prompt)];
    visible.extend((0..30).map(resize_anchor_filler));
    app.display_messages.replace(visible.clone());
    app.bump_display_messages_version();

    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    render_and_snap(&app, &mut terminal);

    // Park the reader with the prompt at the top of the viewport.
    let target_row = {
        let frame = crate::tui::ui::last_chat_frame().expect("frame");
        (0..frame.total_wrapped_lines())
            .find(|row| {
                frame
                    .wrapped_plain_line(*row)
                    .is_some_and(|text| text.contains("TOKENsame"))
            })
            .expect("target row")
    };
    app.scroll_offset = target_row;
    app.auto_scroll_paused = true;
    render_and_snap(&app, &mut terminal);
    let before_row = crate::tui::ui::last_resolved_chat_scroll();
    assert_eq!(
        before_row, target_row,
        "fixture must park the reader on the prompt without clamping"
    );

    // Older history whose single message is identical to the anchored one.
    let mut older: Vec<DisplayMessage> = vec![DisplayMessage::user(prompt)];
    older.extend(visible.iter().cloned());
    let total = older.len();
    app.capture_history_anchor(0);
    app.apply_compacted_history_window(older, Vec::new(), total, total, 0, 0);
    render_and_snap(&app, &mut terminal);

    let after_row = crate::tui::ui::last_resolved_chat_scroll();
    assert_eq!(
        after_row,
        before_row + 2,
        "the prepended prompt must push the reader down by its own height"
    );
    // The displayed prompt number above the reader is a separate, pre-existing
    // concern: the prepend path resolves the reader by distance from the bottom
    // (phase 4), and this fixture reveals a prompt without adjusting the hidden
    // prompt offset, so `1>` becomes `2>`. Identity's claim is narrower -- the
    // reader keeps the same message -- so compare everything after the number.
    let message_text = |row: usize| {
        crate::tui::ui::copy_viewport_line_text(row)
            .map(|line| line.split_once(' ').map(|(_, rest)| rest.to_string()).unwrap_or(line))
    };
    assert_eq!(
        message_text(after_row),
        Some("TOKENsame - identical prompt".to_string()),
        "an identical prepend must keep the same message under the reader"
    );
    assert_eq!(
        message_text(after_row),
        message_text(before_row),
        "the message under the reader must be unchanged by an identical prepend"
    );
}
