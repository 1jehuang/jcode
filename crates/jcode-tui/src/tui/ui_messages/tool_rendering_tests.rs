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
            thought_signature: None,
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
    crate::tui::ui::tools_ui::tests_tool_call_details_override::set(true);
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
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered = extract_line_text(&lines[0]);

    assert!(rendered.contains("bash · Verify compact progress card · $ cargo test"));
    assert_eq!(lines.len(), 1, "Bash output is hidden by default");
    crate::tui::ui::tools_ui::tests_tool_call_details_override::set(false);
}

/// Default (tool_call_details off): a row with an intent renders only the
/// intent; the dimmed technical preview is dropped and no fallback command
/// line is added.
#[test]
fn render_tool_message_hides_technical_preview_by_default() {
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
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered = extract_line_text(&lines[0]);

    assert!(
        rendered.contains("bash · Verify compact progress card"),
        "rendered={rendered}"
    );
    assert!(
        !rendered.contains("cargo test"),
        "technical detail should be hidden by default: {rendered}"
    );
    assert_eq!(lines.len(), 1, "Bash output is hidden by default");
}

/// Failures retain their normal header, with diagnostics on a muted second row.
#[test]
fn render_tool_message_keeps_error_summary_when_details_hidden() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "Error: command not found: cargoo".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_intent_err".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({
                "command": "cargoo test",
                "intent": "Run the test suite"
            }),
            intent: Some("Run the test suite".to_string()),
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered = extract_line_text(&lines[0]);

    assert!(
        rendered.contains("Run the test suite ·"),
        "normal intent and token suffix should remain: {rendered}"
    );
    assert!(!rendered.contains("command not found"));
    assert_eq!(lines.len(), 2);
    assert!(
        extract_line_text(&lines[1])
            .trim_start()
            .starts_with("╰─command not found")
    );
    assert!(
        lines[1]
            .spans
            .iter()
            .all(|span| span.content.trim().is_empty() || span.style.fg == Some(dim_color()))
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
            thought_signature: None,
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
fn render_tool_message_failures_keep_normal_headers_and_muted_reasons() {
    for (name, input, description) in [
        (
            "read",
            serde_json::json!({"file_path": "src/文件.rs"}),
            "src/文件.rs",
        ),
        (
            "edit",
            serde_json::json!({"file_path": "src/文件.rs", "old_string": "old", "new_string": "new"}),
            "src/文件.rs",
        ),
        (
            "bash",
            serde_json::json!({"command": "cat 文件.rs"}),
            "cat 文件.rs",
        ),
    ] {
        for intent in [None, Some("Inspect 文件 e\u{301}")] {
            let msg = DisplayMessage {
                role: "tool".to_string(),
                content: "Error: permission denied 文件\nFull diagnostic retained".to_string(),
                tool_calls: Vec::new(),
                duration_secs: None,
                title: None,
                tool_data: Some(crate::message::ToolCall {
                    id: "failure_style".to_string(),
                    name: name.to_string(),
                    input: input.clone(),
                    intent: intent.map(str::to_string),
                    thought_signature: None,
                }),
            };
            let lines = render_tool_message(&msg, 140, crate::config::DiffDisplayMode::Inline);
            assert_eq!(lines.len(), 2, "{name}");
            let header = extract_line_text(&lines[0]);
            assert!(
                header.trim_start().starts_with(&format!("✗ {name}")),
                "{header}"
            );
            assert!(header.contains(intent.unwrap_or(description)), "{header}");
            assert!(!header.contains("permission denied"));
            assert!(lines[0].spans.iter().any(
                |span| span.content.contains("✗") && span.style.fg == Some(rgb(220, 100, 100))
            ));
            assert!(
                lines[0]
                    .spans
                    .iter()
                    .any(|span| span.content == name && span.style.fg == Some(tool_color()))
            );
            assert!(
                extract_line_text(&lines[1])
                    .trim_start()
                    .starts_with("╰─permission denied")
            );
            assert!(
                lines[1].spans.iter().all(
                    |span| span.content.trim().is_empty() || span.style.fg == Some(dim_color())
                )
            );
            assert!(msg.content.contains("Full diagnostic retained"));
            for width in [0, 1, 2, 5, 8, 16, 32] {
                for line in render_tool_message(&msg, width, crate::config::DiffDisplayMode::Inline)
                {
                    assert!(
                        UnicodeWidthStr::width(extract_line_text(&line).as_str()) <= width as usize
                    );
                }
            }
            let mut successful = msg.clone();
            successful.content = "ok".to_string();
            let success_lines =
                render_tool_message(&successful, 140, crate::config::DiffDisplayMode::Off);
            assert_eq!(
                header.find('✗'),
                extract_line_text(&success_lines[0]).find('✓')
            );
            assert_eq!(
                extract_line_text(&lines[1]).find('╰').unwrap(),
                header.find('✗').unwrap()
            );
            assert!(
                extract_line_text(&success_lines[0])
                    .trim_start()
                    .starts_with(&format!("✓ {name}"))
            );
            assert!(
                !success_lines
                    .iter()
                    .any(|line| extract_line_text(line).contains("╰"))
            );
        }
    }
}

#[test]
fn render_tool_message_mixed_batch_keeps_call_labels_and_nested_failure_reason() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "--- [1] read ---\nError: permission denied 文件\nFull diagnostic\n\n--- [2] bash ---\nok\n\nCompleted: 1 succeeded, 1 failed".to_string(),
        tool_calls: Vec::new(), duration_secs: None, title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "mixed".to_string(), name: "batch".to_string(),
            input: serde_json::json!({"tool_calls": [
                {"tool": "read", "parameters": {"file_path": "文件.rs"}, "intent": "Read 文件"},
                {"tool": "bash", "parameters": {"command": "true"}, "intent": "Check success"}
            ]}), intent: None, thought_signature: None,
        }),
    };
    let lines = render_tool_message(&msg, 140, crate::config::DiffDisplayMode::Off);
    assert_eq!(lines.len(), 4);
    assert_eq!(
        extract_line_text(&lines[2]).find('╰').unwrap(),
        extract_line_text(&lines[1]).find('✗').unwrap()
    );
    assert!(extract_line_text(&lines[0]).contains("⚠ batch"));
    assert!(
        extract_line_text(&lines[1])
            .trim_start()
            .starts_with("✗ read · Read 文件")
    );
    assert!(!extract_line_text(&lines[1]).contains("permission"));
    assert!(
        extract_line_text(&lines[2])
            .trim_start()
            .starts_with("╰─permission denied")
    );
    assert!(
        lines[2]
            .spans
            .iter()
            .all(|span| span.content.trim().is_empty() || span.style.fg == Some(dim_color()))
    );
    assert!(
        extract_line_text(&lines[3])
            .trim_start()
            .starts_with("✓ bash · Check success")
    );
    for width in [0, 1, 2, 8, 16, 32] {
        for line in render_tool_message(&msg, width, crate::config::DiffDisplayMode::Off) {
            assert!(UnicodeWidthStr::width(extract_line_text(&line).as_str()) <= width as usize);
        }
    }
}

