#[test]
fn test_local_error_copy_badge_shortcut_supported() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_error_copy_test_app();

    let initial = render_and_snap(&app, &mut terminal);
    assert!(
        initial.contains("[S]"),
        "expected visible error copy badge: {}",
        initial
    );

    app.handle_key(KeyCode::Char('S'), KeyModifiers::ALT)
        .unwrap();

    assert_eq!(app.status_notice(), Some("Copied error".to_string()));

    let text = render_and_snap(&app, &mut terminal);
    assert!(
        text.contains("Copied!"),
        "expected inline copied feedback: {}",
        text
    );
}

#[test]
fn test_local_tool_error_copy_badge_shortcut_supported() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_tool_error_copy_test_app();

    let initial = render_and_snap(&app, &mut terminal);
    assert!(
        initial.contains("[S]"),
        "expected visible tool error copy badge: {}",
        initial
    );

    app.handle_key(KeyCode::Char('S'), KeyModifiers::ALT)
        .unwrap();

    assert_eq!(app.status_notice(), Some("Copied error".to_string()));

    let text = render_and_snap(&app, &mut terminal);
    assert!(
        text.contains("Copied!"),
        "expected inline copied feedback: {}",
        text
    );
}

#[test]
fn test_local_tool_failed_output_copy_badge_shortcut_supported() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_tool_failed_output_copy_test_app();

    let initial = render_and_snap(&app, &mut terminal);
    assert!(
        initial.contains("[S]"),
        "expected visible failed tool output copy badge: {}",
        initial
    );

    app.handle_key(KeyCode::Char('S'), KeyModifiers::ALT)
        .unwrap();

    assert_eq!(app.status_notice(), Some("Copied output".to_string()));

    let text = render_and_snap(&app, &mut terminal);
    assert!(
        text.contains("Copied!"),
        "expected inline copied feedback: {}",
        text
    );
}

#[test]
fn test_copy_selection_mode_toggle_shows_notification() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_copy_test_app();

    render_and_snap(&app, &mut terminal);
    app.handle_key(KeyCode::Char('y'), KeyModifiers::ALT)
        .unwrap();

    assert!(app.copy_selection_mode);

    let text = render_and_snap(&app, &mut terminal);
    assert!(
        text.contains("Enter/Y copy") || text.contains("drag to copy"),
        "expected selection mode notification, got: {}",
        text
    );
}

#[test]
fn test_copy_selection_select_all_uses_rendered_chat_text_without_copy_badges() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_copy_test_app();

    render_and_snap(&app, &mut terminal);
    app.handle_key(KeyCode::Char('y'), KeyModifiers::ALT)
        .unwrap();
    assert!(app.select_all_in_copy_mode());

    let selected = app
        .current_copy_selection_text()
        .expect("expected selected transcript text");
    assert!(selected.contains("Show me some code"));
    assert!(selected.contains("fn main() {"));
    assert!(selected.contains("println!(\"hello\");"));
    assert!(
        !selected.contains("[Alt]") && !selected.contains("[⌥]"),
        "selection should use chat text, not copy badge chrome: {}",
        selected
    );
}

#[test]
fn test_copy_selection_metrics_match_built_selection_text() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_copy_test_app();

    render_and_snap(&app, &mut terminal);
    app.handle_key(KeyCode::Char('y'), KeyModifiers::ALT)
        .unwrap();
    assert!(app.select_all_in_copy_mode());

    // The allocation-free metrics path used by the status line must agree with
    // the char/line counts of the actually-built selection text.
    let range = app
        .normalized_copy_selection()
        .expect("normalized selection range");
    let text = app
        .current_copy_selection_text()
        .expect("selection text for full transcript");
    let (chars, lines) =
        crate::tui::ui::copy_selection_metrics(range).expect("selection metrics");

    assert_eq!(
        chars,
        text.chars().count(),
        "metrics char count should match built selection text"
    );
    assert_eq!(
        lines,
        text.lines().count().max(1),
        "metrics line count should match built selection text"
    );
}

#[test]
fn test_copy_selection_full_user_prompt_line_skips_prompt_chrome() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_copy_test_app();

    render_and_snap(&app, &mut terminal);
    let (visible_start, visible_end) =
        crate::tui::ui::copy_viewport_visible_range().expect("visible copy range");

    let (prompt_idx, prompt_text) = (visible_start..visible_end)
        .find_map(|abs_line| {
            let text = crate::tui::ui::copy_viewport_line_text(abs_line)?;
            text.contains("Show me some code")
                .then_some((abs_line, text))
        })
        .expect("expected visible user prompt line");

    app.copy_selection_anchor = Some(crate::tui::CopySelectionPoint {
        pane: crate::tui::CopySelectionPane::Chat,
        abs_line: prompt_idx,
        column: 0,
    });
    app.copy_selection_cursor = Some(crate::tui::CopySelectionPoint {
        pane: crate::tui::CopySelectionPane::Chat,
        abs_line: prompt_idx,
        column: unicode_width::UnicodeWidthStr::width(prompt_text.as_str()),
    });

    let selected = app
        .current_copy_selection_text()
        .expect("expected user prompt selection text");
    assert_eq!(selected, "Show me some code");
}

