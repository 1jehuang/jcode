use super::*;

fn chat_swarm_member(session_id: &str) -> crate::protocol::SwarmMemberStatus {
    crate::protocol::SwarmMemberStatus {
        session_id: session_id.to_string(),
        friendly_name: Some("cow".to_string()),
        status: "running".to_string(),
        detail: None,
        task_label: Some("API reviewer".to_string()),
        role: Some("agent".to_string()),
        is_headless: Some(true),
        live_attachments: None,
        status_age_secs: Some(0),
        output_tail: None,
        report_back_to_session_id: Some("coordinator".to_string()),
        todo_progress: Some((1, 3)),
        todo_items: vec![
            crate::protocol::SwarmTodoItem {
                content: "Inspect authentication middleware".to_string(),
                status: "completed".to_string(),
                tool_intents: Vec::new(),
            },
            crate::protocol::SwarmTodoItem {
                content: "Test token refresh flow".to_string(),
                status: "in_progress".to_string(),
                tool_intents: vec![crate::protocol::SwarmToolIntent {
                    tool_call_id: "call-tests".to_string(),
                    tool_name: "bash".to_string(),
                    intent: "Run targeted authentication tests".to_string(),
                    status: "running".to_string(),
                    progress: Some(crate::protocol::SwarmToolProgress {
                        current: 27,
                        total: 43,
                        unit: Some("tests".to_string()),
                    }),
                }],
            },
            crate::protocol::SwarmTodoItem {
                content: "Report findings".to_string(),
                status: "pending".to_string(),
                tool_intents: Vec::new(),
            },
        ],
        runtime: crate::protocol::SwarmMemberRuntime {
            model: Some("openai:gpt-5.6-sol".to_string()),
            provider: Some("OpenAI".to_string()),
            auth_method: Some("OAuth".to_string()),
            effort: Some("high".to_string()),
            elapsed_secs: Some(18),
        },
    }
}

fn nested_chat_swarm_member(
    session_id: &str,
    parent_id: &str,
    name: &str,
    label: &str,
    todo: &str,
) -> crate::protocol::SwarmMemberStatus {
    let mut member = chat_swarm_member(session_id);
    member.friendly_name = Some(name.to_string());
    member.task_label = Some(label.to_string());
    member.report_back_to_session_id = Some(parent_id.to_string());
    member.todo_progress = Some((0, 1));
    member.todo_items = vec![crate::protocol::SwarmTodoItem {
        content: todo.to_string(),
        status: "in_progress".to_string(),
        tool_intents: Vec::new(),
    }];
    member
}

#[test]
fn test_prepare_messages_places_live_swarm_card_beneath_matching_spawn_tool_call() {
    let session_id = "spawned-session-123";
    let state = TestState {
        display_messages: vec![DisplayMessage {
            role: "tool".to_string(),
            content: format!("Spawned new agent: {session_id}"),
            tool_calls: Vec::new(),
            duration_secs: None,
            title: None,
            tool_data: Some(ToolCall {
                id: "call-spawn".to_string(),
                name: "swarm".to_string(),
                input: serde_json::json!({
                    "action": "spawn",
                    "label": "API reviewer",
                    "prompt": "Review the authentication flow"
                }),
                intent: Some("Spawn an authentication reviewer".to_string()),
                thought_signature: None,
            }),
        }],
        swarm_members: vec![chat_swarm_member(session_id)],
        anim_elapsed: 0.16,
        ..Default::default()
    };

    let prepared = prepare::prepare_messages(&state, 110, 30);
    let rendered = prepared
        .materialize_all_lines()
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>();
    let tool_row = rendered
        .iter()
        .position(|line| line.contains("Spawn an authentication reviewer"))
        .expect("missing swarm tool row");
    let card_row = rendered
        .iter()
        .position(|line| line.contains("🐄 ● API reviewer"))
        .expect("missing live member card");
    let all = rendered.join("\n");

    assert_eq!(
        card_row,
        tool_row + 1,
        "card must directly follow tool: {all}"
    );
    // Transcript cards are deliberately stable one-liners: no elapsed time,
    // todos, or tool progress (those live on the dedicated swarm page), so
    // old chat rows do not move while the user is reading them.
    assert!(
        rendered[card_row].contains("🐄 ● API reviewer · Working"),
        "stable card line missing: {all}"
    );
    assert!(
        !all.contains("00:18"),
        "elapsed time must not render in transcript cards: {all}"
    );
    assert!(
        !all.contains("Test token refresh flow"),
        "todos must not render in transcript cards: {all}"
    );
    assert!(
        !all.contains("Run targeted authentication tests"),
        "tool progress must not render in transcript cards: {all}"
    );
}

