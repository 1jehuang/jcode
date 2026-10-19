// These fixtures set counters directly for readability, then feed them through
// the same report API that marks a provider request's usage as fresh.
fn record_reported_test_cache_usage(app: &mut App) -> bool {
    app.apply_stream_usage_input_report(
        Some(app.streaming.streaming_input_tokens),
        app.streaming.streaming_cache_read_tokens,
        app.streaming.streaming_cache_creation_tokens,
    );
    app.record_completed_stream_cache_usage()
}

#[test]
fn cache_accounting_cancelled_request_without_usage_keeps_previous_cache_state() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    for remote_mode in [false, true] {
        let mut app = create_test_app();
        app.is_remote = remote_mode;
        if remote_mode {
            app.runtime_mode = AppRuntimeMode::RemoteClient;
            app.remote_provider_name = Some("Claude".to_string());
            app.remote_provider_model = Some("claude-opus-5-5".to_string());
        }
        let first = vec![Message::user("first")];
        app.begin_kv_cache_request(&first, &[], "system", "");
        app.apply_stream_usage_input_report(Some(314), Some(587_580), Some(1_055));
        app.push_turn_footer(Some(1.0));
        let completed_at = app.kv_cache.kv_cache_baseline.as_ref().unwrap().completed_at;
        let last_api = app.last_api_completed;
        let last_prompt = app.last_turn_input_tokens;
        let total_prompt = app.token_accounting.total_cache_prompt_tokens;
        let next_expected = app.token_accounting.cache_next_optimal_input_tokens;

        let mut next = first;
        next.push(Message::user("cancelled before provider stream opened"));
        if remote_mode {
            app.begin_remote_kv_cache_request(App::kv_cache_request_signature(&next, &[], "system", ""));
        } else {
            app.begin_kv_cache_request(&next, &[], "system", "");
        }
        assert!(app.streaming.streaming_usage_call_reset_pending);
        assert!(!app.record_completed_stream_cache_usage());
        assert!(!app.has_streaming_footer_stats());
        assert_eq!(app.build_turn_footer(None), None);
        assert_eq!(app.build_turn_footer(Some(2.0)), Some(Message::format_duration(2_000)));
        if remote_mode {
            app.current_message_id = Some(42);
            app.is_processing = true;
            app.streaming.streaming_text = "partial response without a usage report".to_string();
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            app.handle_server_event(crate::protocol::ServerEvent::Done { id: 42 }, &mut remote);
        } else {
            app.push_turn_footer(Some(2.0));
        }
        assert!(app.kv_cache.kv_cache_miss_samples.is_empty());
        assert_eq!(app.kv_cache.kv_cache_baseline.as_ref().unwrap().completed_at, completed_at);
        assert_eq!(app.token_accounting.cache_next_optimal_input_tokens, next_expected);
        assert_eq!(app.token_accounting.total_cache_prompt_tokens, total_prompt);
        assert_eq!(app.last_api_completed, last_api);
        assert_eq!(app.last_turn_input_tokens, last_prompt);
        assert!(!app.kv_cache.current_api_usage_recorded);
    }
}