#[test]
fn render_tool_message_batch_level_failure_does_not_invent_successful_children() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "Error: invalid tool_calls payload".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "invalid_batch".to_string(),
            name: "batch".to_string(),
            input: serde_json::json!({"tool_calls": [{"tool": "read"}]}),
            intent: Some("Inspect sources".to_string()),
            thought_signature: None,
        }),
    };
    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    assert_eq!(lines.len(), 2);
    assert!(extract_line_text(&lines[0]).contains("✗ batch · Inspect sources"));
    assert!(extract_line_text(&lines[1]).contains("╰─invalid tool_calls payload"));
}

#[test]
fn compact_error_rows_normalize_logged_wrappers_and_align_under_status() {
    let raw = "Error: [selfdev] Error: `selfdev build` requires a non-empty `reason` so other queued agents can see why the build is needed.";
    let mut msg = DisplayMessage::system(raw);
    msg.role = "tool".to_string();
    msg.tool_data = Some(crate::message::ToolCall {
        id: "logged-selfdev".to_string(),
        name: "selfdev".to_string(),
        input: serde_json::json!({"action": "build"}),
        intent: Some("Build UI".to_string()),
        thought_signature: None,
    });
    let lines = render_tool_message(&msg, 180, crate::config::DiffDisplayMode::Off);
    assert_eq!(lines.len(), 2);
    let reason = extract_line_text(&lines[1]);
    assert!(
        reason.contains("`selfdev build` requires a non-empty `reason`"),
        "{reason}"
    );
    assert!(
        !reason.contains("Error:") && !reason.contains("error:") && !reason.contains("[selfdev]"),
        "{reason}"
    );
    let wide_reason = extract_line_text(&render_tool_failure_reason(raw, 180, 2));
    assert!(
        !wide_reason.contains('…'),
        "wide reasons must not be pre-truncated: {wide_reason}"
    );
    assert_eq!(extract_line_text(&lines[0]).find('✗'), reason.find('╰'));
    assert_eq!(msg.content, raw, "normalization must not change raw logs");
}

