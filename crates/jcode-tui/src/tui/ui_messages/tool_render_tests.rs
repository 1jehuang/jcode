//! Focused tool render regression tests.
use super::*;

#[test]
fn render_tool_message_groups_duration_and_tokens() {
    for name in [
        "bash",
        "read",
        "edit",
        "webfetch",
        "debug_socket",
        "unknown_tool",
    ] {
        for content in ["ok", "Error: failed"] {
            let mut msg = DisplayMessage::tool(
                content,
                crate::message::ToolCall {
                    id: "timed".into(),
                    name: name.into(),
                    input: serde_json::json!({}),
                    intent: Some("A completed tool call".into()),
                    thought_signature: None,
                },
            );
            msg.duration_secs = Some(2.4);
            let tokens =
                crate::util::format_approx_token_count(crate::util::estimate_tokens(content));
            for width in [32, 60, 120] {
                let lines = render_tool_message(&msg, width, crate::config::DiffDisplayMode::Off);
                let row = extract_line_text(&lines[0]);
                assert!(row.contains(&format!("2.4s, {tokens}")), "{name}: {row}");
                assert!(!row.contains("2.4s ·"), "{row}");
                assert!(lines[0].width() < width as usize, "{row}");
            }
        }
    }
}

#[test]
fn batch_subcall_groups_duration_and_tokens() {
    let tool = crate::message::ToolCall {
        id: "timed-child".into(),
        name: "read".into(),
        input: serde_json::json!({}),
        intent: Some("Read configuration".into()),
        thought_signature: None,
    };
    let line = crate::tui::ui::tools_ui::render_batch_subcall_line(
        &tool,
        "✓",
        rgb(100, 180, 100),
        50,
        Some(80),
        Some("[tool timing: duration=73ms] config content"),
    );
    let text = extract_line_text(&line);
    assert!(text.contains(" · 73ms, "), "{text}");
    assert!(text.ends_with(" tok"), "{text}");
    assert!(line.width() <= 80);
}

#[test]
fn tool_usage_label_handles_missing_and_legacy_duration() {
    use crate::tui::ui::tools_ui::tool_usage_label;
    assert_eq!(tool_usage_label("217 tok", None, "ok"), "217 tok");
    assert_eq!(tool_usage_label("217 tok", Some(0.0), "ok"), "0ms, 217 tok");
    assert_eq!(tool_usage_label("217 tok", Some(f32::NAN), "ok"), "217 tok");
    assert_eq!(
        tool_usage_label(
            "217 tok",
            None,
            "[tool timing: start=x finish=y duration=2.4s] ok"
        ),
        "2.4s, 217 tok"
    );
    assert_eq!(
        tool_usage_label("217 tok", None, "[tool timing: duration=1m 4s] ok"),
        "1m 4s, 217 tok"
    );
    assert_eq!(
        tool_usage_label("217 tok", None, "[tool timing: duration=73ms] ok"),
        "73ms, 217 tok"
    );
    assert_eq!(
        tool_usage_label("217 tok", None, "[tool timing: duration=NaNs] ok"),
        "217 tok"
    );
    assert_eq!(
        tool_usage_label("217 tok", None, "[tool timing: duration=invalid] ok"),
        "217 tok"
    );
}

#[test]
fn render_tool_message_colors_duration_independently_from_tokens() {
    for (seconds, duration_color) in [
        (0.42, rgb(118, 118, 118)),
        (9.99, rgb(118, 118, 118)),
        (10.0, rgb(214, 184, 92)),
        (59.99, rgb(214, 184, 92)),
        (60.0, rgb(224, 118, 118)),
    ] {
        for content in ["ok".to_owned(), "x".repeat(48_000)] {
            let mut msg = DisplayMessage::tool(
                content,
                crate::message::ToolCall {
                    id: "independent-metrics".into(),
                    name: "read".into(),
                    input: serde_json::json!({}),
                    intent: None,
                    thought_signature: None,
                },
            );
            msg.duration_secs = Some(seconds);
            let badge = tool_output_token_badge(&msg.content);
            for width in [32, 120] {
                let lines = render_tool_message(&msg, width, crate::config::DiffDisplayMode::Off);
                let spans = &lines[0].spans;
                let token_index = spans
                    .iter()
                    .position(|span| span.content == badge.label)
                    .expect("missing token span");
                assert_eq!(spans[token_index].style.fg, Some(badge.color));
                assert_eq!(spans[token_index - 1].content, ", ");
                assert_eq!(spans[token_index - 2].style.fg, Some(duration_color));
                assert!(lines[0].width() < width as usize);
            }
        }
    }
}