#[test]
fn cache_accounting_claude_reuses_written_boundary_not_uncached_tail() {
    let mut app = create_test_app();
    let first = vec![Message::user("first")];
    app.begin_kv_cache_request(&first, &[], "system", "");
    app.streaming.streaming_input_tokens = 2_482;
    app.streaming.streaming_cache_read_tokens = Some(615_629);
    app.streaming.streaming_cache_creation_tokens = Some(1_375);
    assert!(record_reported_test_cache_usage(&mut app));
    assert_eq!(app.token_accounting.last_cache_prompt_tokens, Some(619_486));
    assert_eq!(app.token_accounting.cache_next_optimal_input_tokens, Some(617_004));
    assert_eq!(app.kv_cache.kv_cache_baseline.as_ref().unwrap().input_tokens, 617_004);

    let mut second = first;
    second.push(Message::user("next"));
    app.begin_kv_cache_request(&second, &[], "system", "");
    app.kv_cache.current_api_usage_recorded = false;
    app.streaming.streaming_input_tokens = 283;
    app.streaming.streaming_cache_read_tokens = Some(617_004);
    app.streaming.streaming_cache_creation_tokens = Some(3_248);
    assert!(record_reported_test_cache_usage(&mut app));
    assert!(app.kv_cache.kv_cache_miss_samples.is_empty());
    assert_eq!(app.token_accounting.last_cache_optimal_input_tokens, Some(617_004));

    second.push(Message::user("third"));
    app.begin_kv_cache_request(&second, &[], "system", "");
    app.kv_cache.current_api_usage_recorded = false;
    app.streaming.streaming_cache_read_tokens = Some(620_252 - 1_782);
    app.streaming.streaming_cache_creation_tokens = Some(2_000);
    assert!(record_reported_test_cache_usage(&mut app));
    assert_eq!(app.kv_cache.kv_cache_miss_samples.len(), 1);
    assert_eq!(app.kv_cache.kv_cache_miss_samples[0].missed_tokens, 1_782);
    assert_eq!(app.kv_cache.kv_cache_miss_samples[0].reason, KvCacheMissReason::LowRead);
}

#[test]
fn cache_accounting_remote_late_claude_usage_keeps_reusable_boundary() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    let mut app = create_test_app();
    app.is_remote = true;
    app.runtime_mode = AppRuntimeMode::RemoteClient;
    app.remote_provider_name = Some("Claude".to_string());
    app.remote_provider_model = Some("claude-opus-5-5".to_string());
    for write in [1_000, 1_375] {
        app.handle_server_event(crate::protocol::ServerEvent::TokenUsage {
            input: 2_482, output: 100,
            cache_read_input: Some(615_629), cache_creation_input: Some(write),
        }, &mut remote);
    }
    assert_eq!(app.kv_cache.kv_cache_baseline.as_ref().unwrap().input_tokens, 617_004);
    assert_eq!(app.token_accounting.cache_next_optimal_input_tokens, Some(617_004));
    assert_eq!(app.last_turn_input_tokens, Some(619_486));
    // A late correction after compaction must not resurrect an old expectation.
    app.handle_compaction_event(crate::compaction::CompactionEvent {
        trigger: "semantic".to_string(),
        pre_tokens: Some(619_486),
        post_tokens: Some(20_000),
        tokens_saved: None,
        duration_ms: None,
        messages_dropped: None,
        messages_compacted: Some(10),
        summary_chars: Some(100),
        active_messages: Some(3),
    });
    app.handle_server_event(crate::protocol::ServerEvent::TokenUsage {
        input: 2_482, output: 101,
        cache_read_input: Some(615_629), cache_creation_input: Some(1_375),
    }, &mut remote);
    assert!(app.kv_cache.kv_cache_baseline.is_none());
    assert!(app.token_accounting.cache_next_optimal_input_tokens.is_none());
}

fn cache_accounting_openai_app() -> App {
    let mut app = create_test_app();
    app.is_remote = true;
    app.runtime_mode = AppRuntimeMode::RemoteClient;
    app.remote_provider_name = Some("OpenAI".to_string());
    app.remote_provider_model = Some("gpt-5.6".to_string());
    app.remote_resolved_credential = Some(jcode_provider_core::ResolvedCredential::ApiKey);
    app
}

fn cache_accounting_stats(app: &mut App) -> String {
    assert!(super::state_ui::handle_info_command(app, "/cache stats"));
    app.display_messages().last().unwrap().content.clone()
}

