use super::*;
use ratatui::style::Modifier;

#[test]
fn live_processing_line_uses_real_tokens_transport_and_tool_intent() {
    let mut status = crate::protocol::SwarmLiveStatus {
        phase: "running_tool".into(),
        elapsed_ms: 12_300,
        input_tokens: Some(12_345),
        output_tokens: Some(456),
        connection_type: Some("websocket/persistent-reuse".into()),
        tool_name: Some("bash".into()),
        tool_intent: Some("Verify 界\noutput".into()),
        ..Default::default()
    };
    let line = live_processing_status_line(&status, Some(0.0), "⠋");
    let text = line.to_string();
    assert!(text.contains("running bash · Verify 界 output"), "{text}");
    assert!(
        text.contains("↑12k ↓456") && text.contains("existing websocket") && text.contains("12s"),
        "{text}"
    );
    assert_eq!(line.spans[0].style.fg, Some(ai_color()));
    assert!(!text.contains('\n'));
    status.phase = "streaming".into();
    status.output_tps_milli = Some(12_500);
    assert!(
        live_processing_status_line(&status, None, "⠋")
            .to_string()
            .contains("12.5 tps")
    );
    status.input_tokens = None;
    status.output_tokens = None;
    status.connection_type = None;
    let text = live_processing_status_line(&status, None, "⠋").to_string();
    assert!(!text.contains('↑') && !text.contains("websocket"));
    status.phase = "stopped".into();
    status.stop_reason = Some("Rate limit\nretry later".into());
    let text = live_processing_status_line(&status, None, "⠋").to_string();
    assert!(text.contains("Rate limit retry later") && !text.contains('⠋'));
}

