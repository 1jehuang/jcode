use super::*;

#[test]
fn resume_canvas_reserves_compact_right_card_with_spatial_focus_and_mouse_regions() {
    for width in [20, 80, 120] {
        let session = make_session("right-sidebar", "sidebar", false, SessionStatus::Closed);
        let mut picker = SessionPicker::new(vec![session]);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 30)).unwrap();
        terminal.draw(|frame| picker.render(frame)).unwrap();
        let preview = picker.last_preview_area.unwrap();
        let sidebar = picker.last_list_area.unwrap();
        assert_eq!(preview, Rect::new(0, 0, width, 30));
        assert!(sidebar.x > preview.x);
        assert!(sidebar.right() < preview.right());
        assert_eq!(
            sidebar.right(),
            preview.right() - 2,
            "one blank column must separate the card and native scrollbar"
        );
        assert!(sidebar.height < preview.height);
        assert!(sidebar.bottom() < preview.bottom());
        assert!(picker.mouse_over_preview(sidebar.x + 1, sidebar.bottom() + 1));
        assert!(picker.mouse_over_preview(preview.x + 1, preview.y + 1));
        assert!(!picker.mouse_over_preview(sidebar.x + 1, sidebar.y + 1));
        for key in [KeyCode::Left, KeyCode::Char('h')] {
            assert!(picker.handle_focus_navigation_key(key, KeyModifiers::empty()));
            assert_eq!(picker.focus, PaneFocus::Preview);
        }
        for key in [KeyCode::Right, KeyCode::Char('l')] {
            assert!(picker.handle_focus_navigation_key(key, KeyModifiers::empty()));
            assert_eq!(picker.focus, PaneFocus::Sessions);
        }
    }
}

#[test]
fn resume_card_does_not_rewrap_or_scroll_the_underlying_session() {
    let _lock = crate::tui::ui::render_state_test_lock();
    for width in [40, 80, 120] {
        let mut session = make_session("overlay-parity", "fox", false, SessionStatus::Closed);
        session.messages_preview.push(PreviewMessage {
            role: "assistant".into(),
            content: (0..70)
                .map(|n| format!("row {n}: {}", "Unicode 界 text ".repeat(12)))
                .collect::<Vec<_>>()
                .join("\n"),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
            timestamp: None,
        });
        let mut actual = SessionPicker::new(vec![session.clone()]);
        let mut baseline = SessionPicker::new(vec![session]);
        let area = Rect::new(0, 0, width, 45);
        // Unbordered conversation canvas, without painting the control card.
        baseline.reserved_panel_area = Some(Rect::default());
        let mut expected =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 45)).unwrap();
        let mut rendered =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 45)).unwrap();
        expected
            .draw(|frame| baseline.render_preview(frame, area))
            .unwrap();
        rendered.draw(|frame| actual.render(frame)).unwrap();
        let card = actual.reserved_panel_area.unwrap();
        assert_eq!(actual.scroll_offset, baseline.scroll_offset);
        assert_eq!(
            actual.preview_cache.as_ref().unwrap().wrapped_lines,
            baseline.preview_cache.as_ref().unwrap().wrapped_lines
        );
        for y in 0..area.height {
            for x in 0..area.width {
                if !card.contains(Position::new(x, y)) {
                    assert_eq!(
                        rendered.backend().buffer()[(x, y)],
                        expected.backend().buffer()[(x, y)],
                        "session pixel outside overlay changed at ({x},{y}), width={width}"
                    );
                }
            }
        }
    }
}

#[test]
fn resume_card_stays_anchored_when_preview_bands_change() {
    let _lock = crate::tui::ui::render_state_test_lock();
    let session = make_session("pinned-placement", "fox", false, SessionStatus::Closed);
    let id = session.id.clone();
    let mut other = session.clone();
    other.id = "other-placement".into();
    other.short_name = "other".into();
    other.server_name = Some("cloud".into());
    other.model = Some("different-model".into());
    let mut picker = SessionPicker::new(vec![session, other]);
    let selected = picker.find_item_index_for_session_id(&id).unwrap();
    picker.list_state.select(Some(selected));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 50)).unwrap();
    terminal.draw(|frame| picker.render(frame)).unwrap();
    let initial_card = picker.reserved_panel_area.unwrap();
    let initial_list = picker.last_list_area.unwrap();
    let payload = serde_json::json!({"todos": (0..20).map(|n| serde_json::json!({
        "id": format!("item-{n}"), "content": format!("Pinned work item {n}"),
        "status": "pending", "priority": "high", "phase": 1, "lane": format!("lane-{n}")
    })).collect::<Vec<_>>()})
    .to_string();
    picker.preview_extras.insert(
        id,
        PreviewExtras {
            todos_payload: Some(payload.clone()),
            ..Default::default()
        },
    );
    terminal.draw(|frame| picker.render(frame)).unwrap();
    let (lines, _) = super::super::super::ui::viewport::pinned_todo_band_lines_for_payload(
        Some(&payload),
        false,
        crate::config::config().display.diff_mode,
        119,
        47,
    );
    assert!(
        !lines.is_empty(),
        "fixture must exercise the pinned todo band"
    );
    let card = picker.reserved_panel_area.unwrap();
    assert_eq!(card, initial_card, "loaded todos must not move the card");
    assert_eq!(card.right(), 118);
    for _ in 0..3 {
        picker.next();
        terminal.draw(|frame| picker.render(frame)).unwrap();
        assert_eq!(picker.reserved_panel_area, Some(initial_card));
        assert_eq!(picker.last_list_area, Some(initial_list));
        picker.previous();
        terminal.draw(|frame| picker.render(frame)).unwrap();
        assert_eq!(picker.reserved_panel_area, Some(initial_card));
        assert_eq!(picker.last_list_area, Some(initial_list));
    }
}

