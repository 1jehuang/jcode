use super::*;

#[test]
fn test_prepare_messages_live_batch_rows_do_not_soft_wrap_on_narrow_width() {
    let state = TestState {
        display_messages: vec![DisplayMessage::user("build it")],
        status: ProcessingStatus::RunningTool("batch".to_string()),
        anim_elapsed: 0.0,
        batch_progress: Some(crate::bus::BatchProgress {
            session_id: "s".to_string(),
            tool_call_id: "tc".to_string(),
            total: 1,
            completed: 0,
            last_completed: None,
            running: vec![ToolCall {
                id: "batch-1-bash".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({
                    "command": "cargo test --package jcode --lib tui::ui::tests::render_tool_message_batch_rows_do_not_soft_wrap_on_narrow_width -- --nocapture"
                }),
                intent: None,
            }],
            subcalls: vec![crate::bus::BatchSubcallProgress {
                index: 1,
                tool_call: ToolCall {
                    id: "batch-1-bash".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({
                        "command": "cargo test --package jcode --lib tui::ui::tests::render_tool_message_batch_rows_do_not_soft_wrap_on_narrow_width -- --nocapture"
                    }),
                    intent: None,
                },
                state: crate::bus::BatchSubcallState::Running,
            }],
        }),
        ..Default::default()
    };

    let prepared = prepare::prepare_messages(&state, 34, 20);
    let rendered: Vec<String> = prepared
        .materialize_all_lines()
        .iter()
        .map(extract_line_text)
        .collect();

    let batch_rows: Vec<&String> = rendered
        .iter()
        .filter(|line| line.contains("batch") || line.contains("bash $ cargo"))
        .collect();
    assert!(batch_rows.len() >= 2, "rendered={rendered:?}");
    assert!(
        batch_rows.iter().all(|line| line.width() <= 33),
        "rendered={rendered:?}"
    );
    assert!(
        batch_rows.iter().any(|line| line.contains('…')),
        "rendered={rendered:?}"
    );
}

#[test]
fn test_prepare_messages_centered_live_batch_rows_keep_dedicated_padding_span() {
    let state = TestState {
        centered_mode: true,
        display_messages: vec![DisplayMessage::user("build it")],
        status: ProcessingStatus::RunningTool("batch".to_string()),
        anim_elapsed: 0.0,
        batch_progress: Some(crate::bus::BatchProgress {
            session_id: "s".to_string(),
            tool_call_id: "tc".to_string(),
            total: 1,
            completed: 0,
            last_completed: None,
            running: vec![ToolCall {
                id: "batch-1-bash".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({
                    "command": "cargo test --package jcode --lib tui::ui::tests::render_tool_message_batch_rows_do_not_soft_wrap_on_narrow_width -- --nocapture --exact with-extra-flags-and-output-to-stretch-the-line"
                }),
                intent: None,
            }],
            subcalls: vec![crate::bus::BatchSubcallProgress {
                index: 1,
                tool_call: ToolCall {
                    id: "batch-1-bash".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({
                        "command": "cargo test --package jcode --lib tui::ui::tests::render_tool_message_batch_rows_do_not_soft_wrap_on_narrow_width -- --nocapture --exact with-extra-flags-and-output-to-stretch-the-line"
                    }),
                    intent: None,
                },
                state: crate::bus::BatchSubcallState::Running,
            }],
        }),
        ..Default::default()
    };

    let prepared = prepare::prepare_messages(&state, 120, 20);
    let prepared_lines = prepared.materialize_all_lines();
    let batch_rows: Vec<&Line<'static>> = prepared_lines
        .iter()
        .filter(|line| {
            let text = extract_line_text(line);
            text.contains("batch") || text.contains("bash")
        })
        .collect();
    let rendered: Vec<String> = batch_rows
        .iter()
        .map(|line| extract_line_text(line))
        .collect();

    assert!(batch_rows.len() >= 2, "rendered={rendered:?}");
    for line in batch_rows {
        let Some(first_span) = line.spans.first() else {
            panic!("missing spans: {rendered:?}");
        };
        assert!(
            !first_span.content.is_empty() && first_span.content.chars().all(|ch| ch == ' '),
            "expected a dedicated padding span for centered live batch rows: {rendered:?}"
        );
    }
}

