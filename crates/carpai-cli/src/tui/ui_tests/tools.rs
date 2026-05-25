use super::*;

#[test]
fn test_summarize_apply_patch_input_ignores_begin_marker() {
    let patch = "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-old\n+new\n*** End Patch\n";
    let summary = tools_ui::summarize_apply_patch_input(patch);
    assert_eq!(summary, "src/lib.rs (6 lines)");
}

#[test]
fn test_summarize_apply_patch_input_multiple_files() {
    let patch = "*** Begin Patch\n*** Update File: a.txt\n@@\n-a\n+b\n*** Update File: b.txt\n@@\n-c\n+d\n*** End Patch\n";
    let summary = tools_ui::summarize_apply_patch_input(patch);
    assert_eq!(summary, "2 files (10 lines)");
}

#[test]
fn test_extract_apply_patch_primary_file() {
    let patch = "*** Begin Patch\n*** Add File: new/file.rs\n+fn main() {}\n*** End Patch\n";
    let file = tools_ui::extract_apply_patch_primary_file(patch);
    assert_eq!(file.as_deref(), Some("new/file.rs"));
}

#[test]
fn test_batch_subcall_params_supports_flat_and_nested_shapes() {
    let flat = serde_json::json!({
        "tool": "read",
        "file_path": "src/session.rs",
        "offset": 0,
        "limit": 420
    });
    let nested = serde_json::json!({
        "tool": "read",
        "parameters": {
            "file_path": "src/main.rs",
            "offset": 2320,
            "limit": 220
        }
    });

    let flat_params = tools_ui::batch_subcall_params(&flat);
    let nested_params = tools_ui::batch_subcall_params(&nested);

    assert_eq!(flat_params["file_path"], "src/session.rs");
    assert_eq!(flat_params["offset"], 0);
    assert_eq!(flat_params["limit"], 420);

    assert_eq!(nested_params["file_path"], "src/main.rs");
    assert_eq!(nested_params["offset"], 2320);
    assert_eq!(nested_params["limit"], 220);
}

#[test]
fn test_batch_subcall_params_excludes_name_key() {
    let with_name = serde_json::json!({
        "name": "read",
        "file_path": "src/lib.rs",
        "offset": 0,
        "limit": 100
    });
    let params = tools_ui::batch_subcall_params(&with_name);
    assert_eq!(params["file_path"], "src/lib.rs");
    assert_eq!(params["offset"], 0);
    assert!(params.get("name").is_none());
    assert!(params.get("tool").is_none());
}

#[test]
fn test_parse_batch_sub_outputs_strips_footer_and_tracks_errors() {
    let content = "--- [1] read ---\n1234\n\n--- [2] grep ---\nError: 12345678\n\nCompleted: 1 succeeded, 1 failed";

    let results = tools_ui::parse_batch_sub_outputs(content);

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].content, "1234");
    assert!(!results[0].errored);
    assert_eq!(results[1].content, "Error: 12345678");
    assert!(results[1].errored);
}

#[test]
fn test_parse_batch_sub_outputs_keeps_final_header_without_trailing_newline() {
    let content = "--- [1] read ---\n1234\n\n--- [2] grep ---";

    let results = tools_ui::parse_batch_sub_outputs(content);

    assert_eq!(results.len(), 2, "results={results:?}");
    assert_eq!(results[0].content, "1234");
    assert_eq!(results[1].content, "");
    assert!(!results[1].errored);
}

#[test]
fn test_render_tool_message_batch_flat_subcall_params_include_read_details() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "--- [1] read ---\nok\n\n--- [2] read ---\nok\n\nCompleted: 2 succeeded, 0 failed"
            .to_string(),
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: Some(ToolCall {
            id: "call_batch_1".to_string(),
            name: "batch".to_string(),
            input: serde_json::json!({
                "tool_calls": [
                    {"tool": "read", "file_path": "src/session.rs", "offset": 0, "limit": 420},
                    {"tool": "read", "file_path": "src/main.rs", "offset": 2320, "limit": 220}
                ]
            }),
            intent: None,
        }),
    };

    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();

    assert_eq!(rendered.len(), 3, "rendered={rendered:?}");
    assert!(
        rendered[0].contains("✓ batch 2 calls"),
        "rendered={rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("✓ read src/session.rs:0-420")),
        "missing first read subtool in {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("✓ read src/main.rs:2320-2540")),
        "missing second read subtool in {rendered:?}"
    );
}