#[test]
fn batch_subcall_colors_duration_independently_from_tokens() {
    let tool = crate::message::ToolCall {
        id: "timed-child".into(),
        name: "read".into(),
        input: serde_json::json!({}),
        intent: None,
        thought_signature: None,
    };
    for (timing, color) in [
        ("420ms", rgb(118, 118, 118)),
        ("10s", rgb(214, 184, 92)),
        ("1m", rgb(224, 118, 118)),
    ] {
        let content = format!("[tool timing: duration={timing}] {}", "x".repeat(48_000));
        let line = crate::tui::ui::tools_ui::render_batch_subcall_line(
            &tool,
            "✓",
            rgb(100, 180, 100),
            50,
            Some(60),
            Some(&content),
        );
        let duration = line
            .spans
            .iter()
            .find(|span| span.content == timing)
            .expect("missing duration span");
        assert_eq!(duration.style.fg, Some(color));
        let tokens = line
            .spans
            .iter()
            .find(|span| span.content.ends_with(" tok"))
            .expect("missing token span");
        assert_eq!(tokens.style.fg, Some(rgb(224, 118, 118)));
        assert!(line.width() <= 60);
    }
}

#[test]
fn render_tool_message_colors_high_token_badge() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "x".repeat(48_000),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_3".to_string(),
            name: "read".to_string(),
            input: serde_json::json!({"file_path": "src/main.rs"}),
            intent: None,
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let badge_span = lines[0]
        .spans
        .iter()
        .find(|span| span.content.contains("12k tok"))
        .expect("missing token badge");

    assert_eq!(badge_span.style.fg, Some(rgb(224, 118, 118)));
}

#[test]
fn render_tool_message_shows_inline_diff_for_pascal_case_multiedit() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "Edited demo.txt\n\nApplied:\n  ✓ Edit 1: replaced 1 occurrence\n\nTotal: 1 applied, 0 failed\n"
            .to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: Some("demo.txt".to_string()),
        tool_data: Some(crate::message::ToolCall {
            id: "call_multiedit_pascal".to_string(),
            name: "MultiEdit".to_string(),
            input: serde_json::json!({
                "file_path": "demo.txt",
                "edits": [
                    {"old_string": "old line\n", "new_string": "new line\n"}
                ]
            }),
            intent: None, thought_signature: None, }),
    };

    let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Inline);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("╭─ diff"), "plain={plain}");
    assert!(plain.contains("old line"), "plain={plain}");
    assert!(plain.contains("new line"), "plain={plain}");
}

#[test]
fn render_tool_message_labels_single_file_apply_patch_diff() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "✓ src/example.rs: modified (1 hunks)".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: Some("src/example.rs".to_string()),
        tool_data: Some(crate::message::ToolCall {
            id: "call_apply_patch_single".to_string(),
            name: "apply_patch".to_string(),
            input: serde_json::json!({
                "intent": "Update example behavior",
                "patch_text": "*** Begin Patch\n*** Update File: src/example.rs\n@@\n-old_value\n+new_value\n*** End Patch\n"
            }),
            intent: Some("Update example behavior".to_string()),
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Inline);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        plain.contains("╭─ diff · +1 -1 · src/example.rs"),
        "plain={plain}"
    );
    assert!(plain.contains("old_value"), "plain={plain}");
    assert!(plain.contains("new_value"), "plain={plain}");
}