#[test]
fn test_prepare_messages_shows_live_batch_progress_in_chat_history() {
    let state = TestState {
        display_messages: vec![DisplayMessage {
            role: "user".to_string(),
            content: "build it".to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        }],
        status: ProcessingStatus::RunningTool("batch".to_string()),
        anim_elapsed: 0.0,
        batch_progress: Some(crate::bus::BatchProgress {
            session_id: "s".to_string(),
            tool_call_id: "tc".to_string(),
            total: 2,
            completed: 1,
            last_completed: Some("read".to_string()),
            running: vec![ToolCall {
                id: "batch-2-bash".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"command": "cargo build --release --workspace"}),
                intent: None,
            }],
            subcalls: vec![
                crate::bus::BatchSubcallProgress {
                    index: 1,
                    tool_call: ToolCall {
                        id: "batch-1-read".to_string(),
                        name: "read".to_string(),
                        input: serde_json::json!({"file_path": "Cargo.toml"}),
                        intent: None,
                    },
                    state: crate::bus::BatchSubcallState::Succeeded,
                },
                crate::bus::BatchSubcallProgress {
                    index: 2,
                    tool_call: ToolCall {
                        id: "batch-2-bash".to_string(),
                        name: "bash".to_string(),
                        input: serde_json::json!({"command": "cargo build --release --workspace"}),
                        intent: None,
                    },
                    state: crate::bus::BatchSubcallState::Running,
                },
            ],
        }),
        ..Default::default()
    };

    let prepared = prepare::prepare_messages(&state, 100, 30);
    let rendered: Vec<String> = prepared
        .materialize_all_lines()
        .iter()
        .map(extract_line_text)
        .collect();

    assert!(
        rendered
            .iter()
            .any(|line| line.contains("⠂ batch · 1/2 done")),
        "missing live batch header in {:?}",
        rendered
    );
    assert!(
        rendered.iter().any(|line| line.contains("… 1 completed")),
        "missing completed subcall summary in {:?}",
        rendered
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("⠂ bash $ cargo build --release --workspace")),
        "missing running batch subcall in {:?}",
        rendered
    );
    assert!(
        rendered
            .iter()
            .all(|line| !line.contains("#1") && !line.contains("#2")),
        "live batch rows should align with completed rows in {:?}",
        rendered
    );
}

#[test]
fn test_prepare_messages_places_live_batch_after_committed_assistant_text() {
    let _guard = crate::storage::lock_test_env();
    clear_test_render_state_for_tests();
    let state = TestState {
        display_messages: vec![
            DisplayMessage::user("build it"),
            DisplayMessage::assistant("Let me inspect the relevant files first."),
        ],
        status: ProcessingStatus::RunningTool("batch".to_string()),
        anim_elapsed: 0.0,
        batch_progress: Some(crate::bus::BatchProgress {
            session_id: "s".to_string(),
            tool_call_id: "tc".to_string(),
            total: 1,
            completed: 0,
            last_completed: None,
            running: vec![ToolCall {
                id: "batch-1-read".to_string(),
                name: "read".to_string(),
                input: serde_json::json!({"file_path": "src/main.rs"}),
                intent: None,
            }],
            subcalls: vec![crate::bus::BatchSubcallProgress {
                index: 1,
                tool_call: ToolCall {
                    id: "batch-1-read".to_string(),
                    name: "read".to_string(),
                    input: serde_json::json!({"file_path": "src/main.rs"}),
                    intent: None,
                },
                state: crate::bus::BatchSubcallState::Running,
            }],
        }),
        ..Default::default()
    };

    let prepared = prepare::prepare_messages(&state, 100, 30);
    let rendered: Vec<String> = prepared
        .materialize_all_lines()
        .iter()
        .map(extract_line_text)
        .collect();

    let assistant_idx = rendered
        .iter()
        .position(|line| line.contains("Let me inspect the relevant files first."))
        .expect("missing assistant text");
    let batch_idx = rendered
        .iter()
        .position(|line| line.contains("batch · 0/1 done"))
        .expect("missing live batch progress");

    assert!(
        assistant_idx < batch_idx,
        "assistant text should render before live batch block in {:?}",
        rendered
    );
}