#[test]
fn cache_accounting_openai_writes_are_subsets_live_and_completed() {
    let mut app = cache_accounting_openai_app();
    app.streaming.streaming_input_tokens = 10_000;
    app.streaming.streaming_cache_read_tokens = Some(6_000);
    app.streaming.streaming_cache_creation_tokens = Some(2_000);
    let live = cache_accounting_stats(&mut app);
    assert!(
        live.contains("cache_read_pct_of_effective_prompt_including_unrecorded_live: 60%"),
        "{live}"
    );
    assert!(
        live.contains("cache_write_pct_of_effective_prompt_including_unrecorded_live: 20%"),
        "{live}"
    );
    assert!(
        live.contains("effective_prompt_tokens_including_unrecorded_live: 10k (10,000)"),
        "{live}"
    );
    assert!(record_reported_test_cache_usage(&mut app));
    assert_eq!(app.token_accounting.total_cache_prompt_tokens, 10_000);
    assert_eq!(
        app.kv_cache
            .kv_cache_baseline
            .as_ref()
            .unwrap()
            .input_tokens,
        10_000
    );
    let stats = cache_accounting_stats(&mut app);
    assert!(
        stats.contains("cache_read_pct_of_effective_prompt: 60%"),
        "{stats}"
    );
    assert!(
        stats.contains("cache_write_pct_of_effective_prompt: 20%"),
        "{stats}"
    );
    assert!(
        stats.contains(
            "effective_prompt_tokens (input+read+creation for split providers): 10k (10,000)"
        ),
        "{stats}"
    );
    assert!(
        stats.contains("active_route_cache_retention: 30 minutes, provider estimate/minimum"),
        "{stats}"
    );
    let info = app.info_widget_data().cache_hit_info.unwrap();
    assert_eq!(info.prompt_tokens, Some(10_000));
    assert!((info.hit_ratio().unwrap() - 0.6).abs() < 0.0001);
    assert!((info.last_ratio().unwrap() - 0.6).abs() < 0.0001);
}

#[test]
fn cache_accounting_remote_snapshots_count_openai_prompt_once() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    let mut app = cache_accounting_openai_app();
    for (input, read, write) in [
        (8_000, 4_000, None),
        (10_000, 6_000, Some(2_000)),
        (10_000, 6_000, Some(2_000)),
    ] {
        app.handle_server_event(
            crate::protocol::ServerEvent::TokenUsage {
                input,
                output: 100,
                cache_read_input: Some(read),
                cache_creation_input: write,
            },
            &mut remote,
        );
    }
    assert_eq!(app.token_accounting.total_cache_prompt_tokens, 10_000);
    assert_eq!(
        app.token_accounting.total_cache_reported_input_tokens,
        10_000
    );
    assert_eq!(app.token_accounting.total_cache_read_tokens, 6_000);
    assert_eq!(app.token_accounting.total_cache_creation_tokens, 2_000);
    assert_eq!(app.last_turn_input_tokens, Some(10_000));
    let stats = cache_accounting_stats(&mut app);
    assert!(
        stats.contains("cache_read_pct_of_effective_prompt: 60%"),
        "{stats}"
    );
    assert!(
        stats.contains("cache_write_pct_of_effective_prompt: 20%"),
        "{stats}"
    );
}

#[test]
fn cache_accounting_mixed_history_and_live_sum_resolved_prompts() {
    let mut app = cache_accounting_openai_app();
    app.token_accounting.cache_next_optimal_input_tokens = Some(10_000);
    // Historic Anthropic request: input 1k + read 7k + write 2k = 10k.
    app.remote_token_usage_totals = Some(crate::protocol::TokenUsageTotals {
        cache_prompt_tokens: Some(10_000),
        input_tokens: 1_000,
        cache_reported_input_tokens: 1_000,
        cache_read_input_tokens: 7_000,
        cache_creation_input_tokens: 2_000,
        ..Default::default()
    });
    app.streaming.streaming_input_tokens = 10_000;
    app.streaming.streaming_cache_read_tokens = Some(6_000);
    app.streaming.streaming_cache_creation_tokens = Some(2_000);
    assert!(record_reported_test_cache_usage(&mut app));
    let stats = cache_accounting_stats(&mut app);
    assert!(
        stats.contains(
            "effective_prompt_tokens (input+read+creation for split providers): 20k (20,000)"
        ),
        "{stats}"
    );
    assert!(
        stats.contains("cache_read_pct_of_effective_prompt: 65%"),
        "{stats}"
    );
    assert!(
        stats.contains("cache_write_pct_of_effective_prompt: 20%"),
        "{stats}"
    );
    assert_eq!(
        app.info_widget_data().cache_hit_info.unwrap().prompt_tokens,
        Some(20_000)
    );
    // Yield is live-scope only: 6k live reads / 10k live optimal.
    let optimal = app
        .info_widget_data()
        .cache_hit_info
        .unwrap()
        .optimal_ratio()
        .unwrap();
    assert!((optimal - 0.6).abs() < 0.0001, "{optimal}");
    assert!(
        stats.contains("cache_read_pct_of_optimal_input: 60%"),
        "{stats}"
    );
}