#[test]
fn test_prepare_messages_keeps_transcript_card_stable_with_nested_descendants() {
    let root_id = "root-reviewer";
    let child_a = nested_chat_swarm_member(
        "child-auth-tests",
        root_id,
        "otter",
        "Auth tests",
        "Run authentication integration tests",
    );
    let child_b = nested_chat_swarm_member(
        "child-token-audit",
        root_id,
        "owl",
        "Token audit",
        "Audit JWT validation",
    );
    let grandchild = nested_chat_swarm_member(
        "grandchild-race-check",
        "child-token-audit",
        "turtle",
        "Race-condition check",
        "Fuzz refresh-token rotation",
    );
    let unrelated = nested_chat_swarm_member(
        "unrelated-worker",
        "another-root",
        "fox",
        "Unrelated worker",
        "Must not appear here",
    );
    let state = TestState {
        display_messages: vec![DisplayMessage {
            role: "tool".to_string(),
            content: format!("Spawned new agent: {root_id}"),
            tool_calls: Vec::new(),
            duration_secs: None,
            title: None,
            tool_data: Some(ToolCall {
                id: "call-nested-spawn".to_string(),
                name: "swarm".to_string(),
                input: serde_json::json!({"action": "spawn", "label": "API reviewer"}),
                intent: Some("Spawn an authentication reviewer".to_string()),
                thought_signature: None,
            }),
        }],
        transcript_swarm_members: Some(vec![
            chat_swarm_member(root_id),
            child_a,
            child_b,
            grandchild,
            unrelated,
        ]),
        ..Default::default()
    };

    let rendered = prepare::prepare_messages(&state, 110, 40)
        .materialize_all_lines()
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>();
    let all = rendered.join("\n");
    // The transcript deliberately renders only the spawned member's stable
    // one-line card. Descendant trees, todos, and current work live on the
    // dedicated live swarm page so chat rows do not churn.
    let root = rendered
        .iter()
        .position(|line| line.contains("🐄 ● API reviewer"))
        .unwrap_or_else(|| panic!("missing root card: {all}"));
    assert!(
        rendered[root].contains("Working"),
        "root card should show status: {all}"
    );
    assert!(
        !all.contains("Auth tests") && !all.contains("Token audit"),
        "descendants must not render in the transcript: {all}"
    );
    assert!(
        !all.contains("Run authentication integration tests")
            && !all.contains("Fuzz refresh-token rotation"),
        "nested current work must not render in the transcript: {all}"
    );
    assert!(!all.contains("Unrelated worker"), "rendered={all}");
    assert!(!all.contains("Must not appear here"), "rendered={all}");
}

#[test]
fn test_prepare_messages_uses_exact_spawn_member_outside_gallery_subtree() {
    let session_id = "spawned-session-outside-filter";
    let state = TestState {
        display_messages: vec![DisplayMessage {
            role: "tool".to_string(),
            content: format!("Spawned new agent: {session_id}"),
            tool_calls: Vec::new(),
            duration_secs: None,
            title: None,
            tool_data: Some(ToolCall {
                id: "call-spawn-filter-race".to_string(),
                name: "swarm".to_string(),
                input: serde_json::json!({"action": "spawn", "label": "API reviewer"}),
                intent: Some("Spawn an authentication reviewer".to_string()),
                thought_signature: None,
            }),
        }],
        // Simulate a stale/missing ownership edge excluding the member from the
        // persistent gallery while the authoritative spawn result still names it.
        swarm_members: Vec::new(),
        transcript_swarm_members: Some(vec![chat_swarm_member(session_id)]),
        ..Default::default()
    };

    let rendered = prepare::prepare_messages(&state, 110, 30)
        .materialize_all_lines()
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("🐄 ● API reviewer"),
        "rendered={rendered}"
    );
}

#[test]
fn test_prepare_messages_matches_real_prefixed_spawn_result_without_input_metadata() {
    let session_id = "spawned-session-123";
    let state = TestState {
        display_messages: vec![DisplayMessage {
            role: "tool".to_string(),
            content: format!("[swarm] Spawned new agent: {session_id}"),
            tool_calls: Vec::new(),
            duration_secs: None,
            title: None,
            tool_data: Some(ToolCall {
                id: "call-spawn".to_string(),
                name: "swarm".to_string(),
                input: serde_json::Value::Null,
                intent: None,
                thought_signature: None,
            }),
        }],
        swarm_members: vec![chat_swarm_member(session_id)],
        ..Default::default()
    };

    let rendered = prepare::prepare_messages(&state, 110, 30)
        .materialize_all_lines()
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("🐄 ● API reviewer"),
        "rendered={rendered}"
    );
}

