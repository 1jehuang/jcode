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
include!("tests/reasoning_region.rs");

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
fn kv_cache_signature_ignores_non_transmitted_message_metadata() {
    use crate::message::{ContentBlock, Message, Role};

    // A boundary assistant message that has already been sent upstream. The
    // provider only ever receives its `Text` block; the struct-level timestamp
    // and tool_duration_ms, plus any history-only ReasoningTrace block, are
    // never part of the prompt token stream.
    let baseline_messages = vec![
        Message::user("first prompt"),
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "boundary answer".to_string(),
                cache_control: None,
            }],
            timestamp: Some(chrono::Utc::now()),
            tool_duration_ms: None,
        },
    ];

    // Next request: the same boundary message but with volatile/harness-only
    // fields backfilled in memory, then two appended messages. This mirrors the
    // real production sequence where PROVIDER_CANONICAL_INPUT stayed append-only
    // yet the harness previously reported harness:_prefix_changed.
    let mut current_messages = vec![
        Message::user("first prompt"),
        Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "boundary answer".to_string(),
                    // Ephemeral cache breakpoint that hops to the newest message.
                    cache_control: Some(crate::message::CacheControl::ephemeral(None)),
                },
                // History-only reasoning trace, never replayed to a provider.
                ContentBlock::ReasoningTrace {
                    text: "internal scratch thinking".to_string(),
                },
            ],
            // Backfilled after the tool ran / a newer turn was committed.
            timestamp: Some(chrono::Utc::now() + chrono::Duration::seconds(5)),
            tool_duration_ms: Some(1234),
        },
    ];
    current_messages.push(Message::user("follow up"));
    current_messages.push(Message::assistant_text("second answer"));

    let baseline = App::kv_cache_request_signature(&baseline_messages, &[], "system", "");
    let current = App::kv_cache_request_signature(&current_messages, &[], "system", "");

    assert!(
        App::kv_cache_signatures_prefix_match(&current, &baseline),
        "non-transmitted metadata changes on the boundary message must not break the cache prefix"
    );
    assert_eq!(
        App::kv_cache_common_prefix_messages(&current, &baseline),
        baseline_messages.len(),
        "the whole prior request should still count as a common prefix"
    );
}