#[test]
fn cache_accounting_legacy_history_does_not_guess_from_writes_or_current_provider() {
    let mut app = cache_accounting_openai_app();
    app.remote_token_usage_totals = Some(crate::protocol::TokenUsageTotals {
        input_tokens: 10_000,
        cache_reported_input_tokens: 10_000,
        cache_read_input_tokens: 6_000,
        cache_creation_input_tokens: 2_000,
        ..Default::default()
    });
    let stats = cache_accounting_stats(&mut app);
    assert!(
        stats.contains("cache_read_pct_of_effective_prompt: unknown"),
        "{stats}"
    );
    assert!(
        stats.contains("total_cache_read_tokens: 6k (6,000)"),
        "{stats}"
    );
    assert!(
        app.info_widget_data()
            .cache_hit_info
            .unwrap()
            .hit_ratio()
            .is_none()
    );
    app.remote_resolved_credential = Some(jcode_provider_core::ResolvedCredential::Oauth);
    let stats = cache_accounting_stats(&mut app);
    assert!(
        stats.contains("active_route_cache_retention: unknown, provider-managed retention"),
        "{stats}"
    );
}

#[test]
fn cache_accounting_fully_cached_anthropic_has_nonzero_denominator() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.runtime_mode = AppRuntimeMode::RemoteClient;
    app.remote_provider_name = Some("Anthropic".to_string());
    app.streaming.streaming_input_tokens = 0;
    app.streaming.streaming_cache_read_tokens = Some(10_000);
    app.streaming.streaming_cache_creation_tokens = Some(0);
    assert!(record_reported_test_cache_usage(&mut app));
    assert_eq!(app.token_accounting.total_cache_prompt_tokens, 10_000);
    let stats = cache_accounting_stats(&mut app);
    assert!(
        stats.contains("cache_read_pct_of_effective_prompt: 100%"),
        "{stats}"
    );
}

#[test]
fn cache_accounting_bare_command_reports_without_changing_preference() {
    let mut app = cache_accounting_openai_app();
    let before = crate::config::config().provider.anthropic_cache_ttl_1h;
    assert!(super::state_ui::handle_info_command(&mut app, "/cache"));
    assert_eq!(
        app.display_messages().last().unwrap().title.as_deref(),
        Some("KV cache stats")
    );
    assert_eq!(
        crate::config::config().provider.anthropic_cache_ttl_1h,
        before
    );
}

#[test]
fn cache_accounting_missing_last_read_remains_unknown() {
    let mut app = cache_accounting_openai_app();
    app.streaming.streaming_input_tokens = 10_000;
    app.streaming.streaming_cache_creation_tokens = Some(2_000);
    assert!(record_reported_test_cache_usage(&mut app));
    assert_eq!(app.token_accounting.last_cache_read_tokens, None);
    assert_eq!(app.token_accounting.last_cache_creation_tokens, Some(2_000));
    assert!(
        app.info_widget_data()
            .cache_hit_info
            .unwrap()
            .last_ratio()
            .is_none()
    );
}

#[test]
fn cache_report_exposes_actual_expiry_notification_policy() {
    let mut app = cache_accounting_openai_app();
    app.begin_kv_cache_request(&[Message::user("test")], &[], "system", "");
    app.streaming.streaming_input_tokens = 10_000;
    app.streaming.streaming_cache_read_tokens = Some(6_000);
    assert!(record_reported_test_cache_usage(&mut app));
    for age in [0, 1770, 1900] {
        app.kv_cache
            .kv_cache_baseline
            .as_mut()
            .unwrap()
            .completed_at = Instant::now() - Duration::from_secs(age);
        let stats = cache_accounting_stats(&mut app);
        assert!(stats.contains("cache_expiry_notification_policy: disabled (retention is estimated or provider-managed)"), "{stats}");
        assert!(
            stats.contains("cache_expiry_notification_active: false"),
            "{stats}"
        );
    }
    app.remote_provider_name = Some("anthropic".into());
    app.remote_provider_model = Some("claude-opus-4-6".into());
    let baseline = app.kv_cache.kv_cache_baseline.as_mut().unwrap();
    baseline.provider = "anthropic".into();
    baseline.model = "claude-opus-4-6".into();
    baseline.cache_ttl_secs = Some(300);
    baseline.completed_at = Instant::now() - Duration::from_secs(310);
    let stats = cache_accounting_stats(&mut app);
    assert!(
        stats.contains("cache_expiry_notification_policy: explicit TTL only"),
        "{stats}"
    );
    assert!(
        stats.contains("cache_expiry_notification_active: true"),
        "{stats}"
    );
}

