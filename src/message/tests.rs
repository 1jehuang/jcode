use super::*;
use anyhow::{Result, anyhow};

#[test]
fn sanitize_tool_id_alphanumeric_passthrough() {
    assert_eq!(
        sanitize_tool_id("toolu_01XFDUDYJgAACzvnptvVer6u"),
        "toolu_01XFDUDYJgAACzvnptvVer6u"
    );
    assert_eq!(sanitize_tool_id("call_abc123"), "call_abc123");
    assert_eq!(
        sanitize_tool_id("call_1234567890_9876543210"),
        "call_1234567890_9876543210"
    );
}

#[test]
fn tool_call_intent_from_input_trims_optional_intent() {
    let input = serde_json::json!({
        "intent": "  Verify compact rendering  ",
        "command": "cargo test"
    });
    assert_eq!(
        ToolCall::intent_from_input(&input).as_deref(),
        Some("Verify compact rendering")
    );
    assert_eq!(
        ToolCall::intent_from_input(&serde_json::json!({"intent": "  "})),
        None
    );
}

#[test]
fn sanitize_tool_id_hyphens_passthrough() {
    assert_eq!(sanitize_tool_id("call-abc-123"), "call-abc-123");
    assert_eq!(
        sanitize_tool_id("tool_use-id_with-mixed"),
        "tool_use-id_with-mixed"
    );
}

#[test]
fn sanitize_tool_id_replaces_dots() {
    assert_eq!(
        sanitize_tool_id("chatcmpl-abc.def.ghi"),
        "chatcmpl-abc_def_ghi"
    );
    assert_eq!(sanitize_tool_id("call.123"), "call_123");
}

#[test]
fn sanitize_tool_id_replaces_colons() {
    assert_eq!(sanitize_tool_id("call:123:456"), "call_123_456");
}

#[test]
fn sanitize_tool_id_replaces_special_chars() {
    assert_eq!(
        sanitize_tool_id("id@with#special$chars"),
        "id_with_special_chars"
    );
    assert_eq!(sanitize_tool_id("id with spaces"), "id_with_spaces");
}

#[test]
fn sanitize_tool_id_empty_returns_unknown() {
    assert_eq!(sanitize_tool_id(""), "unknown");
}

#[test]
fn sanitize_tool_id_copilot_to_anthropic() {
    assert_eq!(
        sanitize_tool_id("chatcmpl-BF2xX.tool_call.0"),
        "chatcmpl-BF2xX_tool_call_0"
    );
}

#[test]
fn sanitize_tool_id_already_valid_unchanged() {
    let valid_ids = [
        "toolu_01XFDUDYJgAACzvnptvVer6u",
        "call_abc123",
        "fallback_text_call_call_1234567890_9876543210",
        "tool_123",
        "a",
        "A",
        "0",
        "_",
        "-",
        "a-b_c",
    ];
    for id in valid_ids {
        assert_eq!(sanitize_tool_id(id), id, "ID '{}' should be unchanged", id);
    }
}

#[test]
fn redact_secrets_redacts_known_direct_token_formats() {
    let input = "access=sk-ant-oat01-ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789\nopenrouter=sk-or-v1-abcdefghijklmnopqrstuvwxyz0123456789\ngithub=ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123\n";
    let out = redact_secrets(input);
    assert!(!out.contains("sk-ant-oat01-"));
    assert!(!out.contains("sk-or-v1-"));
    assert!(!out.contains("ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123"));
    assert!(out.matches("[REDACTED_SECRET]").count() >= 3);
}

