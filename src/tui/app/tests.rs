#![cfg_attr(test, allow(clippy::clone_on_copy))]

include!("tests/support_failover/part_01.rs");
include!("tests/support_failover/part_02.rs");
include!("tests/commands_accounts_01/part_01.rs");
include!("tests/commands_accounts_01/part_02.rs");
include!("tests/commands_accounts_02/part_01.rs");
include!("tests/commands_accounts_02/part_02.rs");
include!("tests/state_model_poke_01/part_01.rs");
include!("tests/state_model_poke_01/part_02.rs");
include!("tests/state_model_poke_02/part_01.rs");
include!("tests/state_model_poke_02/part_02.rs");
include!("tests/state_model_poke_03.rs");
include!("tests/remote_startup_input_01/part_01.rs");
include!("tests/remote_startup_input_01/part_02.rs");
include!("tests/remote_startup_input_02/part_01.rs");
include!("tests/remote_startup_input_02/part_02.rs");
include!("tests/remote_startup_input_03/part_01.rs");
include!("tests/remote_startup_input_03/part_02.rs");
include!("tests/remote_startup_input_04.rs");
include!("tests/remote_events_reload_01/part_01.rs");
include!("tests/remote_events_reload_01/part_02.rs");
include!("tests/remote_events_reload_02/part_01.rs");
include!("tests/remote_events_reload_02/part_02.rs");
include!("tests/remote_events_reload_03/part_01.rs");
include!("tests/remote_events_reload_03/part_02.rs");
include!("tests/remote_events_reload_04.rs");
include!("tests/scroll_copy_01/part_01.rs");
include!("tests/scroll_copy_01/part_02.rs");
include!("tests/scroll_copy_02/part_01.rs");
include!("tests/scroll_copy_02/part_02.rs");
include!("tests/scroll_copy_03.rs");

#[test]
fn kv_cache_signature_prefix_match_allows_appended_messages() {
    let baseline_messages = vec![
        crate::message::Message::user("first prompt"),
        crate::message::Message::assistant_text("first answer"),
    ];
    let mut current_messages = baseline_messages.clone();
    current_messages.push(crate::message::Message::user("follow up"));

    let baseline = App::kv_cache_request_signature(&baseline_messages, &[], "system", "memory a");
    let current = App::kv_cache_request_signature(&current_messages, &[], "system", "memory b");

    assert!(App::kv_cache_signatures_prefix_match(&current, &baseline));
    assert_eq!(
        App::kv_cache_common_prefix_messages(&current, &baseline),
        baseline_messages.len()
    );
    assert_ne!(baseline.ephemeral_hash, current.ephemeral_hash);
}

#[test]
fn kv_cache_signature_prefix_match_detects_prefix_mutation() {
    let baseline_messages = vec![
        crate::message::Message::user("first prompt"),
        crate::message::Message::assistant_text("first answer"),
    ];
    let current_messages = vec![
        crate::message::Message::user("changed first prompt"),
        crate::message::Message::assistant_text("first answer"),
        crate::message::Message::user("follow up"),
    ];

    let baseline = App::kv_cache_request_signature(&baseline_messages, &[], "system", "");
    let current = App::kv_cache_request_signature(&current_messages, &[], "system", "");

    assert!(!App::kv_cache_signatures_prefix_match(&current, &baseline));
    assert_eq!(App::kv_cache_common_prefix_messages(&current, &baseline), 0);
}

#[test]
fn cold_cache_warning_is_persisted_when_starting_next_request() {
    let mut app = create_test_app();
    crate::provider::anthropic::set_cache_ttl_1h(true);
    app.display_messages.push(DisplayMessage::user("first"));
    app.kv_cache_baseline = Some(KvCacheBaseline {
        input_tokens: 911_873,
        completed_at: Instant::now() - Duration::from_secs(3723),
        provider: "anthropic".to_string(),
        model: "claude-opus-4-6".to_string(),
        upstream_provider: None,
        signature: None,
    });

    app.display_messages.push(DisplayMessage::user("second"));
    app.begin_kv_cache_request(&[Message::user("second")], &[], "system", "");

    let warning = app
        .display_messages()
        .iter()
        .find(|message| {
            message.role == "system" && message.content.contains("Prompt cache is cold")
        })
        .expect("cold cache warning should be persisted in the transcript");
    assert!(warning.content.contains("911K"));
    assert!(
        warning.content.contains("3600s TTL expired 123s ago")
            || warning.content.contains("3600s TTL expired 124s ago"),
        "{warning:?}"
    );
    assert!(
        warning.content.contains("last cache write was 3723s ago")
            || warning.content.contains("last cache write was 3724s ago"),
        "{warning:?}"
    );
}

