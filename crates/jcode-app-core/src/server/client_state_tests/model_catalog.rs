use super::*;

#[tokio::test]
async fn handle_get_model_catalog_does_not_wait_for_busy_agent_lock() {
    for tier in [Some("priority"), Some("flex"), None] {
        assert_model_catalog_service_tier(tier, true).await;
    }
}

#[tokio::test]
async fn handle_get_model_catalog_preserves_live_service_tier() {
    for tier in [Some("priority"), Some("flex"), None] {
        assert_model_catalog_service_tier(tier, false).await;
    }
}

#[expect(
    clippy::await_holding_lock,
    reason = "test intentionally keeps the agent busy lock held to exercise model-catalog fallback"
)]
async fn assert_model_catalog_service_tier(tier: Option<&'static str>, busy: bool) {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("create temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let session_id = "session_busy_model_catalog_fallback";
    let mut session = crate::session::Session::create_with_id(
        session_id.to_string(),
        None,
        Some("busy model catalog".to_string()),
    );
    session.model = Some("persisted-model".to_string());
    session.save().expect("save session");

    let provider: Arc<dyn Provider> = Arc::new(MockProvider(tier));
    let agent = Arc::new(Mutex::new(Agent::new_with_session(
        provider.clone(),
        Registry::empty(),
        session.clone(),
        None,
    )));
    let busy_guard = if busy { Some(agent.lock().await) } else { None };

    let (stream_a, mut stream_b) = crate::transport::stream_pair().expect("stream pair");
    let (_reader_a, writer_a) = stream_a.into_split();
    let writer = Arc::new(Mutex::new(writer_a));

    tokio::time::timeout(
        std::time::Duration::from_millis(100),
        handle_get_model_catalog(43, session_id, &agent, &provider, &writer),
    )
    .await
    .expect("model catalog must not wait for busy agent mutex")
    .expect("model catalog fallback should write history event");

    drop(busy_guard);
    drop(writer);

    let mut bytes = Vec::new();
    stream_b
        .read_to_end(&mut bytes)
        .await
        .expect("read model catalog event bytes");
    let mut cursor = std::io::Cursor::new(bytes);
    let mut line = String::new();
    cursor.read_line(&mut line).expect("read first line");
    let event: crate::protocol::ServerEvent =
        serde_json::from_str(line.trim()).expect("decode model catalog event");

    match event {
        crate::protocol::ServerEvent::History {
            id,
            session_id: returned_session_id,
            provider_name,
            provider_model,
            service_tier,
            available_service_tiers,
            reasoning_effort,
            provider_oauth_account_label,
            ..
        } => {
            assert_eq!(id, 43);
            assert_eq!(
                provider_oauth_account_label.as_deref(),
                tier.map(|_| "daemon-work")
            );
            assert_eq!(returned_session_id, session_id);
            assert_eq!(provider_name.as_deref(), Some("Mock Profile"));
            assert_eq!(
                provider_model.as_deref(),
                Some(if busy {
                    "persisted-model"
                } else {
                    "mock-model"
                })
            );
            assert_eq!(service_tier.as_deref(), tier);
            assert_eq!(
                available_service_tiers,
                Some(if tier.is_some() {
                    vec!["priority".to_string(), "flex".to_string()]
                } else {
                    Vec::new()
                })
            );
            assert_eq!(reasoning_effort.as_deref(), Some("high"));
        }
        other => panic!("expected history event, got {:?}", other),
    }

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}