#[test]
fn background_errors_share_cause_selection_and_deduplicate_exit_codes() {
    for (preview, expected) in [
        (
            "Error: [bash] Error: Command exited with code 101\n[stderr] error[E0308]: mismatched types",
            "exit 101 · [E0308]: mismatched types",
        ),
        (
            "Error: [bash] Error: Command exited with code 101",
            "exit 101",
        ),
    ] {
        let msg = DisplayMessage::background_task(format!(
            "**Background task** `logged-bg` · `bash` · ✗ failed · 1.0s · exit 101\n\n```text\n{preview}\n```\n\n_Full output:_ `bg action=\"output\" task_id=\"logged-bg\"`"
        ));
        let lines = render_background_task_message(&msg, 160, crate::config::DiffDisplayMode::Off);
        let reason = extract_line_text(&lines[1]);
        assert!(reason.ends_with(expected), "{reason}");
        assert_eq!(reason.matches("exit 101").count(), 1, "{reason}");
        assert_eq!(extract_line_text(&lines[0]).find('✗'), reason.find('╰'));
    }
}

#[test]
fn background_explicit_failure_reason_survives_without_prefix_or_preview() {
    let msg = DisplayMessage::background_task(
        "**Background task** `reason-only` · `bash` · ✗ failed · 1.0s · exit 1\n\n_Failure:_ permission denied\n\n_No output captured._\n\n_Full output:_ `bg action=\"output\" task_id=\"reason-only\"`",
    );
    let lines = render_background_task_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    assert!(extract_line_text(&lines[1]).ends_with("exit 1 · permission denied"));
}

#[test]
fn tool_failure_reason_handles_exit_codes_fallback_and_unicode_widths() {
    for (content, expected) in [
        (
            "details\n--- Command finished with exit code: 17 ---",
            "exit 17",
        ),
        (
            "[apply_patch] ✗ 文件: Failed to find expected lines\nlarge diff",
            "Failed to find expected lines",
        ),
        (
            "\u{1b}[31mError: 文件 e\u{301} missing\u{1b}[0m\nmore detail",
            "文件 e\u{301} missing",
        ),
        ("", "Tool call failed"),
        (
            "[tool timing: start=2026-10-23T00:00:00Z]\n[stderr] warning: unused variable\n[stderr] error[E0308]: mismatched types\n--- Command finished with exit code: 101 ---",
            "[E0308]: mismatched types",
        ),
        (
            "[tool timing: start=2026-10-23T00:00:00Z]\n[stderr] Error: permission denied",
            "permission denied",
        ),
    ] {
        let reason = render_tool_failure_reason(content, 120, 4);
        let text = extract_line_text(&reason);
        assert!(text.contains(expected), "{text}");
        assert!(!text.contains('\u{1b}'));
        for width in [0, 1, 4, 8, 16, 32] {
            let line = render_tool_failure_reason(content, width, 4);
            assert!(UnicodeWidthStr::width(extract_line_text(&line).as_str()) <= width);
        }
    }
}

#[test]
fn render_tool_message_hides_bash_output() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "<class 'zip'>\n[('p', 'b'), ('a', 'a'), ('l', 'l'), ('e', 'e')]".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_bash_output".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({
                "command": "python3 -c \"s='pale'; t='bale'; print(type(zip(s,t))); print(list(zip(s,t)))\""
            }),
            intent: None,
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered = lines.iter().map(extract_line_text).collect::<Vec<_>>();

    assert!(!rendered.iter().any(|line| line.contains("<class 'zip'>")));
    assert!(!rendered.iter().any(|line| line.contains("[('p', 'b')")));
}

#[test]
fn render_tool_message_shows_bash_output_when_enabled() {
    crate::tui::ui::tools_ui::tests_show_bash_output_override::set(true);
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "one\ntwo\nthree\nfour".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_bash_output_enabled".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({"command": "printf output"}),
            intent: Some("Print output".to_string()),
            thought_signature: None,
        }),
    };

    let rendered = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off)
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>();

    assert_eq!(rendered.len(), 4);
    assert!(!rendered.iter().any(|line| line.trim() == "one"));
    assert!(rendered.iter().any(|line| line.trim() == "two"));
    assert!(rendered.iter().any(|line| line.trim() == "four"));
    crate::tui::ui::tools_ui::tests_show_bash_output_override::set(false);
}