#[test]
fn remote_token_usage_records_cache_stats_before_done_and_dedupes_snapshots() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_remote = true;
    app.remote_provider_name = Some("OpenAI".to_string());
    app.remote_provider_model = Some("gpt-5.5".to_string());
    app.display_messages
        .push(DisplayMessage::user("live prompt"));

    app.handle_server_event(
        crate::protocol::ServerEvent::KvCacheRequest {
            system_static_hash: 1,
            tools_hash: 2,
            messages_hash: 3,
            message_hashes: vec![11, 22],
            message_count: 2,
            tool_count: 33,
            system_static_chars: 11155,
            tools_json_chars: 35228,
            messages_json_chars: 198612,
            ephemeral_hash: None,
            ephemeral_chars: 2,
            ephemeral_message_count: 0,
        },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 63_762,
            output: 153,
            cache_read_input: Some(0),
            cache_creation_input: None,
        },
        &mut remote,
    );

    assert_eq!(app.total_cache_reported_input_tokens, 63_762);
    assert_eq!(app.total_cache_read_tokens, 0);
    assert_eq!(app.last_cache_reported_input_tokens, Some(63_762));
    assert_eq!(app.total_input_tokens, 63_762);
    assert!(app.last_api_completed.is_some());
    assert!(app.pending_kv_cache_request.is_none());

    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 63_762,
            output: 153,
            cache_read_input: Some(0),
            cache_creation_input: None,
        },
        &mut remote,
    );

    assert_eq!(app.total_cache_reported_input_tokens, 63_762);
    assert_eq!(app.total_input_tokens, 63_762);

    assert!(super::state_ui::handle_info_command(
        &mut app,
        "/cache stats"
    ));
    let stats = app.display_messages().last().unwrap().content.clone();
    assert!(
        stats.contains("- total_cache_reported_input_tokens: **63.8k (63,762)**"),
        "{stats}"
    );
    assert!(
        stats.contains("- baseline.signature.messages_json_chars: **198.6k (198,612)**"),
        "{stats}"
    );
    assert!(
        stats.contains("- current_api_usage_recorded: **true**"),
        "{stats}"
    );
}

#[test]
fn cache_stats_uses_remote_history_token_usage_totals() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_total_tokens = Some((1_250_000, 200_000));
    app.remote_token_usage_totals = Some(crate::protocol::TokenUsageTotals {
        messages_with_token_usage: 3,
        input_tokens: 1_250_000,
        output_tokens: 200_000,
        cache_reported_input_tokens: 1_000_000,
        cache_read_input_tokens: 600_000,
        cache_creation_input_tokens: 50_000,
    });

    assert!(super::state_ui::handle_info_command(
        &mut app,
        "/cache stats"
    ));
    let stats = app.display_messages().last().unwrap().content.clone();
    assert!(
        stats.contains("- total_tokens_source: `remote_history`"),
        "{stats}"
    );
    assert!(
        stats.contains("- total_input_tokens: **1.25m (1,250,000)**"),
        "{stats}"
    );
    assert!(
        stats.contains("- cache_totals_source: `remote_history`"),
        "{stats}"
    );
    assert!(
        stats.contains("- total_cache_reported_input_tokens: **1m (1,000,000)**"),
        "{stats}"
    );
    assert!(
        stats.contains("- persisted_token_usage_source: `remote_history`"),
        "{stats}"
    );
    assert!(
        stats.contains("- messages_with_token_usage: **3**"),
        "{stats}"
    );
}

#[test]
fn remote_done_finalizes_resumed_activity_without_current_message_id() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_remote = true;
    app.is_processing = true;
    app.status = ProcessingStatus::RunningTool("bg".to_string());
    app.remote_resume_activity = Some(RemoteResumeActivity {
        session_id: "session_resume_cache_stats".to_string(),
        observed_at: Instant::now(),
        current_tool_name: Some("bg".to_string()),
    });
    app.streaming_input_tokens = 63_762;
    app.streaming_output_tokens = 153;
    app.streaming_cache_read_tokens = Some(0);
    app.stream_message_ended = true;

    app.handle_server_event(crate::protocol::ServerEvent::Done { id: 99 }, &mut remote);

    assert!(!app.is_processing);
    assert!(matches!(app.status, ProcessingStatus::Idle));
    assert!(app.remote_resume_activity.is_none());
    assert!(app.last_api_completed.is_some());
}

#[test]
fn oversized_pasted_submit_is_rejected_and_preserves_input() {
    let mut app = create_test_app();
    let pasted = format!(
        "{}tail",
        "x\n".repeat(crate::tui::app::input::MAX_SUBMITTED_TEXT_BYTES / 2 + 1)
    );

    crate::tui::app::input::handle_text_paste(&mut app, pasted);
    let placeholder = app.input.clone();
    assert!(placeholder.starts_with("[pasted "));

    app.submit_input();

    assert!(
        !app.is_processing,
        "oversized input must not enter sending state"
    );
    assert_eq!(
        app.input, placeholder,
        "placeholder input should be preserved"
    );
    assert_eq!(
        app.pasted_contents.len(),
        1,
        "expanded paste should remain recoverable"
    );
    assert!(
        app.display_messages()
            .iter()
            .any(|message| message.role == "system"
                && message.content.contains("Message is too large to send"))
    );
}