#[test]
fn redact_secrets_redacts_env_style_assignments() {
    let input = "OPENROUTER_API_KEY=sk-or-v1-abc123abc123abc123abc123\nOPENCODE_API_KEY=oc_test_secret\nOPENCODE_GO_API_KEY=ocgo_test_secret\nZAI_API_KEY=zai_secret\nCHUTES_API_KEY=chutes_secret\nCEREBRAS_API_KEY=cerebras_secret\nOPENAI_COMPAT_API_KEY=compat_secret\nCURSOR_API_KEY='my_cursor_secret_value'\nOPENAI_API_KEY=sk-test-openai-example\nAZURE_OPENAI_API_KEY=azure-openai-secret\n";
    let out = redact_secrets(input);
    assert!(out.contains("OPENROUTER_API_KEY=[REDACTED_SECRET]"));
    assert!(out.contains("OPENCODE_API_KEY=[REDACTED_SECRET]"));
    assert!(out.contains("OPENCODE_GO_API_KEY=[REDACTED_SECRET]"));
    assert!(out.contains("ZAI_API_KEY=[REDACTED_SECRET]"));
    assert!(out.contains("CHUTES_API_KEY=[REDACTED_SECRET]"));
    assert!(out.contains("CEREBRAS_API_KEY=[REDACTED_SECRET]"));
    assert!(out.contains("OPENAI_COMPAT_API_KEY=[REDACTED_SECRET]"));
    assert!(out.contains("CURSOR_API_KEY=[REDACTED_SECRET]"));
    assert!(out.contains("OPENAI_API_KEY=[REDACTED_SECRET]"));
    assert!(out.contains("AZURE_OPENAI_API_KEY=[REDACTED_SECRET]"));
    assert!(!out.contains("my_cursor_secret_value"));
}

#[test]
fn redact_secrets_redacts_runtime_key_assignment() {
    let key_var = "JCODE_OPENAI_COMPAT_API_KEY_NAME";
    let prev = std::env::var(key_var).ok();
    crate::env::set_var(key_var, "GROQ_API_KEY");

    let input = "GROQ_API_KEY=my_secret_token_value";
    let out = redact_secrets(input);
    assert_eq!(out, "GROQ_API_KEY=[REDACTED_SECRET]");

    if let Some(v) = prev {
        crate::env::set_var(key_var, v);
    } else {
        crate::env::remove_var(key_var);
    }
}

#[test]
fn redact_secrets_redacts_mixed_case_token_assignments() {
    let input = "my_token=ya29.ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let out = redact_secrets(input);
    assert!(out.contains("[REDACTED_SECRET]"));
    assert!(!out.contains("ya29.ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"));
}

#[test]
fn redact_secrets_leaves_normal_output_unchanged() {
    let input = "Found 5 files\nNo auth errors\nDone.";
    assert_eq!(redact_secrets(input), input);
}

#[test]
fn format_timestamp_is_stable_utc_rfc3339() -> Result<()> {
    let ts = chrono::DateTime::parse_from_rfc3339("2025-03-15T02:24:13.250Z")?.with_timezone(&Utc);
    assert_eq!(Message::format_timestamp(&ts), "2025-03-15T02:24:13.250Z");
    Ok(())
}

#[test]
fn with_timestamps_prepends_utc_prefix_to_user_text() -> Result<()> {
    let ts = chrono::DateTime::parse_from_rfc3339("2025-03-15T02:24:03Z")?.with_timezone(&Utc);
    let stamped = Message::with_timestamps(&[Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "hello".to_string(),
            cache_control: None,
        }],
        timestamp: Some(ts),
        tool_duration_ms: None,
    }]);
    let ContentBlock::Text { text, .. } = &stamped[0].content[0] else {
        return Err(anyhow!(
            "expected text block, got {:?}",
            stamped[0].content[0]
        ));
    };
    assert_eq!(text, "[2025-03-15T02:24:03.000Z] hello");
    Ok(())
}

#[test]
fn with_timestamps_adds_tool_timing_header_with_duration() -> Result<()> {
    let ts = chrono::DateTime::parse_from_rfc3339("2025-03-15T02:24:13Z")?.with_timezone(&Utc);
    let stamped = Message::with_timestamps(&[Message {
        role: Role::User,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "call_1".to_string(),
            content: "ok".to_string(),
            is_error: None,
        }],
        timestamp: Some(ts),
        tool_duration_ms: Some(3_200),
    }]);
    let ContentBlock::ToolResult { content, .. } = &stamped[0].content[0] else {
        return Err(anyhow!(
            "expected tool result block, got {:?}",
            stamped[0].content[0]
        ));
    };
    assert_eq!(
        content,
        "[tool timing: start=2025-03-15T02:24:09.800Z finish=2025-03-15T02:24:13.000Z duration=3.2s] ok"
    );
    Ok(())
}