#[test]
fn test_render_tool_message_batch_subcalls_show_individual_token_badges() {
    let msg = DisplayMessage {
            role: "tool".to_string(),
            content:
                "--- [1] read ---\n1234\n\n--- [2] grep ---\n12345678\n\nCompleted: 2 succeeded, 0 failed"
                    .to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: Some(ToolCall {
                id: "call_batch_tokens".to_string(),
                name: "batch".to_string(),
                input: serde_json::json!({
                    "tool_calls": [
                        {"tool": "read", "file_path": "src/session.rs", "offset": 0, "limit": 1},
                        {"tool": "grep", "pattern": "TODO", "path": "src"}
                    ]
                }),
                intent: None,
            }),
        };

    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();

    assert_eq!(rendered.len(), 3, "rendered={rendered:?}");
    assert!(
        rendered[0].contains("✓ batch 2 calls"),
        "rendered={rendered:?}"
    );
    assert!(
        rendered[1].contains("read src/session.rs:0-1") && rendered[1].contains("1 tok"),
        "rendered={rendered:?}"
    );
    assert!(
        rendered[2].contains("grep 'TODO' in src") && rendered[2].contains("2 tok"),
        "rendered={rendered:?}"
    );
}

#[test]
fn test_render_tool_message_batch_last_subcall_keeps_token_badge_without_trailing_newline() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "--- [1] read ---\n1234\n\n--- [2] grep ---".to_string(),
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: Some(ToolCall {
            id: "call_batch_tokens_no_newline".to_string(),
            name: "batch".to_string(),
            input: serde_json::json!({
                "tool_calls": [
                    {"tool": "read", "file_path": "src/session.rs", "offset": 0, "limit": 1},
                    {"tool": "grep", "pattern": "TODO", "path": "src"}
                ]
            }),
            intent: None,
        }),
    };

    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();

    assert_eq!(rendered.len(), 3, "rendered={rendered:?}");
    assert!(
        rendered[1].contains("read src/session.rs:0-1") && rendered[1].contains("1 tok"),
        "rendered={rendered:?}"
    );
    assert!(
        rendered[2].contains("grep 'TODO' in src") && rendered[2].contains("0 tok"),
        "rendered={rendered:?}"
    );
}

#[test]
fn test_render_tool_message_batch_partial_failure_shows_all_subcalls() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "--- [1] read ---
ok

--- [2] agentgrep ---
Error: missing field `mode`

--- [3] grep ---
ok

Completed: 2 succeeded, 1 failed"
            .to_string(),
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: Some(ToolCall {
            id: "call_batch_partial".to_string(),
            name: "batch".to_string(),
            input: serde_json::json!({
                "tool_calls": [
                    {"tool": "read", "file_path": "src/lib.rs"},
                    {"tool": "agentgrep"},
                    {"tool": "grep", "pattern": "TODO", "path": "src"}
                ]
            }),
            intent: Some("Inspect schemas".to_string()),
        }),
    };

    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();

    assert!(
        rendered[0].contains("⚠ batch · Inspect schemas · 2/3 succeeded"),
        "rendered={rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("✗ agentgrep invalid input: missing mode")),
        "failed subcall should be attributed to agentgrep: {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("✓ read src/lib.rs")),
        "successful read subcall should still be visible: {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("✓ grep 'TODO' in src")),
        "successful grep subcall should still be visible: {rendered:?}"
    );
}