fn record_live_cache_request(app: &mut App, input: u64, read: u64) {
    app.kv_cache.current_api_usage_recorded = false;
    app.streaming.streaming_input_tokens = input;
    app.streaming.streaming_cache_read_tokens = Some(read);
    app.streaming.streaming_cache_creation_tokens = Some(0);
    assert!(app.record_completed_stream_cache_usage());
}

fn kv_cache_summary(app: &App) -> String {
    let info = app.info_widget_data().cache_hit_info.unwrap();
    crate::tui::info_widget::render_kv_cache_summary_line(&info)
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn cache_yield_after_resume_uses_live_scope_not_priming() {
    let mut app = cache_accounting_openai_app();
    // Session resumed/reloaded with prior cache reads in history.
    app.remote_token_usage_totals = Some(crate::protocol::TokenUsageTotals {
        cache_prompt_tokens: Some(10_000),
        input_tokens: 10_000,
        cache_reported_input_tokens: 10_000,
        cache_read_input_tokens: 7_000,
        cache_creation_input_tokens: 0,
        ..Default::default()
    });
    record_live_cache_request(&mut app, 10_000, 0);
    // No baseline yet: the plain hit rate, not a yield.
    assert!(!kv_cache_summary(&app).contains("yield"));
    record_live_cache_request(&mut app, 10_000, 9_000);
    let summary = kv_cache_summary(&app);
    assert!(summary.contains("yield 90%"), "{summary}");
    assert!(!summary.contains("priming"), "{summary}");
    let info = app.info_widget_data().cache_hit_info.unwrap();
    // Yield is live-scope only: 9k live reads / 10k live optimal.
    assert!((info.optimal_ratio().unwrap() - 0.9).abs() < 0.0001);
    // Session hit ratio still includes history: (7k + 9k) / 30k.
    assert!((info.hit_ratio().unwrap() - 16.0 / 30.0).abs() < 0.0001);
}

#[test]
fn cache_yield_new_session_single_request_has_no_yield_baseline() {
    let mut app = cache_accounting_openai_app();
    record_live_cache_request(&mut app, 10_000, 0);
    assert!(!kv_cache_summary(&app).contains("yield"));
    record_live_cache_request(&mut app, 10_000, 8_000);
    assert!(kv_cache_summary(&app).contains("yield 80%"));
}

#[test]
fn cache_yield_after_resume_ignores_first_warm_read_without_optimal() {
    let mut app = cache_accounting_openai_app();
    app.remote_token_usage_totals = Some(crate::protocol::TokenUsageTotals {
        cache_prompt_tokens: Some(10_000),
        input_tokens: 10_000,
        cache_reported_input_tokens: 10_000,
        cache_read_input_tokens: 7_000,
        ..Default::default()
    });
    record_live_cache_request(&mut app, 10_000, 9_000);
    record_live_cache_request(&mut app, 10_000, 5_000);
    let summary = kv_cache_summary(&app);
    assert!(summary.contains("yield 50%"), "{summary}");
    let stats = cache_accounting_stats(&mut app);
    assert!(
        stats.contains("cache_read_pct_of_optimal_input: 50%"),
        "{stats}"
    );
}

#[test]
fn cache_yield_fresh_session_ignores_warm_prefix_first_read() {
    let mut app = cache_accounting_openai_app();
    record_live_cache_request(&mut app, 10_000, 4_000);
    record_live_cache_request(&mut app, 10_000, 8_000);
    let summary = kv_cache_summary(&app);
    assert!(summary.contains("yield 80%"), "{summary}");
}

fn cache_usage_event(
    app: &mut App,
    remote: &mut crate::tui::backend::RemoteConnection,
    input: u64,
    read: u64,
) {
    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input,
            output: 100,
            cache_read_input: Some(read),
            cache_creation_input: Some(0),
        },
        remote,
    );
}