#[test]
fn with_timestamps_skips_internal_system_reminders() -> Result<()> {
    let ts = chrono::DateTime::parse_from_rfc3339("2025-03-15T02:24:13Z")?.with_timezone(&Utc);
    let original = Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "<system-reminder>\ninternal\n</system-reminder>".to_string(),
            cache_control: None,
        }],
        timestamp: Some(ts),
        tool_duration_ms: None,
    };
    let stamped = Message::with_timestamps(std::slice::from_ref(&original));
    let ContentBlock::Text { text, .. } = &stamped[0].content[0] else {
        return Err(anyhow!(
            "expected text block, got {:?}",
            stamped[0].content[0]
        ));
    };
    assert_eq!(text, "<system-reminder>\ninternal\n</system-reminder>");
    Ok(())
}

#[test]
fn ends_with_fresh_user_turn_accepts_plain_user_text() {
    let messages = vec![Message::user("hello")];
    assert!(ends_with_fresh_user_turn(&messages));
}

#[test]
fn ends_with_fresh_user_turn_rejects_trailing_tool_result() {
    let messages = vec![
        Message::user("hello"),
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call_1".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({}),
            }],
            timestamp: Some(Utc::now()),
            tool_duration_ms: None,
        },
        Message::tool_result("call_1", "ok", false),
    ];
    assert!(!ends_with_fresh_user_turn(&messages));
}

#[test]
fn ends_with_fresh_user_turn_skips_internal_system_reminders() {
    let messages = vec![
        Message::user("hello"),
        Message::user("<system-reminder>\ninternal\n</system-reminder>"),
    ];
    assert!(ends_with_fresh_user_turn(&messages));
}

#[test]
fn ends_with_fresh_user_turn_rejects_assistant_tail() {
    let messages = vec![
        Message::user("hello"),
        Message::assistant_text("working on it"),
    ];
    assert!(!ends_with_fresh_user_turn(&messages));
}

#[test]
fn format_background_task_notification_markdown_uses_code_block_preview() {
    let rendered = format_background_task_notification_markdown(&BackgroundTaskCompleted {
        task_id: "abc123".to_string(),
        tool_name: "bash".to_string(),
        display_name: None,
        session_id: "session".to_string(),
        status: BackgroundTaskStatus::Completed,
        exit_code: Some(0),
        output_preview: "[stderr] first line\n[stdout] second line\n".to_string(),
        output_file: std::path::PathBuf::from("/tmp/output.log"),
        duration_secs: 7.1,
        notify: true,
        wake: false,
    });

    assert!(
        rendered.contains("**Background task** `abc123` · `bash` · ✓ completed · 7.1s · exit 0")
    );
    assert!(rendered.contains("```text\n[stderr] first line\n[stdout] second line\n```"));
    assert!(rendered.contains("_Full output:_ `bg action=\"output\" task_id=\"abc123\"`"));
}

#[test]
fn format_background_task_notification_markdown_handles_empty_preview() {
    let rendered = format_background_task_notification_markdown(&BackgroundTaskCompleted {
        task_id: "abc123".to_string(),
        tool_name: "bash".to_string(),
        display_name: None,
        session_id: "session".to_string(),
        status: BackgroundTaskStatus::Failed,
        exit_code: Some(9),
        output_preview: "\n\n".to_string(),
        output_file: std::path::PathBuf::from("/tmp/output.log"),
        duration_secs: 1.0,
        notify: true,
        wake: false,
    });

    assert!(rendered.contains("✗ failed"));
    assert!(rendered.contains("_No output captured._"));
}

