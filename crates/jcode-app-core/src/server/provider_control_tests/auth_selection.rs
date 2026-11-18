use super::*;

#[tokio::test]
async fn manual_account_choice_survives_pending_onboarding_auth_refresh() {
    use crate::provider::{AccountPin, AccountProviderKind};

    let _guard = EnvGuard::save(&[
        "JCODE_RUNTIME_PROVIDER",
        "JCODE_ACTIVE_PROVIDER",
        "JCODE_INITIAL_PROVIDER_EXPLICIT",
    ]);
    for choice in [Some(AccountPin::new("second", None)), None] {
        crate::bus::reset_models_updated_publish_state_for_tests();
        let provider = Arc::new(AuthChangeMockProvider::new());
        provider
            .state
            .auth_refresh_delay_ms
            .store(80, Ordering::Release);
        *provider.state.selected_model.write().unwrap() = Some("gpt-5.6-sol".to_string());
        *provider.state.routes_override.write().unwrap() = Some(
            ["first", "second"]
                .into_iter()
                .map(|label| ModelRoute {
                    catalog_metadata: None,
                    model: "gpt-5.6-sol".to_string(),
                    provider: "OpenAI".to_string(),
                    api_method: format!("openai-oauth@{label}"),
                    available: true,
                    detail: String::new(),
                    usage: None,
                    cheapness: None,
                })
                .collect(),
        );
        let provider: Arc<dyn Provider> = provider;
        let agent = Arc::new(Mutex::new(Agent::new(provider.clone(), Registry::empty())));
        agent
            .lock()
            .await
            .set_account_pin(
                AccountProviderKind::OpenAi,
                Some(AccountPin::new("first", None)),
            )
            .unwrap();
        agent.lock().await.persist_session_now().unwrap();
        let session_id = agent.lock().await.session_id().to_string();
        let sessions: SessionAgents = Arc::new(
            crate::server::session_manager::SessionManager::from_agents(HashMap::new()),
        );
        let (tx, mut rx) = mpsc::unbounded_channel();
        handle_notify_auth_changed(
            249,
            Some("openai".to_string()),
            None,
            true,
            &provider,
            &provider,
            &sessions,
            &session_id,
            &agent,
            &tx,
        )
        .await;
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !matches!(
                rx.recv().await,
                Some(ServerEvent::AvailableModelsUpdated { .. })
            ) {}
        })
        .await
        .unwrap();
        assert!(provider.auth_model_refresh_pending());
        {
            let mut agent = agent.lock().await;
            let generation = agent.provider_model_selection_generation();
            assert!(
                agent
                    .set_account_pin(
                        AccountProviderKind::OpenAi,
                        Some(AccountPin::new("missing", None)),
                    )
                    .is_err()
            );
            assert_eq!(agent.provider_model_selection_generation(), generation);
            agent
                .set_account_pin(AccountProviderKind::OpenAi, choice.clone())
                .unwrap();
        }

        recv_final_catalog_notification(&mut rx).await;

        assert_eq!(provider.account_pin(AccountProviderKind::OpenAi), choice);
        assert_eq!(
            agent.lock().await.account_pins().get("openai"),
            choice.as_ref()
        );
        let saved = crate::session::Session::load(&session_id).unwrap();
        assert_eq!(saved.account_pins.get("openai"), choice.as_ref());
    }
}

#[derive(Clone, Copy)]
struct AuthModelE2eScenario {
    name: &'static str,
    manual_pick_after_first_snapshot: Option<&'static str>,
    prompt_immediately_after_model_pick: bool,
    expected_first_prompt_model: &'static str,
}