#[test]
fn test_copy_selection_swarm_message_skips_rail_chrome() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_copy_test_app();
    app.display_messages = vec![DisplayMessage::swarm("Broadcast", "hello team")];
    app.bump_display_messages_version();

    render_and_snap(&app, &mut terminal);
    let (visible_start, visible_end) =
        crate::tui::ui::copy_viewport_visible_range().expect("visible copy range");
    let (start_idx, _start_text) = (visible_start..visible_end)
        .find_map(|abs_line| {
            let text = crate::tui::ui::copy_viewport_line_text(abs_line)?;
            text.contains("Broadcast").then_some((abs_line, text))
        })
        .expect("expected visible swarm header line");
    let (end_idx, end_text) = (visible_start..visible_end)
        .find_map(|abs_line| {
            let text = crate::tui::ui::copy_viewport_line_text(abs_line)?;
            text.contains("hello team").then_some((abs_line, text))
        })
        .expect("expected visible swarm body line");

    app.copy_selection_anchor = Some(crate::tui::CopySelectionPoint {
        pane: crate::tui::CopySelectionPane::Chat,
        abs_line: start_idx,
        column: 0,
    });
    app.copy_selection_cursor = Some(crate::tui::CopySelectionPoint {
        pane: crate::tui::CopySelectionPane::Chat,
        abs_line: end_idx,
        column: unicode_width::UnicodeWidthStr::width(end_text.as_str()),
    });

    let selected = app
        .current_copy_selection_text()
        .expect("expected selected swarm text");
    assert!(selected.contains("Broadcast"));
    assert!(selected.contains("hello team"));
    assert!(
        !selected.contains('│'),
        "selection should omit swarm rail chrome: {selected:?}"
    );
}

#[test]
fn test_copy_selection_reconstructs_wrapped_chat_lines_without_hard_wraps() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.display_messages = vec![DisplayMessage {
        role: "assistant".to_string(),
        content: "same physical device: i2c-ELAN900C:00 same vendor/product family: 04F3:4216"
            .to_string(),
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: None,
    }];
    app.bump_display_messages_version();

    let backend = ratatui::backend::TestBackend::new(36, 20);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");

    render_and_snap(&app, &mut terminal);
    let (visible_start, visible_end) =
        crate::tui::ui::copy_viewport_visible_range().expect("visible copy range");

    let visible_lines: Vec<(usize, String)> = (visible_start..visible_end)
        .filter_map(|abs_line| {
            let text = crate::tui::ui::copy_viewport_line_text(abs_line)?;
            (!text.is_empty()).then_some((abs_line, text))
        })
        .collect();
    let (first_idx, _first_text) = visible_lines
        .iter()
        .find(|(_, text)| text.contains("i2c-ELAN900C:00"))
        .expect("expected wrapped line containing device path");
    let (second_idx, second_text) = visible_lines
        .iter()
        .find(|(idx, _)| *idx == *first_idx + 1)
        .expect("expected adjacent wrapped continuation line");

    app.copy_selection_anchor = Some(crate::tui::CopySelectionPoint {
        pane: crate::tui::CopySelectionPane::Chat,
        abs_line: *first_idx,
        column: 0,
    });
    app.copy_selection_cursor = Some(crate::tui::CopySelectionPoint {
        pane: crate::tui::CopySelectionPane::Chat,
        abs_line: *second_idx,
        column: unicode_width::UnicodeWidthStr::width(second_text.as_str()),
    });

    let selected = app
        .current_copy_selection_text()
        .expect("expected wrapped selection text");
    assert!(
        !selected.contains('\n'),
        "wrapped chat copy should not include a hard newline: {selected:?}"
    );
    assert!(
        selected.contains("i2c-ELAN900C:00"),
        "selection should include the device path: {selected:?}"
    );
    assert!(
        selected.contains("same vendor/product family"),
        "selection should preserve the natural space across wrapped lines: {selected:?}"
    );
}