#[test]
fn format_background_task_notification_markdown_highlights_failure_reason() -> Result<()> {
    let rendered = format_background_task_notification_markdown(&BackgroundTaskCompleted {
        task_id: "build123".to_string(),
        tool_name: "selfdev-build".to_string(),
        display_name: Some("Build jcode".to_string()),
        session_id: "session".to_string(),
        status: BackgroundTaskStatus::Failed,
        exit_code: Some(101),
        output_preview: "[stderr]    Compiling jcode\nsccache: Compile terminated by signal 15\n[stderr] error: could not compile `jcode` (lib)".to_string(),
        output_file: std::path::PathBuf::from("/tmp/output.log"),
        duration_secs: 62.5,
        notify: true,
        wake: false,
    });

    assert!(rendered.contains("_Failure:_ sccache: Compile terminated by signal 15"));
    let parsed = parse_background_task_notification_markdown(&rendered)
        .ok_or_else(|| anyhow!("failure notification should parse"))?;
    assert_eq!(
        parsed.failure_summary.as_deref(),
        Some("sccache: Compile terminated by signal 15")
    );
    Ok(())
}

#[test]
fn format_background_task_notification_markdown_renders_superseded_status() {
    let rendered = format_background_task_notification_markdown(&BackgroundTaskCompleted {
        task_id: "abc123".to_string(),
        tool_name: "selfdev-build".to_string(),
        display_name: None,
        session_id: "session".to_string(),
        status: BackgroundTaskStatus::Superseded,
        exit_code: Some(0),
        output_preview: "Build completed, but source changed before activation".to_string(),
        output_file: std::path::PathBuf::from("/tmp/output.log"),
        duration_secs: 5.0,
        notify: true,
        wake: false,
    });

    assert!(rendered.contains("↻ superseded"));
    assert!(rendered.contains("exit 0"));
    assert!(rendered.contains("source changed before activation"));
}

#[test]
fn format_background_task_progress_markdown_uses_compact_multiline_layout() {
    let rendered = format_background_task_progress_markdown(&BackgroundTaskProgressEvent {
        task_id: "bgprogress".to_string(),
        tool_name: "bash".to_string(),
        display_name: None,
        session_id: "session".to_string(),
        progress: crate::bus::BackgroundTaskProgress {
            kind: crate::bus::BackgroundTaskProgressKind::Determinate,
            percent: Some(42.0),
            message: Some("Running tests".to_string()),
            current: Some(21),
            total: Some(50),
            unit: Some("tests".to_string()),
            eta_seconds: None,
            updated_at: Utc::now().to_rfc3339(),
            source: crate::bus::BackgroundTaskProgressSource::Reported,
        },
    });

    assert!(rendered.starts_with("**Background task progress** `bgprogress` · `bash`\n\n"));
    assert!(rendered.contains("42% · Running tests"));
    assert!(rendered.contains("(reported)"));
}

#[test]
fn background_task_notifications_include_display_name_when_available() -> Result<()> {
    let rendered = format_background_task_notification_markdown(&BackgroundTaskCompleted {
        task_id: "abc123".to_string(),
        tool_name: "bash".to_string(),
        display_name: Some("Run integration tests".to_string()),
        session_id: "session".to_string(),
        status: BackgroundTaskStatus::Completed,
        exit_code: Some(0),
        output_preview: "done".to_string(),
        output_file: std::path::PathBuf::from("/tmp/output.log"),
        duration_secs: 7.1,
        notify: true,
        wake: false,
    });

    assert!(
        rendered.contains(
            "**Background task** `abc123` · `Run integration tests` (`bash`) · ✓ completed"
        )
    );
    let parsed = parse_background_task_notification_markdown(&rendered)
        .ok_or_else(|| anyhow!("named background task notification should parse"))?;
    assert_eq!(parsed.tool_name, "bash");
    assert_eq!(
        parsed.display_name.as_deref(),
        Some("Run integration tests")
    );
    Ok(())
}

