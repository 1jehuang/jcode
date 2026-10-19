#[test]
fn test_openai_supports_codex_models() {
    let _guard = jcode_base::storage::lock_test_env();
    let home = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", home.path());
    let _model = EnvVarGuard::remove("JCODE_OPENAI_MODEL");

    let creds = CodexCredentials {
        access_token: "catalog-codex-models-test".to_string(),
        refresh_token: String::new(),
        id_token: None,
        account_id: None,
        expires_at: None,
    };

    let provider = OpenAIProvider::new(creds);
    // Model validation uses the loaded credential's catalog, not the active
    // account's generic slot. Seed exactly that scope under an isolated home.
    jcode_base::provider::populate_account_models_for_scope(
        &provider.catalog_scope(),
        vec![
            "gpt-5.6-sol".to_string(),
            "gpt-5.1-codex".to_string(),
            "gpt-5.1-codex-mini".to_string(),
            "gpt-5.2-codex".to_string(),
        ],
    );
    assert!(provider.available_models().contains(&"gpt-5.2-codex"));
    assert!(provider.available_models().contains(&"gpt-5.1-codex-mini"));
    assert!(provider.available_models().contains(&"gpt-5.6-sol"));

    provider.set_model("gpt-5.6-sol").unwrap();
    assert_eq!(provider.model(), "gpt-5.6-sol");

    provider.set_model("gpt-5.1-codex").unwrap();
    assert_eq!(provider.model(), "gpt-5.1-codex");

    provider.set_model("gpt-5.1-codex-mini").unwrap();
    assert_eq!(provider.model(), "gpt-5.1-codex-mini");
}

#[test]
fn test_openai_switching_models_include_dynamic_catalog_entries() {
    let _guard = jcode_base::storage::lock_test_env();
    let dynamic_model = "gpt-5.9-switching-test";
    jcode_base::auth::codex::set_active_account_override(Some("switching-test".to_string()));

    let provider = OpenAIProvider::new(CodexCredentials {
        access_token: "test".to_string(),
        refresh_token: String::new(),
        id_token: None,
        account_id: None,
        expires_at: None,
    });
    // The catalog is scoped to the credential the runtime holds.
    jcode_base::provider::populate_account_models_for_scope(
        &provider.catalog_scope(),
        vec!["gpt-5.4".to_string(), dynamic_model.to_string()],
    );

    let models = provider.available_models_for_switching();
    assert!(models.contains(&"gpt-5.4".to_string()));
    assert!(models.contains(&dynamic_model.to_string()));

    jcode_base::auth::codex::set_active_account_override(None);
}

#[test]
fn test_chatgpt_web_model_bypasses_live_api_catalog() {
    let _guard = jcode_base::storage::lock_test_env();
    let home = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", home.path());
    let _model = EnvVarGuard::set("JCODE_OPENAI_MODEL", CHATGPT_WEB_MODEL);

    let provider = OpenAIProvider::new(CodexCredentials {
        access_token: "catalog-web-model-test".to_string(),
        refresh_token: String::new(),
        id_token: None,
        account_id: None,
        expires_at: None,
    });

    jcode_base::provider::populate_account_models_for_scope(
        &provider.catalog_scope(),
        vec!["gpt-5.6-sol".to_string()],
    );
    assert_eq!(provider.model(), CHATGPT_WEB_MODEL);
    assert_eq!(provider.transport().as_deref(), Some("browser"));
    assert!(
        provider
            .available_models_for_switching()
            .contains(&CHATGPT_WEB_MODEL.to_string())
    );
    provider.set_model("gpt-5.6-sol").unwrap();
    provider.set_model(CHATGPT_WEB_MODEL).unwrap();
    assert_eq!(provider.model(), CHATGPT_WEB_MODEL);
}

