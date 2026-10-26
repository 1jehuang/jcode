// Stable cache routing must survive transport fallback and provider forks.

#[tokio::test]
async fn service_tier_header_changes_invalidate_persistent_socket() {
    for (before, after) in [
        (None, Some("ultrafast")),
        (Some("ultrafast"), None),
        (Some("ultrafast"), Some("priority")),
    ] {
        let (mut state, server) = test_persistent_ws_state().await;
        let creds = prewarm_test_credentials();
        state.identity =
            openai_websocket_prewarm::prewarm_identity(&creds, Some("gpt-6-astra"), before);
        let persistent = Arc::new(Mutex::new(Some(state)));
        let (tx, _rx) = mpsc::channel(10);
        let request = serde_json::json!({"model":"gpt-6-astra", "service_tier":after});
        let result = try_persistent_ws_continuation(
            &persistent,
            &Arc::new(RwLock::new(creds)),
            &request,
            &[],
            0,
            &tx,
        )
        .await;
        assert!(matches!(result, PersistentWsResult::NotAvailable));
        assert!(persistent.lock().await.is_none());
        server.abort();
    }
}

#[tokio::test]
async fn oauth_routing_header_changes_invalidate_persistent_socket() {
    for (model, before, after) in [
        ("gpt-6.1-sol", None, Some("priority")),
        ("gpt-6.1-sol", Some("priority"), Some("ultrafast")),
        ("other-model", None, None),
    ] {
        let (mut state, server) = test_persistent_ws_state().await;
        let mut creds = prewarm_test_credentials();
        creds.refresh_token = "refresh".into();
        state.identity =
            openai_websocket_prewarm::prewarm_identity(&creds, Some("gpt-6.1-sol"), before);
        let persistent = Arc::new(Mutex::new(Some(state)));
        let (tx, _rx) = mpsc::channel(10);
        let request = serde_json::json!({"model":model, "service_tier":after});
        let result = try_persistent_ws_continuation(
            &persistent,
            &Arc::new(RwLock::new(creds)),
            &request,
            &[],
            0,
            &tx,
        )
        .await;
        assert!(matches!(result, PersistentWsResult::NotAvailable));
        assert!(persistent.lock().await.is_none());
        server.abort();
    }
}

#[test]
fn cache_affinity_is_session_scoped_and_preserved_by_forks() {
    let _lock = jcode_base::storage::lock_test_env();
    let _key = EnvVarGuard::remove("JCODE_OPENAI_PROMPT_CACHE_KEY");
    let first = OpenAIProvider::new(prewarm_test_credentials());
    let second = OpenAIProvider::new(prewarm_test_credentials());
    assert_ne!(first.cache_routing_key(), second.cache_routing_key());
    first.set_session_id("session-persisted-a");
    second.set_session_id("session-persisted-b");
    for chatgpt in [false, true] {
        let request = first.response_request_for_model("gpt-5.6-sol", &[], &[], "system", chatgpt);
        assert_eq!(request["prompt_cache_key"], "session-persisted-a");
        assert_eq!(first.cache_routing_key(), "session-persisted-a");
    }
    let fork = first.fork();
    fork.set_session_id("session-new-child");
    assert_eq!(first.cache_routing_key(), "session-persisted-a");
    assert_eq!(second.cache_routing_key(), "session-persisted-b");
    first.set_session_id("session-persisted-a");
    assert_eq!(first.cache_routing_key(), "session-persisted-a");
}

#[test]
fn cache_affinity_explicit_override_controls_oauth_header_and_body() {
    let _lock = jcode_base::storage::lock_test_env();
    let _key = EnvVarGuard::set("JCODE_OPENAI_PROMPT_CACHE_KEY", " explicit-key ");
    let provider = OpenAIProvider::new(prewarm_test_credentials());
    provider.set_session_id("session-real-id");
    let request = provider.response_request_for_model("gpt-5.6-sol", &[], &[], "system", true);
    assert_eq!(request["prompt_cache_key"], "explicit-key");
    let mut credentials = prewarm_test_credentials();
    credentials.refresh_token = "fixture-refresh".into();
    let handshake = openai_websocket_prewarm::websocket_request(
        &credentials,
        &credentials.access_token,
        request["prompt_cache_key"].as_str(),
        request.get("model").and_then(Value::as_str),
        request.get("service_tier").and_then(Value::as_str),
    )
    .unwrap();
    assert_eq!(handshake.headers()["session-id"], "explicit-key");
    assert!(request.get("prompt_cache_retention").is_none());
}

#[tokio::test]
async fn cache_affinity_changed_session_cannot_reuse_old_handshake() {
    let (mut state, server) = test_persistent_ws_state().await;
    state.cache_key = Some("old-session".into());
    let persistent = Arc::new(Mutex::new(Some(state)));
    let input = vec![
        serde_json::json!({"role":"user","content":"previous"}),
        serde_json::json!({"role":"user","content":"next"}),
    ];
    let (tx, _rx) = mpsc::channel(10);
    let result = try_persistent_ws_continuation(
        &persistent,
        &Arc::new(RwLock::new(prewarm_test_credentials())),
        &serde_json::json!({"model":"gpt-5.6-sol", "prompt_cache_key":"new-session"}),
        &input,
        input.len(),
        &tx,
    )
    .await;
    assert!(matches!(result, PersistentWsResult::NotAvailable));
    assert!(persistent.lock().await.is_none());
    server.abort();
}
