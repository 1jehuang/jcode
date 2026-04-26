use super::*;

fn extract_line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

fn leading_spaces(text: &str) -> usize {
    text.chars().take_while(|c| *c == ' ').count()
}

fn system_glyph_env_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};

    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[test]
fn render_system_message_forces_system_color_on_all_spans() {
    let msg = DisplayMessage::system("**Reload complete** — continuing.");

    let lines = render_system_message(&msg, 80, crate::config::DiffDisplayMode::Off);

    assert!(!lines.is_empty(), "expected rendered system message lines");
    for line in lines {
        for span in line.spans {
            assert_eq!(span.style.fg, Some(system_message_color()));
        }
    }
}

#[test]
fn render_system_message_centered_mode_left_aligns_with_padding() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(true);
    let msg = DisplayMessage::system("Reload complete — continuing.");

    let lines = render_system_message(&msg, 80, crate::config::DiffDisplayMode::Off);

    assert!(!lines.is_empty(), "expected rendered system message lines");
    for line in &lines {
        assert_eq!(
            line.alignment,
            Some(ratatui::layout::Alignment::Left),
            "centered system lines should be left-aligned with padding"
        );
        assert!(
            line.spans
                .first()
                .is_some_and(|span| span.content.starts_with(' ')),
            "centered system lines should start with padding"
        );
    }
    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn render_system_message_uses_width_stable_titles_on_kitty() {
    let _guard = system_glyph_env_lock();
    let prev_term_program = std::env::var("TERM_PROGRAM").ok();
    let prev_term = std::env::var("TERM").ok();
    crate::env::set_var("TERM_PROGRAM", "kitty");
    crate::env::set_var("TERM", "xterm-kitty");

    let msg = DisplayMessage::system(
        "⚡ Connection lost — retrying (attempt 2, 7s) — connection reset by server",
    )
    .with_title("Connection");

    let lines = render_system_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("reconnecting"));
    assert!(!plain.contains("⚡ reconnecting"));

    match prev_term_program {
        Some(value) => crate::env::set_var("TERM_PROGRAM", value),
        None => crate::env::remove_var("TERM_PROGRAM"),
    }
    match prev_term {
        Some(value) => crate::env::set_var("TERM", value),
        None => crate::env::remove_var("TERM"),
    }
}

#[test]
fn render_background_task_message_uses_box_and_truncates_preview_lines() {
    let msg = DisplayMessage::background_task(
        "**Background task** `bg123` · `bash` · ✓ completed · 7.1s · exit 0\n\n```text\nline 1\nline 2\nline 3\nline 4\nline 5\n```\n\n_Full output:_ `bg action=\"output\" task_id=\"bg123\"`",
    );

    let lines = render_background_task_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("✓ bg bash completed · bg123"));
    assert!(plain.contains("exit 0 · 7.1s"));
    assert!(plain.contains("line 1"));
    assert!(plain.contains("… +1 more line"));
    assert!(!plain.contains("task bg123 · bash"));
    assert!(!plain.contains("Preview"));
    assert!(!plain.contains("Full output"));
    assert!(!plain.contains("bg action=\"output\" task_id=\"bg123\""));
}

#[test]
fn render_background_task_progress_message_uses_box_with_progress_bar() {
    let msg = DisplayMessage::background_task(
        "**Background task progress** `bg123` · `bash`\n\n[#####-------] 42% · Running tests (reported)",
    );

    let lines = render_background_task_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("◌ bg bash · bg123"));
    assert!(plain.contains("█"));
    assert!(plain.contains("░"));
    assert!(plain.contains("42%"));
    assert!(plain.contains("Running tests"));
    assert!(plain.contains("Latest status: bg action=\"status\" task_id=\"bg123\""));
    assert_eq!(
        plain.matches('│').count(),
        4,
        "expected compact progress row plus status hint:\n{plain}"
    );
    assert!(!plain.contains("Latest update"));
    assert!(!plain.contains("Source: reported"));
    assert!(!plain.contains("**Background task progress**"));
}