#[test]
fn background_task_progress_notifications_include_display_name_when_available() -> Result<()> {
    let rendered = format_background_task_progress_markdown(&BackgroundTaskProgressEvent {
        task_id: "bgprogress".to_string(),
        tool_name: "bash".to_string(),
        display_name: Some("Run integration tests".to_string()),
        session_id: "session".to_string(),
        progress: crate::bus::BackgroundTaskProgress {
            kind: crate::bus::BackgroundTaskProgressKind::Determinate,
            percent: Some(42.0),
            message: Some("Running tests".to_string()),
            current: Some(21),
            total: Some(50),
            unit: Some("tests".to_string()),
            eta_seconds: None,
            updated_at: Utc::now().to_rfc3339(),
            source: crate::bus::BackgroundTaskProgressSource::Reported,
        },
    });

    assert!(rendered.starts_with(
        "**Background task progress** `bgprogress` · `Run integration tests` (`bash`)\n\n"
    ));
    let parsed = parse_background_task_progress_notification_markdown(&rendered)
        .ok_or_else(|| anyhow!("named progress notification should parse"))?;
    assert_eq!(parsed.tool_name, "bash");
    assert_eq!(
        parsed.display_name.as_deref(),
        Some("Run integration tests")
    );
    assert_eq!(parsed.summary, "42% · Running tests");
    Ok(())
}

#[test]
fn parse_background_task_progress_notification_extracts_card_fields() -> Result<()> {
    let parsed = parse_background_task_progress_notification_markdown(
        "**Background task progress** `bgprogress` · `bash`\n\n[#####-------] 42% · Running tests (reported)",
    )
    .ok_or_else(|| anyhow!("progress notification should parse"))?;

    assert_eq!(parsed.task_id, "bgprogress");
    assert_eq!(parsed.tool_name, "bash");
    assert_eq!(parsed.display_name, None);
    assert_eq!(parsed.summary, "42% · Running tests");
    assert_eq!(parsed.source.as_deref(), Some("reported"));
    assert_eq!(parsed.percent, Some(42.0));
    Ok(())
}

#[test]
fn parse_background_task_progress_notification_supports_legacy_inline_layout() -> Result<()> {
    let parsed = parse_background_task_progress_notification_markdown(
        "**Background task progress** `bgprogress` · `bash` · Release run in_progress: - 7/8 jobs completed (reported)",
    )
    .ok_or_else(|| anyhow!("legacy progress notification should parse"))?;

    assert_eq!(parsed.task_id, "bgprogress");
    assert_eq!(parsed.tool_name, "bash");
    assert_eq!(parsed.display_name, None);
    assert_eq!(
        parsed.summary,
        "Release run in_progress: - 7/8 jobs completed"
    );
    assert_eq!(parsed.source.as_deref(), Some("reported"));
    assert_eq!(parsed.percent, None);
    Ok(())
}

#[test]
fn description_token_estimate_uses_chars_per_token_heuristic() {
    let def = ToolDefinition {
        name: "read".to_string(),
        description: "abcdwxyz".to_string(),
        input_schema: serde_json::json!({"type": "object"}),
    };

    assert_eq!(def.description_token_estimate(), 2);
}

#[test]
fn parse_background_task_notification_markdown_extracts_fields() -> Result<()> {
    let rendered = format_background_task_notification_markdown(&BackgroundTaskCompleted {
        task_id: "abc123".to_string(),
        tool_name: "bash".to_string(),
        display_name: None,
        session_id: "session".to_string(),
        status: BackgroundTaskStatus::Completed,
        exit_code: Some(0),
        output_preview: "[stderr] first line\n[stdout] second line\n".to_string(),
        output_file: std::path::PathBuf::from("/tmp/output.log"),
        duration_secs: 7.1,
        notify: true,
        wake: false,
    });

    let parsed = parse_background_task_notification_markdown(&rendered)
        .ok_or_else(|| anyhow!("background task notification should parse"))?;
    assert_eq!(parsed.task_id, "abc123");
    assert_eq!(parsed.tool_name, "bash");
    assert_eq!(parsed.display_name, None);
    assert_eq!(parsed.status, "✓ completed");
    assert_eq!(parsed.duration, "7.1s");
    assert_eq!(parsed.exit_label, "exit 0");
    assert_eq!(parsed.failure_summary, None);
    assert_eq!(
        parsed.preview.as_deref(),
        Some("[stderr] first line\n[stdout] second line")
    );
    assert_eq!(
        parsed.full_output_command,
        "bg action=\"output\" task_id=\"abc123\""
    );
    Ok(())
}