#[tokio::test]
async fn auth_model_first_prompt_e2e_state_space_is_bounded_by_selection_source() {
    let scenarios = [
        AuthModelE2eScenario {
            name: "auth auto-selects matching route when user does not intervene",
            manual_pick_after_first_snapshot: None,
            prompt_immediately_after_model_pick: false,
            expected_first_prompt_model: "logged-in-model",
        },
        AuthModelE2eScenario {
            name: "manual picker selection during auth refresh wins first prompt",
            manual_pick_after_first_snapshot: Some("user-picked-model"),
            prompt_immediately_after_model_pick: true,
            expected_first_prompt_model: "user-picked-model",
        },
    ];

    for scenario in scenarios {
        let _guard = EnvGuard::save(&[
            "JCODE_OPENROUTER_API_BASE",
            "JCODE_OPENROUTER_API_KEY_NAME",
            "JCODE_OPENROUTER_ENV_FILE",
            "JCODE_OPENROUTER_CACHE_NAMESPACE",
            "JCODE_OPENROUTER_PROVIDER_FEATURES",
            "JCODE_OPENROUTER_TRANSPORT_STATE",
            "JCODE_OPENROUTER_MODEL_CATALOG",
            "JCODE_OPENROUTER_AUTH_HEADER",
            "JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER",
            "JCODE_OPENROUTER_MODEL",
            "JCODE_RUNTIME_PROVIDER",
            "JCODE_ACTIVE_PROVIDER",
            "JCODE_INITIAL_PROVIDER_EXPLICIT",
        ]);

        crate::bus::reset_models_updated_publish_state_for_tests();
        let provider_concrete = Arc::new(AuthChangeMockProvider::new());
        provider_concrete
            .state
            .auth_refresh_delay_ms
            .store(80, Ordering::Release);
        *provider_concrete.state.selected_model.write().unwrap() = Some("stale-model".to_string());
        *provider_concrete.state.route_provider.write().unwrap() = "Cerebras".to_string();
        *provider_concrete.state.route_api_method.write().unwrap() =
            "openai-compatible:cerebras".to_string();
        *provider_concrete
            .state
            .expose_selected_model_in_routes
            .write()
            .unwrap() = false;
        let provider: Arc<dyn Provider> = provider_concrete.clone();
        let registry = Registry::empty();
        let agent = Arc::new(Mutex::new(Agent::new(provider.clone(), registry)));
        let session_id = { agent.lock().await.session_id().to_string() };
        let sessions: SessionAgents = Arc::new(
            crate::server::session_manager::SessionManager::from_agents(HashMap::from([(
                format!("test-session-{}", scenario.name),
                Arc::clone(&agent),
            )])),
        );
        let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

        let mut auth = crate::protocol::AuthChanged::new("cerebras");
        auth.credential_source = Some(crate::protocol::AuthCredentialSource::ApiKeyFile);
        auth.auth_method = Some(crate::protocol::AuthMethod::RemoteTuiPasteApiKey);
        auth.expected_runtime = Some(crate::protocol::RuntimeProviderKey::new(
            "openai-compatible",
        ));
        auth.expected_catalog_namespace = Some(crate::protocol::CatalogNamespace::new("cerebras"));

        handle_notify_auth_changed(
            148,
            None,
            Some(auth),
            false,
            &provider,
            &provider,
            &sessions,
            session_id.as_str(),
            &agent,
            &client_event_tx,
        )
        .await;

        assert!(
            matches!(
                client_event_rx.recv().await,
                Some(ServerEvent::Done { id: 148 })
            ),
            "{}: expected auth Done",
            scenario.name
        );

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if matches!(
                    client_event_rx.recv().await,
                    Some(ServerEvent::AvailableModelsUpdated { .. })
                ) {
                    break;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{}: expected immediate auth model snapshot", scenario.name));

        let mut first_prompt_output = None;
        if let Some(model) = scenario.manual_pick_after_first_snapshot {
            handle_set_model(248, model.to_string(), &agent, &client_event_tx).await;
            loop {
                match client_event_rx.recv().await {
                    Some(ServerEvent::ModelChanged {
                        id: 248,
                        model: changed,
                        error,
                        ..
                    }) => {
                        assert_eq!(error, None, "{}: manual model switch failed", scenario.name);
                        assert_eq!(
                            changed, model,
                            "{}: manual model switch mismatch",
                            scenario.name
                        );
                        break;
                    }
                    Some(_) => continue,
                    None => panic!("{}: model switch channel closed", scenario.name),
                }
            }

            if scenario.prompt_immediately_after_model_pick {
                let agent_for_prompt = Arc::clone(&agent);
                let scenario_name = scenario.name;
                first_prompt_output = Some(
                    tokio::time::timeout(std::time::Duration::from_secs(2), async move {
                        let mut agent_guard = agent_for_prompt.lock().await;
                        agent_guard
                            .run_once_capture("first prompt immediately after model selection")
                            .await
                    })
                    .await
                    .unwrap_or_else(|_| panic!("{}: first prompt stalled", scenario_name))
                    .unwrap_or_else(|error| {
                        panic!("{}: first prompt failed: {error:?}", scenario_name)
                    }),
                );
            }
        }

        let final_message = recv_final_catalog_notification(&mut client_event_rx).await;
        assert!(
            final_message.contains(&format!(
                "**Model ready:** `{}`",
                scenario.expected_first_prompt_model
            )),
            "{}: final activity selected wrong model: {}",
            scenario.name,
            final_message
        );

        let first_prompt_output = if let Some(output) = first_prompt_output {
            output
        } else {
            let mut agent_guard = agent.lock().await;
            agent_guard
                .run_once_capture("first prompt after auth/model selection")
                .await
                .unwrap_or_else(|error| panic!("{}: first prompt failed: {error:?}", scenario.name))
        };
        assert!(
            first_prompt_output.contains("ok"),
            "{}: fake provider response not observed: {}",
            scenario.name,
            first_prompt_output
        );
        let completed_models = provider_concrete
            .state
            .complete_models
            .lock()
            .unwrap()
            .clone();
        assert_eq!(
            completed_models.last().map(String::as_str),
            Some(scenario.expected_first_prompt_model),
            "{}: first provider request used wrong model; all completions: {:?}",
            scenario.name,
            completed_models
        );
    }
}

#[tokio::test]
async fn onboarding_auth_refresh_prefers_global_gpt_5_6_route_over_fable() {
    let _guard = EnvGuard::save(&[
        "JCODE_RUNTIME_PROVIDER",
        "JCODE_ACTIVE_PROVIDER",
        "JCODE_INITIAL_PROVIDER_EXPLICIT",
    ]);
    crate::bus::reset_models_updated_publish_state_for_tests();

    let provider = Arc::new(AuthChangeMockProvider::new());
    *provider.state.routes_override.write().unwrap() = Some(vec![
        ModelRoute {
            catalog_metadata: None,
            model: "claude-fable-5".to_string(),
            provider: "Anthropic".to_string(),
            api_method: "claude-oauth".to_string(),
            available: true,
            detail: String::new(),
            usage: None,
            cheapness: None,
        },
        ModelRoute {
            catalog_metadata: None,
            model: "gpt-5.5".to_string(),
            provider: "OpenAI".to_string(),
            api_method: "openai-api-key".to_string(),
            available: true,
            detail: String::new(),
            usage: None,
            cheapness: None,
        },
        ModelRoute {
            catalog_metadata: None,
            model: "gpt-5.6-sol".to_string(),
            provider: "OpenAI".to_string(),
            api_method: "openai-api-key".to_string(),
            available: true,
            detail: String::new(),
            usage: None,
            cheapness: None,
        },
    ]);
    let provider: Arc<dyn Provider> = provider;
    let agent = Arc::new(Mutex::new(Agent::new(provider.clone(), Registry::empty())));
    let session_id = { agent.lock().await.session_id().to_string() };
    let sessions: SessionAgents = Arc::new(
        crate::server::session_manager::SessionManager::from_agents(HashMap::new()),
    );
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

    handle_notify_auth_changed(
        49,
        Some("claude".to_string()),
        None,
        true,
        &provider,
        &provider,
        &sessions,
        session_id.as_str(),
        &agent,
        &client_event_tx,
    )
    .await;

    recv_final_catalog_notification(&mut client_event_rx).await;

    assert_eq!(agent.lock().await.provider_model(), "gpt-5.6-sol");
}
