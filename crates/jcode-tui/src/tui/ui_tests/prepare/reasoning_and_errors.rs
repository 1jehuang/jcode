use super::*;

#[test]
fn test_prepare_messages_renders_reasoning_role_dim_italic_without_sentinel() {
    let _guard = crate::storage::lock_test_env();
    clear_test_render_state_for_tests();

    // A collapsing reasoning message carries sentinel-wrapped dim/italic markup.
    let mut content = String::new();
    content.push_str(&jcode_tui_markdown::reasoning_line_markup(
        "weighing the options",
    ));
    content.push_str(&jcode_tui_markdown::reasoning_line_markup(
        "▸ thought for 3s",
    ));

    let state = TestState {
        display_messages: vec![
            DisplayMessage::user("hi"),
            DisplayMessage::reasoning(content),
        ],
        ..Default::default()
    };

    let prepared = prepare::prepare_messages(&state, 100, 30);
    let lines = prepared.materialize_all_lines();

    // The visible reasoning body is present, dim+italic, and sentinel-free.
    let body = lines
        .iter()
        .find(|l| {
            let joined: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            joined.contains("weighing the options")
        })
        .expect("reasoning body line present");
    let rendered: String = body.spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        !rendered.contains(jcode_tui_markdown::REASONING_SENTINEL),
        "sentinel must be stripped from visible reasoning: {rendered:?}"
    );
    let span = body
        .spans
        .iter()
        .find(|s| s.content.as_ref().contains("weighing"))
        .expect("body span");
    assert!(
        span.style
            .add_modifier
            .contains(ratatui::style::Modifier::ITALIC),
        "reasoning body should be italic: {:?}",
        span.style
    );

    // The summary line is present too.
    assert!(
        lines.iter().any(|l| {
            let joined: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            joined.contains("thought for 3s")
        }),
        "summary line should render"
    );
}

#[test]
fn test_prepare_messages_renders_anchored_reasoning_message_in_flow() {
    let _guard = crate::storage::lock_test_env();
    clear_test_render_state_for_tests();

    // Anchored reasoning traces are ordinary display messages in the body:
    // they render dim+italic (sentinel stripped) between surrounding entries.
    let mut trace = String::new();
    trace.push_str(&jcode_tui_markdown::reasoning_line_markup(
        "anchored thinking",
    ));

    let state = TestState {
        display_messages: vec![
            DisplayMessage::user("hi"),
            DisplayMessage::reasoning(trace),
            DisplayMessage::assistant("Answer body"),
        ],
        ..Default::default()
    };

    let prepared = prepare::prepare_messages(&state, 100, 30);
    let lines = prepared.materialize_all_lines();
    let joined: Vec<String> = lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();

    let reasoning_idx = joined
        .iter()
        .position(|l| l.contains("anchored thinking"))
        .expect("anchored reasoning rendered");
    let answer_idx = joined
        .iter()
        .position(|l| l.contains("Answer body"))
        .expect("answer rendered");
    assert!(
        reasoning_idx < answer_idx,
        "anchored reasoning renders in transcript order: {joined:?}"
    );
    // Sentinel is stripped from the visible reasoning text.
    assert!(
        !joined[reasoning_idx].contains(jcode_tui_markdown::REASONING_SENTINEL),
        "sentinel must be stripped: {:?}",
        joined[reasoning_idx]
    );
}

#[test]
fn test_prepare_messages_restored_history_matches_live_tool_rows() {
    // History restore populates assistant `tool_calls`; live turns do not.
    // The prepared transcript must be identical so a reload never adds a
    // purple `tool: bash` summary above the real tool rows.
    let tool_row = DisplayMessage {
        role: "tool".to_string(),
        content: "ok".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(ToolCall {
            id: "call-bash-restore".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({"command": "git status"}),
            intent: Some("Check repo state".to_string()),
            thought_signature: None,
        }),
    };
    let assistant = |tool_calls: Vec<String>| DisplayMessage {
        role: "assistant".to_string(),
        content: "Checking the repo.".to_string(),
        tool_calls,
        duration_secs: None,
        title: None,
        tool_data: None,
    };
    let render = |messages: Vec<DisplayMessage>| {
        let state = TestState {
            display_messages: messages,
            ..Default::default()
        };
        prepare::prepare_messages(&state, 110, 30)
            .materialize_all_lines()
            .iter()
            .map(extract_line_text)
            .collect::<Vec<_>>()
    };
    let live = render(vec![assistant(Vec::new()), tool_row.clone()]);
    let restored = render(vec![assistant(vec!["bash".to_string()]), tool_row]);
    assert!(
        restored.iter().all(|line| !line.contains("tool: ")),
        "restored={restored:#?}"
    );
    assert!(
        restored
            .iter()
            .any(|line| line.contains("bash · Check repo state")),
        "restored={restored:#?}"
    );
    assert_eq!(live, restored);
}

#[test]
fn test_prepare_messages_keeps_multiline_error_rows() {
    let state = TestState {
        display_messages: vec![DisplayMessage::error(
            "/cloud failed. Nothing moved.\n\nError: remote failed\nCaused by:\n    host down",
        )],
        ..Default::default()
    };
    let rendered = prepare::prepare_messages(&state, 110, 30)
        .materialize_all_lines()
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>();
    let all = rendered.join("\n");
    let first = rendered
        .iter()
        .position(|line| line.contains("✗ /cloud failed. Nothing moved."))
        .unwrap_or_else(|| panic!("missing headline: {all}"));
    let error = rendered
        .iter()
        .position(|line| line.contains("Error: remote failed"))
        .unwrap_or_else(|| panic!("missing error row: {all}"));
    let cause = rendered
        .iter()
        .position(|line| line.contains("    host down"))
        .unwrap_or_else(|| panic!("missing cause row: {all}"));
    assert!(first < error && error < cause, "rows collapsed: {all}");
    assert!(!rendered[first].contains("Error:"), "rows collapsed: {all}");
}

#[test]
fn cloud_failure_reason_is_visible_in_rendered_chat() {
    for width in [40, 100] {
        let state = TestState {
            display_messages: vec![DisplayMessage::error(
                "/cloud failed. Reason: Unreturned cloud changes would be overwritten. Recover the earlier session first.\n\nError: remote receive failed",
            )],
            ..Default::default()
        };
        let rendered = prepare::prepare_messages(&state, width, 40)
            .materialize_all_lines()
            .iter()
            .map(extract_line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("Reason:"), "{rendered}");
        assert!(rendered.contains("session first."), "{rendered}");
    }
}
