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
    let option = |value: &str, label: &str, upstream: &str| {
        serde_json::json!({"value":value,"label":label,"available":true,"upstream":upstream})
    };
    let model = |model: &str, label: &str| {
        serde_json::json!({"model":model,"label":label,"available":true,"detail":"","expected_upstream_models":[]})
    };
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
        let efforts: Vec<_> = crate::tui::app::inferred_reasoning_efforts(
            Some("OpenAI"),
            Some("gpt-6[web]"),
        )
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
fn test_model_picker_lists_chatgpt_web_levels_and_applies_the_chosen_one() {
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

        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("model picker should be open");
        let names: Vec<&str> = picker
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .filter(|name| name.contains("[web]"))
            .collect();
        for level in ["Instant", "Light", "Medium", "High", "Extra High", "Pro"] {
            let row = format!("gpt-6[web] ({level})");
            assert!(names.contains(&row.as_str()), "missing {row}: {names:?}");
        }
        assert!(
            names.contains(&"gpt-6-mini[web]"),
            "single-level mini stays one row: {names:?}"
        );

        let idx = picker
            .entries
            .iter()
            .position(|entry| entry.name == "gpt-6[web] (Extra High)")
            .unwrap();
        assert_eq!(picker.entries[idx].effort.as_deref(), Some("xhigh"));
        let pos = picker.filtered.iter().position(|&i| i == idx).unwrap();
        app.inline_interactive_state.as_mut().unwrap().selected = pos;
        app.handle_key(KeyCode::Enter, KeyModifiers::empty())
            .expect("selecting a ChatGPT web level should succeed");
        assert_eq!(efforts.lock().unwrap().last().map(String::as_str), Some("xhigh"));
    });
}
