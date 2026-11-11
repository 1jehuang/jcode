use super::*;

fn tool(code: &str) -> ToolCall {
    ToolCall {
        id: "codemode-preview".into(),
        name: "codemode".into(),
        input: serde_json::json!({"code": code}),
        intent: Some("Inspect project files".into()),
        thought_signature: None,
    }
}

#[test]
fn codemode_preview_preserves_source_and_bounds_visual_rows() {
    let _guard = viewport_snapshot_test_lock();
    let source = "const value = 1\n  return value";
    let lines = tools_ui::codemode_preview(&tool(source), 80);
    assert!(lines[0].spans.iter().any(|span| span.style.fg.is_some()));
    assert_eq!(
        lines.iter().map(extract_line_text).collect::<Vec<_>>(),
        vec!["    const value = 1", "      return value"]
    );
    let long = (0..10)
        .map(|i| format!("return {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let lines = tools_ui::codemode_preview(&tool(&long), 80);
    assert_eq!(lines.len(), 7);
    assert!(extract_line_text(lines.last().unwrap()).contains("4 more lines"));
    assert_eq!(
        tools_ui::codemode_preview(&tool(&"return 1\n".repeat(6)), 80).len(),
        6
    );
    for width in [1, 2, 4, 8, 20, 40] {
        let lines = tools_ui::codemode_preview(&tool(&"const 世界 = 'abcdef'; ".repeat(30)), width);
        assert!(lines.len() <= 7);
        assert!(
            lines.iter().all(|line| line.width() <= width),
            "{width}: {lines:?}"
        );
    }
    let lines = tools_ui::codemode_preview(&tool(&"x".repeat(200)), 20);
    assert!(extract_line_text(lines.last().unwrap()).contains("code continues"));
    assert!(tools_ui::codemode_preview(&tool(""), 80).is_empty());
    assert!(tools_ui::codemode_preview(&tool("return 1"), 0).is_empty());
    let mut other = tool("return 1");
    other.name = "bash".into();
    assert!(tools_ui::codemode_preview(&other, 80).is_empty());
}

#[test]
fn codemode_preview_visible_on_success_and_failure() {
    let _guard = viewport_snapshot_test_lock();
    for content in ["result: 1", "error: TypeError: example"] {
        let msg = DisplayMessage {
            role: "tool".into(),
            content: content.into(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: Some(tool("return 1")),
        };
        let lines = messages::render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Off);
        let all = lines
            .iter()
            .map(extract_line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains("return 1"), "{all}");
    }
}

#[test]
fn codemode_preview_live_updates_and_disappears_when_idle() {
    let _guard = viewport_snapshot_test_lock();
    for centered in [false, true] {
        let mut state = TestState {
            status: ProcessingStatus::RunningTool("codemode".into()),
            streaming_tools: vec![tool("return 1")],
            centered_mode: centered,
            ..Default::default()
        };
        let render = |state: &TestState| {
            prepare::prepare_messages(state, 80, 30)
                .materialize_all_lines()
                .iter()
                .map(extract_line_text)
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert!(render(&state).contains("return 1"));
        state.streaming_tools[0] = tool("return 2");
        let updated = render(&state);
        assert!(updated.contains("return 2"), "{updated}");
        assert!(!updated.contains("return 1"));
        state.status = ProcessingStatus::Idle;
        assert!(!render(&state).contains("return 2"));
    }
}