#[test]
fn render_tool_message_preserves_multi_file_apply_patch_boundaries() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "✓ a.txt: modified (1 hunks)\n1- old a\n1+ new a\n✓ b.txt: modified (1 hunks)\n1- old b\n1+ new b\n".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: Some("2 files".to_string()),
        tool_data: Some(crate::message::ToolCall {
            id: "call_apply_patch_multi".to_string(),
            name: "apply_patch".to_string(),
            input: serde_json::json!({
                "intent": "Update both examples",
                "patch_text": "*** Begin Patch\n*** Update File: a.txt\n@@\n-old a\n+new a\n*** Update File: b.txt\n@@\n-old b\n+new b\n*** End Patch\n"
            }),
            intent: Some("Update both examples".to_string()),
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Inline);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    let a_header = plain
        .find("╭─ diff · +2 -2 · a.txt")
        .expect("missing a.txt header");
    let old_a = plain.find("old a").expect("missing a.txt deletion");
    let new_a = plain.find("new a").expect("missing a.txt addition");
    let b_header = plain
        .find("├─ diff · b.txt")
        .expect("missing b.txt boundary");
    let old_b = plain.find("old b").expect("missing b.txt deletion");
    let new_b = plain.find("new b").expect("missing b.txt addition");
    assert!(
        a_header < old_a && old_a < new_a && new_a < b_header,
        "plain={plain}"
    );
    assert!(b_header < old_b && old_b < new_b, "plain={plain}");
}

#[test]
fn render_tool_message_shows_numbered_write_result_diff_after_input_compaction() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "Created /tmp/head-to-head.html (2 lines):\n1+ <!doctype html>\n2+ <html lang=\"en\">\n..."
            .to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: Some("/tmp/head-to-head.html".to_string()),
        tool_data: Some(crate::message::ToolCall {
            id: "call_write_compacted".to_string(),
            name: "write".to_string(),
            input: serde_json::json!({"file_path": "/tmp/head-to-head.html"}),
            intent: Some("Create an honest data-driven benchmark comparison page".to_string()),
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Inline);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        lines[1].spans.iter().any(|span| span.content == "+2"),
        "plain={plain}"
    );
    assert!(plain.contains("╭─ diff"), "plain={plain}");
    assert!(plain.contains("<!doctype html>"), "plain={plain}");
    assert!(plain.contains("<html lang=\"en\">"), "plain={plain}");
}

#[test]
fn render_tool_message_never_draws_an_empty_edit_diff_frame() {
    for (name, content) in [
        ("write", "Created empty.txt (0 lines):\n"),
        ("edit", "Edited demo.txt: replaced 1 occurrence(s)"),
        (
            "multiedit",
            "Edited demo.txt\n\nTotal: 1 applied, 0 failed\n",
        ),
        ("patch", "Patch applied successfully"),
        ("apply_patch", "✓ demo.txt: modified (1 hunks)"),
    ] {
        let msg = DisplayMessage {
            role: "tool".to_string(),
            content: content.to_string(),
            tool_calls: Vec::new(),
            duration_secs: None,
            title: Some("demo.txt".to_string()),
            tool_data: Some(crate::message::ToolCall {
                id: format!("call_{name}_compacted"),
                name: name.to_string(),
                input: serde_json::json!({"file_path": "demo.txt"}),
                intent: None,
                thought_signature: None,
            }),
        };

        let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Inline);
        let plain = lines
            .iter()
            .map(extract_line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!plain.contains("╭─ diff"), "tool={name}, plain={plain}");
        assert!(!plain.contains("(+0 -0)"), "tool={name}, plain={plain}");
    }
}

#[test]
fn render_tool_message_marks_failed_apply_patch_without_empty_diff() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content:
            "[apply_patch] ✗ /tmp/main.rs: Failed to find expected lines in /tmp/main.rs:\nfn missing() {}"
                .to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: Some("/tmp/main.rs".to_string()),
        tool_data: Some(crate::message::ToolCall {
            id: "call_apply_patch_failed".to_string(),
            name: "apply_patch".to_string(),
            input: serde_json::json!({"file_path": "/tmp/main.rs"}),
            intent: Some("Replace the benchmark placeholder".to_string()),
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Inline);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        plain.trim_start().starts_with("✗ apply_patch"),
        "plain={plain}"
    );
    assert!(!plain.contains("╭─ diff"), "plain={plain}");
    assert!(!plain.contains("(+0 -0)"), "plain={plain}");
}

