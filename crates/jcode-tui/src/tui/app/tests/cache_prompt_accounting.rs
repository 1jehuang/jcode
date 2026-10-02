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
    assert!(app.record_completed_stream_cache_usage());
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
    assert!(app.record_completed_stream_cache_usage());
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
    assert!(
        app.info_widget_data()
            .cache_hit_info
            .unwrap()
            .optimal_ratio()
            .is_none()
    );
    assert!(
        stats.contains("cache_read_pct_of_optimal_input: None"),
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
    assert!(app.record_completed_stream_cache_usage());
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
    assert!(app.record_completed_stream_cache_usage());
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
    assert!(app.record_completed_stream_cache_usage());
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
