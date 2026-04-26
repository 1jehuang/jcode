#[test]
fn test_openai_supports_codex_models() {
    let _guard = crate::storage::lock_test_env();
    crate::auth::codex::set_active_account_override(Some(
        "openai-supports-codex-models".to_string(),
    ));
    crate::provider::populate_account_models(vec![
        "gpt-5.1-codex".to_string(),
        "gpt-5.1-codex-mini".to_string(),
        "gpt-5.2-codex".to_string(),
    ]);

    let creds = CodexCredentials {
        access_token: "test".to_string(),
        refresh_token: String::new(),
        id_token: None,
        account_id: None,
        expires_at: None,
    };

    let provider = OpenAIProvider::new(creds);
    assert!(provider.available_models().contains(&"gpt-5.2-codex"));
    assert!(provider.available_models().contains(&"gpt-5.1-codex-mini"));

    provider.set_model("gpt-5.1-codex").unwrap();
    assert_eq!(provider.model(), "gpt-5.1-codex");

    provider.set_model("gpt-5.1-codex-mini").unwrap();
    assert_eq!(provider.model(), "gpt-5.1-codex-mini");

    crate::auth::codex::set_active_account_override(None);
}

#[test]
fn test_openai_switching_models_include_dynamic_catalog_entries() {
    let _guard = crate::storage::lock_test_env();
    let dynamic_model = "gpt-5.9-switching-test";
    crate::auth::codex::set_active_account_override(Some("switching-test".to_string()));
    crate::provider::populate_account_models(vec![
        "gpt-5.4".to_string(),
        dynamic_model.to_string(),
    ]);

    let provider = OpenAIProvider::new(CodexCredentials {
        access_token: "test".to_string(),
        refresh_token: String::new(),
        id_token: None,
        account_id: None,
        expires_at: None,
    });

    let models = provider.available_models_for_switching();
    assert!(models.contains(&"gpt-5.4".to_string()));
    assert!(models.contains(&dynamic_model.to_string()));

    crate::auth::codex::set_active_account_override(None);
}

#[test]
fn test_summarize_ws_input_counts_tool_outputs() {
    let items = vec![
        serde_json::json!({
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": "hello"}]
        }),
        serde_json::json!({
            "type": "function_call",
            "call_id": "call_1",
            "name": "bash",
            "arguments": "{}"
        }),
        serde_json::json!({
            "type": "function_call_output",
            "call_id": "call_1",
            "output": "ok"
        }),
        serde_json::json!({"type": "unknown"}),
    ];

    assert_eq!(
        summarize_ws_input(&items),
        WsInputStats {
            total_items: 4,
            message_items: 1,
            function_call_items: 1,
            function_call_output_items: 1,
            other_items: 1,
        }
    );
}

#[test]
fn test_persistent_ws_idle_policy_thresholds() {
    assert!(!persistent_ws_idle_needs_healthcheck(Duration::from_secs(
        5
    )));
    assert!(persistent_ws_idle_needs_healthcheck(Duration::from_secs(
        WEBSOCKET_PERSISTENT_HEALTHCHECK_IDLE_SECS
    )));
    assert!(!persistent_ws_idle_requires_reconnect(Duration::from_secs(
        30
    )));
    assert!(persistent_ws_idle_requires_reconnect(Duration::from_secs(
        WEBSOCKET_PERSISTENT_IDLE_RECONNECT_SECS
    )));
}

#[tokio::test]
#[allow(
    clippy::await_holding_lock,
    reason = "test intentionally serializes process-wide active OpenAI account model cache across async websocket state setup"
)]
async fn test_set_model_clears_persistent_ws_state() {
    let _guard = crate::storage::lock_test_env();
    crate::auth::codex::set_active_account_override(Some("openai-set-model-clears-ws".to_string()));
    crate::provider::populate_account_models(vec!["gpt-5.3-codex".to_string()]);

    let provider = OpenAIProvider::new(CodexCredentials {
        access_token: "test".to_string(),
        refresh_token: String::new(),
        id_token: None,
        account_id: None,
        expires_at: None,
    });
    let (state, server) = test_persistent_ws_state().await;
    *provider.persistent_ws.lock().await = Some(state);

    provider.set_model("gpt-5.3-codex").expect("set model");

    assert!(
        provider.persistent_ws.lock().await.is_none(),
        "changing models should reset the persistent websocket chain"
    );
    server.abort();
    crate::auth::codex::set_active_account_override(None);
}

#[tokio::test]
async fn test_switching_to_https_clears_persistent_ws_state() {
    let provider = OpenAIProvider::new(CodexCredentials {
        access_token: "test".to_string(),
        refresh_token: String::new(),
        id_token: None,
        account_id: None,
        expires_at: None,
    });
    let (state, server) = test_persistent_ws_state().await;
    *provider.persistent_ws.lock().await = Some(state);

    provider
        .set_transport("https")
        .expect("switch transport to https");

    assert!(
        provider.persistent_ws.lock().await.is_none(),
        "switching to HTTPS should drop the websocket continuation chain"
    );
    server.abort();
}

#[test]
fn test_service_tier_can_be_changed_while_a_request_snapshot_is_held() {
    let provider = Arc::new(OpenAIProvider::new(CodexCredentials {
        access_token: "test".to_string(),
        refresh_token: String::new(),
        id_token: None,
        account_id: None,
        expires_at: None,
    }));

    let read_guard = provider
        .service_tier
        .read()
        .expect("service tier read lock should be available");

    let (tx, rx) = std::sync::mpsc::channel();
    let provider_for_write = Arc::clone(&provider);
    let handle = std::thread::spawn(move || {
        let result = provider_for_write.set_service_tier("priority");
        tx.send(result).expect("send result from setter thread");
    });

    std::thread::sleep(Duration::from_millis(20));
    assert!(
        rx.try_recv().is_err(),
        "writer should wait for the in-flight snapshot to finish"
    );

    drop(read_guard);

    rx.recv()
        .expect("receive service tier setter result")
        .expect("service tier update should succeed once read lock is released");
    handle.join().expect("join setter thread");

    assert_eq!(provider.service_tier(), Some("priority".to_string()));
}