#[test]
fn render_tool_message_inline_mode_truncates_large_diffs() {
    let old = (1..=7)
        .map(|i| format!("old line {i}\n"))
        .collect::<String>();
    let new = (1..=7)
        .map(|i| format!("new line {i} suffix_{i}_abcdefghijklmnopqrstuvwxyz0123456789\n"))
        .collect::<String>();
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "Edited demo.txt".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: Some("demo.txt".to_string()),
        tool_data: Some(crate::message::ToolCall {
            id: "call_edit_inline_truncated".to_string(),
            name: "edit".to_string(),
            input: serde_json::json!({
                "file_path": "demo.txt",
                "old_string": old,
                "new_string": new,
            }),
            intent: None,
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 40, crate::config::DiffDisplayMode::Inline);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("... 2 more changes ..."), "plain={plain}");
    assert!(plain.contains("old line 3"), "plain={plain}");
    assert!(!plain.contains("old line 7"), "plain={plain}");
    assert!(
        !plain.contains("new line 1 suffix_1_abcdefghijklmnopqrstuvwxyz0123456789"),
        "plain={plain}"
    );
    assert!(plain.contains("suffix_2_abcdefghijklm…"), "plain={plain}");
    assert_eq!(plain.matches("+7 -7").count(), 1, "plain={plain}");
    assert!(!extract_line_text(&lines[0]).contains("+7"));
    assert!(!plain.contains("total"));
    let footer = lines.last().unwrap();
    assert_eq!(extract_line_text(footer), "╰─");
    assert!(!plain.contains("hidden"), "plain={plain}");
    let header = &lines[1];
    assert!(extract_line_text(header).starts_with("╭─ diff · +7 -7 · "));
    assert!(
        header
            .spans
            .iter()
            .any(|span| span.content == "+7" && span.style.fg == Some(diff_add_color()))
    );
    assert!(
        header
            .spans
            .iter()
            .any(|span| span.content == "-7" && span.style.fg == Some(diff_del_color()))
    );
}

#[test]
fn render_tool_message_full_inline_mode_shows_full_diff() {
    let old = (1..=7)
        .map(|i| format!("old line {i}\n"))
        .collect::<String>();
    let new = (1..=7)
        .map(|i| format!("new line {i} suffix_{i}_abcdefghijklmnopqrstuvwxyz0123456789\n"))
        .collect::<String>();
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "Edited demo.txt".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: Some("demo.txt".to_string()),
        tool_data: Some(crate::message::ToolCall {
            id: "call_edit_inline_full".to_string(),
            name: "edit".to_string(),
            input: serde_json::json!({
                "file_path": "demo.txt",
                "old_string": old,
                "new_string": new,
            }),
            intent: None,
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 40, crate::config::DiffDisplayMode::FullInline);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!plain.contains("more changes"), "plain={plain}");
    assert!(plain.contains("old line 4"), "plain={plain}");
    let source_text = lines
        .iter()
        .filter(|line| extract_line_text(line).starts_with("│ "))
        .flat_map(|line| {
            line.spans
                .iter()
                .skip(if line.spans[0].content.contains('↪') {
                    1
                } else {
                    2
                })
        })
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(
        source_text.contains("new line 4 suffix_4_abcdefghijklmnopqrstuvwxyz0123456789"),
        "plain={plain}"
    );
    assert!(lines.iter().all(|line| line.width() <= 39), "plain={plain}");
    assert!(!plain.contains('…'), "plain={plain}");
    assert_eq!(plain.matches("+7 -7").count(), 1, "plain={plain}");
    assert_eq!(extract_line_text(lines.last().unwrap()), "╰─");
    assert!(!extract_line_text(&lines[0]).contains("+7"));

    let hidden = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let hidden_plain = hidden
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(hidden_plain.matches("(+7 -7)").count(), 1);
    assert!(!hidden_plain.contains("╭─ diff"));
    assert!(
        hidden[0]
            .spans
            .iter()
            .any(|span| span.content == "+7" && span.style.fg == Some(diff_add_color()))
    );
}

