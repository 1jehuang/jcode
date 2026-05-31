#[test]
fn test_cancel_pending_provider_failover_clears_countdown() {
    with_temp_jcode_home(|| {
        write_test_config("[provider]\ncross_provider_failover = \"countdown\"\n");
        let (mut app, _active_provider) = create_switchable_test_app("claude");
        let prompt = crate::provider::ProviderFailoverPrompt {
            from_provider: "claude".to_string(),
            from_label: "Anthropic".to_string(),
            to_provider: "openai".to_string(),
            to_label: "OpenAI".to_string(),
            reason: "OAuth usage exhausted".to_string(),
            estimated_input_chars: 16_000,
            estimated_input_tokens: 4_000,
        };

        app.handle_turn_error(failover_error_message(&prompt));
        assert!(app.pending_provider_failover.is_some());

        app.cancel_pending_provider_failover("Provider auto-switch canceled");

        assert!(app.pending_provider_failover.is_none());
        let last = app.display_messages.last().expect("display message");
        assert_eq!(last.role, "system");
        assert!(last.content.contains("Canceled provider auto-switch"));
        assert!(
            last.content
                .contains("cross_provider_failover = \"manual\"")
        );
    });
}

#[derive(Clone)]
struct FastMockProvider {
    service_tier: StdArc<StdMutex<Option<String>>>,
}

#[async_trait::async_trait]
impl Provider for FastMockProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        unimplemented!("FastMockProvider")
    }

    fn name(&self) -> &str {
        "mock"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }

    fn service_tier(&self) -> Option<String> {
        self.service_tier.lock().unwrap().clone()
    }

    fn set_service_tier(&self, service_tier: &str) -> anyhow::Result<()> {
        let normalized = match service_tier.trim().to_ascii_lowercase().as_str() {
            "priority" | "fast" => Some("priority".to_string()),
            "off" | "default" | "auto" | "none" => None,
            other => anyhow::bail!("unsupported service tier {other}"),
        };
        *self.service_tier.lock().unwrap() = normalized;
        Ok(())
    }
}

#[derive(Clone)]
struct SwitchableMockProvider {
    active_provider: StdArc<StdMutex<String>>,
}

#[async_trait::async_trait]
impl Provider for SwitchableMockProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        unimplemented!("SwitchableMockProvider")
    }

    fn name(&self) -> &str {
        "switchable-mock"
    }

    fn model(&self) -> String {
        match self.active_provider.lock().unwrap().as_str() {
            "openai" => "gpt-test".to_string(),
            _ => "claude-test".to_string(),
        }
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }

    fn switch_active_provider_to(&self, provider: &str) -> Result<()> {
        *self.active_provider.lock().unwrap() = provider.to_string();
        Ok(())
    }
}

