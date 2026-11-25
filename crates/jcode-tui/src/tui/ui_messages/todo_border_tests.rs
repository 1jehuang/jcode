#[test]
fn render_todos_message_shows_grouped_card_with_status_glyphs() {
    fn todo(id: &str, content: &str, status: &str, group: Option<&str>) -> crate::todo::TodoItem {
        crate::todo::TodoItem {
            lane_after: None,
            lane_owner: None,
            id: id.to_string(),
            content: content.to_string(),
            status: status.to_string(),
            priority: "high".to_string(),
            group: group.map(str::to_string),
            confidence: Some(crate::todo::ConfidenceState::from_legacy_score(80)),
            completion_confidence: (status == "completed")
                .then_some(crate::todo::ConfidenceState::from_legacy_score(95)),
            confidence_history: Vec::new(),
            waiting_reason: None,
            requires_user_auth: None,
            waiting_on: Vec::new(),
            blocked_by: Vec::new(),
            assigned_to: None,
            stage: None,
            details: None,
        }
    }

    let todos = vec![
        todo("1", "Wire the hotkey", "completed", Some("todo card")),
        todo("2", "Render the card", "in_progress", Some("todo card")),
        todo("3", "Unrelated cleanup", "pending", None),
    ];
    let msg = DisplayMessage::todos(
        serde_json::json!({
            "todos": todos,
            "plan": {
                "user_intention": "Keep overall progress visible",
                "understands_user_intent": "clear"
            }
        })
        .to_string(),
    );

    let lines = render_todos_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let intent_row = lines
        .iter()
        .position(|line| extract_line_text(line).contains("Keep overall progress visible"))
        .unwrap();
    assert_eq!(
        extract_line_text(&lines[intent_row + 1])
            .trim()
            .trim_matches('│')
            .trim(),
        "●●○"
    );
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("Todos 1/3"), "{plain}");
    assert!(plain.contains("todo card"), "{plain}");
    assert!(plain.contains("other"), "{plain}");
    let todo_card_header = lines
        .iter()
        .map(extract_line_text)
        .find(|line| line.contains("todo card"))
        .unwrap();
    assert_eq!(todo_card_header.matches('●').count(), 2, "{plain}");
    assert_eq!(todo_card_header.matches('○').count(), 0, "{plain}");
    let other_header = lines
        .iter()
        .map(extract_line_text)
        .find(|line| line.contains("other"))
        .unwrap();
    assert_eq!(other_header.matches('○').count(), 1, "{plain}");
    assert!(plain.contains("✓ Wire the hotkey"), "{plain}");
    assert!(plain.contains("● Render the card"), "{plain}");
    assert!(plain.contains("○ Unrelated cleanup"), "{plain}");
    // Confidence remains metadata rather than a visible item suffix.
    assert!(!plain.contains("plausible"), "{plain}");
    // Priority remains metadata and is not repeated in the visible item label.
    assert!(!plain.contains("(high)"), "{plain}");
    assert!(plain.contains('╭'), "todo card should be boxed:\n{plain}");
    assert!(plain.contains('╰'), "todo card should be boxed:\n{plain}");
}

#[test]
fn todo_border_counts_and_rows_fit_the_available_width() {
    let msg = DisplayMessage::todos(
        serde_json::json!([
            {"id":"1", "content":"boxed done", "status":"completed", "priority":"high"},
            {"id":"2", "content":"boxed pending", "status":"pending", "priority":"high"}
        ])
        .to_string(),
    );
    for width in [0, 1, 6, 12, 16, 40, 80, 200] {
        let lines = render_todos_message(&msg, width, crate::config::DiffDisplayMode::Off);
        assert!(
            lines.iter().all(|line| line.width() <= usize::from(width)),
            "{width}: {lines:?}"
        );
        if width >= 16 {
            let rows: Vec<String> = lines.iter().map(extract_line_text).collect();
            assert!(
                rows.first()
                    .is_some_and(|row| row.contains(" Todos 1/2 ") && row.contains('╭')),
                "{rows:?}"
            );
            assert!(rows.last().is_some_and(|row| row.contains('╰')), "{rows:?}");
            if width >= 40 {
                assert!(
                    rows.iter().any(|row| row.contains("boxed pending")),
                    "{rows:?}"
                );
            }
        }
    }
}
