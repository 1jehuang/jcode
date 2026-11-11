use super::*;

// Deliberately retain the original full-copy conversion as an independent
// semantic oracle and a baseline for the ignored representative benchmark.
fn old_wire_messages(session: &Session, start: usize) -> Vec<WireMessage> {
    fn old_truncate(text: &str, max: usize) -> String {
        if text.chars().count() <= max {
            return text.to_string();
        }
        let mut out: String = text.chars().take(max).collect();
        out.push_str("\n… (truncated)");
        out
    }
    session
        .messages
        .iter()
        .enumerate()
        .skip(start)
        .filter_map(|(seq, message)| {
            let mut parts = Vec::new();
            let mut is_tool_result = false;
            for block in &message.content {
                match block {
                    ContentBlock::Text { text, .. }
                        if !text.trim().is_empty()
                            && !text.trim_start().starts_with("<system-reminder>") =>
                    {
                        parts.push(text.clone())
                    }
                    ContentBlock::ToolUse { name, input, .. } => {
                        let detail = input
                            .get("intent")
                            .or_else(|| input.get("command"))
                            .or_else(|| input.get("file_path"))
                            .or_else(|| input.get("query"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        parts.push(if detail.is_empty() {
                            format!("🔧 {name}")
                        } else {
                            format!("🔧 {name}: {}", old_truncate(detail, 200))
                        });
                    }
                    ContentBlock::ToolResult { content, .. } => {
                        is_tool_result = true;
                        parts.push(old_truncate(content, MAX_TOOL_RESULT_CHARS));
                    }
                    _ => {}
                }
            }
            if parts.is_empty() {
                return None;
            }
            let role = if is_tool_result {
                "tool"
            } else if message.display_role.is_some() {
                "system"
            } else {
                match message.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                }
            };
            Some(WireMessage {
                seq,
                role,
                text: old_truncate(&parts.join("\n\n"), MAX_TEXT_CHARS),
                created_at: message.timestamp.map(|t| t.to_rfc3339()),
            })
        })
        .collect()
}

fn text_block(text: String) -> ContentBlock {
    ContentBlock::Text {
        text,
        cache_control: None,
    }
}

fn result_block(content: String) -> ContentBlock {
    ContentBlock::ToolResult {
        tool_use_id: "t".into(),
        content,
        is_error: None,
    }
}

#[test]
fn bounded_wire_conversion_matches_original_unicode_and_boundaries() {
    let mut session = Session::create_with_id("bounded-wire".into(), None, None);
    for unit in ["a", "é", "界", "😀", "e\u{301}"] {
        for size in [
            0,
            1,
            199,
            200,
            201,
            MAX_TOOL_RESULT_CHARS - 1,
            MAX_TOOL_RESULT_CHARS,
            MAX_TOOL_RESULT_CHARS + 1,
            MAX_TEXT_CHARS - 2,
            MAX_TEXT_CHARS - 1,
            MAX_TEXT_CHARS,
            MAX_TEXT_CHARS + 1,
        ] {
            let value = unit.repeat(size);
            session
                .messages
                .push(msg(Role::User, vec![text_block(value.clone())]));
            session
                .messages
                .push(msg(Role::Assistant, vec![result_block(value.clone())]));
            session.messages.push(msg(
                Role::Assistant,
                vec![
                    text_block(value.clone()),
                    result_block(String::new()),
                    text_block("tail".into()),
                    result_block(value.clone()),
                    ContentBlock::ToolUse {
                        id: "t".into(),
                        name: "bash".into(),
                        input: serde_json::json!({"intent": value}),
                        thought_signature: None,
                    },
                ],
            ));
        }
    }
    session.messages.push(msg(
        Role::User,
        vec![
            text_block(" \n\u{2003}<system-reminder>private".into()),
            text_block(" \n\u{2003}".into()),
            ContentBlock::Reasoning {
                text: "private".into(),
            },
        ],
    ));
    session
        .messages
        .push(msg(Role::User, vec![result_block(String::new())]));
    session.messages.push(msg(
        Role::User,
        vec![result_block(String::new()), result_block(String::new())],
    ));
    for name in [String::new(), "😀".repeat(MAX_TEXT_CHARS + 1)] {
        for input in [
            serde_json::json!({}),
            serde_json::json!({"intent": ""}),
            serde_json::json!({"intent": null, "command": "not-used"}),
            serde_json::json!({"command": "界".repeat(201)}),
            serde_json::json!({"file_path": "file"}),
            serde_json::json!({"query": "query"}),
        ] {
            session.messages.push(msg(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "t".into(),
                    name: name.clone(),
                    input,
                    thought_signature: None,
                }],
            ));
        }
    }
    session.messages[3].display_role = Some(crate::session::StoredDisplayRole::System);
    session.messages[6].display_role = Some(crate::session::StoredDisplayRole::BackgroundTask);
    session.messages[1].timestamp = Some(chrono::Utc::now());
    for start in [0, 1, 3, session.messages.len(), session.messages.len() + 1] {
        let expected = old_wire_messages(&session, start);
        let actual = wire_messages(&session, start);
        assert_eq!(actual, expected, "start={start}");
        for end in [0, 1, session.messages.len() / 2, session.messages.len()] {
            assert_eq!(
                wire_fingerprint(&actual, end),
                wire_fingerprint(&expected, end)
            );
        }
    }
    for max in [0, 1, 2, 3, 200] {
        for value in ["", "a", "é😀界", "e\u{301}", "x\n"] {
            let expected = if value.chars().count() <= max {
                value.to_owned()
            } else {
                format!(
                    "{}\n… (truncated)",
                    value.chars().take(max).collect::<String>()
                )
            };
            assert_eq!(truncate(value, max), expected);
        }
    }
}

