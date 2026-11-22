/// Record a one-line viewport whose text contains a clickable URL at row 0.
fn record_ctrl_click_url_viewport() {
    crate::tui::ui::clear_copy_viewport_snapshot();
    crate::tui::ui::record_copy_viewport_snapshot(
        std::sync::Arc::new(vec!["Docs: https://example.com/docs".to_string()]),
        std::sync::Arc::new(vec![0]),
        std::sync::Arc::new(vec!["Docs: https://example.com/docs".to_string()]),
        std::sync::Arc::new(vec![crate::tui::ui::WrappedLineMap {
            raw_line: 0,
            start_col: 0,
            end_col: 30,
        }]),
        0,
        1,
        Rect::new(0, 0, 80, 5),
        &[0],
    );
}

fn click_link_cell(
    app: &mut App,
    kind: MouseEventKind,
    column: u16,
    row: u16,
    modifiers: KeyModifiers,
) -> bool {
    app.handle_mouse_event(MouseEvent {
        kind,
        column,
        row,
        modifiers,
    })
}

/// Terminals reserve plain clicks for text selection: a bare click over a URL
/// must only teach the chord, never launch a browser.
#[test]
fn test_ctrl_click_plain_click_sets_hint_without_opening() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    record_ctrl_click_url_viewport();

    click_link_cell(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        10,
        0,
        KeyModifiers::empty(),
    );
    assert_eq!(app.status_notice(), None, "press must not open or warn");

    let handled = click_link_cell(
        &mut app,
        MouseEventKind::Up(MouseButton::Left),
        10,
        0,
        KeyModifiers::empty(),
    );

    assert!(!handled, "a hinted plain click stays available to selection");
    assert_eq!(
        app.status_notice(),
        Some("Ctrl+Click to open links".to_string())
    );
}

/// Ctrl+Click opens the link. The target is a repository Markdown link so the
/// opener is deterministic (side panel) instead of spawning a browser.
#[test]
fn test_ctrl_click_on_link_opens_it() {
    let _render_lock = scroll_render_test_lock();
    let repository = tempfile::tempdir().unwrap();
    std::fs::create_dir(repository.path().join("docs")).unwrap();
    std::fs::write(
        repository.path().join("docs/guide.md"),
        "# Repository guide\n",
    )
    .unwrap();
    let mut app = create_test_app();
    app.session.working_dir = Some(repository.path().to_string_lossy().into_owned());
    crate::tui::ui::clear_copy_viewport_snapshot();
    crate::tui::ui::record_copy_viewport_snapshot(
        std::sync::Arc::new(vec!["Read the guide".to_string()]),
        std::sync::Arc::new(vec![0]),
        std::sync::Arc::new(vec!["Read the [guide](docs/guide.md#setup)".to_string()]),
        std::sync::Arc::new(vec![crate::tui::ui::WrappedLineMap {
            raw_line: 0,
            start_col: 0,
            end_col: 40,
        }]),
        0,
        1,
        Rect::new(0, 0, 80, 5),
        &[0],
    );

    click_link_cell(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        10,
        0,
        KeyModifiers::CONTROL,
    );
    let handled = click_link_cell(
        &mut app,
        MouseEventKind::Up(MouseButton::Left),
        10,
        0,
        KeyModifiers::CONTROL,
    );

    assert!(!handled, "opening a link requests an immediate redraw");
    let page = app
        .side_panel
        .focused_page()
        .expect("Ctrl+Click must open the linked Markdown file");
    assert_eq!(page.title, "guide.md");
    assert_eq!(page.content, "# Repository guide\n");
    assert_eq!(
        app.status_notice(),
        Some("Opened Markdown: guide.md".to_string())
    );
}

/// A release that ends a text-selection drag must never open a link, even when
/// Ctrl is held on the release cell.
#[test]
fn test_ctrl_click_drag_release_never_opens_link() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    record_ctrl_click_url_viewport();

    click_link_cell(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        2,
        0,
        KeyModifiers::empty(),
    );
    click_link_cell(
        &mut app,
        MouseEventKind::Drag(MouseButton::Left),
        10,
        0,
        KeyModifiers::empty(),
    );
    assert!(
        app.copy_selection_dragging,
        "drag across cells must arm a selection"
    );
    let handled = click_link_cell(
        &mut app,
        MouseEventKind::Up(MouseButton::Left),
        10,
        0,
        KeyModifiers::CONTROL,
    );

    assert!(!handled);
    assert!(!app.copy_selection_dragging, "release ends the drag");
    let notice = app.status_notice();
    assert_ne!(notice.as_deref(), Some("Ctrl+Click to open links"));
    assert!(
        notice
            .as_deref()
            .is_none_or(|notice| !notice.contains("link")),
        "drag release must not report a link action, got {notice:?}"
    );
}