#[test]
fn render_background_task_messages_prefer_display_name() {
    let completion = DisplayMessage::background_task(
        "**Background task** `bg123` · `Run integration tests` (`bash`) · ✓ completed · 7.1s · exit 0\n\n_No output captured._\n\n_Full output:_ `bg action=\"output\" task_id=\"bg123\"`",
    );
    let completion_plain =
        render_background_task_message(&completion, 100, crate::config::DiffDisplayMode::Off)
            .iter()
            .map(extract_line_text)
            .collect::<Vec<_>>()
            .join("\n");
    assert!(completion_plain.contains("✓ bg Run integration tests completed · bg123"));

    let progress = DisplayMessage::background_task(
        "**Background task progress** `bg123` · `Run integration tests` (`bash`)\n\n[#####-------] 42% · Running tests (reported)",
    );
    let progress_plain =
        render_background_task_message(&progress, 100, crate::config::DiffDisplayMode::Off)
            .iter()
            .map(extract_line_text)
            .collect::<Vec<_>>()
            .join("\n");
    assert!(progress_plain.contains("◌ bg Run integration tests · bg123"));
}

#[test]
fn render_system_message_uses_scheduled_task_card() {
    let msg = DisplayMessage::system(
        "[Scheduled task]\nA scheduled task for this session is now due.\n\nTask: Follow up on the scheduler test\nWorking directory: /home/jeremy/jcode\nRelevant files: src/tui/ui_messages.rs\nBranch: master\n\nBackground: Verify the scheduled task card styling\nSuccess criteria: The due task renders clearly\nScheduled by session: session_test",
    );

    let lines = render_system_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("⏰ scheduled task due"));
    assert!(plain.contains("This scheduled task is now active in this session."));
    assert!(plain.contains("Follow up on the scheduler test"));
    assert!(plain.contains("Verify the scheduled task card styling"));
    assert!(!plain.contains("[Scheduled task]"));
    assert!(!plain.contains("A scheduled task for this session is now due."));
}

#[test]
fn render_tool_message_uses_scheduled_card() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "Scheduled task 'Follow up on the scheduler test' for in 1m (id: sched_abc123)\nWorking directory: /home/jeremy/jcode\nRelevant files: src/tui/ui_messages.rs\nTarget: resume session session_test".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: Some("scheduled: Follow up on the scheduler test".to_string()),
        tool_data: Some(crate::message::ToolCall {
            id: "call_schedule_card".to_string(),
            name: "schedule".to_string(),
            input: serde_json::json!({
                "task": "Follow up on the scheduler test",
                "wake_in_minutes": 1,
                "target": "resume"
            }),
            intent: None,
        }),
    };

    let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains(width_stable_system_title("⏰ scheduled", "scheduled")));
    assert!(plain.contains("Will run in 1m."));
    assert!(plain.contains("Follow up on the scheduler test"));
    assert!(plain.contains("session session_test"));
    assert!(plain.contains("sched_abc123"));
    assert!(!plain.contains("✓ schedule"));
}