#[test]
fn cache_yield_follows_later_usage_snapshots_for_same_request() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    let mut app = cache_accounting_openai_app();
    // First request primes a 10k baseline.
    cache_usage_event(&mut app, &mut remote, 10_000, 0);
    // Second request reports 4k reads, then 6k for the same request.
    app.kv_cache.current_api_usage_recorded = false;
    app.streaming.streaming_cache_read_tokens = None;
    app.streaming.streaming_cache_creation_tokens = None;
    cache_usage_event(&mut app, &mut remote, 10_000, 4_000);
    cache_usage_event(&mut app, &mut remote, 10_000, 6_000);
    assert_eq!(app.token_accounting.total_cache_optimal_read_tokens, 6_000);
    let summary = kv_cache_summary(&app);
    assert!(summary.contains("yield 60%"), "{summary}");
    let stats = cache_accounting_stats(&mut app);
    assert!(
        stats.contains("cache_read_pct_of_optimal_input: 60%"),
        "{stats}"
    );
}

#[test]
fn history_refresh_same_session_clears_optimal_baseline() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    let mut app = cache_accounting_openai_app();
    let history = || crate::protocol::ServerEvent::History {
        messages_base_count: None,
        images_unchanged: false,
        provider_oauth_account_label: None,
        available_service_tiers: None,
        id: 1,
        session_id: "session_same".to_string(),
        messages: vec![],
        images: vec![],
        provider_name: Some("OpenAI".to_string()),
        provider_model: Some("gpt-5.6".to_string()),
        provider_oauth_email: None,
        subagent_model: None,
        agent_model_overrides: Default::default(),
        autoreview_enabled: None,
        autojudge_enabled: None,
        available_models: vec![],
        available_model_routes: vec![],
        mcp_servers: vec![],
        skills: vec![],
        total_tokens: None,
        token_usage_totals: Some(crate::protocol::TokenUsageTotals {
            // Real servers always send a prompt total (Session::token_usage_totals
            // starts at Some(0)); without it the KV line is not rendered at all.
            cache_prompt_tokens: Some(10_000),
            input_tokens: 10_000,
            cache_reported_input_tokens: 10_000,
            ..Default::default()
        }),
        all_sessions: vec![],
        client_count: None,
        is_canary: None,
        reload_recovery: None,
        server_version: None,
        server_name: None,
        server_icon: None,
        server_has_update: None,
        was_interrupted: None,
        connection_type: None,
        status_detail: None,
        upstream_provider: None,
        resolved_credential: None,
        reasoning_effort: None,
        service_tier: None,
        account_labels: Vec::new(),
        compaction_mode: crate::config::CompactionMode::Reactive,
        activity: None,
        side_panel: crate::side_panel::SidePanelSnapshot::default(),
        applets: Default::default(),
    };
    app.handle_server_event(history(), &mut remote);
    record_live_cache_request(&mut app, 10_000, 0);
    assert!(
        app.token_accounting
            .cache_next_optimal_input_tokens
            .is_some()
    );
    // Reconnect: History for the same session refreshes usage.
    app.handle_server_event(history(), &mut remote);
    assert_eq!(app.token_accounting.cache_next_optimal_input_tokens, None);
    // First request after refresh has no baseline: no yield yet.
    record_live_cache_request(&mut app, 10_000, 8_000);
    assert_eq!(app.token_accounting.total_cache_optimal_input_tokens, 0);
    let summary = kv_cache_summary(&app);
    assert!(!summary.contains("yield"), "{summary}");
    // Second request starts a clean measurement.
    record_live_cache_request(&mut app, 10_000, 5_000);
    let summary = kv_cache_summary(&app);
    assert!(summary.contains("yield 50%"), "{summary}");
}