#[test]
fn test_prepare_messages_live_batch_spinner_advances_between_frames() {
    let batch_progress = crate::bus::BatchProgress {
        session_id: "s".to_string(),
        tool_call_id: "tc".to_string(),
        total: 1,
        completed: 0,
        last_completed: None,
        running: vec![ToolCall {
            id: "batch-1-bash".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({"command": "sleep 1"}),
            intent: None,
        }],
        subcalls: vec![crate::bus::BatchSubcallProgress {
            index: 1,
            tool_call: ToolCall {
                id: "batch-1-bash".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"command": "sleep 1"}),
                intent: None,
            },
            state: crate::bus::BatchSubcallState::Running,
        }],
    };

    let first = TestState {
        status: ProcessingStatus::RunningTool("batch".to_string()),
        anim_elapsed: 0.0,
        batch_progress: Some(batch_progress.clone()),
        ..Default::default()
    };
    let second = TestState {
        status: ProcessingStatus::RunningTool("batch".to_string()),
        anim_elapsed: 0.1,
        batch_progress: Some(batch_progress),
        ..Default::default()
    };

    let first_rendered: Vec<String> = prepare::prepare_messages(&first, 100, 20)
        .materialize_all_lines()
        .iter()
        .map(extract_line_text)
        .collect();
    let second_rendered: Vec<String> = prepare::prepare_messages(&second, 100, 20)
        .materialize_all_lines()
        .iter()
        .map(extract_line_text)
        .collect();

    assert!(
        first_rendered
            .iter()
            .any(|line| line.contains("⠂ batch · 0/1 done")),
        "expected first spinner frame in {:?}",
        first_rendered
    );
    assert!(
        second_rendered
            .iter()
            .any(|line| line.contains("⠆ batch · 0/1 done")),
        "expected second spinner frame in {:?}",
        second_rendered
    );
    assert_ne!(
        first_rendered, second_rendered,
        "batch progress should rerender as spinner advances"
    );
}

#[test]
fn test_prepare_messages_live_batch_centered_mode_uses_left_aligned_padding() {
    let state = TestState {
        centered_mode: true,
        status: ProcessingStatus::RunningTool("batch".to_string()),
        anim_elapsed: 0.0,
        batch_progress: Some(crate::bus::BatchProgress {
            session_id: "s".to_string(),
            tool_call_id: "tc".to_string(),
            total: 1,
            completed: 0,
            last_completed: None,
            running: vec![ToolCall {
                id: "batch-1-read".to_string(),
                name: "read".to_string(),
                input: serde_json::json!({"file_path": "Cargo.toml"}),
                intent: None,
            }],
            subcalls: vec![crate::bus::BatchSubcallProgress {
                index: 1,
                tool_call: ToolCall {
                    id: "batch-1-read".to_string(),
                    name: "read".to_string(),
                    input: serde_json::json!({"file_path": "Cargo.toml"}),
                    intent: None,
                },
                state: crate::bus::BatchSubcallState::Running,
            }],
        }),
        ..Default::default()
    };

    let prepared = prepare::prepare_messages(&state, 100, 20);
    let prepared_lines = prepared.materialize_all_lines();
    let batch_lines: Vec<&Line<'static>> = prepared_lines
        .iter()
        .filter(|line| {
            let text = extract_line_text(line);
            text.contains("batch") || text.contains("Cargo.toml")
        })
        .collect();

    assert!(!batch_lines.is_empty(), "expected centered batch lines");
    for line in batch_lines {
        assert_eq!(
            line.alignment,
            Some(Alignment::Left),
            "centered live batch lines should be left-aligned with padding"
        );
        assert!(
            line.spans
                .first()
                .is_some_and(|span| span.content.starts_with(' ')),
            "centered live batch lines should start with padding"
        );
    }
}