#[test]
fn test_chatgpt_browser_only_runtime_rejects_api_models_and_uses_local_compaction() {
    let _guard = jcode_base::storage::lock_test_env();
    let provider = OpenAIProvider::new_browser_only();

    assert_eq!(provider.model(), CHATGPT_WEB_MODEL);
    assert_eq!(provider.available_models(), vec![CHATGPT_WEB_MODEL]);
    assert_eq!(
        provider.available_models_for_switching(),
        vec![CHATGPT_WEB_MODEL.to_string()]
    );
    assert!(provider.supports_compaction());
    assert!(provider.uses_jcode_compaction());
    assert_eq!(provider.available_transports(), vec!["browser"]);
    provider.set_transport("browser").unwrap();
    assert!(provider.set_transport("auto").is_err());
    let err = provider
        .set_model("gpt-5.6-sol")
        .expect_err("browser-only runtime must not expose API models");
    assert!(err.to_string().contains("OpenAI API credentials"));
}

#[test]
fn test_chatgpt_web_model_environment_override_is_trimmed() {
    let _guard = jcode_base::storage::lock_test_env();
    let _model = EnvVarGuard::set("JCODE_OPENAI_MODEL", "  gpt-5.6-pro[web]  ");
    let provider = OpenAIProvider::new(CodexCredentials {
        access_token: "test".to_string(),
        refresh_token: String::new(),
        id_token: None,
        account_id: None,
        expires_at: None,
    });
    assert_eq!(provider.model(), CHATGPT_WEB_MODEL);
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

    // Default idle-reconnect window: reuse below threshold, reconnect at/above it.
    let default = WEBSOCKET_PERSISTENT_IDLE_RECONNECT_SECS_DEFAULT;
    assert!(!idle_requires_reconnect_with(
        Some(default),
        Duration::from_secs(default - 1)
    ));
    assert!(idle_requires_reconnect_with(
        Some(default),
        Duration::from_secs(default)
    ));

    // Disabled (None / env=0): never force a reconnect on idle alone.
    assert!(!idle_requires_reconnect_with(
        None,
        Duration::from_secs(u32::MAX as u64)
    ));
}

#[tokio::test]
async fn persistent_ws_response_idle_expiry_ignores_recent_ping_activity() {
    let (mut state, server) = test_persistent_ws_state().await;
    state.last_activity_at = Instant::now();
    state.last_response_completed_at =
        Instant::now() - Duration::from_secs(WEBSOCKET_PERSISTENT_IDLE_RECONNECT_SECS_DEFAULT);

    let outcome = ensure_persistent_ws_is_healthy(&mut state, false)
        .await
        .expect("idle policy should not require websocket I/O");
    assert!(matches!(
        outcome,
        PersistentWsHealth::Reconnect {
            reset_reason: "idle_reconnect",
            ..
        }
    ));
    server.abort();
}

#[tokio::test]
async fn persistent_ws_background_keepalive_round_trips_and_answers_server_ping() {
    let (mut state, server, ping_notify, pong_notify) =
        test_persistent_ws_state_with_ping_notify().await;
    state.last_activity_at =
        Instant::now() - Duration::from_secs(WEBSOCKET_PERSISTENT_HEALTHCHECK_IDLE_SECS + 1);
    let connection_started_at = state.connected_at;
    let persistent_ws = Arc::new(Mutex::new(Some(state)));

    let keepalive = spawn_persistent_ws_keepalive_with_interval(
        Arc::downgrade(&persistent_ws),
        connection_started_at,
        "gpt-test".to_string(),
        Duration::from_millis(5),
    );

    tokio::time::timeout(Duration::from_secs(1), ping_notify.notified())
        .await
        .expect("background keepalive should send a ping");
    tokio::time::timeout(Duration::from_secs(1), pong_notify.notified())
        .await
        .expect("background keepalive should answer a server ping");
    keepalive.abort();

    let mut guard = persistent_ws.lock().await;
    let state = guard.as_mut().expect("keepalive should retain the socket");
    assert!(
        !persistent_ws_idle_needs_healthcheck(state.last_activity_at.elapsed()),
        "successful background round trip should refresh socket activity"
    );
    drop(guard);

    *persistent_ws.lock().await = None;
    server.abort();
}