#[test]
fn overscroll_merges_cloud_machine_with_dir_and_fits_narrow() {
    let text =
        |spans: &[Span<'static>]| spans.iter().map(|s| s.content.as_ref()).collect::<String>();
    let facts = OverscrollFacts {
        machine: Some("jcode-cloud".into()),
        dir: Some("~/jcode".into()),
        branch: None,
        git: None,
        context: None,
        account_remaining: None,
        auth: None,
        provider: None,
        model: Some("Opus".into()),
        effort: None,
    };
    let (wide, _) = overscroll_fit_facts(&facts, 80);
    assert_eq!(text(&wide), "☁ jcode-cloud:~/jcode Opus");
    let (narrow, fit) = overscroll_fit_facts(&facts, 14);
    assert!(fit);
    assert_eq!(text(&narrow), "☁ ~/jcode Opus");
    let (tiny, _) = overscroll_fit_facts(&facts, 12);
    assert_eq!(text(&tiny), "☁ jcode Opus");
    let local = OverscrollFacts {
        machine: None,
        ..facts
    };
    let (spans, _) = overscroll_fit_facts(&local, 80);
    assert!(!text(&spans).contains('☁'));
}
#[test]
fn overscroll_context_shows_counts_without_bar_and_compacts_when_narrow() {
    let facts = OverscrollFacts {
        machine: None,
        dir: Some("~/jcode".into()),
        branch: None,
        git: None,
        context: Some((74_000, 256_000)),
        account_remaining: None,
        auth: None,
        provider: None,
        model: Some("Opus".into()),
        effort: None,
    };
    for width in [16, 40, 200] {
        let (spans, fits) = overscroll_fit_facts(&facts, width);
        let text: String = spans.iter().map(|span| span.content.as_ref()).collect();
        assert!(fits, "{width}: {text}");
        assert!(text.contains("29%"), "{width}: {text}");
        assert!(!text.contains('▰') && !text.contains('▱'));
        if width >= 40 {
            assert!(text.contains("74k/256k 29%"), "{width}: {text}");
        } else {
            assert!(!text.contains("74k"), "{width}: {text}");
        }
    }
    for (used, limit, expected, color) in [
        (0, 0, "0%", rgb(100, 200, 100)),
        (50, 100, "50%", rgb(255, 200, 100)),
        (80, 100, "80%", rgb(255, 100, 100)),
        (200, 100, "100%", rgb(255, 100, 100)),
    ] {
        let span = overscroll_context_percentage(used, limit);
        assert_eq!(span.content, expected);
        assert_eq!(span.style.fg, Some(color));
    }
}

#[test]
fn overscroll_account_remaining_uses_available_matching_subscription_limits() {
    use crate::tui::info_widget::{AuthMethod, InfoWidgetData, UsageInfo, UsageProvider};
    let mut data = InfoWidgetData::default();
    assert_eq!(overscroll_account_remaining(&data), None);
    data.auth_method = AuthMethod::OpenAIOAuth;
    data.usage_info = Some(UsageInfo {
        provider: UsageProvider::OpenAI,
        available: true,
        five_hour: 0.25,
        seven_day: 0.60,
        ..Default::default()
    });
    assert_eq!(overscroll_account_remaining(&data), Some(40));
    data.usage_info.as_mut().unwrap().spark = Some(0.8);
    assert_eq!(overscroll_account_remaining(&data), Some(20));
    data.usage_info.as_mut().unwrap().available = false;
    assert_eq!(overscroll_account_remaining(&data), None);
    data.usage_info.as_mut().unwrap().limit_reached = true;
    assert_eq!(overscroll_account_remaining(&data), Some(0));
    data.auth_method = AuthMethod::OpenAIApiKey;
    assert_eq!(overscroll_account_remaining(&data), None);
    data.auth_method = AuthMethod::AnthropicOAuth;
    assert_eq!(overscroll_account_remaining(&data), None);
    let info = data.usage_info.as_mut().unwrap();
    info.provider = UsageProvider::Anthropic;
    info.available = true;
    info.limit_reached = false;
    info.spark = None;
    assert_eq!(overscroll_account_remaining(&data), Some(40));
    data.usage_info.as_mut().unwrap().five_hour = f32::NAN;
    assert_eq!(overscroll_account_remaining(&data), None);
}

#[test]
fn overscroll_context_formats_small_and_large_token_counts() {
    for (tokens, expected) in [
        (0, "0"),
        (999, "999"),
        (1_234, "1.2k"),
        (74_000, "74k"),
        (256_000, "256k"),
        (1_000_000, "1.0M"),
    ] {
        assert_eq!(overscroll_format_tokens(tokens), expected);
    }
}

#[test]
fn reset_status_hint_requires_openai_oauth_and_fresh_exhausted_account() {
    use crate::tui::info_widget::AuthMethod;
    let mut usage = crate::usage::OpenAIUsageData {
        openai_reset_credits: Some(crate::usage::OpenAiResetCredits {
            available_count: 2,
            available_expirations: Vec::new(),
            account_label: Some("work".into()),
            ordinary_usage_allowed: Some(false),
        }),
        fetched_at: Some(std::time::Instant::now()),
        ..Default::default()
    };
    assert_eq!(
        openai_reset_status_hint(AuthMethod::OpenAIOAuth, &usage, Some("work")),
        Some("2 resets available · expiry unknown · /reset".to_owned())
    );
    for auth in [
        AuthMethod::OpenAIApiKey,
        AuthMethod::AnthropicOAuth,
        AuthMethod::Unknown,
    ] {
        assert_eq!(openai_reset_status_hint(auth, &usage, Some("work")), None);
    }
    assert_eq!(
        openai_reset_status_hint(AuthMethod::OpenAIOAuth, &usage, Some("other")),
        None
    );
    usage
        .openai_reset_credits
        .as_mut()
        .unwrap()
        .ordinary_usage_allowed = Some(true);
    assert_eq!(
        openai_reset_status_hint(AuthMethod::OpenAIOAuth, &usage, Some("work")),
        None
    );
    usage
        .openai_reset_credits
        .as_mut()
        .unwrap()
        .ordinary_usage_allowed = Some(false);
    usage.fetched_at = None;
    assert_eq!(
        openai_reset_status_hint(AuthMethod::OpenAIOAuth, &usage, Some("work")),
        None
    );
}

#[test]
fn reset_status_hint_lists_each_expiry_and_marks_missing_metadata() {
    use crate::tui::info_widget::AuthMethod;
    let mut usage = crate::usage::OpenAIUsageData {
        openai_reset_credits: Some(crate::usage::OpenAiResetCredits {
            available_count: 4,
            available_expirations: vec![
                Some("2099-05-01T03:30:00+03:00".into()),
                Some("2099-06-01T00:00:00Z".into()),
                Some("not-a-date".into()),
            ],
            account_label: None,
            ordinary_usage_allowed: Some(false),
        }),
        fetched_at: Some(std::time::Instant::now()),
        ..Default::default()
    };
    assert_eq!(
        openai_reset_status_hint(AuthMethod::OpenAIOAuth, &usage, None).unwrap(),
        format!(
            "4 resets available · expires in {} · /reset",
            crate::usage::format_reset_time("2099-05-01T00:30:00Z")
        )
    );
    let credits = usage.openai_reset_credits.as_mut().unwrap();
    credits.available_count = 1;
    credits.available_expirations = vec![None];
    assert_eq!(
        openai_reset_status_hint(AuthMethod::OpenAIOAuth, &usage, None).unwrap(),
        "1 reset available · expiry unknown · /reset"
    );
    usage.openai_reset_credits.as_mut().unwrap().available_count = 0;
    assert!(openai_reset_status_hint(AuthMethod::OpenAIOAuth, &usage, None).is_none());
}

#[test]
fn swarm_effort_model_status_uses_shared_label() {
    assert_eq!(
        overscroll_short_reasoning("swarm"),
        Some(crate::tui::app::effort_display_label("swarm"))
    );
    assert_eq!(overscroll_short_reasoning(" high "), Some("high"));
    assert_eq!(overscroll_short_reasoning(" "), None);
}

#[test]
fn running_tool_header_emphasizes_detail_over_tool_name() {
    let accent = Color::Rgb(12, 34, 56);
    let spans = running_tool_header_spans("*", "bash", Some("cargo test"), accent);

    assert_eq!(spans.len(), 3);
    assert_eq!(spans[0].content.as_ref(), "*");
    assert_eq!(spans[0].style.fg, Some(accent));
    assert_eq!(spans[1].content.as_ref(), " running bash");
    assert_eq!(spans[1].style.fg, Some(dim_color()));
    assert!(!spans[1].style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(spans[2].content.as_ref(), " · cargo test");
    assert_eq!(spans[2].style.fg, Some(accent));
    assert!(spans[2].style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn visual_line_move_follows_soft_wrapped_rows() {
    // 20 chars, width 10 => two visual rows, no newline in the input.
    let input = "abcdefghijklmnopqrst";
    // Cursor at col 3 of row 1 (char 13) moving up lands on col 3 of row 0.
    assert_eq!(visual_line_move(input, 13, 10, -1), Some(3));
    // And back down again.
    assert_eq!(visual_line_move(input, 3, 10, 1), Some(13));
    // Already on the first/last row => None so history recall can take over.
    assert_eq!(visual_line_move(input, 3, 10, -1), None);
    assert_eq!(visual_line_move(input, 13, 10, 1), None);
}

#[test]
fn visual_line_move_clamps_to_shorter_target_row() {
    let input = "abcdefghij\nxy";
    // Cursor at end of the short second row, up goes to col 2 of row 0.
    assert_eq!(visual_line_move(input, input.len(), 10, -1), Some(2));
    // From far along row 0, down clamps to the end of the short row.
    assert_eq!(visual_line_move(input, 8, 10, 1), Some(input.len()));
}

#[test]
fn overscroll_provider_display_is_credential_neutral() {
    // The credential (OAuth vs API key) is reported by the adjacent auth
    // chip from canonical resolution; the provider name must not bake in a
    // credential or the two can contradict (e.g. "Claude OAuth · API key").
    assert_eq!(overscroll_provider_display("claude"), "Claude");
    assert_eq!(overscroll_provider_display("anthropic"), "Anthropic");
    assert!(!overscroll_provider_display("claude").contains("OAuth"));
    assert!(!overscroll_provider_display("anthropic").contains("API"));
}

#[test]
fn session_history_warning_is_clear_and_occasional() {
    assert!(occasional_session_history_warning(249_999, 0, None, 100, 0).is_none());
    assert!(occasional_session_history_warning(300_000, 0, None, 63, 0).is_none());
    assert!(occasional_session_history_warning(300_000, 0, None, 100, 10).is_none());
    assert!(occasional_session_history_warning(199_999, 0, Some(200_000), 100, 0).is_none());
    assert!(occasional_session_history_warning(300_000, 0, Some(400_000), 100, 0).is_none());
    assert!(occasional_session_history_warning(450_000, 0, Some(400_000), 100, 0).is_none());

    let warning = occasional_session_history_warning(2_500_000, 4, Some(500_000), 100, 0)
        .expect("large sessions should get a brief reminder");
    assert!(warning.contains("Session history: 2.5M tokens processed and 4 compacts"));
    assert!(warning.contains("/clear starts fresh context"));
    assert!(!warning.contains("Context usage"));
}

#[test]
fn command_suggestion_window_start_scrolls_after_visible_limit() {
    let limit = app::COMMAND_SUGGESTION_VISIBLE_LIMIT;
    assert_eq!(command_suggestion_window_start(0, limit + 3), 0);
    assert_eq!(command_suggestion_window_start(limit - 1, limit + 3), 0);
    assert_eq!(command_suggestion_window_start(limit, limit + 3), 1);
    assert_eq!(command_suggestion_window_start(limit + 2, limit + 3), 3);
}

#[test]
fn command_suggestions_overlay_prefers_space_below_input() {
    let frame = Rect::new(0, 0, 80, 20);
    let input_area = Rect::new(0, 10, 80, 1);

    assert_eq!(
        command_suggestions_overlay_rect(input_area, 3, frame),
        Some(Rect::new(0, 11, 80, 3))
    );
}

#[test]
fn command_suggestions_overlay_flips_above_at_terminal_bottom() {
    let frame = Rect::new(0, 0, 80, 20);
    let input_area = Rect::new(0, 19, 80, 1);

    assert_eq!(
        command_suggestions_overlay_rect(input_area, 3, frame),
        Some(Rect::new(0, 16, 80, 3))
    );
}

#[test]
fn command_suggestions_overlay_handles_zero_lines_and_no_space() {
    let frame = Rect::new(0, 0, 80, 20);
    let input_area = Rect::new(0, 10, 80, 1);
    assert_eq!(command_suggestions_overlay_rect(input_area, 0, frame), None);

    // Input fills the whole frame: nowhere to float, so no popover.
    let full = Rect::new(0, 0, 80, 20);
    assert_eq!(command_suggestions_overlay_rect(full, 3, frame), None);
}

#[test]
fn batch_progress_spans_use_batch_chroma_for_initial_count() {
    let mut spans = Vec::new();
    let anim_color = rgb(12, 34, 56);

    append_batch_progress_spans(&mut spans, anim_color, None, Some(3));

    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].content.as_ref(), " · 0/3 done");
    assert_eq!(spans[0].style.fg, Some(anim_color));
    assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn batch_progress_spans_make_last_completed_explicit() {
    let mut spans = Vec::new();

    append_batch_progress_spans(
        &mut spans,
        rgb(120, 130, 140),
        Some(crate::bus::BatchProgress {
            session_id: "s".to_string(),
            tool_call_id: "tc".to_string(),
            total: 3,
            completed: 1,
            last_completed: Some("read".to_string()),
            running: Vec::new(),
            subcalls: Vec::new(),
        }),
        Some(3),
    );

    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].content.as_ref(), " · 1/3 done");
    assert_eq!(spans[1].content.as_ref(), " · last done: read");
}

#[test]
fn batch_progress_spans_hide_last_completed_when_batch_finished() {
    let mut spans = Vec::new();

    append_batch_progress_spans(
        &mut spans,
        rgb(120, 130, 140),
        Some(crate::bus::BatchProgress {
            session_id: "s".to_string(),
            tool_call_id: "tc".to_string(),
            total: 3,
            completed: 3,
            last_completed: Some("read".to_string()),
            running: Vec::new(),
            subcalls: Vec::new(),
        }),
        Some(3),
    );

    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].content.as_ref(), " · 3/3 done");
}