#[test]
fn test_prepare_messages_centers_meta_footer_in_centered_mode() {
    let state = TestState {
        centered_mode: true,
        display_messages: vec![
            DisplayMessage::assistant("Done."),
            DisplayMessage {
                role: "meta".to_string(),
                content: "1.2s · ↑12 ↓34".to_string(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            },
        ],
        ..Default::default()
    };

    let prepared = prepare::prepare_messages(&state, 100, 20);
    let prepared_lines = prepared.materialize_all_lines();
    let footer = prepared_lines
        .iter()
        .find(|line| extract_line_text(line).contains("↑12 ↓34"))
        .expect("missing meta footer line");

    assert_eq!(
        footer.alignment,
        Some(Alignment::Center),
        "meta footer should stay centered in centered mode"
    );
}

#[test]
fn test_prepare_messages_recomputes_when_streaming_text_changes_same_length() {
    let first = TestState {
        status: ProcessingStatus::Streaming,
        streaming_text: "alpha".to_string(),
        anim_elapsed: 10.0,
        ..Default::default()
    };
    let second = TestState {
        status: ProcessingStatus::Streaming,
        streaming_text: "omega".to_string(),
        anim_elapsed: 10.0,
        ..Default::default()
    };

    let first_rendered: Vec<String> = prepare::prepare_messages(&first, 80, 20)
        .materialize_all_lines()
        .iter()
        .map(extract_line_text)
        .collect();
    let second_rendered: Vec<String> = prepare::prepare_messages(&second, 80, 20)
        .materialize_all_lines()
        .iter()
        .map(extract_line_text)
        .collect();

    assert!(
        first_rendered.iter().any(|line| line.contains("alpha")),
        "expected first streaming text in {:?}",
        first_rendered
    );
    assert!(
        second_rendered.iter().any(|line| line.contains("omega")),
        "expected second streaming text in {:?}",
        second_rendered
    );
    assert_ne!(
        first_rendered, second_rendered,
        "prepared frame cache should invalidate on same-length streaming text changes"
    );
}

#[test]
fn test_prepare_messages_tool_row_refreshes_after_message_version_bump() {
    let tool_call = ToolCall {
        id: "tool-1".to_string(),
        name: "read".to_string(),
        input: serde_json::json!({"file_path": "src/main.rs"}),
        intent: None,
    };

    let placeholder = DisplayMessage {
        role: "tool".to_string(),
        content: "pending".to_string(),
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: Some(tool_call.clone()),
    };
    let final_message = DisplayMessage {
        role: "tool".to_string(),
        content: "x".repeat(7_600),
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: Some(tool_call),
    };

    let first = TestState {
        display_messages: vec![placeholder],
        messages_version: 0,
        ..Default::default()
    };
    let refreshed = TestState {
        display_messages: vec![final_message],
        messages_version: 1,
        ..Default::default()
    };

    let first_rendered: Vec<String> = prepare::prepare_messages(&first, 120, 20)
        .materialize_all_lines()
        .iter()
        .map(extract_line_text)
        .collect();
    let refreshed_rendered: Vec<String> = prepare::prepare_messages(&refreshed, 120, 20)
        .materialize_all_lines()
        .iter()
        .map(extract_line_text)
        .collect();

    assert!(
        first_rendered.iter().any(|line| line.contains("1 tok")),
        "expected initial render to reflect placeholder tool output: {first_rendered:?}"
    );
    assert!(
        refreshed_rendered
            .iter()
            .any(|line| line.contains("1.9k tok")),
        "expected refreshed render to include final tool token badge: {refreshed_rendered:?}"
    );
    assert!(
        refreshed_rendered
            .iter()
            .any(|line| line.contains("✓ read src/main.rs · 1.9k tok")),
        "expected refreshed tool row summary in final render: {refreshed_rendered:?}"
    );
}

#[test]
fn test_prepare_messages_centered_streaming_uses_center_alignment_without_left_padding() {
    let state = TestState {
            centered_mode: true,
            status: ProcessingStatus::Streaming,
            streaming_text: "streaming-block streaming-block streaming-block streaming-block streaming-block streaming-block streaming-block streaming-block".to_string(),
            anim_elapsed: 10.0,
            ..Default::default()
        };

    let prepared = prepare::prepare_messages(&state, 120, 20);
    let prepared_lines = prepared.materialize_all_lines();
    let stream_lines: Vec<&Line<'static>> = prepared_lines
        .iter()
        .filter(|line| extract_line_text(line).contains("streaming-block"))
        .collect();

    assert!(
        stream_lines.len() >= 2,
        "expected wrapped centered streaming lines: {:?}",
        prepared
            .materialize_all_lines()
            .iter()
            .map(extract_line_text)
            .collect::<Vec<_>>()
    );

    for line in stream_lines {
        assert_eq!(
            line.alignment,
            Some(Alignment::Center),
            "centered streaming lines should use center alignment"
        );
        assert_eq!(
            extract_line_text(line)
                .chars()
                .take_while(|c| *c == ' ')
                .count(),
            0,
            "streamed assistant lines should not be manually left padded"
        );
    }
}

#[test]
fn test_prepare_messages_centered_streaming_recenters_structured_markdown_like_final_render() {
    let content = "- stream-centering-alpha\n- stream-centering-beta";

    let streaming = TestState {
        centered_mode: true,
        status: ProcessingStatus::Streaming,
        streaming_text: content.to_string(),
        anim_elapsed: 10.0,
        ..Default::default()
    };
    let finalized = TestState {
        centered_mode: true,
        display_messages: vec![DisplayMessage::assistant(content)],
        ..Default::default()
    };

    let streaming_prepared = prepare::prepare_messages(&streaming, 120, 20);
    let finalized_prepared = prepare::prepare_messages(&finalized, 120, 20);

    let streaming_bullets: Vec<String> = streaming_prepared
        .materialize_all_lines()
        .iter()
        .map(extract_line_text)
        .filter(|line| {
            line.contains("stream-centering-alpha") || line.contains("stream-centering-beta")
        })
        .collect();
    let finalized_bullets: Vec<String> = finalized_prepared
        .materialize_all_lines()
        .iter()
        .map(extract_line_text)
        .filter(|line| {
            line.contains("stream-centering-alpha") || line.contains("stream-centering-beta")
        })
        .collect();

    assert_eq!(
        streaming_bullets.len(),
        2,
        "streaming={streaming_bullets:?}"
    );
    assert_eq!(
        streaming_bullets, finalized_bullets,
        "streaming structured markdown should match finalized centering"
    );
    assert!(
        streaming_bullets[0]
            .chars()
            .take_while(|ch| *ch == ' ')
            .count()
            > 40,
        "expected centered streaming list to keep left padding inside the centered block: {streaming_bullets:?}"
    );
}

#[test]
fn test_render_tool_message_batch_nested_subcall_params_still_render() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "--- [1] grep ---\nok\n\nCompleted: 1 succeeded, 0 failed".to_string(),
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: Some(ToolCall {
            id: "call_batch_2".to_string(),
            name: "batch".to_string(),
            input: serde_json::json!({
                "tool_calls": [
                    {"tool": "grep", "parameters": {"pattern": "TODO", "path": "src"}}
                ]
            }),
            intent: None,
        }),
    };

    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();

    assert_eq!(rendered.len(), 2, "rendered={rendered:?}");
    assert!(
        rendered[0].contains("✓ batch 1 call"),
        "rendered={rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("✓ grep 'TODO' in src")),
        "missing grep subtool in {rendered:?}"
    );
}

