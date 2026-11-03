#[test]
fn cache_timer_snapshots_retention_at_request_start() {
    let _guard = crate::storage::lock_test_env();
    struct RestoreTtl(bool);
    impl Drop for RestoreTtl {
        fn drop(&mut self) {
            crate::provider::anthropic::set_cache_ttl_1h(self.0);
        }
    }
    let _restore = RestoreTtl(crate::provider::anthropic::is_cache_ttl_1h());
    let mut app = create_test_app();
    app.is_remote = true;
    app.runtime_mode = AppRuntimeMode::RemoteClient;
    app.remote_session_id = Some("cache_snapshot".to_string());
    app.remote_provider_name = Some("anthropic".to_string());
    app.remote_provider_model = Some("claude-opus-4-6".to_string());
    crate::provider::anthropic::set_cache_ttl_1h(true);
    app.begin_kv_cache_request(&[Message::user("first")], &[], "system", "");
    let requested_ttl = app
        .kv_cache
        .pending_kv_cache_request
        .as_ref()
        .unwrap()
        .cache_ttl_secs;
    assert_eq!(requested_ttl, Some(3600));
    // Model a preference change in flight. It cannot extend or shorten the
    // request that already selected its cache-control policy.
    crate::provider::anthropic::set_cache_ttl_1h(false);
    app.streaming.streaming_input_tokens = 42_000;
    assert!(record_reported_test_cache_usage(&mut app));
    let timer = <App as TuiState>::cache_ttl_status(&app).unwrap();
    assert_eq!(timer.ttl_secs, 3600);
    assert!(!timer.is_estimate);
    assert!(timer.remaining_secs > 3500);
    crate::provider::anthropic::set_cache_ttl_1h(true);
    assert_eq!(
        <App as TuiState>::cache_ttl_status(&app).unwrap().ttl_secs,
        3600
    );
    app.kv_cache
        .kv_cache_baseline
        .as_mut()
        .unwrap()
        .cache_ttl_secs = Some(300);
    assert_eq!(
        <App as TuiState>::cache_ttl_status(&app).unwrap().ttl_secs,
        300
    );
}

#[test]
fn cache_timer_openai_routes_distinguish_api_oauth_and_unknown() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.runtime_mode = AppRuntimeMode::RemoteClient;
    app.remote_session_id = Some("cache_routes".to_string());
    app.remote_provider_name = Some("OpenAI".to_string());
    app.remote_provider_model = Some("gpt-5.6".to_string());
    for (credential, route, identity, has_timer) in [
        (None, None, "openai", false),
        (
            Some(jcode_provider_core::ResolvedCredential::Oauth),
            None,
            "openai-oauth",
            false,
        ),
        (
            Some(jcode_provider_core::ResolvedCredential::ApiKey),
            None,
            "openai-api",
            true,
        ),
        (None, Some("openai-api-key"), "openai-api", true),
        (None, Some("openai-oauth"), "openai-oauth", false),
    ] {
        app.remote_resolved_credential = credential;
        app.session.route_api_method = route.map(str::to_string);
        assert_eq!(app.kv_cache_provider_name(), identity);
        app.begin_kv_cache_request(&[Message::user("first")], &[], "system", "");
        app.streaming.streaming_input_tokens = 42_000;
        assert!(record_reported_test_cache_usage(&mut app));
        let timer = <App as TuiState>::cache_ttl_status(&app);
        assert_eq!(timer.is_some(), has_timer, "{identity}");
        if let Some(timer) = timer {
            assert!(timer.is_estimate);
        }
    }
    // A route change must not inherit another credential's warm timer.
    app.remote_provider_name = Some("openai-api".to_string());
    app.begin_kv_cache_request(&[Message::user("first")], &[], "system", "");
    app.streaming.streaming_input_tokens = 42_000;
    assert!(record_reported_test_cache_usage(&mut app));
    assert!(
        <App as TuiState>::cache_ttl_status(&app)
            .unwrap()
            .is_estimate
    );
    app.remote_provider_name = Some("openai-oauth".to_string());
    assert!(<App as TuiState>::cache_ttl_status(&app).is_none());
}

#[test]
fn cache_timer_estimate_never_claims_definite_expiry() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.runtime_mode = AppRuntimeMode::RemoteClient;
    app.remote_session_id = Some("cache_estimate".to_string());
    app.remote_provider_name = Some("openai-api".to_string());
    app.remote_provider_model = Some("gpt-5.6".to_string());
    app.display_messages.push(DisplayMessage::user("first"));
    app.begin_kv_cache_request(&[Message::user("first")], &[], "system", "");
    app.streaming.streaming_input_tokens = 42_000;
    app.streaming.streaming_cache_read_tokens = Some(0);
    assert!(record_reported_test_cache_usage(&mut app));
    let baseline = app.kv_cache.kv_cache_baseline.as_mut().unwrap();
    baseline.completed_at =
        Instant::now() - Duration::from_secs(baseline.cache_ttl_secs.unwrap() + 10);
    let baseline = baseline.clone();
    let before = app.display_messages.len();
    assert!(!app.maybe_push_idle_cold_cache_warning());
    app.maybe_push_cold_cache_warning(2, 1, Some(&baseline));
    assert_eq!(
        app.display_messages.len(),
        before,
        "elapsed estimates must not create unsolicited eviction warnings"
    );
    let request = app.fallback_pending_kv_cache_request();
    assert_ne!(
        app.classify_kv_cache_miss_reason(&request, &baseline, 0, 0),
        KvCacheMissReason::Expired
    );
    assert!(
        <App as TuiState>::cache_ttl_status(&app)
            .unwrap()
            .is_estimate
    );
}