#[test]
fn render_assistant_message_truncates_tool_calls_to_single_line() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(false);
    let msg = DisplayMessage {
        role: "assistant".to_string(),
        content: "Done.".to_string(),
        tool_calls: vec![
            "read".to_string(),
            "grep".to_string(),
            "apply_patch".to_string(),
            "batch".to_string(),
        ],
        duration_secs: None,
        title: None,
        tool_data: None,
    };

    let lines = render_assistant_message(&msg, 20, crate::config::DiffDisplayMode::Off);
    assert_eq!(extract_line_text(&lines[1]), "");
    let tool_lines: Vec<String> = lines
        .iter()
        .skip(2)
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect();

    assert!(
        tool_lines.len() == 1,
        "expected single-line tool-call summary: {tool_lines:?}"
    );
    assert!(
        tool_lines[0].contains("tools:"),
        "expected tool summary label on first line: {tool_lines:?}"
    );
    assert!(
        tool_lines.iter().all(|line| line.width() <= 20),
        "tool-call summary line should respect available width: {tool_lines:?}"
    );
    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn render_assistant_message_centers_single_line_tool_summary() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(true);
    let msg = DisplayMessage {
        role: "assistant".to_string(),
        content: "Done.".to_string(),
        tool_calls: vec![
            "read".to_string(),
            "grep".to_string(),
            "apply_patch".to_string(),
            "batch".to_string(),
        ],
        duration_secs: None,
        title: None,
        tool_data: None,
    };

    let lines = render_assistant_message(&msg, 28, crate::config::DiffDisplayMode::Off);
    assert_eq!(extract_line_text(&lines[1]), "");
    let tool_lines: Vec<String> = lines
        .iter()
        .skip(2)
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect();

    assert!(
        tool_lines.len() == 1,
        "expected single-line tool-call summary: {tool_lines:?}"
    );
    let first_pad = tool_lines[0].chars().take_while(|c| *c == ' ').count();
    assert!(
        first_pad > 0,
        "tool summary should still be padded/centered as a block: {tool_lines:?}"
    );
    assert!(
        lines
            .iter()
            .skip(2)
            .all(|line| line.alignment == Some(ratatui::layout::Alignment::Left)),
        "centered tool summary should use a shared left-aligned block pad"
    );

    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn render_assistant_message_without_body_does_not_add_extra_blank_line_before_tool_summary() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(false);
    let msg = DisplayMessage {
        role: "assistant".to_string(),
        content: String::new(),
        tool_calls: vec!["read".to_string()],
        duration_secs: None,
        title: None,
        tool_data: None,
    };

    let lines = render_assistant_message(&msg, 28, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();

    assert_eq!(rendered.len(), 1, "rendered={rendered:?}");
    assert!(rendered[0].contains("tool:"), "rendered={rendered:?}");

    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn render_assistant_message_centered_mode_keeps_markdown_unpadded_for_center_alignment() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(true);
    let msg = DisplayMessage::assistant(
        "streaming-block streaming-block streaming-block streaming-block",
    );

    let lines = render_assistant_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let content_line = lines
        .iter()
        .find(|line| extract_line_text(line).contains("streaming-block"))
        .expect("expected assistant markdown line");

    let first_pad = extract_line_text(content_line)
        .chars()
        .take_while(|c| *c == ' ')
        .count();
    assert_eq!(
        first_pad, 0,
        "centered assistant markdown should not inject left padding: {lines:?}"
    );
    assert_eq!(
        content_line.alignment, None,
        "assistant render should leave centered prose alignment unset for outer centering"
    );

    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn render_assistant_message_recenters_structured_markdown_to_actual_width() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(true);
    let msg = DisplayMessage::assistant("- one\n- two");

    let lines = render_assistant_message(&msg, 140, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();
    let bullets: Vec<&String> = rendered.iter().filter(|line| line.contains("• ")).collect();

    assert_eq!(
        bullets.len(),
        2,
        "expected two rendered bullet lines: {rendered:?}"
    );
    let first_pad = leading_spaces(bullets[0]);
    let second_pad = leading_spaces(bullets[1]);
    assert_eq!(
        first_pad, second_pad,
        "simple list should share a block pad: {rendered:?}"
    );
    assert!(
        first_pad > 45,
        "list should be re-centered to the full display width: {rendered:?}"
    );
    assert!(
        bullets
            .iter()
            .all(|line| line[leading_spaces(line)..].starts_with("• ")),
        "bullet markers should remain flush-left within the centered block: {rendered:?}"
    );

    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn render_system_message_centered_mode_caps_wrap_width_for_visible_gutters() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(true);
    let msg = DisplayMessage::system(
        "This is a long centered-mode system notification that should keep visible side gutters instead of stretching nearly edge to edge in a wide terminal.",
    );

    let lines = render_system_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect();

    assert!(
        rendered.iter().all(|line| line.starts_with("          ")),
        "centered system message should retain visible left padding in wide layouts: {rendered:?}"
    );

    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn render_system_message_uses_reload_card_for_reload_title() {
    let msg = DisplayMessage::system("Reloading server with newer binary...").with_title("Reload");

    let lines = render_system_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        plain.contains("reload"),
        "expected reload card title: {plain}"
    );
    assert!(plain.contains("Reloading server with newer binary"));
}

#[test]
fn render_system_message_uses_connection_card_for_reconnect_status() {
    let msg = DisplayMessage::system(
        "⚡ Connection lost — retrying (attempt 2, 7s) — connection reset by server · resume: jcode --resume koala",
    )
    .with_title("Connection");

    let lines = render_system_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        plain.contains("reconnecting"),
        "expected reconnect card title: {plain}"
    );
    assert!(plain.contains("Retrying · attempt 2 · 7s"));
    assert!(plain.contains("connection reset by server"));
    assert!(plain.contains("jcode --resume koala"));
}