#[test]
fn test_render_tool_message_batch_flat_grep_subcall_uses_pattern_and_path() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "--- [1] grep ---\nok\n\nCompleted: 1 succeeded, 0 failed".to_string(),
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: Some(ToolCall {
            id: "call_batch_3".to_string(),
            name: "batch".to_string(),
            input: serde_json::json!({
                "tool_calls": [
                    {"tool": "grep", "pattern": "TODO", "path": "src"}
                ]
            }),
            intent: None,
        }),
    };

    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();

    assert_eq!(rendered.len(), 2, "rendered={rendered:?}");
    assert!(
        rendered[0].contains("✓ batch 1 call"),
        "rendered={rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("✓ grep 'TODO' in src")),
        "missing grep subtool in {rendered:?}"
    );
}

#[test]
fn test_render_tool_message_batch_subcall_lines_alignment_unset() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "--- [1] read ---\nok\n\n--- [2] grep ---\nok\n\nCompleted: 2 succeeded, 0 failed"
            .to_string(),
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: Some(ToolCall {
            id: "call_batch_align".to_string(),
            name: "batch".to_string(),
            input: serde_json::json!({
                "tool_calls": [
                    {"tool": "read", "file_path": "src/session.rs", "offset": 0, "limit": 420},
                    {"tool": "grep", "pattern": "TODO", "path": "src"}
                ]
            }),
            intent: None,
        }),
    };

    // In non-centered mode, lines have no alignment set
    crate::tui::markdown::set_center_code_blocks(false);
    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    for line in &lines {
        assert_eq!(
            line.alignment, None,
            "non-centered tool lines should have no alignment set"
        );
    }

    // In centered mode, lines are left-aligned with padding prepended
    crate::tui::markdown::set_center_code_blocks(true);
    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    for line in &lines {
        assert_eq!(
            line.alignment,
            Some(Alignment::Left),
            "centered tool lines should be left-aligned with padding"
        );
        assert!(
            line.spans[0].content.starts_with(' '),
            "first span should be padding spaces"
        );
    }
    crate::tui::markdown::set_center_code_blocks(false);
}
