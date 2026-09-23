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
    app.display_messages = (0..40)
        .map(|i| DisplayMessage::assistant(format!("TOKEN{i:03} - {}", "filler ".repeat(8))))
        .collect();
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
    // A resize anchor captured before older history is prepended has a stale
    // occurrence ordinal: a prepended duplicate becomes occurrence zero, so the
    // resolver would name that older message instead of the reader's. The
    // prepend anchor is authoritative and must supersede it.
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
fn a_prepend_in_flight_blocks_a_resize_anchor() {
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
        app.pending_resize_anchor.is_none(),
        "a prepend in flight must block a resize anchor capture"
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
    app.display_messages = visible.clone();
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

    // The invariant behind that outcome: the prepend's anchor is authoritative,
    // so the stale-ordinal resize anchor is gone.
    assert!(
        app.pending_resize_anchor.is_none(),
        "the prepend must supersede the resize anchor"
    );
}