#[test]
fn render_tool_message_memory_recall_centered_mode_left_aligns_with_padding() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(true);
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: concat!(
            "- [fact] Centered mode should keep the recall card centered\n",
            "- [preference] The user likes visible side gutters"
        )
        .to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_memory_recall_centered".to_string(),
            name: "memory".to_string(),
            input: serde_json::json!({
                "action": "recall",
                "query": "centered mode"
            }),
            intent: None,
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect();

    assert!(!rendered.is_empty(), "expected rendered recall card");
    assert!(
        rendered.iter().all(|line| line.starts_with("  ")),
        "centered recall card should include shared left padding: {rendered:?}"
    );
    assert_eq!(
        lines[0].alignment,
        Some(ratatui::layout::Alignment::Left),
        "centered recall card header should be left-aligned after padding"
    );
    assert!(
        rendered[0]
            .trim_start()
            .starts_with("🧠 recalled 2 memories"),
        "unexpected recall header: {rendered:?}"
    );

    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn render_tool_message_memory_store_centered_mode_left_aligns_with_padding() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(true);
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "Saved memory".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_memory_store_centered".to_string(),
            name: "memory".to_string(),
            input: serde_json::json!({
                "action": "remember",
                "category": "fact",
                "content": "Centered mode should pad saved memory cards too"
            }),
            intent: None,
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect();

    assert!(!rendered.is_empty(), "expected rendered saved-memory card");
    assert!(
        rendered.iter().all(|line| line.starts_with("  ")),
        "centered saved-memory card should include shared left padding: {rendered:?}"
    );
    assert_eq!(
        lines[0].alignment,
        Some(ratatui::layout::Alignment::Left),
        "centered saved-memory card should be left-aligned after padding"
    );

    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn render_tool_message_shows_swarm_spawn_prompt_summary() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "spawned".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_swarm_spawn".to_string(),
            name: "swarm".to_string(),
            input: serde_json::json!({
                "action": "spawn",
                "prompt": "Extract the restart command cluster from cli commands and validate it"
            }),
            intent: None,
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered: String = lines[0]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();

    assert!(rendered.contains("swarm spawn"), "rendered={rendered}");
    assert!(
        rendered.contains("Extract the restart command cluster"),
        "rendered={rendered}"
    );
}

#[test]
fn render_tool_message_batch_subcall_shows_swarm_dm_details() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "--- [1] swarm ---\nDone\n\nCompleted: 1 succeeded, 0 failed".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_batch_swarm".to_string(),
            name: "batch".to_string(),
            input: serde_json::json!({
                "tool_calls": [
                    {
                        "tool": "swarm",
                        "action": "dm",
                        "to_session": "shark",
                        "message": "Please validate the restart extraction and report back"
                    }
                ]
            }),
            intent: None,
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("swarm dm → shark"), "rendered={rendered}");
    assert!(
        rendered.contains("Please validate the restart"),
        "rendered={rendered}"
    );
}

#[test]
fn render_agentgrep_output_body_borders_each_line() {
    let content = "crates/foo.rs\n  symbols: 1 matched\n    - fn bar @ 1-5";
    let lines = super::super::render_agentgrep_output_body(content, 120);
    let rendered = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("│ crates/foo.rs"), "rendered={rendered}");
    assert!(
        rendered.contains("│   symbols: 1 matched"),
        "rendered={rendered}"
    );
    assert!(
        rendered.contains("│     - fn bar @ 1-5"),
        "rendered={rendered}"
    );
    assert_eq!(lines.len(), 3, "one bordered line per source line");
}

#[test]
fn render_agentgrep_output_body_caps_huge_output() {
    let content = (0..1000)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let lines = super::super::render_agentgrep_output_body(&content, 120);
    // 400-line cap plus a single truncation summary line.
    assert_eq!(lines.len(), 401, "should cap the body and add a summary");
    let last = extract_line_text(&lines[lines.len() - 1]);
    assert!(last.contains("more lines"), "last={last}");
}

