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
include!("tests/onboarding_flow.rs");
include!("tests/onboarding_golden.rs");

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
        stats.contains("- total_cache_reported_input_tokens: 63.8k (63,762)"),
        "{stats}"
    );
    assert!(
        stats.contains("- baseline.signature.messages_json_chars: 198.6k (198,612)"),
        "{stats}"
    );
    assert!(
        stats.contains("- current_api_usage_recorded: true"),
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
        stats.contains("- total_tokens_source: remote_history"),
        "{stats}"
    );
    assert!(
        stats.contains("- total_input_tokens: 1.25m (1,250,000)"),
        "{stats}"
    );
    assert!(
        stats.contains("- cache_totals_source: remote_history"),
        "{stats}"
    );
    assert!(
        stats.contains("- total_cache_reported_input_tokens: 1m (1,000,000)"),
        "{stats}"
    );
    assert!(
        stats.contains("- persisted_token_usage_source: remote_history"),
        "{stats}"
    );
    assert!(stats.contains("- messages_with_token_usage: 3"), "{stats}");
}

#[test]
fn version_command_shows_remote_server_identity_and_update_status() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_server_short_name = Some("blazing".to_string());
    app.remote_server_icon = Some("🔥".to_string());
    app.remote_server_version = Some("v0.14.2-dev (old)".to_string());
    app.remote_server_has_update = Some(true);

    assert!(super::state_ui::handle_info_command(&mut app, "/version"));
    let content = app.display_messages().last().unwrap().content.clone();
    assert!(content.contains("jcode client:"), "{content}");
    assert!(content.contains("mode: remote/shared-server"), "{content}");
    assert!(content.contains("server: 🔥 blazing"), "{content}");
    assert!(
        content.contains("server version: v0.14.2-dev (old)"),
        "{content}"
    );
    assert!(content.contains("reload recommended"), "{content}");
}

#[test]
fn update_command_reloads_stale_remote_server_before_client_update_check() {
    use tokio::io::AsyncBufReadExt;

    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_server_has_update = Some(true);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut line = String::new();
    let reloaded = rt.block_on(async {
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        let peer = remote
            .take_dummy_peer()
            .expect("dummy remote should retain peer stream");
        let (reader, _writer) = peer.into_split();
        let mut reader = tokio::io::BufReader::new(reader);

        let reloaded =
            super::remote::reload_stale_remote_server_before_update(&mut app, &mut remote)
                .await
                .expect("stale server reload request should send");
        reader
            .read_line(&mut line)
            .await
            .expect("reload request should be readable by peer");
        reloaded
    });

    assert!(reloaded);
    assert!(matches!(
        serde_json::from_str::<crate::protocol::Request>(&line)
            .expect("reload request should deserialize"),
        crate::protocol::Request::Reload { id: 1, force: true }
    ));
    let content = app.display_messages().last().unwrap().content.clone();
    assert!(content.contains("Reloading stale server"), "{content}");
}

#[test]
fn stale_server_history_is_deferred_before_remote_state_is_applied() {
    crate::env::remove_var("JCODE_ALLOW_SERVER_VERSION_MISMATCH");
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_remote = true;
    app.remote_session_id = Some("session_existing".to_string());
    app.connection_type = Some("websocket".to_string());

    let redraw = app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "session_from_stale_server".to_string(),
            messages: vec![crate::protocol::HistoryMessage {
                role: "assistant".to_string(),
                content: "stale answer".to_string(),
                tool_calls: None,
                tool_data: None,
            }],
            images: vec![],
            provider_name: Some("stale-provider".to_string()),
            provider_model: Some("stale-model".to_string()),
            subagent_model: Some("stale-subagent".to_string()),
            autoreview_enabled: Some(true),
            autojudge_enabled: Some(true),
            available_models: vec!["stale-model".to_string()],
            available_model_routes: vec![],
            mcp_servers: vec!["stale-mcp:1".to_string()],
            skills: vec!["stale-skill".to_string()],
            total_tokens: Some((99, 100)),
            token_usage_totals: None,
            all_sessions: vec!["session_from_stale_server".to_string()],
            client_count: Some(42),
            is_canary: Some(false),
            reload_recovery: None,
            server_version: Some("v0.0.1-stale".to_string()),
            server_name: Some("stale-server".to_string()),
            server_icon: Some("🧟".to_string()),
            server_has_update: Some(true),
            was_interrupted: None,
            connection_type: Some("stale-connection".to_string()),
            status_detail: Some("stale-status".to_string()),
            upstream_provider: Some("stale-upstream".to_string()),
            reasoning_effort: Some("high".to_string()),
            service_tier: Some("stale-tier".to_string()),
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: None,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
        },
        &mut remote,
    );

    assert!(!redraw);
    assert!(app.pending_server_reload);
    assert_eq!(app.remote_server_has_update, Some(true));
    assert_eq!(app.remote_server_version.as_deref(), Some("v0.0.1-stale"));
    assert_eq!(app.remote_session_id.as_deref(), Some("session_existing"));
    assert_eq!(remote.session_id(), None);
    assert_eq!(app.connection_type.as_deref(), Some("websocket"));
    assert!(app.remote_skills.is_empty());
    assert!(app.remote_sessions.is_empty());
    assert_eq!(app.remote_client_count, None);
    assert_eq!(app.remote_total_tokens, None);
    assert_ne!(
        app.session.subagent_model.as_deref(),
        Some("stale-subagent")
    );
    let content = app.display_messages().last().unwrap().content.clone();
    assert!(
        content.contains("Reloading the server before applying remote session state"),
        "{content}"
    );
}

