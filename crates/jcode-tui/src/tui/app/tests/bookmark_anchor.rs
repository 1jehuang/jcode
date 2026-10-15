// Scroll bookmark stored in content coordinates (epic #1411 phase 5b).
//
// The bookmark used to hold a wrapped line index, so setting it, resizing, and
// returning landed the reader somewhere else.

fn bookmark_test_app() -> crate::tui::app::App {
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

fn bookmark_chat_area(terminal: &ratatui::Terminal<ratatui::backend::TestBackend>) -> String {
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

fn bookmark_top_token(chat: &str) -> Option<String> {
    chat.lines()
        .find(|line| line.contains("TOKEN"))
        .and_then(|line| {
            let start = line.find("TOKEN")?;
            Some(line[start..].chars().take(8).collect())
        })
}

#[test]
fn bookmark_returns_to_the_same_message() {
    let _lock = scroll_render_test_lock();
    let mut app = bookmark_test_app();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    render_and_snap(&app, &mut terminal);

    app.scroll_up(20);
    render_and_snap(&app, &mut terminal);
    let bookmarked = bookmark_top_token(&bookmark_chat_area(&terminal)).expect("token");

    app.toggle_scroll_bookmark();
    assert!(app.scroll_bookmark.is_some());
    assert!(
        !app.auto_scroll_paused,
        "setting a bookmark jumps to the bottom"
    );

    app.toggle_scroll_bookmark();
    assert!(app.scroll_bookmark.is_none());
    render_and_snap(&app, &mut terminal);
    assert_eq!(
        bookmark_top_token(&bookmark_chat_area(&terminal)).as_deref(),
        Some(bookmarked.as_str())
    );
}

#[test]
fn bookmark_survives_a_resize_between_set_and_return() {
    let _lock = scroll_render_test_lock();
    let mut app = bookmark_test_app();
    let mut wide = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    render_and_snap(&app, &mut wide);
    app.scroll_up(20);
    render_and_snap(&app, &mut wide);
    let bookmarked = bookmark_top_token(&bookmark_chat_area(&wide)).expect("token");
    app.toggle_scroll_bookmark();

    // Resize while the bookmark is set. The stored line index no longer means
    // what it meant at set time.
    let mut narrow = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 30)).unwrap();
    app.should_redraw_after_resize();
    render_and_snap(&app, &mut narrow);
    let _ = app.reconcile_resize_anchor();

    app.toggle_scroll_bookmark();
    render_and_snap(&app, &mut narrow);
    assert_eq!(
        bookmark_top_token(&bookmark_chat_area(&narrow)).as_deref(),
        Some(bookmarked.as_str()),
        "returning must land on the bookmarked message, not a stale index"
    );
}

#[test]
fn bookmark_on_a_removed_message_keeps_the_reader_put() {
    let _lock = scroll_render_test_lock();
    let mut app = bookmark_test_app();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    render_and_snap(&app, &mut terminal);
    app.scroll_up(20);
    render_and_snap(&app, &mut terminal);
    app.toggle_scroll_bookmark();
    assert!(app.scroll_bookmark.is_some());

    // The bookmarked message goes away (compaction/pruning leaves a shorter
    // transcript behind).
    app.display_messages = vec![DisplayMessage::assistant("short replacement")];
    app.bump_display_messages_version();
    render_and_snap(&app, &mut terminal);
    let before = app.scroll_offset;

    app.toggle_scroll_bookmark();
    assert!(app.scroll_bookmark.is_none());
    assert_eq!(
        app.scroll_offset, before,
        "an unresolvable bookmark must not move the viewport"
    );
}