#[test]
fn batch_progress_spans_show_running_subcall_detail() {
    let mut spans = Vec::new();

    append_batch_progress_spans(
        &mut spans,
        rgb(120, 130, 140),
        Some(crate::bus::BatchProgress {
            session_id: "s".to_string(),
            tool_call_id: "tc".to_string(),
            total: 2,
            completed: 0,
            last_completed: None,
            running: vec![crate::message::ToolCall {
                id: "batch-1-bash".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"command": "cargo test -p jcode"}),
                intent: None,
                thought_signature: None,
            }],
            subcalls: Vec::new(),
        }),
        Some(2),
    );

    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].content.as_ref(), " · 0/2 done");
    assert_eq!(spans[1].content.as_ref(), " · running: #1 bash");
}

#[test]
fn batch_progress_spans_show_multiple_running_subcalls() {
    let mut spans = Vec::new();

    append_batch_progress_spans(
        &mut spans,
        rgb(120, 130, 140),
        Some(crate::bus::BatchProgress {
            session_id: "s".to_string(),
            tool_call_id: "tc".to_string(),
            total: 3,
            completed: 0,
            last_completed: None,
            running: vec![
                crate::message::ToolCall {
                    id: "batch-2-grep".to_string(),
                    name: "grep".to_string(),
                    input: serde_json::json!({"pattern": "foo", "path": "src"}),
                    intent: None,
                    thought_signature: None,
                },
                crate::message::ToolCall {
                    id: "batch-1-bash".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "cargo build --release --workspace"}),
                    intent: None,
                    thought_signature: None,
                },
                crate::message::ToolCall {
                    id: "batch-3-read".to_string(),
                    name: "read".to_string(),
                    input: serde_json::json!({"file_path": "README.md"}),
                    intent: None,
                    thought_signature: None,
                },
            ],
            subcalls: Vec::new(),
        }),
        Some(3),
    );

    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].content.as_ref(), " · 0/3 done");
    assert_eq!(spans[1].content.as_ref(), " · running: #1 bash +2");
}

