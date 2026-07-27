use super::*;

fn rendered_tool_lines(content: &str, active: bool) -> Vec<SingleSessionStyledLine> {
    let mut lines = Vec::new();
    append_tool_lines(&mut lines, content, active, None, None);
    lines
}

fn rendered_tool_text(content: &str, active: bool) -> Vec<String> {
    rendered_tool_lines(content, active)
        .into_iter()
        .map(|line| line.text)
        .collect()
}

fn test_model_choice(model: &str) -> DesktopModelChoice {
    DesktopModelChoice {
        model: model.to_string(),
        provider: Some("test-provider".to_string()),
        api_method: Some("chat".to_string()),
        detail: Some("available".to_string()),
        available: true,
    }
}

fn test_session_card(session_id: &str, title: &str) -> workspace::SessionCard {
    workspace::SessionCard {
        session_id: session_id.to_string(),
        title: title.to_string(),
        subtitle: "active · test-model".to_string(),
        detail: "4 msgs · just now · test".to_string(),
        preview_lines: vec!["user latest compact prompt".to_string()],
        detail_lines: vec![
            "user first question".to_string(),
            "assistant first answer".to_string(),
            "tool bash completed".to_string(),
        ],
        transcript_messages: Vec::new(),
    }
}

fn assert_render_line_count_matches(app: &SingleSessionApp) {
    assert_eq!(
        app.render_inline_widget_line_count(),
        app.render_inline_widget_styled_lines().len(),
        "render line count should match styled-line rendering"
    );
}

#[test]
fn session_backed_app_skips_external_cli_scan() {
    // Constructing an app for a workspace surface (session present) must not
    // walk external CLI history: workspace rendering builds one ephemeral
    // app per visible surface every frame, so this scan would be hot.
    let card = workspace::SessionCard {
        session_id: "scan-guard".to_string(),
        title: "scan guard".to_string(),
        subtitle: String::new(),
        detail: String::new(),
        preview_lines: Vec::new(),
        detail_lines: Vec::new(),
        transcript_messages: Vec::new(),
    };

    let before = EXTERNAL_CLI_SCAN_CALLS.with(|calls| calls.get());
    let _app = SingleSessionApp::new(Some(card));
    let after_session = EXTERNAL_CLI_SCAN_CALLS.with(|calls| calls.get());
    assert_eq!(
        after_session, before,
        "session-backed app construction must not scan external CLI history"
    );

    // The fresh welcome (no session) still performs the scan so the
    // continuation suggestion can be rendered.
    let _fresh = SingleSessionApp::new(None);
    let after_fresh = EXTERNAL_CLI_SCAN_CALLS.with(|calls| calls.get());
    assert_eq!(
        after_fresh,
        after_session + 1,
        "fresh welcome construction should perform exactly one external CLI scan"
    );
}

#[test]
fn latest_external_cli_suggestion_uses_newest_candidate_context() {
    let old = ExternalCliSessionCandidate {
        source: "Claude Code",
        modified: SystemTime::UNIX_EPOCH,
        working_dir: Some("/tmp/old-project".to_string()),
        context: Some("old task".to_string()),
    };
    let new = ExternalCliSessionCandidate {
        source: "Codex",
        modified: SystemTime::UNIX_EPOCH + Duration::from_secs(10),
        working_dir: Some("/home/user/jcode".to_string()),
        context: Some("implement startup continuation suggestions".to_string()),
    };

    let suggestion = latest_external_cli_continuation_suggestion_from_candidates(vec![old, new])
        .expect("newest external session should produce a suggestion");

    assert_eq!(
        suggestion,
        "continue the latest Codex session in jcode: implement startup continuation suggestions"
    );
}