#[test]
fn cold_cache_warning_is_persisted_when_starting_next_request() {
    let mut app = create_test_app();
    crate::provider::anthropic::set_cache_ttl_1h(true);
    app.display_messages.push(DisplayMessage::user("first"));
    let session_id = app.kv_cache_session_id();
    app.kv_cache.kv_cache_baseline = Some(KvCacheBaseline {
        session_id,
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
fn kv_cache_baseline_from_other_session_is_ignored() {
    // A single App can stream multiple sessions over its lifetime. A baseline
    // captured for a large session must not be diffed against a fresh, smaller
    // session, or the new history looks like a broken prefix and emits a
    // spurious `harness:_prefix_changed` miss. See the false positives in
    // ~/.jcode/logs KV_CACHE_USAGE telemetry (common_prefix=0, current
    // message_count << baseline_message_count, yet read_pct=100/miss=none).
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_session_id = Some("session_big".to_string());

    let big_history: Vec<Message> = (0..40)
        .map(|i| Message::user(format!("big session message {i}").as_str()))
        .collect();
    let big_signature = App::kv_cache_request_signature(&big_history, &[], "system", "");
    app.kv_cache.kv_cache_baseline = Some(KvCacheBaseline {
        session_id: Some("session_big".to_string()),
        input_tokens: 200_000,
        completed_at: Instant::now(),
        provider: "anthropic".to_string(),
        model: "claude-opus-4-6".to_string(),
        upstream_provider: None,
        signature: Some(big_signature),
    });

    // Switch to a brand-new, much smaller session and start its first request.
    app.remote_session_id = Some("session_small".to_string());
    let small_signature = App::kv_cache_request_signature(
        &[Message::user("hello from small session")],
        &[],
        "system",
        "",
    );
    app.begin_remote_kv_cache_request(small_signature);

    let request = app
        .kv_cache
        .pending_kv_cache_request
        .as_ref()
        .expect("request should be pending");
    assert!(
        request.baseline.is_none(),
        "foreign-session baseline must be treated as absent: {:?}",
        request.baseline
    );
    assert_eq!(
        request.baseline_messages_prefix_matches, None,
        "no cross-session prefix comparison should happen"
    );
}

#[test]
fn kv_cache_baseline_same_session_still_compares() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_session_id = Some("session_same".to_string());

    let history = vec![
        Message::user("first prompt"),
        Message::assistant_text("first answer"),
    ];
    let baseline_signature = App::kv_cache_request_signature(&history, &[], "system", "");
    app.kv_cache.kv_cache_baseline = Some(KvCacheBaseline {
        session_id: Some("session_same".to_string()),
        input_tokens: 1_000,
        completed_at: Instant::now(),
        provider: "anthropic".to_string(),
        model: "claude-opus-4-6".to_string(),
        upstream_provider: None,
        signature: Some(baseline_signature),
    });

    // Append-only growth in the same session keeps the prefix intact.
    let mut grown = history.clone();
    grown.push(Message::user("follow up"));
    let grown_signature = App::kv_cache_request_signature(&grown, &[], "system", "");
    app.begin_remote_kv_cache_request(grown_signature);

    let request = app
        .kv_cache
        .pending_kv_cache_request
        .as_ref()
        .expect("request should be pending");
    assert!(
        request.baseline.is_some(),
        "same-session baseline should be retained"
    );
    assert_eq!(
        request.baseline_messages_prefix_matches,
        Some(true),
        "append-only same-session growth keeps the cached prefix"
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

    assert_eq!(
        app.token_accounting.total_cache_reported_input_tokens,
        63_762
    );
    assert_eq!(app.token_accounting.total_cache_read_tokens, 0);
    assert_eq!(
        app.token_accounting.last_cache_reported_input_tokens,
        Some(63_762)
    );
    assert_eq!(app.token_accounting.total_input_tokens, 63_762);
    assert!(app.last_api_completed.is_some());
    assert!(app.kv_cache.pending_kv_cache_request.is_none());

    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 63_762,
            output: 153,
            cache_read_input: Some(0),
            cache_creation_input: None,
        },
        &mut remote,
    );

    assert_eq!(
        app.token_accounting.total_cache_reported_input_tokens,
        63_762
    );
    assert_eq!(app.token_accounting.total_input_tokens, 63_762);

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
fn skills_command_lists_loaded_and_endorsed_skills() {
    let mut app = create_test_app();

    assert!(super::state_ui::handle_info_command(&mut app, "/skills"));
    let content = app.display_messages().last().unwrap().content.clone();

    assert!(content.contains("Loaded skills"), "{content}");
    assert!(
        content.contains("Endorsed skills (recommended by jcode)"),
        "{content}"
    );
    // Every endorsed skill should appear with an install status marker.
    for endorsed in crate::skill::endorsed_skills() {
        assert!(
            content.contains(&format!("/{}", endorsed.name)),
            "expected endorsed skill /{} in:\n{content}",
            endorsed.name
        );
    }
    assert!(
        content.contains("[installed]") || content.contains("[not installed]"),
        "{content}"
    );
    // NVIDIA CUDA-X skills are grouped under their own category with install hints.
    assert!(content.contains("NVIDIA CUDA-X"), "{content}");
    assert!(
        content.contains("/cuopt-numerical-optimization-api-python"),
        "{content}"
    );
    assert!(
        content.contains("install: npx skills add nvidia/skills"),
        "{content}"
    );
    assert!(
        content.contains("https://github.com/NVIDIA/skills"),
        "{content}"
    );
    assert_eq!(
        app.display_messages().last().unwrap().title.as_deref(),
        Some("Skills")
    );
}

#[test]
fn skills_command_marks_active_skill_in_remote_mode() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_skills = vec!["optimization".to_string(), "firefox-browser".to_string()];
    app.active_skill = Some("optimization".to_string());

    assert!(super::state_ui::handle_info_command(&mut app, "/skills"));
    let content = app.display_messages().last().unwrap().content.clone();

    assert!(content.contains("- /optimization (active)"), "{content}");
    assert!(content.contains("- /firefox-browser\n"), "{content}");
    // Endorsed list should mark remote-installed skills as installed.
    assert!(
        content.contains("/firefox-browser [installed]"),
        "{content}"
    );
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
            resolved_credential: None,
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
fn deferred_stale_server_history_captures_session_id_for_reload_handoff() {
    // Issue #328: when a fresh client connects to a still-running older server
    // (e.g. right after an auto-update), the History payload is deferred because
    // of the version mismatch and the handler returns BEFORE assigning
    // `remote_session_id`. On a fresh client that id is `None`, so the later
    // client reload handoff used to fabricate a `ses_<ts>_<rand>` id that no
    // store can resolve, leaving the user stuck at "No session found matching
    // ...". We must stash the real session id so the re-exec resumes the actual
    // server session instead.
    let _env_guard = crate::storage::lock_test_env();
    crate::env::remove_var("JCODE_ALLOW_SERVER_VERSION_MISMATCH");
    crate::env::set_var("JCODE_TEST_CLIENT_VERSION_OVERRIDE", "v0.21.0 (deadbeef)");

    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_remote = true;
    // Fresh client: no session id learned yet (the #328 reproduction).
    app.remote_session_id = None;
    assert!(app.pending_reload_session_id.is_none());

    let redraw = app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "session_real_server_owned".to_string(),
            messages: vec![crate::protocol::HistoryMessage {
                role: "assistant".to_string(),
                content: "stale answer".to_string(),
                tool_calls: None,
                tool_data: None,
            }],
            images: vec![],
            provider_name: Some("stale-provider".to_string()),
            provider_model: Some("stale-model".to_string()),
            subagent_model: None,
            autoreview_enabled: None,
            autojudge_enabled: None,
            available_models: vec![],
            available_model_routes: vec![],
            mcp_servers: vec![],
            skills: vec![],
            total_tokens: None,
            token_usage_totals: None,
            all_sessions: vec![],
            client_count: None,
            is_canary: None,
            reload_recovery: None,
            // Ancient server that predates self-reported staleness; the client's
            // own release-version comparison drives the deferral.
            server_version: Some("v0.20.4".to_string()),
            server_name: Some("stale-server".to_string()),
            server_icon: Some("🧟".to_string()),
            server_has_update: None,
            was_interrupted: None,
            connection_type: None,
            status_detail: None,
            upstream_provider: None,
            resolved_credential: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: None,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
        },
        &mut remote,
    );

    // History is deferred (no redraw, reload pending, remote_session_id still
    // unset) but the real session id is captured for the reload handoff.
    assert!(!redraw);
    assert!(app.pending_server_reload);
    assert_eq!(app.remote_session_id.as_deref(), None);
    assert_eq!(
        app.pending_reload_session_id.as_deref(),
        Some("session_real_server_owned")
    );

    crate::env::remove_var("JCODE_TEST_CLIENT_VERSION_OVERRIDE");
}