#[test]
fn connection_phase_waiting_label_is_generic_response_wait() {
    assert_eq!(
        connection_phase_label(&ConnectionPhase::WaitingForResponse),
        "waiting for response"
    );
}

#[test]
fn streaming_liveness_label_shows_quiet_stream_warning_before_message_end() {
    assert_eq!(
        streaming_liveness_label("4.2s".to_string(), Some(3.4), false),
        "(no tokens 3s) · 4.2s"
    );
    assert_eq!(
        streaming_liveness_label("12.0s".to_string(), Some(12.1), false),
        "(stalled 12s) · 12.0s"
    );
}

#[test]
fn streaming_liveness_label_suppresses_quiet_stream_warning_after_message_end() {
    assert_eq!(
        streaming_liveness_label("4.2s".to_string(), Some(3.4), true),
        "4.2s"
    );
    assert_eq!(
        streaming_liveness_label("12.0s".to_string(), Some(12.1), true),
        "12.0s"
    );
}

#[test]
fn streaming_status_spans_keep_spinner_while_finalizing() {
    let spans = streaming_status_spans("⠋", "4.2s".to_string(), false, false, " · +1 queued");

    assert_eq!(spans.len(), 3);
    assert_eq!(spans[0].content.as_ref(), "⠋");
    assert_eq!(spans[1].content.as_ref(), " 4.2s");
    assert_eq!(spans[2].content.as_ref(), " · +1 queued");
}