fn discovery_message(content: &str, input: serde_json::Value) -> DisplayMessage {
    DisplayMessage {
        role: "tool".to_string(),
        content: content.to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_discovery".to_string(),
            name: "integration_tools".to_string(),
            input,
            intent: None,
            thought_signature: None,
        }),
    }
}

#[test]
fn render_tool_message_shows_discovery_browse_results_and_rationale() {
    let msg = discovery_message(
        "Discoverable tools in 'payments' (Jcode tool directory; recommendations must be based only on fit; details: https://jcode.sh/discovery-tools):\n\n- agentcard: prepaid virtual Visa cards for AI agents (https://agentcard.sh/?via=jcode-discovery)\n\nSearch request ID: `11111111-2222-4333-8444-555555555555`",
        serde_json::json!({
            "action": "search",
            "category": "payments",
            "query": "manage Stripe sandbox products and recurring prices",
            "reason": "the task needs test-mode catalog administration through scoped agent access"
        }),
    );
    let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(plain.contains("agentcard"), "{plain}");
    assert!(
        !plain.contains("1 integration"),
        "single-result browse shows only the entry name: {plain}"
    );
    assert!(
        !plain.contains("why:"),
        "browse results stay to a single line without rationale: {plain}"
    );
    assert!(
        !plain.contains("prepaid virtual Visa cards"),
        "browse results must not render descriptions: {plain}"
    );
    assert!(
        !plain.contains("agentcard.sh"),
        "browse results must not render URLs: {plain}"
    );
    assert!(
        !plain.contains("Listings are vetted"),
        "discovery results must not render the disclosure notice: {plain}"
    );
    assert!(!plain.contains("sponsored result"), "{plain}");
    assert!(
        lines.len() <= 8,
        "compact discovery details used {} lines: {plain}",
        lines.len()
    );
    assert!(
        !plain.contains("\n\n"),
        "compact details contain a blank row: {plain}"
    );
    assert!(
        !plain
            .chars()
            .any(|ch| matches!(ch, '╭' | '╮' | '╰' | '╯' | '│')),
        "discovery details must remain borderless: {plain}"
    );
    assert!(
        !plain.contains("11111111-2222"),
        "request IDs stay model-only: {plain}"
    );
}

#[test]
fn batched_discovery_renders_without_disclosure_notice() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "--- [1] integration_tools ---\nAvailable integrations in 'payments' (Jcode tool directory; recommendations must be based only on fit; details: https://jcode.sh/discovery-tools):\n\n- agentcard: prepaid virtual Visa cards for AI agents (https://agentcard.sh/?via=jcode-discovery)\n\nSearch request ID: `11111111-2222-4333-8444-555555555555`\n\nCompleted: 1 succeeded, 0 failed".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_batch_discovery".to_string(),
            name: "batch".to_string(),
            input: serde_json::json!({
                "tool_calls": [{
                    "tool": "integration_tools",
                    "parameters": {
                        "action": "search",
                        "category": "payments",
                        "query": "issue a capped virtual card",
                        "reason": "the task requires a payment instrument with a hard limit"
                    }
                }]
            }),
            intent: None,
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("agentcard"), "{plain}");
    assert!(
        !plain.contains("1 integration"),
        "single-result browse shows only the entry name: {plain}"
    );
    assert!(
        !plain.contains("Listings are vetted"),
        "batched discovery must not render the disclosure notice: {plain}"
    );
    assert!(
        !plain
            .chars()
            .any(|ch| matches!(ch, '╭' | '╮' | '╰' | '╯' | '│')),
        "batched discovery details must remain borderless: {plain}"
    );
}

#[test]
fn render_tool_message_shows_selected_discovery_setup() {
    let msg = discovery_message(
        "Selected 'agentcard' from 'payments' (Jcode tool directory; selection must be based only on fit; details: https://jcode.sh/discovery-tools):\n\nagentcard: prepaid virtual Visa cards for AI agents (https://agentcard.sh/?via=jcode-discovery)\n\nSetup: Run `npx -y agentcard-mcp@1.2.3`, then connect the resulting MCP server.\n\nConsequential actions (signups, spending) must note the partnership in the confirmation shown to the user.",
        serde_json::json!({
            "action": "select",
            "category": "payments",
            "tool": "agentcard",
            "query": "create a capped virtual card for an online purchase",
            "reason": "selected because capped cards fit the purchase constraints better than alternatives"
        }),
    );
    let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(plain.contains("selected agentcard"), "{plain}");
    assert!(!plain.contains("sponsored"), "{plain}");
    assert!(
        plain.contains("details: prepaid virtual Visa cards"),
        "{plain}"
    );
    assert!(plain.contains("https://agentcard.sh"), "{plain}");
    assert!(plain.contains("setup:"), "{plain}");
    assert!(plain.contains("agentcard-mcp@1.2.3"), "{plain}");
    assert!(
        !plain.contains("Listings are vetted"),
        "discovery results must not render the disclosure notice: {plain}"
    );
}