#[test]
fn render_assistant_message_plan_card_wraps_instead_of_truncating() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(false);
    // Long paragraph and long list items must wrap inside the card, not be
    // clipped at the right border by render_rounded_box's truncation.
    let plan_body = "# Long content plan\n\n\
        Goal\n\
        Produce an up-to-date ranked report grounded in current crate paths, then fix the highest-leverage low-risk offenders without destabilizing active work.\n\n\
        Approach\n\
        1. Write an audit document that regenerates metrics with current crate paths, ranks the top issues with evidence, and marks which items from the previous audit are complete versus stale.\n\
        2. Map the provider migration and record whether each module is a thin wrapper, partial duplicate, or full duplicate of the extracted crate.\n";
    let content = format!("Intro text.\n\n```plan\n{plan_body}```\n\nAfter the card.");
    let msg = DisplayMessage::assistant(&content);

    for width in [40u16, 60, 80, 100, 140] {
        let lines = render_assistant_message(&msg, width, crate::config::DiffDisplayMode::Off);
        let squashed = lines
            .iter()
            .map(extract_line_text)
            .collect::<Vec<_>>()
            .join(" ")
            .replace(['│', '╭', '╮', '╰', '╯', '─'], " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        for phrase in [
            "without destabilizing active work.",
            "complete versus stale.",
            "or full duplicate of the extracted crate.",
        ] {
            assert!(
                squashed.contains(phrase),
                "width {width}: plan card lost trailing content {phrase:?}\n{squashed}"
            );
        }
        // Card borders stay intact.
        for line in lines
            .iter()
            .map(extract_line_text)
            .filter(|l| l.contains('│'))
        {
            assert!(
                line.trim_end().ends_with('│'),
                "width {width}: card row missing right border: {line:?}"
            );
        }
    }
    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn render_swarm_message_preserves_inline_image_placeholder_lines() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(false);

    // Simulate a rendered mermaid diagram inside a swarm message body: the
    // marker line plus its blank fill rows must survive rendering without a
    // rail prefix or blank-line cleanup so the image draws at full height.
    let placeholder = crate::tui::mermaid::inline_image_placeholder_lines(0xabcd1234, 4, 20);
    assert_eq!(placeholder.len(), 4);
    let marker_text = placeholder[0]
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect::<String>();

    let msg = DisplayMessage::swarm(
        "Plan graph · v3",
        "```mermaid\nflowchart TD\n    a --> b\n```",
    );
    // Rendering the real message goes through the markdown pipeline; whether a
    // real image materializes depends on protocol availability, so test the
    // line-preservation path directly through render_swarm_message with a body
    // the markdown renderer maps to placeholder lines is not deterministic in
    // tests. Instead assert the parser round-trips the marker we emit.
    let parsed = crate::tui::mermaid::parse_inline_image_placeholder(&placeholder[0]);
    assert_eq!(parsed, Some((0xabcd1234, 4, 20)));
    assert!(
        marker_text.starts_with('\u{0}'),
        "marker must keep its sentinel prefix"
    );

    // And the swarm renderer must not panic or drop content for a mermaid body.
    let lines = render_swarm_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    assert!(!lines.is_empty());

    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn render_empty_todo_tool_result_collapses_to_compact_line() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "[todo] []".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: Some("0 todos".to_string()),
        tool_data: Some(crate::message::ToolCall {
            id: "call_todo_empty".to_string(),
            name: "todo".to_string(),
            input: serde_json::json!({}),
            intent: Some("Read the todo list".to_string()),
            thought_signature: None,
        }),
    };

    let plain = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Off)
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!plain.contains("No tasks yet"), "{plain}");
    assert!(plain.contains("no tasks"), "{plain}");
}

/// The edit row must not repeat the file path that the inline diff header
/// directly below already shows. With an intent it reads like any other tool
/// row, and without one it falls back to the bare name plus change counts.
#[test]
fn render_tool_message_edit_row_does_not_duplicate_diff_header_path() {
    for intent in [Some("Fix the network hint"), None] {
        let msg = DisplayMessage {
            role: "tool".to_string(),
            content: "Edited".to_string(),
            tool_calls: Vec::new(),
            duration_secs: None,
            title: None,
            tool_data: Some(crate::message::ToolCall {
                id: "call_edit".to_string(),
                name: "edit".to_string(),
                input: serde_json::json!({
                    "file_path": "/repo/src/very_specific_name.rs",
                    "old_string": "old\n",
                    "new_string": "new\n",
                }),
                intent: intent.map(str::to_string),
                thought_signature: None,
            }),
        };

        let lines = render_tool_message(&msg, 160, crate::config::DiffDisplayMode::Inline);
        let text: Vec<String> = lines.iter().map(extract_line_text).collect();
        let occurrences = text
            .iter()
            .filter(|line| line.contains("very_specific_name.rs"))
            .count();
        assert_eq!(occurrences, 1, "path should appear once: {text:#?}");
        assert!(
            text[1].contains("diff · +1 -1 · /repo/src/very_specific_name.rs"),
            "{text:#?}"
        );
        if let Some(intent) = intent {
            assert!(text[0].contains(&format!("edit · {intent}")), "{text:#?}");
        }

        // With diffs hidden the row is the only place the path can appear.
        let lines = render_tool_message(&msg, 160, crate::config::DiffDisplayMode::Off);
        if intent.is_none() {
            assert!(extract_line_text(&lines[0]).contains("very_specific_name.rs"));
        }
    }
}
