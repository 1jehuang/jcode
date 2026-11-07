use super::*;

#[test]
fn blocker_footer_is_cell_bounded_and_clears_when_resolved() {
    let payload = serde_json::json!({"todos": [
        {"id":"release", "content":"Release", "status":"pending", "priority":"high",
         "external_blocker":{"reason":"Jeremy must approve rollout 恢复", "review_at":"2099-01-01T00:00:00Z"}},
        {"id":"device", "content":"Device check", "status":"in_progress", "priority":"high",
         "completely_blocked":{"reason":"Connect an iPhone"}},
        {"id":"done", "content":"Old task", "status":"completed", "priority":"high",
         "completely_blocked":{"reason":"stale reason"}}
    ]}).to_string();
    for width in [0, 1, 8, 24, 80, 160] {
        let lines = messages::todo_blocker_footer(Some(&payload), width);
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|line| line.width() <= usize::from(width)));
        if width > 0 {
            for line in &lines {
                assert_eq!(line.style.fg, Some(Color::Rgb(225, 180, 80)));
            }
        }
        if width >= 80 {
            assert!(
                lines[0]
                    .to_string()
                    .contains("Jeremy must approve rollout 恢复")
            );
            assert!(lines[1].to_string().contains("Connect an iPhone"));
        }
    }
    assert!(messages::todo_blocker_footer(None, 80).is_empty());
    assert!(messages::todo_blocker_footer(Some("invalid"), 80).is_empty());
}

#[test]
fn blocker_footer_renders_immediately_above_input() {
    let _guard = viewport_snapshot_test_lock();
    clear_test_render_state_for_tests();
    let state = TestState {
        input: "composer-marker".into(),
        pinned_todos_payload: Some(
            serde_json::json!({"todos":[{
                "id":"device", "content":"Device check", "status":"pending", "priority":"high",
                "completely_blocked":{"reason":"Connect an iPhone"}
            }]})
            .to_string(),
        ),
        ..Default::default()
    };
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
    terminal.draw(|frame| draw(frame, &state)).unwrap();
    let rows: Vec<String> = terminal
        .backend()
        .buffer()
        .content
        .chunks(120)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect())
        .collect();
    let input_row = rows
        .iter()
        .position(|row| row.contains("composer-marker"))
        .unwrap();
    assert!(
        rows[input_row - 1].contains("Blocked · Device check: Connect an iPhone"),
        "{rows:#?}"
    );
    let blocker_row = &terminal.backend().buffer().content[(input_row - 1) * 120..input_row * 120];
    for cell in blocker_row.iter().take("Blocked".len()) {
        assert_eq!(cell.fg, Color::Rgb(225, 180, 80));
    }
}

#[test]
fn blocked_reason_lives_below_response_not_in_pinned_todos() {
    let _guard = viewport_snapshot_test_lock();
    let reason = "No working hardware audio sink available. 恢复音频 requires an external device.";
    let payload = serde_json::json!({"todos": [{
        "id": "audio", "content": "Restore audio", "status": "in_progress", "priority": "high",
        "stage": 1, "group": "Audio",
        "completely_blocked": {"reason": reason}
    }]})
    .to_string();
    let mut state = TestState {
        display_messages: vec![DisplayMessage {
            role: "assistant".into(),
            content: "The launcher is repaired.".into(),
            tool_calls: Vec::new(),
            duration_secs: None,
            title: None,
            tool_data: None,
        }],
        pinned_todos_payload: Some(payload.clone()),
        ..Default::default()
    };
    let msg = DisplayMessage {
        role: "todos".into(),
        content: payload,
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: None,
    };
    for width in [1, 8, 24, 80, 160] {
        let pinned =
            messages::render_pinned_todos_message(&msg, width, crate::config::DiffDisplayMode::Off);
        assert!(pinned.iter().all(|line| line.width() <= width as usize));
        assert!(
            !pinned
                .iter()
                .any(|line| line.to_string().contains("hardware"))
        );
        if width >= 80 {
            assert!(
                pinned
                    .iter()
                    .any(|line| line.to_string().contains("Restore audio"))
            );
        }
    }
    let prepared = prepare::prepare_messages(&state, 120, 40);
    let text = prepared
        .materialize_all_lines()
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.find("The launcher is repaired.").unwrap() < text.find("Completely blocked:").unwrap()
    );
    assert!(text.contains(reason));
    assert_eq!(text.matches(reason).count(), 1);

    state.pinned_todos_payload = Some(
        serde_json::json!({"todos": [{
            "id": "audio", "content": "Restore audio", "status": "completed", "priority": "high",
            "completely_blocked": {"reason": reason}
        }]})
        .to_string(),
    );
    let prepared = prepare::prepare_messages(&state, 120, 40);
    let text = prepared
        .materialize_all_lines()
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!text.contains(reason));
}