#[test]
fn test_copy_selection_centered_list_keeps_logical_list_text() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.set_centered(true);
    app.display_messages = vec![DisplayMessage {
        role: "assistant".to_string(),
        content: concat!(
            "A goal should support\n\n",
            "1. Create a goal\n",
            "\n",
            "- title\n",
            "- description / \"why this matters\"\n",
            "- success criteria\n",
        )
        .to_string(),
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: None,
    }];
    app.bump_display_messages_version();

    let backend = ratatui::backend::TestBackend::new(28, 20);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");

    render_and_snap(&app, &mut terminal);
    let (visible_start, visible_end) =
        crate::tui::ui::copy_viewport_visible_range().expect("visible copy range");
    let visible_lines: Vec<(usize, String)> = (visible_start..visible_end)
        .filter_map(|abs_line| {
            let text = crate::tui::ui::copy_viewport_line_text(abs_line)?;
            (!text.is_empty()).then_some((abs_line, text))
        })
        .collect();

    let (start_idx, _) = visible_lines
        .iter()
        .find(|(_, text)| text.contains("1. Create a goal"))
        .expect("numbered list line");
    let (end_idx, end_text) = visible_lines
        .iter()
        .rev()
        .find(|(_, text)| text.contains("success criteria") || text.contains("matters"))
        .expect("last list line");

    app.copy_selection_anchor = Some(crate::tui::CopySelectionPoint {
        pane: crate::tui::CopySelectionPane::Chat,
        abs_line: *start_idx,
        column: 0,
    });
    app.copy_selection_cursor = Some(crate::tui::CopySelectionPoint {
        pane: crate::tui::CopySelectionPane::Chat,
        abs_line: *end_idx,
        column: unicode_width::UnicodeWidthStr::width(end_text.as_str()),
    });

    let selected = app
        .current_copy_selection_text()
        .expect("expected selected list text");

    assert!(
        selected.contains("1. Create a goal"),
        "numbered list item should be copied without centered padding: {selected:?}"
    );
    assert!(
        selected.contains("• title"),
        "bullet item should be copied without centered padding: {selected:?}"
    );
    assert!(
        selected.contains("why this matters"),
        "wrapped bullet item should copy logical text: {selected:?}"
    );
}

#[test]
fn test_copy_selection_mouse_drag_extracts_expected_multiline_range() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_copy_test_app();

    render_and_snap(&app, &mut terminal);
    app.handle_key(KeyCode::Char('y'), KeyModifiers::ALT)
        .unwrap();

    let layout = crate::tui::ui::last_layout_snapshot().expect("layout snapshot");
    let (visible_start, visible_end) =
        crate::tui::ui::copy_viewport_visible_range().expect("visible copy range");

    let mut fn_line = None;
    let mut print_line = None;
    for abs_line in visible_start..visible_end {
        let text = crate::tui::ui::copy_viewport_line_text(abs_line).unwrap_or_default();
        if text.contains("fn main() {") {
            fn_line = Some((abs_line, text.clone()));
        }
        if text.contains("println!(\"hello\");") {
            print_line = Some((abs_line, text));
        }
    }

    let (fn_line_idx, fn_text) = fn_line.expect("fn line");
    let (print_line_idx, print_text) = print_line.expect("println line");
    let fn_byte = fn_text.find("fn main() {").expect("fn column");
    let fn_col = unicode_width::UnicodeWidthStr::width(&fn_text[..fn_byte]) as u16;
    let _print_end_col = (print_text.find(");").expect("print end") + 2) as u16;

    let base_y = layout.messages_area.y;
    let start_row = base_y + (fn_line_idx - visible_start) as u16;
    let end_row = base_y + (print_line_idx - visible_start) as u16;

    let start_x = (layout.messages_area.x..layout.messages_area.x + layout.messages_area.width)
        .find(|&column| {
            crate::tui::ui::copy_viewport_point_from_screen(column, start_row)
                .map(|point| point.abs_line == fn_line_idx && point.column == fn_col as usize)
                .unwrap_or(false)
        })
        .expect("screen x for selection start");

    let end_x = (layout.messages_area.x..layout.messages_area.x + layout.messages_area.width)
        .filter_map(|column| {
            crate::tui::ui::copy_viewport_point_from_screen(column, end_row)
                .filter(|point| point.abs_line == print_line_idx)
                .map(|point| (column, point.column))
        })
        .max_by_key(|(_, mapped_col)| *mapped_col)
        .map(|(column, _)| column)
        .expect("screen x for selection end");

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: start_x,
        row: start_row,
        modifiers: KeyModifiers::empty(),
    });
    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: end_x,
        row: end_row,
        modifiers: KeyModifiers::empty(),
    });

    let selected = app
        .current_copy_selection_text()
        .expect("expected multiline selection");
    let range = app.normalized_copy_selection().expect("normalized range");
    assert_eq!(range.start.abs_line, fn_line_idx);
    assert_eq!(range.end.abs_line, print_line_idx);
    assert!(
        selected.contains("fn main() {"),
        "selection missing fn line: {selected}"
    );
    assert!(
        selected.contains("println!(\"hello\");"),
        "selection missing println line: {selected}"
    );
    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: end_x,
        row: end_row,
        modifiers: KeyModifiers::empty(),
    });
    assert!(app.copy_selection_mode);
    assert!(!app.copy_selection_dragging);
}