#[test]
fn test_render_tool_message_batch_all_failed_marks_all_children_failed() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "--- [1] agentgrep ---\nError: missing field `mode`\n\n--- [2] agentgrep ---\nError: missing field `mode`\n\n--- [3] agentgrep ---\nError: missing field `mode`\n\nCompleted: 0 succeeded, 3 failed"
            .to_string(),
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: Some(ToolCall {
            id: "call_batch_all_failed".to_string(),
            name: "batch".to_string(),
            input: serde_json::json!({
                "tool_calls": [
                    {"tool": "agentgrep"},
                    {"tool": "agentgrep"},
                    {"tool": "agentgrep"}
                ]
            }),
            intent: None,
        }),
    };

    let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();

    assert!(
        rendered[0].contains("✗ batch 3/3 failed"),
        "rendered={rendered:?}"
    );
    let failed_children = rendered
        .iter()
        .filter(|line| line.contains("✗ agentgrep invalid input: missing mode"))
        .count();
    assert_eq!(failed_children, 3, "rendered={rendered:?}");
    assert!(
        !rendered
            .iter()
            .any(|line| line.contains("✓ agentgrep") || line.contains("agentgrep missing mode")),
        "rendered={rendered:?}"
    );
}

#[test]
fn test_tool_summary_read_supports_start_line_end_line() {
    let tool = ToolCall {
        id: "call_read_range".to_string(),
        name: "read".to_string(),
        input: serde_json::json!({
            "file_path": "src/tool/read.rs",
            "start_line": 10,
            "end_line": 20
        }),
        intent: None,
    };

    let summary = tools_ui::get_tool_summary_with_budget(&tool, 50, Some(40));
    assert!(summary.contains("read.rs:10-20"), "summary={summary:?}");
}