#[test]
fn ancient_server_history_is_deferred_via_client_side_release_check() {
    // Issue #295: a server old enough to predate the self-reported staleness
    // machinery sends `server_has_update: None`, so it can never tell the client
    // it is stale. The client must independently compare release versions and
    // defer + reload anyway, instead of attaching to the ancient daemon (which
    // would then reject newer protocol requests like `set_route`).
    let _env_guard = crate::storage::lock_test_env();
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
            resolved_credential: None,
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
fn older_server_reporting_no_update_is_still_deferred_via_client_check() {
    // The "current client, stale server" report: the daemon self-reports
    // `server_has_update: Some(false)` (its own shared-server channel still
    // points at its old binary, so locally it sees nothing newer), but the
    // client can PROVE it is an older release. Before this fix, Some(false)
    // short-circuited and the client trusted the old server forever. Now the
    // client's release-order check wins: defer + reload (after repairing the
    // shared-server channel client-side).
    let _env_guard = crate::storage::lock_test_env();
    crate::env::remove_var("JCODE_ALLOW_SERVER_VERSION_MISMATCH");
    crate::env::set_var("JCODE_TEST_CLIENT_VERSION_OVERRIDE", "v0.22.0 (abcd1234)");

    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_remote = true;
    app.remote_session_id = Some("session_existing".to_string());

    let redraw = app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "session_from_old_server".to_string(),
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
            all_sessions: vec![],
            client_count: Some(1),
            is_canary: Some(false),
            reload_recovery: None,
            // Older clean release than the client, but the daemon insists it has
            // no newer binary to reload into.
            server_version: Some("v0.14.6 (deadbeef)".to_string()),
            server_name: Some("old-server".to_string()),
            server_icon: Some("🕰".to_string()),
            server_has_update: Some(false),
            was_interrupted: None,
            connection_type: Some("websocket".to_string()),
            status_detail: None,
            upstream_provider: None,
            resolved_credential: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: None,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
        },
        &mut remote,
    );

    crate::env::remove_var("JCODE_TEST_CLIENT_VERSION_OVERRIDE");

    assert!(!redraw);
    assert!(
        app.pending_server_reload,
        "client-proven-older server must defer + reload even when it reports Some(false)"
    );
    assert_eq!(app.remote_server_has_update, Some(false));
    // Remote session state must NOT have been applied from the old server.
    assert_eq!(app.remote_session_id.as_deref(), Some("session_existing"));
    assert_eq!(remote.session_id(), None);
    let content = app.display_messages().last().unwrap().content.clone();
    assert!(
        content.contains("older release") && content.contains("jcode server stop"),
        "{content}"
    );
}