#[test]
fn resume_canvas_uses_right_columns_below_search_card() {
    let mut session = make_session("canvas-flow", "canvas", false, SessionStatus::Closed);
    session.messages_preview = vec![PreviewMessage {
        role: "assistant".into(),
        content: (0..80)
            .map(|_| "W".repeat(115))
            .collect::<Vec<_>>()
            .join("\n"),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: None,
        timestamp: None,
    }];
    let mut picker = SessionPicker::new(vec![session]);
    picker.auto_scroll_preview = false;
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 60)).unwrap();
    terminal.draw(|frame| picker.render(frame)).unwrap();
    let panel = picker.reserved_panel_area.unwrap();
    let buffer = terminal.backend().buffer();
    assert!(
        (panel.bottom()..58).any(|y| (panel.x..115).any(|x| buffer[(x, y)].symbol() == "W")),
        "Transcript must reclaim the right-side columns below the bounded card"
    );
    let search: String = (panel.x..panel.right())
        .map(|x| buffer[(x, panel.y)].symbol())
        .collect();
    assert!(search.contains("Search sessions"));
}

#[test]
fn preview_cache_preserves_live_tool_metadata_and_invalidates_on_changes() {
    let mut session = make_session(
        "render-parity",
        "render-parity",
        false,
        SessionStatus::Closed,
    );
    session.messages_preview.push(PreviewMessage {
        role: "tool".into(),
        content: "Build succeeded".into(),
        tool_calls: Vec::new(),
        duration_secs: Some(2.3),
        title: Some("Saved tool title".into()),
        tool_data: Some(crate::message::ToolCall {
            id: "timed-preview".into(),
            name: "bash".into(),
            input: serde_json::json!({"command": "echo hello"}),
            intent: Some("Check saved timing".into()),
            thought_signature: None,
        }),
        timestamp: None,
    });
    let picker = SessionPicker::new(vec![session.clone()]);
    let mode = crate::config::DiffDisplayMode::Off;
    let hash = picker.preview_content_hash(&session, mode);
    for width in [8, 30, 80] {
        let inner = Rect::new(0, 0, width, 20);
        let key = PreviewCacheKey {
            content_hash: hash,
            inner_width: width,
            inner_height: 20,
            diff_mode: mode,
            search_query: String::new(),
        };
        let cache = picker.build_preview_cache(&session, inner, inner, key, Alignment::Left, mode);
        let messages: Vec<_> = session
            .messages_preview
            .iter()
            .map(|msg| DisplayMessage {
                role: msg.role.clone(),
                content: msg.content.clone(),
                tool_calls: msg.tool_calls.clone(),
                duration_secs: msg.duration_secs,
                title: msg.title.clone(),
                tool_data: msg.tool_data.clone(),
            })
            .collect();
        assert_eq!(cache.messages.len(), messages.len(), "width={width}");
        let last = cache.messages.last().unwrap();
        assert_eq!(last.duration_secs, Some(2.3));
        assert_eq!(last.title.as_deref(), Some("Saved tool title"));
        assert_eq!(
            last.tool_data.as_ref().map(|t| t.name.as_str()),
            Some("bash")
        );
    }
    session.messages_preview.last_mut().unwrap().duration_secs = Some(4.5);
    let changed_timing = picker.preview_content_hash(&session, mode);
    assert_ne!(hash, changed_timing);
    session.messages_preview.last_mut().unwrap().title = Some("Updated title".into());
    assert_ne!(changed_timing, picker.preview_content_hash(&session, mode));
}

#[test]
fn completed_empty_preview_is_not_loaded_again() {
    let mut session = make_session("empty_preview", "empty", false, SessionStatus::Closed);
    session.messages_preview.clear();
    let mut picker = SessionPicker::new(vec![session]);
    let (tx, rx) = std::sync::mpsc::channel();
    picker.pending_preview_load = Some(PendingSessionPreviewLoad {
        session_id: "empty_preview".into(),
        receiver: rx,
    });
    tx.send(Some(LoadedSessionPreview {
        messages: vec![],
        search: None,
        extras: PreviewExtras::default(),
    }))
    .unwrap();
    assert!(picker.poll_preview_load());
    picker.ensure_selected_preview_loading();
    assert!(picker.pending_preview_load.is_none());
}

#[test]
fn failed_preview_is_not_retried_or_rendered_as_loading() {
    let mut session = make_session("failed_preview", "failed", false, SessionStatus::Closed);
    session.messages_preview.clear();
    let mut picker = SessionPicker::new(vec![session]);
    let (tx, rx) = std::sync::mpsc::channel();
    picker.pending_preview_load = Some(PendingSessionPreviewLoad {
        session_id: "failed_preview".into(),
        receiver: rx,
    });
    tx.send(None).unwrap();
    assert!(picker.poll_preview_load());
    picker.ensure_selected_preview_loading();
    assert!(picker.pending_preview_load.is_none());
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(160, 30)).unwrap();
    terminal.draw(|frame| picker.render(frame)).unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(text.contains("Preview unavailable"));
    assert!(!text.contains("Loading preview"));
}