#[test]
fn test_render_tool_message_batch_includes_start_end_read_details() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "--- [1] read ---\nok\n\nCompleted: 1 succeeded, 0 failed".to_string(),
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: Some(ToolCall {
            id: "call_batch_range".to_string(),
            name: "batch".to_string(),
            input: serde_json::json!({
                "tool_calls": [
                    {"tool": "read", "file_path": "src/tool/read.rs", "start_line": 10, "end_line": 20}
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
            .any(|line| line.contains("✓ read src/tool/read.rs:10-20")),
        "missing read subtool in {rendered:?}"
    );
}

#[test]
fn test_tool_summary_path_truncation_keeps_filename_tail() {
    let tool = ToolCall {
        id: "call_read_tail".to_string(),
        name: "read".to_string(),
        input: serde_json::json!({
            "file_path": "src/tui/really/long/nested/location/ui_messages.rs",
            "offset": 120,
            "limit": 40
        }),
        intent: None,
    };

    let summary = tools_ui::get_tool_summary_with_budget(&tool, 50, Some(28));

    assert!(summary.contains("ui_messages.rs"), "summary={summary:?}");
    assert!(summary.contains(":120-160"), "summary={summary:?}");
    assert!(summary.contains('…'), "summary={summary:?}");
    assert!(unicode_width::UnicodeWidthStr::width(summary.as_str()) <= 28);
}

#[test]
fn test_tool_summary_grep_truncation_prefers_middle() {
    let tool = ToolCall {
        id: "call_grep_middle".to_string(),
        name: "grep".to_string(),
        input: serde_json::json!({
            "pattern": "prefix_[A-Z0-9]+_important_middle_token_[a-z]+_suffix",
            "path": "src/some/really/long/module"
        }),
        intent: None,
    };

    let summary = tools_ui::get_tool_summary_with_budget(&tool, 50, Some(34));

    assert!(
        summary.contains("importan") || summary.contains("token"),
        "summary={summary:?}"
    );
    assert!(
        summary.contains("suffix") || summary.contains("module"),
        "summary={summary:?}"
    );
    assert!(summary.contains('…'), "summary={summary:?}");
    assert!(unicode_width::UnicodeWidthStr::width(summary.as_str()) <= 34);
}

#[test]
fn test_tool_summary_bash_truncation_keeps_start_and_end() {
    let tool = ToolCall {
        id: "call_bash_middle".to_string(),
        name: "bash".to_string(),
        input: serde_json::json!({
            "command": "cargo test --package jcode --lib tui::ui::tests::render_tool_message_batch_flat_subcall_params_include_read_details -- --nocapture"
        }),
        intent: None,
    };

    let summary = tools_ui::get_tool_summary_with_budget(&tool, 32, Some(34));

    assert!(summary.starts_with("$ cargo"), "summary={summary:?}");
    assert!(
        summary.contains("nocapture") || summary.contains("read_details"),
        "summary={summary:?}"
    );
    assert!(summary.contains('…'), "summary={summary:?}");
    assert!(unicode_width::UnicodeWidthStr::width(summary.as_str()) <= 34);
}

#[test]
fn test_tool_summary_bash_keeps_full_command_when_width_fits() {
    let tool = ToolCall {
        id: "call_bash_full".to_string(),
        name: "bash".to_string(),
        input: serde_json::json!({
            "command": "cargo test --package jcode --lib tui::ui::tests::render_tool_message_batch_rows_do_not_soft_wrap_on_narrow_width -- --nocapture"
        }),
        intent: None,
    };

    let summary = tools_ui::get_tool_summary_with_budget(&tool, 32, Some(160));

    assert_eq!(
        summary,
        "$ cargo test --package jcode --lib tui::ui::tests::render_tool_message_batch_rows_do_not_soft_wrap_on_narrow_width -- --nocapture"
    );
    assert!(!summary.contains('…'), "summary={summary:?}");
}

#[test]
fn test_render_batch_subcall_line_keeps_full_bash_summary_when_row_fits() {
    let tool = ToolCall {
        id: "batch-1-bash".to_string(),
        name: "bash".to_string(),
        input: serde_json::json!({
            "command": "cargo test --package jcode --lib tui::ui::tests::render_tool_message_batch_rows_do_not_soft_wrap_on_narrow_width -- --nocapture"
        }),
        intent: None,
    };

    let line =
        tools_ui::render_batch_subcall_line(&tool, "✓", rgb(100, 180, 100), 32, Some(160), None);
    let rendered = extract_line_text(&line);

    assert!(
        rendered.contains("bash $ cargo test --package jcode"),
        "rendered={rendered:?}"
    );
    assert!(rendered.contains("-- --nocapture"), "rendered={rendered:?}");
    assert!(!rendered.contains('…'), "rendered={rendered:?}");
}

#[test]
fn test_agentgrep_summary_uses_default_grep_mode_query() {
    let tool = ToolCall {
        id: "agentgrep-default-mode".to_string(),
        name: "agentgrep".to_string(),
        input: serde_json::json!({
            "query": "pending_soft_interrupt",
            "path": "src/tui"
        }),
        intent: None,
    };

    let summary = tools_ui::get_tool_summary_with_budget(&tool, 50, Some(120));

    assert_eq!(summary, "grep 'pending_soft_interrupt'");
}

#[test]
fn test_render_batch_subcall_line_shows_first_subcall_token_badge() {
    let tool = ToolCall {
        id: "agentgrep-default-mode".to_string(),
        name: "agentgrep".to_string(),
        input: serde_json::json!({
            "query": "pending_soft_interrupt",
            "path": "src/tui"
        }),
        intent: None,
    };

    let line = tools_ui::render_batch_subcall_line(
        &tool,
        "✓",
        rgb(100, 180, 100),
        50,
        Some(120),
        Some("query: pending_soft_interrupt\nmatches: 1 in 1 files\n"),
    );
    let rendered = extract_line_text(&line);

    assert!(
        rendered.contains("agentgrep grep 'pending_soft_interrupt'"),
        "rendered={rendered:?}"
    );
    assert!(rendered.contains("tok"), "rendered={rendered:?}");
}

#[test]
fn test_common_tool_summaries_keep_full_text_when_row_budget_fits() {
    let cases = vec![
        (
            ToolCall {
                id: "read-wide".to_string(),
                name: "read".to_string(),
                input: serde_json::json!({
                    "file_path": "src/tui/ui_messages.rs",
                    "offset": 120,
                    "limit": 40
                }),
                intent: None,
            },
            "src/tui/ui_messages.rs:120-160",
        ),
        (
            ToolCall {
                id: "grep-wide".to_string(),
                name: "grep".to_string(),
                input: serde_json::json!({
                    "pattern": "render_batch_subcall_line",
                    "path": "src/tui"
                }),
                intent: None,
            },
            "'render_batch_subcall_line' in src/tui",
        ),
        (
            ToolCall {
                id: "glob-wide".to_string(),
                name: "glob".to_string(),
                input: serde_json::json!({
                    "pattern": "src/tui/**/*.rs"
                }),
                intent: None,
            },
            "'src/tui/**/*.rs'",
        ),
        (
            ToolCall {
                id: "webfetch-wide".to_string(),
                name: "webfetch".to_string(),
                input: serde_json::json!({
                    "url": "https://example.com/docs/api/reference"
                }),
                intent: None,
            },
            "https://example.com/docs/api/reference",
        ),
        (
            ToolCall {
                id: "open-wide".to_string(),
                name: "open".to_string(),
                input: serde_json::json!({
                    "action": "open",
                    "target": "src/tui/ui.rs"
                }),
                intent: None,
            },
            "open src/tui/ui.rs",
        ),
        (
            ToolCall {
                id: "memory-wide".to_string(),
                name: "memory".to_string(),
                input: serde_json::json!({
                    "action": "recall",
                    "query": "tool summary truncation"
                }),
                intent: None,
            },
            "recall 'tool summary truncation'",
        ),
        (
            ToolCall {
                id: "codesearch-wide".to_string(),
                name: "codesearch".to_string(),
                input: serde_json::json!({
                    "query": "rust unicode width truncation examples"
                }),
                intent: None,
            },
            "'rust unicode width truncation examples'",
        ),
        (
            ToolCall {
                id: "debug-wide".to_string(),
                name: "debug_socket".to_string(),
                input: serde_json::json!({
                    "command": "tester:list"
                }),
                intent: None,
            },
            "tester:list",
        ),
    ];

    for (tool, expected) in cases {
        let summary = tools_ui::get_tool_summary_with_budget(&tool, 50, Some(200));
        assert_eq!(summary, expected, "tool={tool:?} summary={summary:?}");
        assert!(!summary.contains('…'), "tool={tool:?} summary={summary:?}");
    }
}

#[test]
fn test_tool_summary_browser_open_shows_url() {
    let tool = ToolCall {
        id: "browser-open".to_string(),
        name: "browser".to_string(),
        input: serde_json::json!({
            "action": "open",
            "url": "https://example.com/docs/reference/browser-tool"
        }),
        intent: None,
    };

    let summary = tools_ui::get_tool_summary_with_budget(&tool, 50, Some(200));
    assert_eq!(
        summary,
        "open https://example.com/docs/reference/browser-tool"
    );
}

#[test]
fn test_tool_summary_browser_type_hides_typed_text() {
    let tool = ToolCall {
        id: "browser-type".to_string(),
        name: "browser".to_string(),
        input: serde_json::json!({
            "action": "type",
            "selector": "#password",
            "text": "super-secret-value"
        }),
        intent: None,
    };

    let summary = tools_ui::get_tool_summary_with_budget(&tool, 50, Some(200));
    assert_eq!(summary, "type #password (18 chars)");
    assert!(
        !summary.contains("super-secret-value"),
        "summary={summary:?}"
    );
}

#[test]
fn test_tool_summary_browser_type_without_selector_still_hides_text() {
    let tool = ToolCall {
        id: "browser-type-no-selector".to_string(),
        name: "browser".to_string(),
        input: serde_json::json!({
            "action": "type",
            "text": "secret-token-123"
        }),
        intent: None,
    };

    let summary = tools_ui::get_tool_summary_with_budget(&tool, 50, Some(200));
    assert_eq!(summary, "type (16 chars)");
    assert!(!summary.contains("secret-token-123"), "summary={summary:?}");
}

#[test]
fn test_tool_summary_browser_eval_truncates_script() {
    let tool = ToolCall {
        id: "browser-eval".to_string(),
        name: "browser".to_string(),
        input: serde_json::json!({
            "action": "eval",
            "script": "return window.__APP_STATE__?.reallyLongNestedValue?.items?.map(item => item.name).join(', ')"
        }),
        intent: None,
    };

    let summary = tools_ui::get_tool_summary_with_budget(&tool, 50, Some(34));
    assert!(summary.starts_with("eval "), "summary={summary:?}");
    assert!(summary.contains('…'), "summary={summary:?}");
    assert!(unicode_width::UnicodeWidthStr::width(summary.as_str()) <= 34);
}

#[test]
fn test_tool_summary_agentgrep_smart_uses_terms_subject_relation() {
    let tool = ToolCall {
        id: "agentgrep-smart-terms".to_string(),
        name: "agentgrep".to_string(),
        input: serde_json::json!({
            "mode": "smart",
            "terms": ["subject:agentgrep", "relation:build_args", "path:src/tool"]
        }),
        intent: None,
    };

    let summary = tools_ui::get_tool_summary_with_budget(&tool, 50, Some(200));
    assert_eq!(summary, "smart agentgrep:build_args");
}

#[test]
fn test_tool_summary_agentgrep_smart_uses_query_subject_relation() {
    let tool = ToolCall {
        id: "agentgrep-smart-query".to_string(),
        name: "agentgrep".to_string(),
        input: serde_json::json!({
            "mode": "smart",
            "query": "subject:agentgrep relation:build_args path:src/tool"
        }),
        intent: None,
    };

    let summary = tools_ui::get_tool_summary_with_budget(&tool, 50, Some(200));
    assert_eq!(summary, "smart agentgrep:build_args");
}

#[test]
fn test_tool_summary_bg_infers_wait_from_intent_when_action_missing() {
    let tool = ToolCall {
        id: "bg-intent-only".to_string(),
        name: "bg".to_string(),
        input: serde_json::json!({
            "intent": "Wait for library tests",
            "latest": true
        }),
        intent: Some("Wait for library tests".to_string()),
    };

    let summary = tools_ui::get_tool_summary_with_budget(&tool, 50, Some(200));
    assert_eq!(summary, "wait");
}

#[test]
fn test_render_tool_message_batch_rows_do_not_soft_wrap_on_narrow_width() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "--- [1] read ---\nok\n\nCompleted: 1 succeeded, 0 failed".to_string(),
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: Some(ToolCall {
            id: "call_batch_narrow".to_string(),
            name: "batch".to_string(),
            input: serde_json::json!({
                "tool_calls": [
                    {
                        "tool": "read",
                        "file_path": "src/tui/really/long/nested/location/ui_messages.rs",
                        "offset": 120,
                        "limit": 40
                    }
                ]
            }),
            intent: None,
        }),
    };

    let lines = render_tool_message(&msg, 32, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();

    assert_eq!(rendered.len(), 2, "rendered={rendered:?}");
    assert!(
        rendered.iter().all(|line| line.width() <= 31),
        "rendered={rendered:?}"
    );
    assert!(rendered[1].contains('…'), "rendered={rendered:?}");
    assert!(rendered[1].contains("tok"), "rendered={rendered:?}");
}

#[test]
fn test_render_tool_message_keeps_token_badge_when_intent_is_truncated() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "ok".to_string(),
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: Some(ToolCall {
            id: "call_long_intent".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({
                "command": "cargo test --package jcode --lib tui::ui::tests::very_long_test_name -- --nocapture"
            }),
            intent: Some(
                "Inspect and validate the extremely long wrapping behavior for tool rows"
                    .to_string(),
            ),
        }),
    };

    let lines = render_tool_message(&msg, 48, crate::config::DiffDisplayMode::Off);
    let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();

    assert!(!rendered.is_empty(), "rendered={rendered:?}");
    assert!(rendered[0].width() <= 47, "rendered={rendered:?}");
    assert!(rendered[0].contains('…'), "rendered={rendered:?}");
    assert!(rendered[0].contains("tok"), "rendered={rendered:?}");
}