#[test]
fn cache_timer_local_explicit_openai_route_does_not_probe_auto_credentials() {
    #[derive(Clone)]
    struct PinnedOpenAI(Option<jcode_provider_core::ResolvedCredential>);
    #[async_trait::async_trait]
    impl Provider for PinnedOpenAI {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[crate::message::ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> Result<crate::provider::EventStream> {
            unimplemented!("route-only fixture")
        }
        fn name(&self) -> &str {
            "OpenAI"
        }
        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(self.clone())
        }
        fn active_explicit_credential(&self) -> Option<jcode_provider_core::ResolvedCredential> {
            self.0
        }
        fn active_resolved_credential(&self) -> Option<jcode_provider_core::ResolvedCredential> {
            panic!("cache timer must not read auto credentials from disk on every frame")
        }
    }
    let mut app = create_test_app();
    for (credential, expected) in [
        (
            Some(jcode_provider_core::ResolvedCredential::ApiKey),
            "openai-api",
        ),
        (
            Some(jcode_provider_core::ResolvedCredential::Oauth),
            "openai-oauth",
        ),
        (None, "openai"),
    ] {
        app.provider = Arc::new(PinnedOpenAI(credential));
        assert_eq!(app.kv_cache_provider_name(), expected);
    }
}

#[test]
fn cache_warning_does_not_leak_anthropic_expiry_into_openai_route() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.runtime_mode = AppRuntimeMode::RemoteClient;
    app.remote_provider_name = Some("openai-api".into());
    app.remote_provider_model = Some("gpt-6-astra".into());
    app.display_messages.push(DisplayMessage::user("first"));
    let baseline = KvCacheBaseline {
        session_id: app.kv_cache_session_id(),
        cache_generation: app.kv_cache.cache_generation,
        input_tokens: 42_000,
        completed_at: Instant::now() - Duration::from_secs(3700),
        utc_day: 0,
        cache_ttl_secs: Some(3600),
        provider: "anthropic".into(),
        model: "claude-opus-4-6".into(),
        upstream_provider: None,
        account: None,
        signature: None,
    };
    app.kv_cache.kv_cache_baseline = Some(baseline.clone());
    let before = app.display_messages.len();
    assert!(!app.maybe_push_idle_cold_cache_warning());
    app.maybe_push_cold_cache_warning(2, 1, Some(&baseline));
    assert_eq!(app.display_messages.len(), before);
    assert!(<App as TuiState>::cache_ttl_status(&app).is_none());
}

#[test]
fn cache_miss_requires_explicit_read_telemetry_even_with_writes() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.runtime_mode = AppRuntimeMode::RemoteClient;
    app.remote_provider_name = Some("openai-api".into());
    app.remote_provider_model = Some("gpt-6-astra".into());
    let messages = [Message::user("first")];
    let baseline = KvCacheBaseline {
        session_id: app.kv_cache_session_id(),
        cache_generation: app.kv_cache.cache_generation,
        input_tokens: 42_000,
        completed_at: Instant::now(),
        utc_day: 0,
        cache_ttl_secs: Some(1800),
        provider: "openai-api".into(),
        model: "gpt-6-astra".into(),
        upstream_provider: None,
        account: None,
        signature: Some(App::kv_cache_request_signature(
            &messages,
            &[],
            "before",
            "",
        )),
    };
    for writes in [None, Some(2_000)] {
        app.kv_cache.kv_cache_baseline = Some(baseline.clone());
        app.begin_kv_cache_request(&messages, &[], "changed system", "");
        app.streaming.streaming_input_tokens = 42_000;
        app.streaming.streaming_cache_read_tokens = None;
        app.streaming.streaming_cache_creation_tokens = writes;
        assert!(record_reported_test_cache_usage(&mut app));
        assert!(app.kv_cache.kv_cache_miss_samples.is_empty());
        assert!(
            !app.display_messages
                .iter()
                .any(|m| m.content.contains("KV cache miss"))
        );
    }
    // An explicitly reported zero remains meaningful and is not suppressed.
    app.kv_cache.kv_cache_baseline = Some(baseline);
    app.begin_kv_cache_request(&messages, &[], "changed system", "");
    app.streaming.streaming_input_tokens = 42_000;
    app.streaming.streaming_cache_read_tokens = Some(0);
    assert!(record_reported_test_cache_usage(&mut app));
    assert_eq!(app.kv_cache.kv_cache_miss_samples.len(), 1);
}