#[test]
fn test_copy_selection_mouse_click_does_not_enter_mode() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_copy_test_app();

    render_and_snap(&app, &mut terminal);

    let layout = crate::tui::ui::last_layout_snapshot().expect("layout snapshot");
    let (visible_start, visible_end) =
        crate::tui::ui::copy_viewport_visible_range().expect("visible copy range");

    let target = (visible_start..visible_end)
        .find_map(|abs_line| {
            let text = crate::tui::ui::copy_viewport_line_text(abs_line)?;
            let byte = text.find("println!(\"hello\");")?;
            let col = unicode_width::UnicodeWidthStr::width(&text[..byte]) as u16;
            Some((abs_line, col))
        })
        .expect("println line");

    let row = layout.messages_area.y + (target.0 - visible_start) as u16;
    let col = (layout.messages_area.x..layout.messages_area.x + layout.messages_area.width)
        .find(|&column| {
            crate::tui::ui::copy_viewport_point_from_screen(column, row)
                .map(|point| point.abs_line == target.0 && point.column == target.1 as usize)
                .unwrap_or(false)
        })
        .expect("screen x for println");

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: col,
        row,
        modifiers: KeyModifiers::empty(),
    });
    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: col,
        row,
        modifiers: KeyModifiers::empty(),
    });

    assert!(!app.copy_selection_mode);
    assert!(app.copy_selection_anchor.is_none());
    assert!(app.copy_selection_cursor.is_none());
}

#[test]
fn test_copy_selection_mouse_drag_auto_copies_and_exits_mode() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_copy_test_app();
    let copied = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let copied_for_closure = copied.clone();

    render_and_snap(&app, &mut terminal);

    let layout = crate::tui::ui::last_layout_snapshot().expect("layout snapshot");
    let (visible_start, visible_end) =
        crate::tui::ui::copy_viewport_visible_range().expect("visible copy range");

    let mut fn_line = None;
    let mut print_line = None;
    for abs_line in visible_start..visible_end {
        let text = crate::tui::ui::copy_viewport_line_text(abs_line).unwrap_or_default();
        if text.contains("fn main() {") {
            fn_line = Some((abs_line, text.clone()));
        }
        if text.contains("println!(\"hello\");") {
            print_line = Some((abs_line, text));
        }
    }

    let (fn_line_idx, fn_text) = fn_line.expect("fn line");
    let (print_line_idx, _print_text) = print_line.expect("println line");
    let fn_byte = fn_text.find("fn main() {").expect("fn column");
    let fn_col = unicode_width::UnicodeWidthStr::width(&fn_text[..fn_byte]) as u16;

    let base_y = layout.messages_area.y;
    let start_row = base_y + (fn_line_idx - visible_start) as u16;
    let end_row = base_y + (print_line_idx - visible_start) as u16;

    let start_x = (layout.messages_area.x..layout.messages_area.x + layout.messages_area.width)
        .find(|&column| {
            crate::tui::ui::copy_viewport_point_from_screen(column, start_row)
                .map(|point| point.abs_line == fn_line_idx && point.column == fn_col as usize)
                .unwrap_or(false)
        })
        .expect("screen x for selection start");

    let end_x = (layout.messages_area.x..layout.messages_area.x + layout.messages_area.width)
        .filter_map(|column| {
            crate::tui::ui::copy_viewport_point_from_screen(column, end_row)
                .filter(|point| point.abs_line == print_line_idx)
                .map(|point| (column, point.column))
        })
        .max_by_key(|(_, mapped_col)| *mapped_col)
        .map(|(column, _)| column)
        .expect("screen x for selection end");

    app.handle_copy_selection_mouse_with(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: start_x,
            row: start_row,
            modifiers: KeyModifiers::empty(),
        },
        |_| true,
    );
    app.handle_copy_selection_mouse_with(
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: end_x,
            row: end_row,
            modifiers: KeyModifiers::empty(),
        },
        |_| true,
    );
    app.handle_copy_selection_mouse_with(
        MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: end_x,
            row: end_row,
            modifiers: KeyModifiers::empty(),
        },
        |text| {
            *copied_for_closure.lock().unwrap() = text.to_string();
            true
        },
    );

    assert!(!app.copy_selection_mode);
    assert!(app.copy_selection_anchor.is_none());
    assert!(app.copy_selection_cursor.is_none());
    assert!(copied.lock().unwrap().contains("println!(\"hello\");"));
    assert_eq!(app.status_notice(), Some("Copied selection".to_string()));
}