#[test]
fn test_prepare_messages_does_not_attach_member_to_unmatched_spawn_result() {
    let state = TestState {
        display_messages: vec![DisplayMessage {
            role: "tool".to_string(),
            content: "Spawned new agent: another-session".to_string(),
            tool_calls: Vec::new(),
            duration_secs: None,
            title: None,
            tool_data: Some(ToolCall {
                id: "call-spawn".to_string(),
                name: "swarm".to_string(),
                input: serde_json::json!({"action": "spawn", "label": "reviewer"}),
                intent: None,
                thought_signature: None,
            }),
        }],
        swarm_members: vec![chat_swarm_member("spawned-session-123")],
        ..Default::default()
    };

    let rendered = prepare::prepare_messages(&state, 110, 30)
        .materialize_all_lines()
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !rendered.contains("🐄 ● API reviewer"),
        "rendered={rendered}"
    );
}

#[test]
fn test_prepare_messages_matches_spawn_member_by_unique_label_when_result_is_reformatted() {
    let mut member = chat_swarm_member("spawned-session-123");
    member.task_label = Some("API reviewer".to_string());
    let state = TestState {
        display_messages: vec![DisplayMessage {
            role: "tool".to_string(),
            content: "Agent created successfully".to_string(),
            tool_calls: Vec::new(),
            duration_secs: None,
            title: None,
            tool_data: Some(ToolCall {
                id: "call-spawn".to_string(),
                name: "swarm".to_string(),
                input: serde_json::json!({"action": "spawn", "label": "API reviewer"}),
                intent: Some("Spawn an authentication reviewer".to_string()),
                thought_signature: None,
            }),
        }],
        swarm_members: vec![member],
        ..Default::default()
    };

    let rendered = prepare::prepare_messages(&state, 110, 30)
        .materialize_all_lines()
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("🐄 ● API reviewer"),
        "rendered={rendered}"
    );
}

#[test]
fn test_prepare_messages_does_not_guess_when_spawn_label_is_ambiguous() {
    let mut first = chat_swarm_member("spawned-session-123");
    first.task_label = Some("reviewer".to_string());
    let mut second = chat_swarm_member("spawned-session-456");
    second.task_label = Some("reviewer".to_string());
    let state = TestState {
        display_messages: vec![DisplayMessage {
            role: "tool".to_string(),
            content: "Agent created successfully".to_string(),
            tool_calls: Vec::new(),
            duration_secs: None,
            title: None,
            tool_data: Some(ToolCall {
                id: "call-spawn".to_string(),
                name: "swarm".to_string(),
                input: serde_json::json!({"action": "spawn", "label": "reviewer"}),
                intent: None,
                thought_signature: None,
            }),
        }],
        swarm_members: vec![first, second],
        ..Default::default()
    };

    let rendered = prepare::prepare_messages(&state, 110, 30)
        .materialize_all_lines()
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !rendered.contains("🐄 ● API reviewer"),
        "rendered={rendered}"
    );
}

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
                thought_signature: None,
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
                    thought_signature: None,
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
                thought_signature: None,
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
                    thought_signature: None,
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
                thought_signature: None,
            }],
            subcalls: vec![
                crate::bus::BatchSubcallProgress {
                    index: 1,
                    tool_call: ToolCall {
                        id: "batch-1-read".to_string(),
                        name: "read".to_string(),
                        input: serde_json::json!({"file_path": "Cargo.toml"}),
                        intent: None,
                        thought_signature: None,
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
                        thought_signature: None,
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
            .any(|line| line.contains("⠋ batch · 1/2 done")),
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
            .any(|line| line.contains("⠋ bash $ cargo build --release --workspace")),
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
                thought_signature: None,
            }],
            subcalls: vec![crate::bus::BatchSubcallProgress {
                index: 1,
                tool_call: ToolCall {
                    id: "batch-1-read".to_string(),
                    name: "read".to_string(),
                    input: serde_json::json!({"file_path": "src/main.rs"}),
                    intent: None,
                    thought_signature: None,
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
            thought_signature: None,
        }],
        subcalls: vec![crate::bus::BatchSubcallProgress {
            index: 1,
            tool_call: ToolCall {
                id: "batch-1-bash".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"command": "sleep 1"}),
                intent: None,
                thought_signature: None,
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
            .any(|line| line.contains("⠋ batch · 0/1 done")),
        "expected first spinner frame in {:?}",
        first_rendered
    );
    assert!(
        second_rendered
            .iter()
            .any(|line| line.contains("⠙ batch · 0/1 done")),
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
                thought_signature: None,
            }],
            subcalls: vec![crate::bus::BatchSubcallProgress {
                index: 1,
                tool_call: ToolCall {
                    id: "batch-1-read".to_string(),
                    name: "read".to_string(),
                    input: serde_json::json!({"file_path": "Cargo.toml"}),
                    intent: None,
                    thought_signature: None,
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
        thought_signature: None,
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
            thought_signature: None,
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
            thought_signature: None,
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
            thought_signature: None,
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