#[test]
fn render_swarm_message_centered_mode_caps_wrap_width_for_long_notifications() {
    let saved = crate::tui::markdown::center_code_blocks();
    crate::tui::markdown::set_center_code_blocks(true);
    let msg = DisplayMessage::swarm(
        "File activity",
        "/home/jeremy/jcode/src/tui/ui_messages.rs — moss just edited this file while you were working nearby, so the notification should still read as centered in wide layouts.",
    );

    let lines = render_swarm_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();
    let first_pad = rendered[0].chars().take_while(|c| *c == ' ').count();

    assert!(
        first_pad >= 8,
        "centered swarm notification should keep a clearly visible left gutter: {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .all(|line| line.is_empty() || line.starts_with(&" ".repeat(first_pad))),
        "centered swarm notification should share one left pad across wrapped lines: {rendered:?}"
    );

    crate::tui::markdown::set_center_code_blocks(saved);
}

#[test]
fn render_tool_message_prefers_subagent_title_with_model() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "done".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: Some("Verify subagent model (general · gpt-5.4)".to_string()),
        tool_data: Some(crate::message::ToolCall {
            id: "call_1".to_string(),
            name: "subagent".to_string(),
            input: serde_json::json!({
                "description": "Verify subagent model",
                "subagent_type": "general"
            }),
            intent: None,
        }),
    };

    let lines = render_tool_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let rendered: String = lines[0]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();

    assert!(rendered.contains("subagent Verify subagent model (general · gpt-5.4)"));
}

#[test]
fn render_tool_message_shows_intent_and_technical_preview_on_one_line() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "ok".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_intent".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({
                "command": "cargo test -p jcode render_background_task --lib",
                "intent": "Verify compact progress card"
            }),
            intent: Some("Verify compact progress card".to_string()),
        }),
    };

    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered = extract_line_text(&lines[0]);

    assert!(rendered.contains("bash · Verify compact progress card · $ cargo test"));
    assert_eq!(
        lines.len(),
        1,
        "intent should not add vertical space: {rendered}"
    );
}

#[test]
fn render_tool_message_shows_token_badge() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "x".repeat(7_600),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_2".to_string(),
            name: "read".to_string(),
            input: serde_json::json!({"file_path": "src/main.rs"}),
            intent: None,
        }),
    };

    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let badge_span = lines[0]
        .spans
        .iter()
        .find(|span| span.content.contains("1.9k tok"))
        .expect("missing token badge");

    assert_eq!(badge_span.style.fg, Some(rgb(118, 118, 118)));
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
            intent: None,
        }),
    };

    let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Inline);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("┌─ diff"), "plain={plain}");
    assert!(plain.contains("old line"), "plain={plain}");
    assert!(plain.contains("new line"), "plain={plain}");
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
    assert!(
        plain.contains("new line 4 suffix_4_abcdefghijklmnopqrstuvwxyz0123456789"),
        "plain={plain}"
    );
    assert!(!plain.contains('…'), "plain={plain}");
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