#[test]
fn test_side_panel_mouse_drag_extracts_expected_text() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    let copied = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let copied_for_closure = copied.clone();
    app.side_panel = crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("plan".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "plan".to_string(),
            title: "Plan".to_string(),
            file_path: "".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "alpha\nbeta highlight target\ngamma".to_string(),
            updated_at_ms: 1,
        }],
    };

    let backend = ratatui::backend::TestBackend::new(100, 20);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
    render_and_snap(&app, &mut terminal);

    let layout = crate::tui::ui::last_layout_snapshot().expect("layout snapshot");
    let diff_area = layout.diff_pane_area.expect("side pane area");
    let (visible_start, visible_end) =
        crate::tui::ui::side_pane_visible_range().expect("side pane visible range");

    let (line_idx, _line_text) = (visible_start..visible_end)
        .find_map(|abs_line| {
            let text = crate::tui::ui::side_pane_line_text(abs_line)?;
            text.contains("beta highlight target")
                .then_some((abs_line, text))
        })
        .expect("target side pane line");
    let (row, column) = (diff_area.y..diff_area.y + diff_area.height)
        .find_map(|screen_y| {
            (diff_area.x..diff_area.x + diff_area.width)
                .find(|&screen_x| {
                    crate::tui::ui::side_pane_point_from_screen(screen_x, screen_y)
                        .map(|point| point.abs_line == line_idx)
                        .unwrap_or(false)
                })
                .map(|screen_x| (screen_y, screen_x))
        })
        .expect("screen x for side selection start");
    let end_column = (diff_area.x..diff_area.x + diff_area.width)
        .filter_map(|screen_x| {
            crate::tui::ui::side_pane_point_from_screen(screen_x, row)
                .filter(|point| point.abs_line == line_idx)
                .map(|point| (screen_x, point.column))
        })
        .max_by_key(|(_, mapped)| *mapped)
        .map(|(screen_x, _)| screen_x)
        .expect("screen x for side selection end");

    app.handle_copy_selection_mouse_with(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::empty(),
        },
        |_| true,
    );
    app.handle_copy_selection_mouse_with(
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: end_column,
            row,
            modifiers: KeyModifiers::empty(),
        },
        |_| true,
    );

    let selected = app
        .current_copy_selection_text()
        .expect("expected side pane selection");
    assert!(
        selected.contains("beta highlight target"),
        "selected={selected}"
    );
    assert_eq!(
        app.current_copy_selection_pane(),
        Some(crate::tui::CopySelectionPane::SidePane)
    );

    app.handle_copy_selection_mouse_with(
        MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: end_column,
            row,
            modifiers: KeyModifiers::empty(),
        },
        |text| {
            *copied_for_closure.lock().unwrap() = text.to_string();
            true
        },
    );
    assert!(copied.lock().unwrap().contains("beta highlight target"));
    assert!(!app.copy_selection_mode);
}

#[test]
fn test_copy_selection_copy_action_uses_clipboard_hook_and_exits_mode() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_copy_test_app();
    let copied = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let copied_for_closure = copied.clone();

    render_and_snap(&app, &mut terminal);
    app.handle_key(KeyCode::Char('y'), KeyModifiers::ALT)
        .unwrap();
    assert!(app.select_all_in_copy_mode());

    let success = app.copy_current_selection_to_clipboard_with(|text| {
        *copied_for_closure.lock().unwrap() = text.to_string();
        true
    });

    assert!(success);
    assert!(!app.copy_selection_mode);
    assert!(app.copy_selection_anchor.is_none());
    assert!(app.copy_selection_cursor.is_none());
    assert!(copied.lock().unwrap().contains("println!(\"hello\");"));
    assert_eq!(app.status_notice(), Some("Copied selection".to_string()));
}

#[test]
fn test_ctrl_a_copies_chat_viewport_with_context_when_input_empty() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    let copied = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let copied_for_closure = copied.clone();

    let lines = (1..=40)
        .map(|idx| format!("line {idx:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    app.display_messages = vec![DisplayMessage {
        role: "assistant".to_string(),
        content: lines,
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: None,
    }];
    app.bump_display_messages_version();
    app.scroll_offset = 12;
    app.auto_scroll_paused = true;

    let backend = ratatui::backend::TestBackend::new(40, 8);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(&app, &mut terminal);

    let (visible_start, visible_end) =
        crate::tui::ui::copy_viewport_visible_range().expect("visible copy range");
    let line_count = crate::tui::ui::copy_viewport_line_count().expect("line count");
    let context = 4usize;
    let expected_start = visible_start.saturating_sub(context);
    let expected_end = visible_end
        .saturating_add(context)
        .saturating_sub(1)
        .min(line_count.saturating_sub(1));
    assert!(app.select_chat_viewport_context());
    let range = app
        .normalized_copy_selection()
        .expect("expected viewport context range");
    assert_eq!(range.start.pane, crate::tui::CopySelectionPane::Chat);
    assert_eq!(range.end.pane, crate::tui::CopySelectionPane::Chat);
    assert_eq!(range.start.abs_line, expected_start);
    assert_eq!(range.end.abs_line, expected_end);
    let preselected_text = app
        .current_copy_selection_text()
        .expect("expected viewport context text");
    assert!(
        !preselected_text.trim().is_empty(),
        "viewport context selection should not be empty"
    );

    let success = app.copy_current_selection_to_clipboard_with(|text| {
        *copied_for_closure.lock().unwrap() = text.to_string();
        true
    });

    assert!(success);
    let copied_text = copied.lock().unwrap().clone();
    assert!(
        copied_text == preselected_text,
        "copied text should match selected viewport context: {copied_text:?}"
    );
    assert_eq!(app.status_notice(), Some("Copied selection".to_string()));
    assert!(!app.copy_selection_mode);
    assert!(app.copy_selection_anchor.is_none());
    assert!(app.copy_selection_cursor.is_none());
}