#[test]
fn ancient_server_history_is_deferred_via_client_side_release_check() {
    // Issue #295: a server old enough to predate the self-reported staleness
    // machinery sends `server_has_update: None`, so it can never tell the client
    // it is stale. The client must independently compare release versions and
    // defer + reload anyway, instead of attaching to the ancient daemon (which
    // would then reject newer protocol requests like `set_route`).
    crate::env::remove_var("JCODE_ALLOW_SERVER_VERSION_MISMATCH");
    // The test binary's own version is dev/dirty (unorderable), so use the
    // test-only override to give the client a clean release version newer than
    // the simulated ancient server.
    crate::env::set_var("JCODE_TEST_CLIENT_VERSION_OVERRIDE", "v0.17.0 (d741696f)");

    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_remote = true;
    app.remote_session_id = Some("session_existing".to_string());
    app.connection_type = Some("websocket".to_string());

    let redraw = app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "session_from_ancient_server".to_string(),
            messages: vec![crate::protocol::HistoryMessage {
                role: "assistant".to_string(),
                content: "ancient answer".to_string(),
                tool_calls: None,
                tool_data: None,
            }],
            images: vec![],
            provider_name: Some("ancient-provider".to_string()),
            provider_model: Some("ancient-model".to_string()),
            subagent_model: Some("ancient-subagent".to_string()),
            autoreview_enabled: Some(true),
            autojudge_enabled: Some(true),
            available_models: vec!["ancient-model".to_string()],
            available_model_routes: vec![],
            mcp_servers: vec!["ancient-mcp:1".to_string()],
            skills: vec!["ancient-skill".to_string()],
            total_tokens: Some((99, 100)),
            token_usage_totals: None,
            all_sessions: vec!["session_from_ancient_server".to_string()],
            client_count: Some(42),
            is_canary: Some(false),
            reload_recovery: None,
            // Clean older release, and crucially server_has_update is None: the
            // ancient daemon does not know how to self-assess.
            server_version: Some("v0.14.2 (38452185)".to_string()),
            server_name: Some("ancient-server".to_string()),
            server_icon: Some("🦖".to_string()),
            server_has_update: None,
            was_interrupted: None,
            connection_type: Some("ancient-connection".to_string()),
            status_detail: Some("ancient-status".to_string()),
            upstream_provider: Some("ancient-upstream".to_string()),
            reasoning_effort: Some("high".to_string()),
            service_tier: Some("ancient-tier".to_string()),
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: None,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
        },
        &mut remote,
    );

    crate::env::remove_var("JCODE_TEST_CLIENT_VERSION_OVERRIDE");

    assert!(!redraw);
    assert!(app.pending_server_reload);
    // Remote session state must NOT have been applied from the ancient server.
    assert_eq!(app.remote_session_id.as_deref(), Some("session_existing"));
    assert_eq!(remote.session_id(), None);
    assert!(app.remote_skills.is_empty());
    assert!(app.remote_sessions.is_empty());
    assert_ne!(
        app.session.subagent_model.as_deref(),
        Some("ancient-subagent")
    );
    let content = app.display_messages().last().unwrap().content.clone();
    assert!(
        content.contains("older release") && content.contains("jcode server stop"),
        "{content}"
    );
}

#[test]
fn current_release_server_history_is_not_deferred_by_client_check() {
    // A server on the SAME or NEWER clean release as the client, with
    // server_has_update: None, must be trusted and attached normally. This
    // guards against the client-side check over-firing and looping reloads.
    crate::env::remove_var("JCODE_ALLOW_SERVER_VERSION_MISMATCH");
    crate::env::set_var("JCODE_TEST_CLIENT_VERSION_OVERRIDE", "v0.17.0 (d741696f)");

    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_remote = true;
    app.remote_session_id = Some("session_existing".to_string());

    let redraw = app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "session_current".to_string(),
            messages: vec![],
            images: vec![],
            provider_name: Some("p".to_string()),
            provider_model: Some("m".to_string()),
            subagent_model: None,
            autoreview_enabled: None,
            autojudge_enabled: None,
            available_models: vec!["m".to_string()],
            available_model_routes: vec![],
            mcp_servers: vec![],
            skills: vec![],
            total_tokens: None,
            token_usage_totals: None,
            all_sessions: vec!["session_current".to_string()],
            client_count: Some(1),
            is_canary: Some(false),
            reload_recovery: None,
            server_version: Some("v0.17.0 (d741696f)".to_string()),
            server_name: Some("current-server".to_string()),
            server_icon: Some("🟢".to_string()),
            server_has_update: None,
            was_interrupted: None,
            connection_type: Some("websocket".to_string()),
            status_detail: None,
            upstream_provider: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: None,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
        },
        &mut remote,
    );

    crate::env::remove_var("JCODE_TEST_CLIENT_VERSION_OVERRIDE");

    // Attached normally: session id applied, no pending reload triggered by the
    // client-side staleness check. (The History arm always returns false for
    // redraw; the meaningful signal is that state was actually applied.)
    let _ = redraw;
    assert!(!app.pending_server_reload);
    assert_eq!(app.remote_session_id.as_deref(), Some("session_current"));
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