#[test]
fn render_tool_message_does_not_duplicate_selected_when_tool_is_missing() {
    let msg = discovery_message(
        "Selection recorded.",
        serde_json::json!({
            "action": "select",
            "category": "web-search",
            "query": "find current public estimates"
        }),
    );
    let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(plain.contains("selected tool"), "{plain}");
    assert!(!plain.contains("selected selected tool"), "{plain}");
}

#[test]
fn render_tool_message_marks_off_catalog_selection_without_fake_details() {
    let msg = discovery_message(
        "Selected off-catalog product 'firecrawl' for 'web-data'.\n\nSelection recorded as demand data. Jcode does not list or partner with this product, so no provider information, recommendation, or setup instructions are provided.",
        serde_json::json!({
            "action": "select",
            "category": "web-data",
            "tool": "firecrawl",
            "query": "crawl a documentation site and extract structured markdown",
            "reason": "the user explicitly requested Firecrawl instead of the catalog listing"
        }),
    );
    let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(plain.contains("selected off-catalog firecrawl"), "{plain}");
    assert!(
        plain.contains("why: the user explicitly requested"),
        "{plain}"
    );
    assert!(!plain.contains("details:"), "{plain}");
    assert!(!plain.contains("setup:"), "{plain}");
}

#[test]
fn render_tool_message_shows_catalog_suggestion_receipt_and_trust_line() {
    let msg = discovery_message(
        "Catalog suggestion submitted.\n\nSuggestion ID: aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee\nCategory: payments\nKind: known_product\nCapability: manage Stripe sandbox products\nCatalog gap: no matching catalog entry\nProduct: Stripe sandbox MCP\nPublic URL: https://example.com/stripe-mcp\n\nStatus: received for Jcode maintainer review. Suggestions are not sent to partners. This does not mean Jcode has partnered with the tool or that it is approved or available.",
        serde_json::json!({
            "action": "suggest",
            "category": "payments",
            "query": "manage Stripe sandbox products and recurring prices through scoped agent access",
            "reason": "the listed payment tool only provides cards and cannot administer Stripe test data",
            "suggestion_kind": "known_product",
            "product_name": "Stripe sandbox MCP",
            "product_url": "https://example.com/stripe-mcp",
            "gap_evidence": "Agentcard handles cards rather than Stripe test-mode objects.",
            "requirements": [
                "Scoped authentication without exposing a secret key",
                "Create recurring prices in test mode"
            ],
            "prior_request_id": "11111111-2222-4333-8444-555555555555"
        }),
    );
    let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(plain.contains("suggestion sent"), "{plain}");
    assert!(
        plain.contains("Known product · Stripe sandbox MCP"),
        "{plain}"
    );
    assert!(plain.contains("gap: the listed payment tool"), "{plain}");
    assert!(plain.contains("needs:"), "{plain}");
    assert!(plain.contains("Jcode maintainers only"), "{plain}");
    assert!(plain.contains("not approval or availability"), "{plain}");
    assert!(
        !plain.contains("11111111-2222"),
        "prior request ID must stay hidden: {plain}"
    );
}

#[test]
fn discovery_cards_wrap_within_narrow_transcript_width() {
    let msg = discovery_message(
        "Catalog suggestion submitted.\n\nStatus: received for Jcode maintainer review.",
        serde_json::json!({
            "action": "suggest",
            "category": "cloud-infrastructure",
            "query": "a deliberately long capability description that must wrap cleanly in a narrow terminal",
            "reason": "the current catalog entries do not satisfy several detailed infrastructure constraints",
            "suggestion_kind": "capability_gap",
            "requirements": ["A long requirement that also needs reliable narrow-width wrapping"]
        }),
    );
    let lines = render_tool_message(&msg, 48, crate::config::DiffDisplayMode::Off);
    assert!(
        lines.iter().all(|line| line.width() <= 47),
        "discovery card exceeded width: {:?}",
        lines.iter().map(extract_line_text).collect::<Vec<_>>()
    );
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
