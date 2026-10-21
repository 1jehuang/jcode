// Regression: ChatGPT web models offered no reasoning levels in the model
// picker, /effort or the effort hotkeys, so picking "gpt-6[web]" always sent
// effort=none and the account catalog's ladder (Instant ... Pro) was
// unreachable from the TUI.

#[derive(Clone)]
struct ChatGptWebPickerProvider {
    efforts: StdArc<StdMutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl Provider for ChatGptWebPickerProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        unimplemented!("ChatGptWebPickerProvider")
    }

    fn name(&self) -> &str {
        "OpenAI"
    }

    fn model(&self) -> String {
        "gpt-6[web]".to_string()
    }

    fn model_routes(&self) -> Vec<crate::provider::ModelRoute> {
        ["gpt-6[web]", "gpt-6-mini[web]"]
            .into_iter()
            .map(|model| crate::provider::ModelRoute {
                model: model.to_string(),
                provider: "OpenAI".to_string(),
                api_method: "chatgpt-web".to_string(),
                available: true,
                detail: String::new(),
                usage: None,
                cheapness: None,
                catalog_metadata: None,
            })
            .collect()
    }

    fn available_efforts(&self) -> Vec<&'static str> {
        vec!["none", "low", "medium", "high", "xhigh", "max"]
    }

    fn set_reasoning_effort(&self, effort: &str) -> Result<()> {
        self.efforts.lock().unwrap().push(effort.to_string());
        Ok(())
    }

    fn set_model(&self, _model: &str) -> Result<()> {
        Ok(())
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

/// The catalog snapshot `jcode browser chatgpt-login` stores.
fn write_chatgpt_web_catalog(home: &std::path::Path) {
    let option = |value: &str, label: &str, upstream: &str| serde_json::json!({"value":value,"label":label,"available":true,"upstream":upstream});
    let model = |model: &str, label: &str| serde_json::json!({"model":model,"label":label,"available":true,"detail":"","expected_upstream_models":[]});
    let snapshot = serde_json::json!({
        "version": 1,
        "observed_at": 1,
        "models": [model("gpt-6[web]", "GPT-6"), model("gpt-6-mini[web]", "GPT-6 mini")],
        "reasoning": {
            "gpt-6[web]": [
                option("none","Instant","gpt-6-instant"),
                option("low","Light","gpt-6-thinking:min"),
                option("medium","Medium","gpt-6-thinking:standard"),
                option("high","High","gpt-6-thinking:extended"),
                option("xhigh","Extra High","gpt-6-thinking:max"),
                option("max","Pro","gpt-6-pro:standard")
            ],
            "gpt-6-mini[web]": [option("none","Instant","gpt-6-mini")]
        }
    });
    std::fs::write(
        home.join("chatgpt_web_model_catalog_cache.json"),
        snapshot.to_string(),
    )
    .unwrap();
}

#[test]
fn test_chatgpt_web_efforts_come_from_the_account_catalog() {
    with_temp_jcode_home(|| {
        let home = std::path::PathBuf::from(std::env::var("JCODE_HOME").unwrap());
        write_chatgpt_web_catalog(&home);
        let efforts: Vec<_> =
            crate::tui::app::inferred_reasoning_efforts(Some("OpenAI"), Some("gpt-6[web]"))
                .into_iter()
                .filter(|e| !e.starts_with("swarm"))
                .collect();
        assert_eq!(efforts, ["none", "low", "medium", "high", "xhigh", "max"]);
        assert_eq!(
            crate::tui::app::chatgpt_web_effort_label("gpt-6[web]", "xhigh").as_deref(),
            Some("Extra High")
        );
        // Single-level routes offer no ladder, so effort hotkeys and /effort
        // report nothing to change instead of jumping to a swarm rung.
        assert!(
            crate::tui::app::inferred_reasoning_efforts(Some("OpenAI"), Some("gpt-6-mini[web]"))
                .is_empty()
        );
    });
}

#[test]
fn test_model_picker_offers_chatgpt_web_levels_and_applies_the_chosen_one() {
    with_temp_jcode_home(|| {
        let home = std::path::PathBuf::from(std::env::var("JCODE_HOME").unwrap());
        write_chatgpt_web_catalog(&home);
        let efforts = StdArc::new(StdMutex::new(Vec::new()));
        let provider: Arc<dyn Provider> = Arc::new(ChatGptWebPickerProvider {
            efforts: efforts.clone(),
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
        let mut app = App::new_for_test_harness(provider, registry);
        app.open_model_picker();
        wait_for_model_picker_load(&mut app);

        // One row per model; the ChatGPT levels are its second step.
        select_model_row(&mut app, "gpt-6[web]", "chatgpt-web");
        app.handle_key(KeyCode::Enter, KeyModifiers::empty())
            .expect("opening the level step should succeed");
        assert_eq!(
            effort_step_names(&app),
            ["Instant", "Light", "Medium", "High", "Extra High", "Pro"]
        );
        select_step_row(&mut app, "Extra High");
        app.handle_key(KeyCode::Enter, KeyModifiers::empty())
            .expect("selecting a ChatGPT web level should succeed");
        assert_eq!(
            efforts.lock().unwrap().last().map(String::as_str),
            Some("xhigh")
        );

        // A single-level mini has no level step.
        app.open_model_picker();
        wait_for_model_picker_load(&mut app);
        select_model_row(&mut app, "gpt-6-mini[web]", "chatgpt-web");
        app.handle_key(KeyCode::Enter, KeyModifiers::empty())
            .expect("selecting the mini should succeed");
        assert!(
            app.inline_interactive_state
                .as_ref()
                .is_none_or(|picker| picker.effort_step.is_none()),
            "a single-level route opens no level step"
        );
    });
}

/// Build the actual server routes under a different home from the client.
/// No browser, account, or network is involved.
fn isolated_chatgpt_web_server_event() -> crate::protocol::ServerEvent {
    let client_home = std::env::var_os("JCODE_HOME").unwrap();
    let server_home = tempfile::tempdir().unwrap();
    crate::env::set_var("JCODE_HOME", server_home.path());
    write_chatgpt_web_catalog(server_home.path());
    let routes = ["gpt-6[web]", "gpt-6-mini[web]"]
        .into_iter()
        .map(|model| crate::provider::build_chatgpt_web_route(model, true, "server catalog"))
        .collect();
    let event = crate::protocol::ServerEvent::AvailableModelsUpdated {
        provider_name: Some("OpenAI".into()),
        provider_model: Some("gpt-6[web]".into()),
        available_models: vec!["gpt-6[web]".into(), "gpt-6-mini[web]".into()],
        available_model_routes: routes,
    };
    let wire = crate::protocol::encode_event(&event);
    crate::env::set_var("JCODE_HOME", client_home);
    serde_json::from_str(&wire).unwrap()
}

#[test]
fn test_remote_chatgpt_web_efforts_survive_empty_client_catalog() {
    with_temp_jcode_home(|| {
        let event = isolated_chatgpt_web_server_event();
        assert!(crate::provider::chatgpt_web::cached_models().is_empty());
        assert!(crate::provider::chatgpt_web::cached_reasoning_options("gpt-6[web]").is_empty());

        let mut app = create_test_app();
        app.is_remote = true;
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        app.handle_server_event(event, &mut remote);

        assert_eq!(
            super::inferred_reasoning_efforts(Some("OpenAI"), Some("gpt-6[web]")),
            [
                "none",
                "low",
                "medium",
                "high",
                "xhigh",
                "max",
                "swarm",
                "swarm-deep"
            ]
        );
        assert!(
            super::inferred_reasoning_efforts(Some("OpenAI"), Some("gpt-6-mini[web]")).is_empty()
        );
        assert_eq!(
            super::chatgpt_web_effort_label("gpt-6[web]", "max").as_deref(),
            Some("Pro")
        );

        // The remote /effort path uses the same server ladder as the picker.
        app.set_input_for_test("/effort");
        rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
            .unwrap();
        let report = &app.display_messages.last().unwrap().content;
        assert!(!report.contains("not available"), "{report}");
        assert!(report.contains("xHigh"), "{report}");

        app.open_model_picker();
        wait_for_model_picker_load(&mut app);
        select_model_row(&mut app, "gpt-6[web]", "chatgpt-web");
        app.handle_key(KeyCode::Enter, KeyModifiers::empty())
            .unwrap();
        assert_eq!(
            effort_step_names(&app),
            ["Instant", "Light", "Medium", "High", "Extra High", "Pro"]
        );
        select_step_row(&mut app, "Pro");
        app.handle_key(KeyCode::Enter, KeyModifiers::empty())
            .unwrap();
        assert_eq!(app.pending_reasoning_effort.as_deref(), Some("max"));
        assert_eq!(
            app.pending_route_selection.as_ref().unwrap().api_method,
            "chatgpt-web"
        );
    });
}

#[test]
fn test_remote_chatgpt_web_metadata_overrides_local_choices_and_legacy_falls_back() {
    with_temp_jcode_home(|| {
        let home = std::path::PathBuf::from(std::env::var("JCODE_HOME").unwrap());
        write_chatgpt_web_catalog(&home);
        assert!(!crate::provider::chatgpt_web::cached_reasoning_options("gpt-6[web]").is_empty());
        let mut event = isolated_chatgpt_web_server_event();
        let mut app = create_test_app();
        app.is_remote = true;
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        if let crate::protocol::ServerEvent::AvailableModelsUpdated {
            available_model_routes,
            ..
        } = &mut event
        {
            let metadata = available_model_routes[0].catalog_metadata.as_mut().unwrap();
            metadata.reasoning_efforts.clear();
            metadata.reasoning_effort_labels.clear();
        }
        app.handle_server_event(event.clone(), &mut remote);
        assert!(super::inferred_reasoning_efforts(Some("OpenAI"), Some("gpt-6[web]")).is_empty());
        assert_eq!(super::chatgpt_web_effort_label("gpt-6[web]", "max"), None);

        // Names-only broadcasts retain the full catalog already received.
        app.handle_server_event(
            crate::protocol::ServerEvent::AvailableModelsUpdated {
                provider_name: Some("OpenAI".into()),
                provider_model: Some("gpt-6[web]".into()),
                available_models: vec!["gpt-6[web]".into(), "gpt-6-mini[web]".into()],
                available_model_routes: Vec::new(),
            },
            &mut remote,
        );
        assert!(super::inferred_reasoning_efforts(Some("OpenAI"), Some("gpt-6[web]")).is_empty());

        // Older servers omit metadata, so the existing local fallback survives.
        if let crate::protocol::ServerEvent::AvailableModelsUpdated {
            available_model_routes,
            ..
        } = &mut event
        {
            for route in available_model_routes {
                route.catalog_metadata = None;
            }
        }
        app.handle_server_event(event, &mut remote);
        assert!(
            super::inferred_reasoning_efforts(Some("OpenAI"), Some("gpt-6[web]")).contains(&"max")
        );
        assert_eq!(
            super::chatgpt_web_effort_label("gpt-6[web]", "max").as_deref(),
            Some("Pro")
        );
    });
}