#[test]
fn latest_external_cli_suggestion_missing_roots_returns_none() {
    let home =
        std::env::temp_dir().join(format!("jcode-missing-external-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);

    assert_eq!(
        latest_external_cli_continuation_suggestion_from_home(&home),
        None
    );
}

#[test]
fn latest_external_cli_suggestion_ignores_malformed_jsonl() {
    let home = std::env::temp_dir().join(format!(
        "jcode-malformed-external-cli-{}-{:?}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let codex_dir = home.join(".codex/sessions");
    std::fs::create_dir_all(&codex_dir).expect("create fake codex dir");
    std::fs::write(
        codex_dir.join("broken.jsonl"),
        "not json\n{\"type\":\"message\",\"role\":\"assistant\",\"content\":[]\n",
    )
    .expect("write malformed jsonl");

    assert_eq!(
        latest_external_cli_continuation_suggestion_from_home(&home),
        None
    );

    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn inline_widget_line_count_matches_rendered_lines_for_active_widgets() {
    let mut help = SingleSessionApp::new(None);
    help.show_help = true;
    assert_render_line_count_matches(&help);

    let mut info = SingleSessionApp::new(None);
    info.show_session_info = true;
    info.error = Some("test error".to_string());
    info.session = Some(test_session_card("session_info", "Info Session"));
    assert_render_line_count_matches(&info);

    let mut slash = SingleSessionApp::new(None);
    slash.handle_key(KeyInput::Character("/re".to_string()));
    assert_render_line_count_matches(&slash);

    let mut model = SingleSessionApp::new(None);
    model.model_picker.open = true;
    model.model_picker.choices = vec![test_model_choice("alpha"), test_model_choice("beta")];
    model.model_picker.selected = 1;
    model.model_picker.refresh_visible_indices();
    assert_render_line_count_matches(&model);

    let mut switcher = SingleSessionApp::new(None);
    switcher.session_switcher.open = true;
    switcher.session_switcher.sessions = vec![
        test_session_card("session_alpha", "Alpha"),
        test_session_card("session_beta", "Beta"),
    ];
    switcher.session_switcher.selected = 1;
    switcher.session_switcher.refresh_visible_indices();
    assert_render_line_count_matches(&switcher);
}

#[test]
fn tool_header_uses_status_icons_and_compact_summary() {
    assert_eq!(
        format_tool_header_line(&parse_tool_header("▾ bash done: completed successfully")),
        "  ✓ bash · done · completed successfully"
    );
    assert_eq!(
        format_tool_header_line(&parse_tool_header("▾ browser failed: selector missing")),
        "  ✕ browser · failed · selector missing"
    );
}

#[test]
fn bash_tool_rendering_shows_intent_command_and_background_flag() {
    let lines = rendered_tool_text(
        "▾ bash running\n  input: {\"intent\":\"run the desktop tests\",\"command\":\"cargo test -p jcode-desktop\",\"run_in_background\":true}",
        true,
    );
    assert_eq!(
        lines,
        vec![
            "  ● bash · running · $ cargo test -p jcode-desktop · background: yes",
            "    waiting for tool output…",
        ]
    );
}

#[test]
fn active_tool_lines_carry_visual_metadata_for_native_cards() {
    let lines = rendered_tool_lines(
        "▾ bash running\n  input: {\"command\":\"cargo test -p jcode-desktop\"}",
        true,
    );

    let header = lines[0]
        .tool
        .as_ref()
        .expect("tool header should carry native card metadata");
    assert_eq!(header.name, "bash");
    assert_eq!(header.state, SingleSessionToolVisualState::Running);
    assert_eq!(header.kind, SingleSessionToolLineKind::Header);
    assert!(header.active);
    assert!(header.expanded);

    let detail = lines[1]
        .tool
        .as_ref()
        .expect("tool detail should share native card metadata");
    assert_eq!(detail.call_id, header.call_id);
    assert_eq!(detail.kind, SingleSessionToolLineKind::Detail);
    assert!(detail.active);
}

#[test]
fn desktop_tool_metadata_prioritizes_tui_like_summary_over_intent() {
    assert_eq!(
        formatted_tool_input_lines(
            "agentgrep",
            "{\"intent\":\"Locate rendering code\",\"query\":\"tool call\",\"path\":\"src/tui\"}",
        ),
        vec!["grep 'tool call'", "in src/tui"]
    );
    assert_eq!(
        formatted_tool_input_lines(
            "side_panel",
            "{\"intent\":\"Show notes\",\"action\":\"write\",\"title\":\"Plan\"}",
        ),
        vec!["write Plan"]
    );
    assert_eq!(
        formatted_tool_input_lines(
            "subagent",
            "{\"intent\":\"Delegate\",\"description\":\"Inspect parser\",\"subagent_type\":\"agent\"}",
        ),
        vec!["Inspect parser (agent)"]
    );
}

#[test]
fn tool_result_content_renders_inside_inline_widget() {
    let lines = rendered_tool_text(
        "▾ bash failed: tests failed\n  input: {\"command\":\"cargo test -p jcode-desktop\"}\n  error[E0425]: cannot find value `foo` in this scope\n  test result: FAILED",
        true,
    );

    assert_eq!(
        lines[0],
        "  ✕ bash · failed · tests failed · $ cargo test -p jcode-desktop"
    );
    assert_eq!(
        lines[1],
        "    error[E0425]: cannot find value `foo` in this scope"
    );
    assert_eq!(lines[2], "    test result: FAILED");
}

#[test]
fn inactive_tool_result_compacts_to_metadata_only() {
    let lines = rendered_tool_text(
        "▸ bash done: tests passed\n  input: {\"command\":\"cargo test -p jcode-desktop\"}\n  test result: ok",
        false,
    );

    assert_eq!(
        lines,
        vec!["  ✓ bash · done · tests passed · $ cargo test -p jcode-desktop"]
    );
}

#[test]
fn expanded_inactive_tool_result_shows_detail_widget() {
    let lines = rendered_tool_text(
        "▾ edit done: updated file\n  input: {\"file_path\":\"src/lib.rs\",\"old_string\":\"old\",\"new_string\":\"new\"}\n  Edited src/lib.rs: replaced 1 occurrence",
        false,
    );

    assert_eq!(lines[0], "  ✓ edit · done · updated file · src/lib.rs");
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Edited src/lib.rs: replaced 1 occurrence")),
        "expanded inactive edit tool should render its detail widget: {lines:?}"
    );
}

#[test]
fn unknown_tool_falls_back_to_prioritized_key_value_lines() {
    let lines = formatted_tool_input_lines(
        "custom",
        "{\"token\":\"secret\",\"query\":\"tool calls\",\"extra\":42}",
    );
    assert_eq!(lines, vec!["query: tool calls", "extra: 42", "token: ••••"]);
}

#[test]
fn unknown_tool_uses_intent_only_as_fallback() {
    let lines = formatted_tool_input_lines(
        "custom",
        "{\"intent\":\"describe action\",\"query\":\"tool calls\"}",
    );
    assert_eq!(lines, vec!["query: tool calls", "intent: describe action"]);
}

#[test]
fn plain_tool_input_skips_json_probe_and_renders_compactly() {
    let lines = formatted_tool_input_lines("bash", " chunk-0 chunk-1 chunk-2");
    assert_eq!(lines, vec!["input: chunk-0 chunk-1 chunk-2"]);
    assert!(!looks_like_json_value("chunk-0"));
    assert!(looks_like_json_value("{\"command\":\"cargo test\"}"));
}

#[test]
fn active_tool_cache_key_ignores_input_suffix_after_visible_preview_stabilizes() {
    let mut app = SingleSessionApp::new(None);
    app.apply_session_event(DesktopSessionEvent::ToolStarted {
        id: Some("tool-a".to_string()),
        name: "bash".to_string(),
    });
    app.apply_session_event(DesktopSessionEvent::ToolExecuting {
        id: Some("tool-a".to_string()),
        name: "bash".to_string(),
    });
    app.apply_session_event(DesktopSessionEvent::ToolInput {
        id: Some("tool-a".to_string()),
        delta: "a".repeat(160),
    });

    let body_before = app.body_lines();
    let body_key_before = app.rendered_body_cache_key((900, 700));
    let static_key_before = app.rendered_body_static_cache_key((900, 700));

    app.apply_session_event(DesktopSessionEvent::ToolInput {
        id: Some("tool-a".to_string()),
        delta: "b".repeat(40),
    });

    assert_eq!(app.body_lines(), body_before);
    assert_eq!(app.rendered_body_cache_key((900, 700)), body_key_before);
    assert_eq!(
        app.rendered_body_static_cache_key((900, 700)),
        static_key_before
    );
}

#[test]
fn compact_tool_text_collapses_whitespace_and_stops_after_visible_prefix() {
    assert_eq!(
        compact_tool_text("  alpha\n\tbeta   gamma  ", 32),
        "alpha beta gamma"
    );
    assert_eq!(compact_tool_text("alpha beta gamma", 10), "alpha beta…");
    assert_eq!(compact_tool_text("你好 世界 again", 5), "你好 世界…");
    assert_eq!(compact_tool_text("alpha", 0), "…");
    assert_eq!(compact_tool_text("   ", 0), "");
}

#[test]
fn safe_utf8_prefix_len_rounds_down_to_char_boundary() {
    let text = "aé🚀";

    assert_eq!(safe_utf8_prefix_len(text, 0), 0);
    assert_eq!(safe_utf8_prefix_len(text, 1), 1);
    assert_eq!(safe_utf8_prefix_len(text, 2), 1);
    assert_eq!(safe_utf8_prefix_len(text, 3), 3);
    assert_eq!(safe_utf8_prefix_len(text, 6), 3);
    assert_eq!(safe_utf8_prefix_len(text, usize::MAX), text.len());
}

#[test]
fn reasoning_deltas_stream_into_the_transcript_before_any_answer_text() {
    let mut app = SingleSessionApp::new(None);
    app.apply_session_event(DesktopSessionEvent::ReasoningDelta(
        "checking the event plumbing\n".to_string(),
    ));

    let body = app.body_lines();
    assert!(
        body.iter()
            .any(|line| line.contains("checking the event plumbing")),
        "live reasoning should be visible immediately, got {body:?}"
    );
    assert_eq!(app.status.as_deref(), Some("thinking"));
}

#[test]
fn reasoning_tail_is_bounded_and_collapses_to_a_trace_when_done() {
    let mut app = SingleSessionApp::new(None);
    for index in 0..8 {
        app.apply_session_event(DesktopSessionEvent::ReasoningDelta(format!(
            "step-{index}\n"
        )));
    }

    let body = app.body_lines();
    assert!(
        !body.iter().any(|line| line.contains("step-0")),
        "old reasoning lines should scroll out of the live tail, got {body:?}"
    );
    assert!(body.iter().any(|line| line.contains("step-7")));

    app.apply_session_event(DesktopSessionEvent::ReasoningDone {
        duration_ms: Some(4200),
    });
    let body = app.body_lines();
    assert!(
        body.iter().any(|line| line.contains("thought for 4.2s")),
        "finished reasoning should leave a collapsed trace, got {body:?}"
    );
    assert!(!body.iter().any(|line| line.contains("step-7")));
}

#[test]
fn answer_text_and_tool_calls_close_an_open_reasoning_region() {
    let mut app = SingleSessionApp::new(None);
    app.apply_session_event(DesktopSessionEvent::ReasoningDelta("pondering".to_string()));
    app.apply_session_event(DesktopSessionEvent::TextDelta("the answer".to_string()));
    let body = app.body_lines();
    assert!(!body.iter().any(|line| line.contains("pondering")));
    assert!(body.iter().any(|line| line.contains("thought")));

    let mut app = SingleSessionApp::new(None);
    app.apply_session_event(DesktopSessionEvent::ReasoningDelta("pondering".to_string()));
    app.apply_session_event(DesktopSessionEvent::ToolStarted {
        id: Some("tool-a".to_string()),
        name: "bash".to_string(),
    });
    let body = app.body_lines();
    assert!(!body.iter().any(|line| line.contains("pondering")));
    assert!(body.iter().any(|line| line.contains("thought")));
}

#[test]
fn completed_tool_args_keep_their_concrete_summary_over_the_intent() {
    // Once the full arguments parse, the tool-specific summary is more
    // informative than the model's prose intent.
    let lines = formatted_tool_input_lines(
        "bash",
        r#"{"intent": "run the desktop tests", "command": "cargo test"}"#,
    );
    assert_eq!(lines.first().map(String::as_str), Some("$ cargo test"));
}

#[test]
fn tool_intent_is_visible_from_partially_streamed_json() {
    let lines = formatted_tool_input_lines("bash", r#"{"intent": "run the desktop tests", "comm"#);
    assert_eq!(
        lines,
        vec!["run the desktop tests".to_string()],
        "a streaming intent should preview before the args finish arriving"
    );
}

#[test]
fn nested_intent_fields_do_not_hijack_the_tool_headline() {
    assert_eq!(
        partial_json_string_field(r#"{"tool_calls": [{"intent": "inner"}]}"#, "intent"),
        None
    );
    assert_eq!(
        partial_json_string_field(
            r#"{"tool_calls": [{"intent": "inner"}], "intent": "outer"}"#,
            "intent"
        ),
        Some("outer".to_string())
    );
}

#[test]
fn partial_json_intent_handles_escapes_and_multibyte_text() {
    assert_eq!(
        partial_json_string_field(r#"{"intent": "say \"héllo\" now"#, "intent"),
        Some("say \"héllo\" now".to_string())
    );
}

#[test]
fn thinking_display_off_is_surfaced_instead_of_failing_silently() {
    // The desktop often attaches to an already-running shared server whose
    // config wins over the spawn-time default, so a silent empty transcript
    // has to become an explainable one.
    let mut app = SingleSessionApp::new(None);
    app.push_thinking_display_notice("off");
    let body = app.body_lines();
    assert!(
        body.iter()
            .any(|line| line.contains("thinking display is off")),
        "an off mode should explain the empty transcript, got {body:?}"
    );
    assert!(
        body.iter().any(|line| line.contains("reasoning_display")),
        "the hint should name the setting to change, got {body:?}"
    );

    for mode in ["current", "full"] {
        let mut app = SingleSessionApp::new(None);
        app.push_thinking_display_notice(mode);
        assert!(
            app.body_lines()
                .iter()
                .all(|line| !line.contains("thinking display is off")),
            "{mode} mode should not warn"
        );
    }
}

#[test]
fn thinking_display_notice_is_shown_at_most_once_per_session() {
    let mut app = SingleSessionApp::new(None);
    app.apply_session_event(DesktopSessionEvent::SessionStarted {
        session_id: "session-a".to_string(),
    });
    app.apply_session_event(DesktopSessionEvent::SessionStarted {
        session_id: "session-a".to_string(),
    });
    let notices = app
        .body_lines()
        .iter()
        .filter(|line| line.contains("thinking display is off"))
        .count();
    assert!(notices <= 1, "reconnects must not repeat the hint");
}