#[test]
fn test_copy_selection_drag_to_top_edge_auto_scrolls_chat() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();

    // Tall transcript so there is more content above the viewport to scroll into.
    let lines = (1..=200)
        .map(|idx| format!("line {idx:03}"))
        .collect::<Vec<_>>()
        .join("\n");
    app.display_messages = vec![DisplayMessage {
        role: "assistant".to_string(),
        content: lines,
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: None,
    }];
    app.bump_display_messages_version();
    app.scroll_offset = 0;
    app.auto_scroll_paused = false;
    app.is_processing = false;
    app.streaming.streaming_text.clear();
    app.status = ProcessingStatus::Idle;

    let backend = ratatui::backend::TestBackend::new(60, 12);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(&app, &mut terminal);

    app.handle_key(KeyCode::Char('y'), KeyModifiers::ALT)
        .unwrap();

    let layout = crate::tui::ui::last_layout_snapshot().expect("layout snapshot");
    let area = layout.messages_area;
    let top_row = area.y;
    let lower_row = area.y + area.height / 2;
    let col = area.x + 1;

    // Anchor in the middle of the viewport.
    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: col,
        row: lower_row,
        modifiers: KeyModifiers::empty(),
    });

    let before = app.scroll_offset();
    // Dragging to the top boundary row should pull more transcript into view,
    // just like selecting past the top edge of a browser window.
    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: col,
        row: top_row,
        modifiers: KeyModifiers::empty(),
    });

    assert!(
        app.scroll_offset() < before,
        "drag to top edge should auto-scroll chat up (before={before}, after={})",
        app.scroll_offset()
    );

    // Browser-style continuous scroll: holding the drag at the edge keeps pulling
    // in more transcript on subsequent ticks without any further mouse movement.
    let after_drag = app.scroll_offset();
    assert!(app.progress_copy_selection_edge_autoscroll());
    assert!(
        app.scroll_offset() < after_drag,
        "edge autoscroll tick should keep scrolling (after_drag={after_drag}, after_tick={})",
        app.scroll_offset()
    );

    // The redraw loop must stay responsive while the mouse is held at the edge,
    // even though the transcript is otherwise idle and no further mouse events
    // arrive. Otherwise the deep-idle 5s cadence would stall the autoscroll.
    assert!(
        crate::tui::TuiState::copy_selection_edge_autoscroll_active(&app),
        "edge autoscroll should be reported active while drag is held at edge"
    );
    assert!(
        crate::tui::periodic_redraw_required(&app),
        "periodic redraw must be required while edge autoscroll is armed"
    );
    let policy = crate::perf::tui_policy();
    let interval = crate::tui::redraw_interval_with_policy(&app, &policy);
    assert!(
        interval <= crate::tui::REDRAW_IDLE,
        "redraw interval should stay fast during edge autoscroll, got {interval:?}"
    );

    // Simulate the real tick loop driving several frames with the mouse held
    // still (no further drag events): it should keep scrolling toward the top.
    let mut prev = app.scroll_offset();
    for _ in 0..5 {
        if prev == 0 {
            break;
        }
        assert!(app.progress_copy_selection_edge_autoscroll());
        assert!(
            app.scroll_offset() < prev,
            "held-still tick should keep scrolling up (prev={prev}, now={})",
            app.scroll_offset()
        );
        prev = app.scroll_offset();
    }

    // Releasing the mouse stops the continuous autoscroll.
    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: col,
        row: top_row,
        modifiers: KeyModifiers::empty(),
    });
    assert!(!app.progress_copy_selection_edge_autoscroll());
    assert!(!crate::tui::TuiState::copy_selection_edge_autoscroll_active(
        &app
    ));
}