#[tokio::test]
#[allow(
    clippy::await_holding_lock,
    reason = "test intentionally serializes process-wide active OpenAI account model cache across async websocket state setup"
)]
async fn test_set_model_clears_persistent_ws_state() {
    let _guard = jcode_base::storage::lock_test_env();
    jcode_base::auth::codex::set_active_account_override(Some(
        "openai-set-model-clears-ws".to_string(),
    ));
    let provider = OpenAIProvider::new(CodexCredentials {
        access_token: "test".to_string(),
        refresh_token: String::new(),
        id_token: None,
        account_id: None,
        expires_at: None,
    });
    jcode_base::provider::populate_account_models_for_scope(
        &provider.catalog_scope(),
        vec!["gpt-5.3-codex".to_string()],
    );
    let (state, server) = test_persistent_ws_state().await;
    *provider.persistent_ws.lock().await = Some(state);

    provider.set_model("gpt-5.3-codex").expect("set model");

    assert!(
        provider.persistent_ws.lock().await.is_none(),
        "changing models should reset the persistent websocket chain"
    );
    server.abort();
    jcode_base::auth::codex::set_active_account_override(None);
}

#[tokio::test]
// The test-env lock is intentionally held across awaits to serialize
// process-global env access with other tests (see comment below).
#[allow(clippy::await_holding_lock)]
async fn test_switching_to_https_clears_persistent_ws_state() {
    // Serialize with the tests that set JCODE_OPENAI_MODEL via EnvVarGuard:
    // provider construction reads that process-global env var, so an
    // unsynchronized overlap can construct this provider pinned to the
    // browser-only web model and fail the HTTPS transport switch below.
    let _guard = jcode_base::storage::lock_test_env();
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

/// The OpenAI catalog endpoint and the chat endpoint must be selected by the
/// same authoritative discriminator: the loaded credential's *shape*
/// (`is_chatgpt_mode`), not the requested credential mode or a token-string
/// sniff. A platform API key (`sk-*`, no refresh/id token) must route to the
/// platform endpoints; a ChatGPT/Codex OAuth session must route to the Codex
/// endpoints. If these ever diverge, OpenAI returns 401.
#[test]
fn openai_catalog_and_chat_endpoints_agree_on_credential_shape() {
    // API-key-shaped credential: no refresh token, no id token.
    let api_key_creds = CodexCredentials {
        access_token: "sk-platform-key".to_string(),
        refresh_token: String::new(),
        id_token: None,
        account_id: None,
        expires_at: None,
    };
    assert!(
        !OpenAIProvider::is_chatgpt_mode(&api_key_creds),
        "platform API key must not be treated as ChatGPT/Codex mode"
    );
    assert!(
        OpenAIProvider::responses_url(&api_key_creds).starts_with(OPENAI_API_BASE),
        "platform API key chat requests must use the platform API base"
    );

    // OAuth-shaped credential: has a refresh token (Codex/ChatGPT session).
    let oauth_creds = CodexCredentials {
        access_token: "oauth-access".to_string(),
        refresh_token: "oauth-refresh".to_string(),
        id_token: None,
        account_id: None,
        expires_at: None,
    };
    assert!(
        OpenAIProvider::is_chatgpt_mode(&oauth_creds),
        "OAuth session with a refresh token must be treated as ChatGPT/Codex mode"
    );
    assert!(
        OpenAIProvider::responses_url(&oauth_creds).starts_with(CHATGPT_API_BASE),
        "OAuth chat requests must use the ChatGPT/Codex API base"
    );

    // An id-token-only credential is also a ChatGPT/Codex session.
    let id_token_creds = CodexCredentials {
        access_token: "oauth-access".to_string(),
        refresh_token: String::new(),
        id_token: Some("id-token".to_string()),
        account_id: None,
        expires_at: None,
    };
    assert!(
        OpenAIProvider::is_chatgpt_mode(&id_token_creds),
        "credential with an id token must be treated as ChatGPT/Codex mode"
    );
}

/// Issue #343: the native `openai-api` (Responses API) base URL must be
/// overridable for API-key usage so local/proxied Responses endpoints work,
/// while ChatGPT/Codex OAuth mode stays pinned to the Codex backend.
#[test]
fn responses_url_honors_api_base_override_in_api_key_mode() {
    let _guard = jcode_base::storage::lock_test_env();
    let _b = EnvVarGuard::remove("JCODE_OPENAI_API_BASE");
    let _c = EnvVarGuard::remove("OPENAI_BASE_URL");
    let _d = EnvVarGuard::remove("OPENAI_API_BASE");

    let api_key_creds = CodexCredentials {
        access_token: "sk-platform-key".to_string(),
        refresh_token: String::new(),
        id_token: None,
        account_id: None,
        expires_at: None,
    };

    // Default base when unset.
    assert_eq!(
        OpenAIProvider::responses_url(&api_key_creds),
        format!("{}/responses", OPENAI_API_BASE),
    );

    // Override is applied (and a trailing slash is tolerated).
    let _override = EnvVarGuard::set("JCODE_OPENAI_API_BASE", "http://127.0.0.1:8317/v1/");
    assert_eq!(
        OpenAIProvider::responses_url(&api_key_creds),
        "http://127.0.0.1:8317/v1/responses",
    );
    // WS URL derives from the same base.
    assert_eq!(
        OpenAIProvider::responses_ws_url(&api_key_creds),
        "ws://127.0.0.1:8317/v1/responses",
    );
    // Compact endpoint too.
    assert_eq!(
        OpenAIProvider::responses_compact_url(&api_key_creds),
        "http://127.0.0.1:8317/v1/responses/compact",
    );
}

#[test]
fn responses_url_ignores_override_in_chatgpt_mode() {
    let _guard = jcode_base::storage::lock_test_env();
    let _override = EnvVarGuard::set("JCODE_OPENAI_API_BASE", "http://127.0.0.1:8317/v1");

    let oauth_creds = CodexCredentials {
        access_token: "oauth-access".to_string(),
        refresh_token: "oauth-refresh".to_string(),
        id_token: None,
        account_id: None,
        expires_at: None,
    };
    // ChatGPT/Codex OAuth backend must stay fixed regardless of the override.
    assert!(
        OpenAIProvider::responses_url(&oauth_creds).starts_with(CHATGPT_API_BASE),
        "ChatGPT/Codex mode must ignore the API base override"
    );
}

#[test]
fn resolve_api_base_precedence_and_validation() {
    let _guard = jcode_base::storage::lock_test_env();
    let _a = EnvVarGuard::remove("JCODE_OPENAI_API_BASE");
    let _b = EnvVarGuard::remove("OPENAI_BASE_URL");
    let _c = EnvVarGuard::remove("OPENAI_API_BASE");

    // Default.
    assert_eq!(OpenAIProvider::resolve_api_base(), OPENAI_API_BASE);

    // JCODE_OPENAI_API_BASE wins over OPENAI_BASE_URL / OPENAI_API_BASE.
    let _p1 = EnvVarGuard::set("OPENAI_API_BASE", "https://c.example/v1");
    let _p2 = EnvVarGuard::set("OPENAI_BASE_URL", "https://b.example/v1");
    let _p3 = EnvVarGuard::set("JCODE_OPENAI_API_BASE", "https://a.example/v1");
    assert_eq!(OpenAIProvider::resolve_api_base(), "https://a.example/v1");

    // A trailing /responses is trimmed so callers don't double it.
    let _p4 = EnvVarGuard::set("JCODE_OPENAI_API_BASE", "https://a.example/v1/responses");
    assert_eq!(OpenAIProvider::resolve_api_base(), "https://a.example/v1");

    // Non-URL values are ignored, falling through to the next candidate.
    let _p5 = EnvVarGuard::set("JCODE_OPENAI_API_BASE", "not-a-url");
    assert_eq!(OpenAIProvider::resolve_api_base(), "https://b.example/v1");
}

#[test]
fn test_ultrafast_service_tier_round_trips_into_request_payload() {
    let _guard = jcode_base::storage::lock_test_env();
    for credentials in [api_key_creds("sk-ultrafast-test"), oauth_creds()] {
        let provider = OpenAIProvider::new(credentials);
        provider.model_service_tiers.write().unwrap().clear();
        for model in ["gpt-6-astra", "gpt-6.1-sol", "gpt-5.6-sol"] {
            // Avoid depending on the machine's cached account catalog.
            *provider.model.try_write().unwrap() = model.to_string();
            assert_eq!(
                provider.available_service_tiers(),
                vec!["priority", "ultrafast", "flex"]
            );
            for alias in ["ultrafast", "ultra", "Ultra-Fast"] {
                provider.set_service_tier(alias).unwrap();
                assert_eq!(provider.service_tier().as_deref(), Some("ultrafast"));
            }
            for is_chatgpt_mode in [false, true] {
                let request =
                    provider.response_request_for_model(model, &[], &[], "sys", is_chatgpt_mode);
                assert_eq!(request["service_tier"], serde_json::json!("ultrafast"));
            }
            let fork = provider.fork();
            assert_eq!(fork.service_tier().as_deref(), Some("ultrafast"));
            provider.set_service_tier("off").unwrap();
            assert_eq!(provider.service_tier(), None);
            assert_eq!(fork.service_tier().as_deref(), Some("ultrafast"));
            let request = provider.response_request_for_model(model, &[], &[], "sys", true);
            assert!(request.get("service_tier").is_none());
        }
        *provider.model.try_write().unwrap() = "gpt-5.5".to_string();
        assert_eq!(provider.available_service_tiers(), vec!["priority", "flex"]);
    }
}

#[test]
fn test_ultrafast_rejects_unsupported_models_and_filters_saved_or_retained_tiers() {
    let _guard = jcode_base::storage::lock_test_env();
    for credentials in [api_key_creds("sk-ultrafast-test"), oauth_creds()] {
        let provider = OpenAIProvider::new(credentials);
        provider.model_service_tiers.write().unwrap().clear();
        *provider.model.try_write().unwrap() = "gpt-5.5".to_string();
        provider.set_service_tier("priority").unwrap();
        for alias in ["ultrafast", "ultra", "ultra-fast"] {
            assert!(provider.set_service_tier(alias).is_err());
            assert_eq!(provider.service_tier().as_deref(), Some("priority"));
        }

        // Exercise exactly the normalized state a saved config override loads.
        *provider.service_tier.write().unwrap() = OpenAIProvider::load_service_tier(Some("ultra"));
        assert_eq!(provider.service_tier(), None);
        for is_chatgpt_mode in [false, true] {
            let request =
                provider.response_request_for_model("gpt-5.5", &[], &[], "sys", is_chatgpt_mode);
            assert!(request.get("service_tier").is_none());
        }

        *provider.model.try_write().unwrap() = "gpt-6.1-sol".to_string();
        assert_eq!(provider.service_tier().as_deref(), Some("ultrafast"));
        provider.set_service_tier("ultrafast").unwrap();
        // Gate against the actual request model, not only the selected model.
        let request = provider.response_request_for_model("gpt-5.5", &[], &[], "sys", false);
        assert!(request.get("service_tier").is_none());
        *provider.model.try_write().unwrap() = "gpt-5.5".to_string();
        assert_eq!(provider.service_tier(), None);
        *provider.model.try_write().unwrap() = "gpt-6.1-sol".to_string();
        let request = provider.response_request_for_model("gpt-6.1-sol", &[], &[], "sys", false);
        assert_eq!(request["service_tier"], "ultrafast");
        provider.set_service_tier("off").unwrap();
        assert_eq!(provider.service_tier(), None);
    }
}

#[test]
fn advertised_service_tiers_filter_selection_badge_and_actual_request_model() {
    let _guard = jcode_base::storage::lock_test_env();
    for credentials in [api_key_creds("sk-tier-metadata"), oauth_creds()] {
        let provider = OpenAIProvider::new(credentials);
        *provider.model.try_write().unwrap() = "gpt-6-astra".into();
        *provider.model_service_tiers.write().unwrap() = HashMap::from([
            ("gpt-6-astra".into(), vec!["priority".into()]),
            ("future-ultra".into(), vec!["ultrafast".into()]),
            ("empty-model".into(), vec![]),
        ]);
        *provider.service_tier.write().unwrap() = Some("ultrafast".into());
        assert_eq!(provider.service_tier(), None);
        assert_eq!(provider.available_service_tiers(), vec!["priority", "flex"]);
        assert!(provider.set_service_tier("ultra").is_err());
        for model in [
            "gpt-6-astra[1m]",
            "openai/gpt-6-astra",
            " OPENAI/GPT-6-ASTRA[1m] ",
        ] {
            *provider.model.try_write().unwrap() = model.into();
            assert_eq!(provider.service_tier(), None);
            assert_eq!(provider.available_service_tiers(), vec!["priority", "flex"]);
            assert!(provider.set_service_tier("ultra").is_err());
            assert!(
                provider
                    .response_request_for_model(model, &[], &[], "sys", true)
                    .get("service_tier")
                    .is_none()
            );
        }
        *provider.model.try_write().unwrap() = "gpt-6-astra".into();
        for chatgpt in [false, true] {
            let request =
                provider.response_request_for_model("gpt-6-astra", &[], &[], "sys", chatgpt);
            assert!(request.get("service_tier").is_none());
            let routing = openai_websocket_prewarm::routing_header(
                chatgpt,
                request.get("model").and_then(Value::as_str),
                request.get("service_tier").and_then(Value::as_str),
            );
            if chatgpt {
                assert_eq!(routing.unwrap().1, "model=gpt-6-astra");
            } else {
                assert!(routing.is_none());
            }
            // Explicit support overrides the legacy model-name allowlist.
            let request =
                provider.response_request_for_model("future-ultra", &[], &[], "sys", chatgpt);
            assert_eq!(request["service_tier"], "ultrafast");
        }
        *provider.model.try_write().unwrap() = "future-ultra".into();
        provider.set_service_tier("ultra").unwrap();
        assert_eq!(provider.service_tier().as_deref(), Some("ultrafast"));
        assert_eq!(
            provider.available_service_tiers(),
            vec!["ultrafast", "flex"]
        );
        assert!(provider.set_service_tier("priority").is_err());
        let fork = provider.fork();
        provider.model_service_tiers.write().unwrap().clear();
        assert_eq!(fork.service_tier().as_deref(), Some("ultrafast"));
        assert_eq!(provider.service_tier(), None);
        // Restore explicit empty metadata and verify flex is always exempt.
        provider
            .model_service_tiers
            .write()
            .unwrap()
            .insert("empty-model".into(), vec![]);
        *provider.model.try_write().unwrap() = "empty-model".into();
        assert_eq!(provider.available_service_tiers(), vec!["flex"]);
        assert!(provider.set_service_tier("priority").is_err());
        provider.set_service_tier("flex").unwrap();
        assert_eq!(provider.service_tier().as_deref(), Some("flex"));
        assert_eq!(
            provider.response_request_for_model("empty-model", &[], &[], "sys", true)["service_tier"],
            "flex"
        );
        // Missing/legacy metadata keeps priority and the old ultra allowlist.
        *provider.model.try_write().unwrap() = "gpt-6.1-sol".into();
        provider.set_service_tier("ultra").unwrap();
        assert_eq!(provider.service_tier().as_deref(), Some("ultrafast"));
        assert_eq!(
            provider.available_service_tiers(),
            vec!["priority", "ultrafast", "flex"]
        );
        assert_eq!(fork.service_tier().as_deref(), Some("ultrafast"));
    }
}

#[test]
fn service_tier_capability_refresh_only_for_unknown_oauth_metadata() {
    let _guard = jcode_base::storage::lock_test_env();
    for (credentials, expected) in [
        (oauth_creds(), true),
        (api_key_creds("sk-refresh-probe"), false),
    ] {
        let provider = OpenAIProvider::new(credentials);
        provider.model_service_tiers.write().unwrap().clear();
        *provider.model.try_write().unwrap() = "gpt-6-astra[1m]".into();
        assert_eq!(provider.model_capabilities_need_refresh(), expected);
        {
            let _credentials = provider.credentials.try_write().unwrap();
            assert!(
                !provider.model_capabilities_need_refresh(),
                "sync probe must not wait for credentials"
            );
        }
        provider
            .model_service_tiers
            .write()
            .unwrap()
            .insert("gpt-6-astra".into(), vec![]);
        for model in ["gpt-6-astra", "gpt-6-astra[1m]", "openai/gpt-6-astra"] {
            *provider.model.try_write().unwrap() = model.into();
            assert!(
                !provider.model_capabilities_need_refresh(),
                "known empty metadata is not missing"
            );
        }
        *provider.model.try_write().unwrap() = "gpt-6.1-sol".into();
        assert_eq!(provider.model_capabilities_need_refresh(), expected);
        provider.browser_only.store(true, AtomicOrdering::Release);
        assert!(!provider.model_capabilities_need_refresh());
    }
}

fn api_key_creds(key: &str) -> CodexCredentials {
    CodexCredentials {
        access_token: key.to_string(),
        refresh_token: String::new(),
        id_token: None,
        account_id: None,
        expires_at: None,
    }
}

fn oauth_creds() -> CodexCredentials {
    CodexCredentials {
        access_token: "oauth-access".to_string(),
        refresh_token: "oauth-refresh".to_string(),
        id_token: None,
        account_id: Some("acct".to_string()),
        expires_at: None,
    }
}

/// Regression: the ChatGPT/Codex OAuth catalog and the platform API-key
/// catalog used to share one per-account scope. Whichever refresh landed last
/// overwrote the other, so selecting `openai-api:<api-only model>` failed with
/// "Unsupported OpenAI model" right after an OAuth catalog refresh.
#[test]
fn oauth_catalog_refresh_does_not_clobber_api_key_catalog() {
    let _guard = jcode_base::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    jcode_base::auth::codex::set_active_account_override(Some("split-scope".to_string()));

    let api = OpenAIProvider::new(api_key_creds("sk-split-scope-test"));
    let oauth = OpenAIProvider::new(oauth_creds());
    let api_scope = api.catalog_scope();
    let oauth_scope = oauth.catalog_scope();
    assert_ne!(api_scope, oauth_scope);
    assert!(
        !api_scope.contains("sk-split-scope-test"),
        "raw key leaked into scope"
    );

    // API-key refresh lists an API-only model, then an OAuth refresh lands.
    jcode_base::provider::populate_account_models_for_scope(
        &api_scope,
        vec!["gpt-5.5".into(), "gpt-split-api-only".into()],
    );
    jcode_base::provider::populate_account_models_for_scope(
        &oauth_scope,
        vec!["gpt-5.5".into(), "gpt-split-oauth-only".into()],
    );

    api.set_model("gpt-split-api-only")
        .expect("API-key runtime must validate against its own catalog");
    let err = api
        .set_model("gpt-split-oauth-only")
        .unwrap_err()
        .to_string();
    assert!(err.contains("Unsupported OpenAI model"), "{err}");

    oauth
        .set_model("gpt-split-oauth-only")
        .expect("OAuth runtime must validate against its own catalog");
    let err = oauth
        .set_model("gpt-split-api-only")
        .unwrap_err()
        .to_string();
    assert!(err.contains("Unsupported OpenAI model"), "{err}");

    // A runtime unavailability marker on one route must not hide the model on
    // the other route.
    jcode_base::provider::record_model_unavailable_for_scope(&oauth_scope, "gpt-5.5", "denied");
    assert!(api.set_model("gpt-5.5").is_ok());

    jcode_base::auth::codex::set_active_account_override(None);
}

#[test]
fn openai_api_key_scope_is_per_key() {
    let a = OpenAIProvider::catalog_scope_for(&api_key_creds("sk-key-a"));
    let b = OpenAIProvider::catalog_scope_for(&api_key_creds("sk-key-b"));
    assert_ne!(a, b);
    assert_eq!(
        a,
        OpenAIProvider::catalog_scope_for(&api_key_creds(" sk-key-a "))
    );
}