fn create_switchable_test_app(initial_provider: &str) -> (App, StdArc<StdMutex<String>>) {
    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let active_provider = StdArc::new(StdMutex::new(initial_provider.to_string()));
    let provider: Arc<dyn Provider> = Arc::new(SwitchableMockProvider {
        active_provider: active_provider.clone(),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new_for_test_harness(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    (app, active_provider)
}

#[derive(Clone)]
struct AuthRefreshingMockProvider {
    logged_in: StdArc<StdMutex<bool>>,
}

#[async_trait::async_trait]
impl Provider for AuthRefreshingMockProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        unimplemented!("AuthRefreshingMockProvider")
    }

    fn name(&self) -> &str {
        "auth-refresh-mock"
    }

    fn model(&self) -> String {
        if *self.logged_in.lock().unwrap() {
            "claude-opus-4.6".to_string()
        } else {
            "gpt-5.4".to_string()
        }
    }

    fn available_models_display(&self) -> Vec<String> {
        if *self.logged_in.lock().unwrap() {
            vec![
                "claude-opus-4.6".to_string(),
                "grok-code-fast-1".to_string(),
            ]
        } else {
            vec!["gpt-5.4".to_string()]
        }
    }

    fn model_routes(&self) -> Vec<crate::provider::ModelRoute> {
        if *self.logged_in.lock().unwrap() {
            vec![
                crate::provider::ModelRoute {
                    model: "claude-opus-4.6".to_string(),
                    provider: "Copilot".to_string(),
                    api_method: "copilot".to_string(),
                    available: true,
                    detail: String::new(),
                    cheapness: None,
                },
                crate::provider::ModelRoute {
                    model: "grok-code-fast-1".to_string(),
                    provider: "Copilot".to_string(),
                    api_method: "copilot".to_string(),
                    available: true,
                    detail: String::new(),
                    cheapness: None,
                },
            ]
        } else {
            vec![crate::provider::ModelRoute {
                model: "gpt-5.4".to_string(),
                provider: "OpenAI".to_string(),
                api_method: "openai-oauth".to_string(),
                available: true,
                detail: String::new(),
                cheapness: None,
            }]
        }
    }

    fn on_auth_changed(&self) {
        *self.logged_in.lock().unwrap() = true;
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

#[derive(Clone)]
struct AsyncAuthRefreshingMockProvider {
    started: StdArc<AtomicBool>,
    completed: StdArc<AtomicBool>,
    delay: Duration,
}

#[async_trait::async_trait]
impl Provider for AsyncAuthRefreshingMockProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        unimplemented!("AsyncAuthRefreshingMockProvider")
    }

    fn name(&self) -> &str {
        "async-auth-refresh-mock"
    }

    fn on_auth_changed(&self) {
        self.started.store(true, Ordering::SeqCst);
        std::thread::sleep(self.delay);
        self.completed.store(true, Ordering::SeqCst);
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

fn create_auth_refresh_test_app() -> App {
    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let provider: Arc<dyn Provider> = Arc::new(AuthRefreshingMockProvider {
        logged_in: StdArc::new(StdMutex::new(false)),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new_for_test_harness(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app
}

#[derive(Clone)]
struct AntigravityMockProvider {
    model: StdArc<StdMutex<String>>,
}

#[async_trait::async_trait]
impl Provider for AntigravityMockProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        unimplemented!("AntigravityMockProvider")
    }

    fn name(&self) -> &str {
        "Antigravity"
    }

    fn model(&self) -> String {
        self.model.lock().unwrap().clone()
    }

    fn set_model(&self, model: &str) -> Result<()> {
        let resolved = model
            .strip_prefix("antigravity:")
            .unwrap_or(model)
            .to_string();
        *self.model.lock().unwrap() = resolved;
        Ok(())
    }

    fn model_routes(&self) -> Vec<crate::provider::ModelRoute> {
        vec![
            crate::provider::ModelRoute {
                model: "claude-sonnet-4-6".to_string(),
                provider: "Antigravity".to_string(),
                api_method: "cli".to_string(),
                available: true,
                detail: "cached catalog".to_string(),
                cheapness: None,
            },
            crate::provider::ModelRoute {
                model: "gpt-oss-120b-medium".to_string(),
                provider: "Antigravity".to_string(),
                api_method: "cli".to_string(),
                available: true,
                detail: "cached catalog".to_string(),
                cheapness: None,
            },
        ]
    }

    fn available_models_display(&self) -> Vec<String> {
        vec![
            "claude-sonnet-4-6".to_string(),
            "gpt-oss-120b-medium".to_string(),
        ]
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

fn create_antigravity_picker_test_app() -> App {
    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let provider: Arc<dyn Provider> = Arc::new(AntigravityMockProvider {
        model: StdArc::new(StdMutex::new("default".to_string())),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new_for_test_harness(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app
}

fn render_model_picker_text(app: &mut App, width: u16, height: u16) -> String {
    let _render_lock = scroll_render_test_lock();
    if app.display_messages.is_empty() {
        app.display_messages = vec![DisplayMessage::system("seed render state")];
        app.bump_display_messages_version();
    }
    app.open_model_picker();
    wait_for_model_picker_load(app);
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(app, &mut terminal)
}

#[derive(Clone)]
struct LoginSmokeModelProvider;

#[async_trait::async_trait]
impl Provider for LoginSmokeModelProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        unimplemented!("LoginSmokeModelProvider")
    }

    fn name(&self) -> &str {
        "login-smoke"
    }

    fn model(&self) -> String {
        "gpt-5.4".to_string()
    }

    fn model_routes(&self) -> Vec<crate::provider::ModelRoute> {
        vec![
            crate::provider::ModelRoute {
                model: "gpt-5.4".to_string(),
                provider: "OpenAI".to_string(),
                api_method: "openai-oauth".to_string(),
                available: true,
                detail: String::new(),
                cheapness: None,
            },
            crate::provider::ModelRoute {
                model: "gpt-5.4".to_string(),
                provider: "OpenAI".to_string(),
                api_method: "openai-api-key".to_string(),
                available: true,
                detail: String::new(),
                cheapness: None,
            },
            crate::provider::ModelRoute {
                model: "openai/gpt-5.5".to_string(),
                provider: "OpenAI".to_string(),
                api_method: "openrouter".to_string(),
                available: true,
                detail: String::new(),
                cheapness: None,
            },
            crate::provider::ModelRoute {
                model: "glm-51-nvfp4".to_string(),
                provider: "Comtegra GPU Cloud".to_string(),
                api_method: "openai-compatible:comtegra".to_string(),
                available: true,
                detail: "recently added · https://llm.comtegra.cloud/v1".to_string(),
                cheapness: None,
            },
            crate::provider::ModelRoute {
                model: "claude-opus-4.6".to_string(),
                provider: "Copilot".to_string(),
                api_method: "copilot".to_string(),
                available: true,
                detail: String::new(),
                cheapness: None,
            },
            crate::provider::ModelRoute {
                model: "deepseek/deepseek-v4-pro".to_string(),
                provider: "auto".to_string(),
                api_method: "openrouter".to_string(),
                available: true,
                detail: String::new(),
                cheapness: None,
            },
            crate::provider::ModelRoute {
                model: "deepseek/deepseek-v4-pro".to_string(),
                provider: "DeepSeek".to_string(),
                api_method: "openrouter".to_string(),
                available: true,
                detail: String::new(),
                cheapness: None,
            },
            crate::provider::ModelRoute {
                model: "moonshotai/kimi-k2.5".to_string(),
                provider: "auto".to_string(),
                api_method: "openrouter".to_string(),
                available: true,
                detail: String::new(),
                cheapness: None,
            },
        ]
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

fn create_login_smoke_model_app() -> App {
    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let provider: Arc<dyn Provider> = Arc::new(LoginSmokeModelProvider);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new_for_test_harness(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app
}

#[derive(Clone)]
struct FailingModelSwitchProvider;

#[async_trait::async_trait]
impl Provider for FailingModelSwitchProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        unimplemented!("FailingModelSwitchProvider")
    }

    fn name(&self) -> &str {
        "failing-model-switch"
    }

    fn model(&self) -> String {
        "gpt-5.4".to_string()
    }

    fn model_routes(&self) -> Vec<crate::provider::ModelRoute> {
        vec![crate::provider::ModelRoute {
            model: "claude-opus-4.6".to_string(),
            provider: "Copilot".to_string(),
            api_method: "copilot".to_string(),
            available: true,
            detail: String::new(),
            cheapness: None,
        }]
    }

    fn set_model(&self, _model: &str) -> Result<()> {
        anyhow::bail!("credentials expired")
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

fn create_failing_model_switch_test_app() -> App {
    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let provider: Arc<dyn Provider> = Arc::new(FailingModelSwitchProvider);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new_for_test_harness(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app
}

fn write_test_config(contents: &str) {
    let path = crate::config::Config::path().expect("config path");
    std::fs::create_dir_all(path.parent().expect("config dir")).expect("config dir");
    std::fs::write(path, contents).expect("write config");
}

fn failover_error_message(prompt: &crate::provider::ProviderFailoverPrompt) -> String {
    format!(
        "[jcode-provider-failover]{}\nignored",
        serde_json::to_string(prompt).expect("serialize failover prompt")
    )
}

fn create_fast_test_app() -> App {
    let provider: Arc<dyn Provider> = Arc::new(FastMockProvider {
        service_tier: StdArc::new(StdMutex::new(None)),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new_for_test_harness(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app
}

fn create_gemini_test_app() -> App {
    struct GeminiMockProvider;

    #[async_trait::async_trait]
    impl Provider for GeminiMockProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[crate::message::ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> Result<crate::provider::EventStream> {
            unimplemented!("Mock provider")
        }

        fn name(&self) -> &str {
            "gemini"
        }

        fn model(&self) -> String {
            "gemini-2.5-pro".to_string()
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(GeminiMockProvider)
        }
    }

    let provider: Arc<dyn Provider> = Arc::new(GeminiMockProvider);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new_for_test_harness(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app
}