#[test]
fn test_copy_selection_drag_near_top_edge_keeps_auto_scrolling() {
    // Regression: holding the cursor *near* (not exactly on) the top boundary
    // row used to fall outside the edge trigger, which disarmed the continuous
    // autoscroll. The drag then only nudged one step per mouse movement and
    // stalled entirely while the cursor was held still. A small browser-style
    // "hot zone" band near each edge keeps the autoscroll armed so the
    // transcript keeps scrolling while the mouse is held anywhere near the edge.
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();

    let lines = (1..=200)
        .map(|idx| format!("line {idx:03}"))
        .collect::<Vec<_>>()
        .join("\n");
    app.display_messages = vec![DisplayMessage {
        role: "assistant".to_string(),
        content: lines,
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: None,
    }];
    app.bump_display_messages_version();
    app.scroll_offset = 0;
    app.auto_scroll_paused = false;
    app.is_processing = false;
    app.streaming.streaming_text.clear();
    app.status = ProcessingStatus::Idle;

    let backend = ratatui::backend::TestBackend::new(60, 16);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(&app, &mut terminal);

    app.handle_key(KeyCode::Char('y'), KeyModifiers::ALT)
        .unwrap();

    let layout = crate::tui::ui::last_layout_snapshot().expect("layout snapshot");
    let area = layout.messages_area;
    let lower_row = area.y + area.height / 2;
    let col = area.x + 1;
    // One row *inside* the top boundary: this is the spot that used to fail.
    let near_top_row = area.y + 1;
    assert!(
        near_top_row > area.y,
        "test must drag strictly inside the top boundary row"
    );

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: col,
        row: lower_row,
        modifiers: KeyModifiers::empty(),
    });

    let before = app.scroll_offset();
    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: col,
        row: near_top_row,
        modifiers: KeyModifiers::empty(),
    });
    assert!(
        app.scroll_offset() < before,
        "drag near (not on) the top edge should auto-scroll up (before={before}, after={})",
        app.scroll_offset()
    );

    // The autoscroll must stay armed so holding the cursor still keeps pulling in
    // more transcript on subsequent ticks (the original bug stalled here).
    assert!(
        crate::tui::TuiState::copy_selection_edge_autoscroll_active(&app),
        "holding near the top edge should keep the continuous autoscroll armed"
    );

    let mut prev = app.scroll_offset();
    for _ in 0..5 {
        if prev == 0 {
            break;
        }
        assert!(app.progress_copy_selection_edge_autoscroll());
        assert!(
            app.scroll_offset() < prev,
            "held-still tick near the edge should keep scrolling up (prev={prev}, now={})",
            app.scroll_offset()
        );
        prev = app.scroll_offset();
    }

    // Releasing the mouse stops the continuous autoscroll.
    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: col,
        row: near_top_row,
        modifiers: KeyModifiers::empty(),
    });
    assert!(!crate::tui::TuiState::copy_selection_edge_autoscroll_active(
        &app
    ));
}

#[test]
fn test_copy_selection_drag_to_bottom_edge_when_pinned_does_not_snap_or_autoscroll() {
    // Regression: when the transcript is already pinned to the bottom (the common
    // case), dragging a selection into the bottom edge "hot zone" used to always
    // snap the cursor to the very last visible line and arm a downward autoscroll,
    // even though there is nothing more below to scroll into. That made it
    // impossible to precisely highlight the bottom rows: the selection kept
    // jumping to the end. With nothing to scroll, the edge band must stay inert so
    // the selection lands on the exact line under the cursor.
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();

    // Tall transcript pinned to the bottom: the bottom rows of the pane are
    // filled with real content, and there is nothing below to scroll into.
    let lines = (1..=200)
        .map(|idx| format!("line {idx:03}"))
        .collect::<Vec<_>>()
        .join("\n");
    app.display_messages = vec![DisplayMessage {
        role: "assistant".to_string(),
        content: lines,
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: None,
    }];
    app.bump_display_messages_version();
    app.scroll_offset = 0;
    app.auto_scroll_paused = false;
    app.is_processing = false;
    app.streaming.streaming_text.clear();
    app.status = ProcessingStatus::Idle;

    let backend = ratatui::backend::TestBackend::new(60, 16);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(&app, &mut terminal);

    app.handle_key(KeyCode::Char('y'), KeyModifiers::ALT)
        .unwrap();

    let (visible_start, visible_end) =
        crate::tui::ui::copy_viewport_visible_range().expect("visible copy range");
    let line_count = crate::tui::ui::copy_viewport_line_count().expect("line count");
    assert_eq!(
        visible_end, line_count,
        "test precondition: view must be pinned to the bottom with no content below"
    );
    assert!(
        visible_start > 0,
        "test precondition: tall transcript must have content scrolled above the view"
    );

    let layout = crate::tui::ui::last_layout_snapshot().expect("layout snapshot");
    let area = layout.messages_area;
    let col = area.x + 1;

    // Pick a real content line near (but not at) the bottom to target.
    let target_line = visible_end.saturating_sub(2);
    assert!(target_line >= visible_start, "need a visible target line");
    let target_row = area.y + (target_line - visible_start) as u16;
    // The bottom edge band covers the last few rows; target_row must sit inside
    // it for this regression to be meaningful.
    let last_row = area.y + area.height - 1;
    assert!(
        target_row >= last_row.saturating_sub(2),
        "target line must fall within the bottom edge hot zone"
    );

    // Anchor higher up in the viewport.
    let anchor_row = area.y + 1;
    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: col,
        row: anchor_row,
        modifiers: KeyModifiers::empty(),
    });
    let before_scroll = app.scroll_offset();

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: col,
        row: target_row,
        modifiers: KeyModifiers::empty(),
    });

    // No autoscroll should be armed: there is nothing below to pull in.
    assert!(
        !crate::tui::TuiState::copy_selection_edge_autoscroll_active(&app),
        "edge autoscroll must not arm when pinned to the bottom with no content below"
    );
    assert_eq!(
        app.scroll_offset(),
        before_scroll,
        "dragging into the bottom band while pinned must not scroll"
    );

    // The selection end should land on the exact line under the cursor, not snap
    // to the very last line of the transcript.
    let range = app.normalized_copy_selection().expect("normalized range");
    assert_eq!(
        range.end.abs_line, target_line,
        "selection should extend to the line under the cursor, not snap to the last line"
    );

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: col,
        row: target_row,
        modifiers: KeyModifiers::empty(),
    });
}

