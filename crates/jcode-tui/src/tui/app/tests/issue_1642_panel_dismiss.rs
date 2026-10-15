#[test]
fn test_side_panel_dismiss_rendered_click_preserves_pages_and_reopens() {
    let _lock = scroll_render_test_lock();
    for fullscreen in [false, true] {
        let mut app = create_test_app();
        app.diagram_mode = crate::config::DiagramDisplayMode::None;
        app.diagram_pane_enabled = false;
        app.side_panel = test_side_panel_snapshot("plan", &"Long title ".repeat(30));
        let mut linked = app.side_panel.pages[0].clone();
        linked.id = "linked".to_string();
        linked.source = crate::side_panel::SidePanelPageSource::LinkedFile;
        app.side_panel.pages.push(linked);
        app.side_panel_fullscreen = fullscreen;
        app.diff_pane_focus = true;
        let original_pages = serde_json::to_value(&app.side_panel.pages).unwrap();
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        render_and_snap(&app, &mut terminal);
        let layout = crate::tui::ui::last_layout_snapshot().unwrap();
        let target =
            crate::tui::ui::side_panel_dismiss_area(layout.diff_pane_area.unwrap()).unwrap();
        let label: String = (target.x..target.right())
            .map(|x| terminal.backend().buffer()[(x, target.y)].symbol())
            .collect();
        assert_eq!(label, " [close]", "label must survive long titles");

        // Neither a different button nor the adjacent content row dismisses.
        for (kind, row) in [
            (MouseEventKind::Down(MouseButton::Right), target.y),
            (MouseEventKind::Down(MouseButton::Left), target.y + 1),
        ] {
            app.handle_mouse_event(MouseEvent {
                kind,
                column: target.x + 2,
                row,
                modifiers: KeyModifiers::NONE,
            });
            assert!(app.side_panel.focused_page().is_some());
        }
        assert!(!app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: target.x + 2,
            row: target.y,
            modifiers: KeyModifiers::NONE,
        }));
        assert!(app.side_panel.focused_page_id.is_none());
        assert!(!app.side_panel_fullscreen);
        assert!(app.side_panel_user_hidden && app.side_panel_explicit_hidden);
        assert!(!app.diff_pane_focus);
        assert_eq!(app.last_side_panel_focus_id.as_deref(), Some("plan"));
        assert_eq!(
            serde_json::to_value(&app.side_panel.pages).unwrap(),
            original_pages
        );

        // Ordinary snapshot refresh must not override an explicit dismissal.
        let mut refresh = app.side_panel.clone();
        refresh.focused_page_id = Some("plan".to_string());
        app.set_side_panel_snapshot(refresh);
        assert!(app.side_panel.focused_page_id.is_none());
        let hidden_frame = render_and_snap(&app, &mut terminal);
        assert!(!hidden_frame.contains("[close]"));
        app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT)
            .unwrap();
        assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("plan"));
        assert!(!app.side_panel_user_hidden && !app.side_panel_explicit_hidden);
        assert!(!app.side_panel_fullscreen);
        assert!(render_and_snap(&app, &mut terminal).contains("[close]"));
    }
}

#[test]
fn test_side_panel_dismiss_target_requires_renderable_panel() {
    use ratatui::layout::Rect;
    assert!(crate::tui::ui::side_panel_dismiss_area(Rect::new(0, 0, 9, 3)).is_none());
    assert!(crate::tui::ui::side_panel_dismiss_area(Rect::new(0, 0, 10, 2)).is_none());
    assert_eq!(
        crate::tui::ui::side_panel_dismiss_area(Rect::new(5, 7, 10, 3)),
        Some(Rect::new(7, 7, 8, 1)),
    );
}