#[test]
fn older_server_history_repairs_stale_shared_server_channel_end_to_end() {
    // Full-path sandbox: a real temp JCODE_HOME set up in the exact field state
    // (shared-server pinned to an OLD build, stable advanced to a NEW release by
    // a previous install). When the current client attaches to a server that
    // self-reports an older release with `server_has_update: Some(false)`, the
    // production History handler must repair the shared-server channel so the
    // forced reload it queues has a strictly-newer binary to exec into.
    use std::time::{Duration, SystemTime};
    let _env_guard = crate::storage::lock_test_env();
    crate::env::remove_var("JCODE_ALLOW_SERVER_VERSION_MISMATCH");
    crate::env::set_var("JCODE_TEST_CLIENT_VERSION_OVERRIDE", "v0.22.0 (abcd1234)");
    let temp = tempfile::TempDir::new().expect("temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    // Build the field state: shared-server -> OLD, stable -> NEW (newer mtime).
    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let write_version = |version: &str, mtime: SystemTime| {
        let dir = crate::build::builds_dir()
            .unwrap()
            .join("versions")
            .join(version);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(crate::build::binary_name());
        std::fs::write(&path, format!("bin {version}")).unwrap();
        std::fs::File::open(&path)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
    };
    let old = "0.14.6";
    let new = "0.22.0";
    write_version(old, base);
    write_version(new, base + Duration::from_secs(60));
    crate::build::update_shared_server_symlink(old).expect("pin shared-server old");
    crate::build::update_stable_symlink(new).expect("stable new");

    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    app.is_remote = true;
    app.remote_session_id = Some("session_existing".to_string());

    let _redraw = app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "session_from_old_server".to_string(),
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
            all_sessions: vec![],
            client_count: Some(1),
            is_canary: Some(false),
            reload_recovery: None,
            server_version: Some("v0.14.6 (deadbeef)".to_string()),
            server_name: Some("old-server".to_string()),
            server_icon: Some("🕰".to_string()),
            server_has_update: Some(false),
            was_interrupted: None,
            connection_type: Some("websocket".to_string()),
            status_detail: None,
            upstream_provider: None,
            resolved_credential: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: None,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
        },
        &mut remote,
    );

    let repaired = crate::build::read_shared_server_version().ok().flatten();
    let pending = app.pending_server_reload;

    // Restore env before asserting so a panic cannot leak global state.
    crate::env::remove_var("JCODE_TEST_CLIENT_VERSION_OVERRIDE");
    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }

    assert!(pending, "older server must queue a reload");
    assert_eq!(
        repaired.as_deref(),
        Some(new),
        "the History handler must repair the stale shared-server channel to the newer stable \
         release so the queued reload upgrades the server instead of re-execing the old binary"
    );
}

#[test]
fn current_release_server_history_is_not_deferred_by_client_check() {
    // A server on the SAME or NEWER clean release as the client, with
    // server_has_update: None, must be trusted and attached normally. This
    // guards against the client-side check over-firing and looping reloads.
    let _env_guard = crate::storage::lock_test_env();
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
            resolved_credential: None,
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
    app.streaming.streaming_input_tokens = 63_762;
    app.streaming.streaming_output_tokens = 153;
    app.streaming.streaming_cache_read_tokens = Some(0);
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