#[test]
fn bounded_text_matches_join_then_truncate() {
    let parts = ["", "a", "é", "界😀", "e\u{301}", "\n", "   "];
    for max in 0..12 {
        for a in parts {
            for b in parts {
                for c in parts {
                    let joined = [a, b, c].join("\n\n");
                    let expected = if joined.chars().count() <= max {
                        joined.clone()
                    } else {
                        format!(
                            "{}\n… (truncated)",
                            joined.chars().take(max).collect::<String>()
                        )
                    };
                    let mut actual = BoundedText::new(max);
                    for (index, part) in [a, b, c].iter().enumerate() {
                        if index != 0 {
                            actual.push("\n\n");
                        }
                        actual.push(part);
                    }
                    assert_eq!(actual.text, expected, "max={max}, parts={:?}", [a, b, c]);
                }
            }
        }
    }
}

#[test]
#[ignore = "comparative benchmark, run with --ignored --nocapture"]
fn benchmark_bounded_wire_conversion() {
    use std::hint::black_box;
    use std::time::Instant;
    for (label, size, blocks) in [
        ("typical", 1_000, 4),
        ("long-ascii", 2_000_000, 4),
        ("long-unicode", 1_000_000, 8),
    ] {
        let mut session = Session::create_with_id("wire-bench".into(), None, None);
        let unit = if label == "long-unicode" {
            "界😀é"
        } else {
            "a"
        };
        for _ in 0..8 {
            let mut content = (0..blocks)
                .map(|_| text_block(unit.repeat(size)))
                .collect::<Vec<_>>();
            content.push(result_block("tool output".repeat(1_000)));
            session.messages.push(msg(Role::Assistant, content));
        }
        assert_eq!(wire_messages(&session, 0), old_wire_messages(&session, 0));
        let iterations = if label == "typical" { 100 } else { 5 };
        let old_start = Instant::now();
        for _ in 0..iterations {
            black_box(old_wire_messages(black_box(&session), 0));
        }
        let old = old_start.elapsed();
        let new_start = Instant::now();
        for _ in 0..iterations {
            black_box(wire_messages(black_box(&session), 0));
        }
        let new = new_start.elapsed();
        eprintln!(
            "mobile wire {label}: iterations={iterations} old={old:?} bounded={new:?} ratio={:.2}x",
            old.as_secs_f64() / new.as_secs_f64()
        );
    }
}

#[test]
fn incremental_updates_replay_rewritten_and_shrunk_prefixes() {
    let mut session = Session::create_with_id("rewrite".into(), None, None);
    for text in ["first", "second"] {
        session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: text.into(),
                cache_control: None,
            }],
        );
    }
    let (initial, fingerprint) = incremental_messages(&session, &SessionProgress::default());
    assert_eq!(initial.len(), 2);
    let progress = SessionProgress {
        uploaded: 2,
        fingerprint: Some(fingerprint),
        ..Default::default()
    };
    assert!(incremental_messages(&session, &progress).0.is_empty());
    session.add_message(
        Role::Assistant,
        vec![ContentBlock::Text {
            text: "appended".into(),
            cache_control: None,
        }],
    );
    let appended = incremental_messages(&session, &progress).0;
    assert_eq!(appended.len(), 1);
    assert_eq!(appended[0].seq, 2);
    session.messages[0].content = vec![ContentBlock::Text {
        text: "rewritten".into(),
        cache_control: None,
    }];
    let rewritten = incremental_messages(&session, &progress).0;
    assert_eq!(rewritten.len(), 3);
    assert_eq!(rewritten[0].text, "rewritten");
    session.truncate_messages(1);
    assert_eq!(incremental_messages(&session, &progress).0.len(), 1);
}
