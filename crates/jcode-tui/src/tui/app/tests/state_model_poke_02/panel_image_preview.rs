#[test]
fn test_panel_image_preview_click_render_dismiss_and_restore() {
    let _lock = scroll_render_test_lock();
    struct ResetImageMode;
    impl Drop for ResetImageMode {
        fn drop(&mut self) {
            crate::tui::mermaid::set_video_export_mode(false);
        }
    }
    crate::tui::mermaid::set_video_export_mode(true);
    let _reset = ResetImageMode;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("preview.png");
    ::image::RgbaImage::from_pixel(800, 400, ::image::Rgba([0, 80, 255, 255]))
        .save(&path)
        .unwrap();
    let mut app = create_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app.side_panel = crate::side_panel::SidePanelSnapshot {
        focus_revision: 0,
        focused_page_id: Some("preview".into()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "preview".into(),
            title: "Preview fixture".into(),
            file_path: "".into(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            pdf_data: None,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: format!(
                "# Preview fixture\n\n![Image]({})\n\nAfter image",
                path.display()
            ),
            updated_at_ms: 1,
        }],
    };
    app.input = "keep my draft".into();
    app.cursor_pos = app.input.len();
    app.diff_pane_auto_scroll = false;
    app.diff_pane_scroll = 2;
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
    let text = render_and_snap(&app, &mut terminal);
    assert!(text.contains("Preview fixture"), "{text}");
    let (x, y, hash) = (0..40)
        .find_map(|y| {
            (0..120).find_map(|x| {
                crate::tui::ui::panel_image_preview::image_at(x, y).map(|hash| (x, y, hash))
            })
        })
        .expect("rendered image should have a click target");
    let mouse = |kind, column, row| MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::empty(),
    };
    // Click targets exclude the text header and chat.
    assert_eq!(crate::tui::ui::panel_image_preview::image_at(0, 0), None);
    // Dragging across image placeholder rows remains a selection, not a click.
    app.handle_mouse_event(mouse(MouseEventKind::Down(MouseButton::Left), x, y));
    app.handle_mouse_event(mouse(MouseEventKind::Drag(MouseButton::Left), x, y + 1));
    app.handle_mouse_event(mouse(MouseEventKind::Up(MouseButton::Left), x, y + 1));
    assert_eq!(app.panel_image_preview, None);
    app.handle_mouse_event(mouse(MouseEventKind::Down(MouseButton::Left), x, y));
    assert_eq!(app.panel_image_preview, None, "open on release, not press");
    app.handle_mouse_event(mouse(MouseEventKind::Drag(MouseButton::Left), x, y));
    app.handle_mouse_event(mouse(MouseEventKind::Up(MouseButton::Left), x, y));
    assert_eq!(app.panel_image_preview, Some(hash));
    let text = render_and_snap(&app, &mut terminal);
    assert!(text.contains("Image preview"), "{text}");
    assert!(text.contains("Click or Esc to close"), "{text}");
    assert!(
        !text.contains("Preview fixture"),
        "preview replaces the split layout"
    );
    assert_eq!(crate::tui::ui::panel_image_preview::image_at(x, y), None);
    // Modal input must not edit the prompt or scroll the panel behind it.
    app.handle_key(KeyCode::Char('x'), KeyModifiers::empty())
        .unwrap();
    app.handle_mouse_event(mouse(MouseEventKind::ScrollDown, x, y));
    assert_eq!(app.input, "keep my draft");
    assert_eq!(app.diff_pane_scroll, 2);
    app.handle_key(KeyCode::Esc, KeyModifiers::empty()).unwrap();
    assert_eq!(app.panel_image_preview, None);
    assert_eq!(app.diff_pane_scroll, 2);
    let text = render_and_snap(&app, &mut terminal);
    assert!(text.contains("Preview fixture"));
    // Reopening and clicking anywhere closes without clicking through.
    app.handle_mouse_event(mouse(MouseEventKind::Down(MouseButton::Left), x, y));
    app.handle_mouse_event(mouse(MouseEventKind::Up(MouseButton::Left), x, y));
    assert_eq!(app.panel_image_preview, Some(hash));
    app.handle_mouse_event(mouse(MouseEventKind::Down(MouseButton::Left), 0, 0));
    assert_eq!(app.panel_image_preview, Some(hash));
    app.handle_mouse_event(mouse(MouseEventKind::Up(MouseButton::Left), 0, 0));
    assert_eq!(app.panel_image_preview, None);
    assert_eq!(app.input, "keep my draft");
    // Hiding the panel must remove its old clickable regions.
    app.side_panel = Default::default();
    render_and_snap(&app, &mut terminal);
    assert_eq!(crate::tui::ui::panel_image_preview::image_at(x, y), None);
}

#[test]
fn test_panel_image_preview_missing_image_and_tiny_terminal_are_dismissible() {
    let _lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.panel_image_preview = Some(u64::MAX - 123);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 15)).unwrap();
    assert!(render_and_snap(&app, &mut terminal).contains("Image is no longer available"));
    let mut tiny = ratatui::Terminal::new(ratatui::backend::TestBackend::new(1, 1)).unwrap();
    render_and_snap(&app, &mut tiny);
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();
    assert_eq!(app.panel_image_preview, None);
}

#[test]
fn test_panel_image_preview_remote_keys_and_session_reset() {
    let mut app = create_test_app();
    app.input = "unsent draft".into();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    for close_key in [KeyCode::Esc, KeyCode::Enter, KeyCode::Char('q')] {
        app.panel_image_preview = Some(42);
        rt.block_on(app.handle_remote_key(KeyCode::Char('x'), KeyModifiers::empty(), &mut remote))
            .unwrap();
        assert_eq!(app.input, "unsent draft");
        assert_eq!(app.panel_image_preview, Some(42));
        rt.block_on(app.handle_remote_key(close_key, KeyModifiers::empty(), &mut remote))
            .unwrap();
        assert_eq!(app.panel_image_preview, None);
        assert_eq!(app.input, "unsent draft");
    }
    app.panel_image_preview = Some(42);
    crate::tui::app::commands_review::clear_side_panel_for_new_session(&mut app);
    assert_eq!(app.panel_image_preview, None);
}
