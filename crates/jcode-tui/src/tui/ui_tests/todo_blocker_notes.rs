use super::*;

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