#[test]
fn streaming_status_spans_keep_spinner_after_message_end_while_finalizing() {
    let spans = streaming_status_spans("⠋", "finalizing".to_string(), true, false, "");

    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].content.as_ref(), "⠋");
    assert_eq!(spans[1].content.as_ref(), " finalizing");
}

#[test]
fn push_queued_suffix_appends_only_when_present() {
    let mut spans: Vec<Span<'static>> = Vec::new();
    push_queued_suffix(&mut spans, "");
    assert!(spans.is_empty(), "empty suffix should add no span");

    push_queued_suffix(&mut spans, " · +2 queued");
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].content.as_ref(), " · +2 queued");
    assert_eq!(spans[0].style.fg, Some(queued_color()));
}

#[test]
fn display_connection_type_uses_reader_friendly_labels() {
    assert_eq!(display_connection_type("https/sse"), "https");
    assert_eq!(
        display_connection_type("websocket/persistent-fresh"),
        "websocket"
    );
    assert_eq!(
        display_connection_type("websocket/persistent-reuse"),
        "existing websocket"
    );
}

#[test]
fn normalize_status_detail_uses_reader_friendly_labels() {
    assert_eq!(
        normalize_status_detail("fresh websocket").as_deref(),
        Some("opening websocket")
    );
    assert_eq!(
        normalize_status_detail("reusing websocket").as_deref(),
        Some("using existing websocket")
    );
    assert_eq!(
        normalize_status_detail("websocket healthcheck").as_deref(),
        Some("verifying websocket")
    );
    assert_eq!(
        normalize_status_detail("https fallback").as_deref(),
        Some("using https fallback")
    );
}