#[test]
fn test_copy_selection_drag_below_last_line_fully_selects_last_line() {
    // Dragging *past* the last content line (into the empty area below the
    // chat pane) should fully select that last line through its end, just like
    // native terminal and browser selection. The chat pane is sized to its
    // content, so a downward drag that overshoots reports a row at/below the
    // bottom boundary that maps to no line at all; that used to silently drop
    // the extension so the bottom line could never be fully highlighted.
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();

    let lines = (1..=6)
        .map(|idx| format!("line {idx:03}"))
        .collect::<Vec<_>>()
        .join("\n");
    app.display_messages = vec![DisplayMessage {
        role: "assistant".to_string(),
        content: lines,
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: None,
    }];
    app.bump_display_messages_version();
    app.scroll_offset = 0;
    app.auto_scroll_paused = false;
    app.is_processing = false;
    app.streaming.streaming_text.clear();
    app.status = ProcessingStatus::Idle;

    // Tall terminal so there is empty space below the content-sized chat pane.
    let backend = ratatui::backend::TestBackend::new(60, 20);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(&app, &mut terminal);

    app.handle_key(KeyCode::Char('y'), KeyModifiers::ALT)
        .unwrap();

    let (visible_start, visible_end) =
        crate::tui::ui::copy_viewport_visible_range().expect("visible copy range");
    let line_count = crate::tui::ui::copy_viewport_line_count().expect("line count");
    assert_eq!(visible_end, line_count, "view must be pinned to the bottom");

    // The last line that maps to a real screen point.
    let last_line = (visible_start..visible_end)
        .rev()
        .find(|&ln| {
            crate::tui::ui::copy_viewport_line_text(ln)
                .map(|t| unicode_width::UnicodeWidthStr::width(t.as_str()) > 0)
                .unwrap_or(false)
        })
        .expect("a non-empty visible content line");
    let last_text = crate::tui::ui::copy_viewport_line_text(last_line).unwrap_or_default();
    let last_width = unicode_width::UnicodeWidthStr::width(last_text.as_str());

    let layout = crate::tui::ui::last_layout_snapshot().expect("layout snapshot");
    let area = layout.messages_area;

    // Anchor on a valid cell at the START of the last content line.
    let last_content_row = area.y + (last_line - visible_start) as u16;
    let anchor_x = (area.x..area.x + area.width)
        .find(|&x| {
            crate::tui::ui::copy_viewport_point_from_screen(x, last_content_row)
                .map(|p| p.abs_line == last_line)
                .unwrap_or(false)
        })
        .expect("a screen column mapping to the last content line");
    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: anchor_x,
        row: last_content_row,
        modifiers: KeyModifiers::empty(),
    });

    // Drag straight down, past the bottom of the pane, with the cursor x landing
    // partway through (not at the end of) the last line. Even so the whole last
    // line should be selected, because we have overshot it vertically.
    let mid_x = anchor_x + 1;
    let below_row = (area.y + area.height + 2).min(terminal.backend().size().unwrap().height - 1);
    assert!(
        below_row > last_content_row,
        "test must drag strictly below the last content row"
    );
    let before_scroll = app.scroll_offset();
    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: mid_x,
        row: below_row,
        modifiers: KeyModifiers::empty(),
    });

    // No autoscroll (nothing below), and no scroll movement.
    assert!(
        !crate::tui::TuiState::copy_selection_edge_autoscroll_active(&app),
        "edge autoscroll must not arm dragging past the last line"
    );
    assert_eq!(app.scroll_offset(), before_scroll, "must not scroll");

    // The selection should now extend through the END of the last line.
    let range = app.normalized_copy_selection().expect("normalized range");
    assert_eq!(
        range.end.abs_line, last_line,
        "selection should extend to the last content line"
    );
    assert_eq!(
        range.end.column, last_width,
        "selection should cover the full last line (through its end)"
    );
    let selected = app
        .current_copy_selection_text()
        .expect("expected selection text");
    assert!(
        selected.contains(last_text.trim_end()),
        "selection should include the full last line text: got {selected:?}"
    );

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: mid_x,
        row: below_row,
        modifiers: KeyModifiers::empty(),
    });
}

#[test]
fn test_alt_a_copies_chat_viewport_with_context_when_input_empty() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();

    let lines = (1..=20)
        .map(|idx| format!("line {idx:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    app.display_messages = vec![DisplayMessage {
        role: "assistant".to_string(),
        content: lines,
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: None,
    }];
    app.bump_display_messages_version();
    app.scroll_offset = 4;
    app.auto_scroll_paused = true;

    let backend = ratatui::backend::TestBackend::new(40, 8);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(&app, &mut terminal);

    let handled = super::input::handle_alt_key(&mut app, KeyCode::Char('a'));
    assert!(handled);
    assert!(matches!(
        app.status_notice().as_deref(),
        Some("Copied viewport context")
            | Some("Failed to copy viewport context")
            | Some("Nothing visible to copy")
    ));
}