#[test]
fn collect_transport_context_labels_dedupes_overlapping_transport_text() {
    assert_eq!(
        collect_transport_context_labels(
            normalize_status_detail("reusing websocket"),
            Some(display_connection_type("websocket/persistent-reuse")),
            Some("OpenRouter".to_string())
        ),
        vec![
            "using existing websocket".to_string(),
            "via OpenRouter".to_string()
        ]
    );

    assert_eq!(
        collect_transport_context_labels(
            normalize_status_detail("https fallback"),
            Some(display_connection_type("https/sse")),
            None,
        ),
        vec!["using https fallback".to_string()]
    );
}

#[test]
fn composer_mode_detects_shell_input_before_commands() {
    assert_eq!(
        composer_mode(" ! cargo test ", false),
        ComposerMode::ShellLocal
    );
    assert_eq!(
        composer_mode("! cargo test", true),
        ComposerMode::ShellRemote
    );
    assert_eq!(composer_mode(" /help", false), ComposerMode::SlashCommand);
    assert_eq!(composer_mode("hello", false), ComposerMode::Chat);
}

#[test]
fn shell_mode_hint_reflects_execution_target() {
    assert_eq!(
        shell_mode_hint(ComposerMode::ShellLocal),
        Some("  shell mode · Enter runs locally")
    );
    assert_eq!(
        shell_mode_hint(ComposerMode::ShellRemote),
        Some("  shell mode · Enter runs on server")
    );
    assert_eq!(shell_mode_hint(ComposerMode::Chat), None);
}

#[test]
fn shell_mode_color_is_distinct() {
    assert_eq!(shell_mode_color(), rgb(110, 214, 151));
}

#[test]
fn normalize_repaint_sensitive_notice_text_drops_warning_variation_selector() {
    assert_eq!(
        normalize_repaint_sensitive_notice_text("⚠️ File activity: read lines 1-9"),
        "⚠ File activity: read lines 1-9"
    );
    assert_eq!(
        normalize_repaint_sensitive_notice_text("all clear"),
        "all clear"
    );
}
