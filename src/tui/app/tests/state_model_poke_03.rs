#[test]
fn test_model_picker_preview_arrow_keys_navigate() {
    let mut app = create_test_app();
    configure_test_remote_models(&mut app);

    // Type /model to open preview
    for c in "/model".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker preview should be open");
    assert!(picker.preview);
    let initial_selected = picker.selected;

    // Down arrow should navigate in preview mode
    app.handle_key(KeyCode::Down, KeyModifiers::empty())
        .unwrap();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("picker should still be open");
    assert!(picker.preview, "should remain in preview mode");
    assert_eq!(picker.selected, initial_selected + 1);

    // Up arrow should navigate back
    app.handle_key(KeyCode::Up, KeyModifiers::empty()).unwrap();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("picker should still be open");
    assert!(picker.preview, "should remain in preview mode");
    assert_eq!(picker.selected, initial_selected);

    // Input should be preserved
    assert_eq!(app.input(), "/model");
}

#[test]
fn test_open_model_picker_without_routes_shows_actionable_guidance() {
    let mut app = create_test_app();

    app.open_model_picker();
    wait_for_model_picker_load(&mut app);

    assert!(app.inline_interactive_state.is_none());
    assert_eq!(app.status_notice(), Some("No models available".to_string()));

    let last = app.display_messages.last().expect("display message");
    assert_eq!(last.role, "system");
    assert!(last.content.contains("/login"));
    assert!(last.content.contains("/account"));
    assert!(last.content.contains("/model"));
}

#[derive(Clone)]
struct CountingModelRoutesProvider {
    calls: StdArc<AtomicUsize>,
    route_count: usize,
    delay: Duration,
}

#[derive(Clone)]
struct MixedModelRoutesProvider {
    model: StdArc<StdMutex<String>>,
}

#[derive(Clone)]
struct AuthUxStateSpaceProvider {
    authed: StdArc<AtomicBool>,
    refreshes: StdArc<AtomicUsize>,
    model: StdArc<StdMutex<String>>,
    set_model_requests: StdArc<StdMutex<Vec<String>>>,
    provider_id: &'static str,
    provider_label: &'static str,
    models: &'static [&'static str],
    include_wrong_profile_first: bool,
    include_generic_profile_duplicate: bool,
}

#[derive(Clone)]
struct EmptyPostLoginCatalogProvider {
    refreshes: StdArc<AtomicUsize>,
    set_model_attempts: StdArc<AtomicUsize>,
}

#[derive(Clone)]
struct FailingPostLoginCatalogProvider {
    refreshes: StdArc<AtomicUsize>,
    set_model_attempts: StdArc<AtomicUsize>,
}

impl AuthUxStateSpaceProvider {
    fn routes(&self) -> Vec<crate::provider::ModelRoute> {
        let authed = self.authed.load(Ordering::SeqCst);
        let mut routes = Vec::new();
        if self.include_wrong_profile_first {
            routes.push(crate::provider::ModelRoute {
                model: "wrong-profile-first".to_string(),
                provider: self.provider_label.to_string(),
                api_method: "openai-compatible:other-provider".to_string(),
                available: authed,
                detail: if authed {
                    "fresh wrong-profile catalog route".to_string()
                } else {
                    "no API key".to_string()
                },
                cheapness: None,
            });
        }
        for model in self.models {
            routes.push(crate::provider::ModelRoute {
                model: (*model).to_string(),
                provider: self.provider_label.to_string(),
                api_method: format!("openai-compatible:{}", self.provider_id),
                available: authed,
                detail: if authed {
                    "fresh catalog route".to_string()
                } else {
                    "no API key".to_string()
                },
                cheapness: None,
            });
            if self.include_generic_profile_duplicate {
                routes.push(crate::provider::ModelRoute {
                    model: (*model).to_string(),
                    provider: self.provider_label.to_string(),
                    api_method: "openai-compatible".to_string(),
                    available: authed,
                    detail: if authed {
                        "duplicate generic direct route".to_string()
                    } else {
                        "no API key".to_string()
                    },
                    cheapness: None,
                });
            }
        }
        routes
    }
}

impl MixedModelRoutesProvider {
    fn routes() -> Vec<crate::provider::ModelRoute> {
        vec![
            crate::provider::ModelRoute {
                model: "gpt-5.5".to_string(),
                provider: "OpenAI".to_string(),
                api_method: "openai-oauth".to_string(),
                available: true,
                detail: String::new(),
                cheapness: None,
            },
            crate::provider::ModelRoute {
                model: "claude-opus-4-6".to_string(),
                provider: "Anthropic".to_string(),
                api_method: "claude-oauth".to_string(),
                available: true,
                detail: String::new(),
                cheapness: None,
            },
            crate::provider::ModelRoute {
                model: "Qwen/Qwen3-Coder-480B-A35B-Instruct".to_string(),
                provider: "Chutes".to_string(),
                api_method: "openai-compatible:chutes".to_string(),
                available: true,
                detail: "https://llm.chutes.ai/v1".to_string(),
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
        ]
    }
}

#[async_trait::async_trait]
impl Provider for AuthUxStateSpaceProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        unimplemented!("AuthUxStateSpaceProvider")
    }

    fn name(&self) -> &str {
        "openrouter"
    }

    fn model(&self) -> String {
        self.model.lock().unwrap().clone()
    }

    fn available_models_display(&self) -> Vec<String> {
        self.routes()
            .into_iter()
            .filter(|route| route.available)
            .map(|route| route.model)
            .collect()
    }

    fn model_routes(&self) -> Vec<crate::provider::ModelRoute> {
        self.routes()
    }

    fn set_model(&self, model: &str) -> Result<()> {
        self.set_model_requests
            .lock()
            .unwrap()
            .push(model.to_string());
        let model = model
            .strip_prefix(&format!("{}:", self.provider_id))
            .unwrap_or(model);
        let found = self
            .routes()
            .into_iter()
            .any(|route| route.available && route.model == model);
        if !found {
            anyhow::bail!("model {model} is not available in the refreshed catalog");
        }
        *self.model.lock().unwrap() = model.to_string();
        Ok(())
    }

    fn on_auth_changed(&self) {
        self.authed.store(true, Ordering::SeqCst);
    }

    async fn refresh_model_catalog(&self) -> Result<crate::provider::ModelCatalogRefreshSummary> {
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        Ok(crate::provider::ModelCatalogRefreshSummary {
            model_count_before: 0,
            model_count_after: 2,
            models_added: 2,
            models_removed: 0,
            models_added_names: Vec::new(),
            models_removed_names: Vec::new(),
            route_count_before: 0,
            route_count_after: 2,
            routes_added: 2,
            routes_removed: 0,
            routes_changed: 0,
        })
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

#[async_trait::async_trait]
impl Provider for MixedModelRoutesProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        unimplemented!("MixedModelRoutesProvider")
    }

    fn name(&self) -> &str {
        "mixed"
    }

    fn model(&self) -> String {
        self.model.lock().unwrap().clone()
    }

    fn available_models_display(&self) -> Vec<String> {
        Self::routes().into_iter().map(|route| route.model).collect()
    }

    fn model_routes(&self) -> Vec<crate::provider::ModelRoute> {
        Self::routes()
    }

    fn set_model(&self, model: &str) -> Result<()> {
        let model = model.strip_prefix("chutes:").unwrap_or(model);
        if !Self::routes().iter().any(|route| route.model == model) {
            anyhow::bail!("model {model} is not available in the mixed catalog");
        }
        *self.model.lock().unwrap() = model.to_string();
        Ok(())
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

#[async_trait::async_trait]
impl Provider for EmptyPostLoginCatalogProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        unimplemented!("EmptyPostLoginCatalogProvider")
    }

    fn name(&self) -> &str {
        "empty-catalog"
    }

    fn model(&self) -> String {
        "pre-auth-model".to_string()
    }

    fn model_routes(&self) -> Vec<crate::provider::ModelRoute> {
        vec![]
    }

    fn set_model(&self, model: &str) -> Result<()> {
        self.set_model_attempts.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("unexpected attempt to switch to {model}")
    }

    async fn refresh_model_catalog(&self) -> Result<crate::provider::ModelCatalogRefreshSummary> {
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        Ok(crate::provider::ModelCatalogRefreshSummary {
            model_count_before: 0,
            model_count_after: 0,
            models_added: 0,
            models_removed: 0,
            models_added_names: Vec::new(),
            models_removed_names: Vec::new(),
            route_count_before: 0,
            route_count_after: 0,
            routes_added: 0,
            routes_removed: 0,
            routes_changed: 0,
        })
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

#[async_trait::async_trait]
impl Provider for FailingPostLoginCatalogProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        unimplemented!("FailingPostLoginCatalogProvider")
    }

    fn name(&self) -> &str {
        "failing-catalog"
    }

    fn model(&self) -> String {
        "pre-auth-model".to_string()
    }

    fn model_routes(&self) -> Vec<crate::provider::ModelRoute> {
        vec![]
    }

    fn set_model(&self, model: &str) -> Result<()> {
        self.set_model_attempts.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("unexpected attempt to switch to {model}")
    }

    async fn refresh_model_catalog(&self) -> Result<crate::provider::ModelCatalogRefreshSummary> {
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("fixture refresh failed before server auth-change catalog refresh")
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

#[async_trait::async_trait]
impl Provider for CountingModelRoutesProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        unimplemented!("CountingModelRoutesProvider")
    }

    fn name(&self) -> &str {
        "counting"
    }

    fn model(&self) -> String {
        "counting-a".to_string()
    }

    fn model_routes(&self) -> Vec<crate::provider::ModelRoute> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
        (0..self.route_count)
            .map(|idx| crate::provider::ModelRoute {
                model: format!("counting-{}", (b'a' + idx as u8) as char),
                provider: "Counting".to_string(),
                api_method: "test".to_string(),
                available: true,
                detail: String::new(),
                cheapness: None,
            })
            .collect()
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

#[test]
fn test_model_picker_reuses_cached_entries_until_invalidated() {
    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let calls = StdArc::new(AtomicUsize::new(0));
    let provider: Arc<dyn Provider> = Arc::new(CountingModelRoutesProvider {
        calls: StdArc::clone(&calls),
        route_count: 2,
        delay: Duration::ZERO,
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new_for_test_harness(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;

    app.open_model_picker();
    wait_for_model_picker_load(&mut app);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(app.model_picker_cache.is_some());

    app.open_model_picker();
    wait_for_model_picker_load(&mut app);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "second open should reuse cached picker entries"
    );

    app.invalidate_model_picker_cache();
    app.open_model_picker();
    wait_for_model_picker_load(&mut app);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "invalidating should force rebuilding provider routes"
    );
}

#[test]
fn test_tui_api_key_auth_refreshes_catalog_shows_diff_without_opening_picker() {
    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let provider = AuthUxStateSpaceProvider {
        authed: StdArc::new(AtomicBool::new(false)),
        refreshes: StdArc::new(AtomicUsize::new(0)),
        model: StdArc::new(StdMutex::new("pre-auth-model".to_string())),
        set_model_requests: StdArc::new(StdMutex::new(Vec::new())),
        provider_id: "state-space",
        provider_label: "StateSpace",
        models: &["state-space-alpha", "state-space-beta"],
        include_wrong_profile_first: true,
        include_generic_profile_duplicate: false,
    };
    let refreshes = provider.refreshes.clone();
    let provider: Arc<dyn Provider> = Arc::new(provider);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new_for_test_harness(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;

    let mut bus_rx = crate::bus::Bus::global().subscribe();
    while bus_rx.try_recv().is_ok() {}

    let _guard = rt.enter();
    app.start_openai_compatible_post_login_activation(
        "state-space".to_string(),
        "StateSpace".to_string(),
    );
    assert_eq!(
        app.status_notice(),
        Some("StateSpace: fetching models...".to_string())
    );
    assert!(
        app.inline_interactive_state.is_none(),
        "auth-triggered discovery should not open /model automatically"
    );

    let activation = rt.block_on(async {
        loop {
            match tokio::time::timeout(Duration::from_secs(2), bus_rx.recv()).await {
                Ok(Ok(event @ crate::bus::BusEvent::ProviderModelActivated { .. })) => break event,
                Ok(Ok(_)) => continue,
                other => panic!("expected ProviderModelActivated event, got {other:?}"),
            }
        }
    });
    assert_eq!(
        refreshes.load(Ordering::SeqCst),
        1,
        "auth completion must refresh the model catalog exactly once"
    );

    super::local::handle_bus_event(&mut app, Ok(activation));
    assert!(
        app.inline_interactive_state.is_none(),
        "activation completion should still not open /model automatically"
    );
    assert_eq!(app.session.model.as_deref(), Some("state-space-alpha"));
    let last = app.display_messages.last().expect("activation message");
    assert!(last.content.contains("Added models:"));
    assert!(last.content.contains("`state-space-alpha`"));
    assert!(last.content.contains("`state-space-beta`"));
    assert!(last.content.contains("Use `/model`"));
    assert!(!last.content.contains("model picker is open"));

    assert!(super::model_context::handle_model_command(
        &mut app,
        "/model state-space-beta"
    ));
    assert_eq!(app.session.model.as_deref(), Some("state-space-beta"));
    assert_eq!(
        app.status_notice(),
        Some("Model → state-space-beta".to_string())
    );
}

#[test]
fn test_tui_cerebras_paste_key_lifecycle_has_no_degraded_success_messages() {
    let _env_lock = crate::storage::lock_test_env();
    let _guard = AzureLoginEnvGuard::save(&[
        "CEREBRAS_API_KEY",
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_OPENROUTER_PROVIDER_FEATURES",
        "JCODE_OPENROUTER_MODEL_CATALOG",
        "JCODE_OPENROUTER_AUTH_HEADER",
        "JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER",
        "JCODE_RUNTIME_PROVIDER",
        "JCODE_ACTIVE_PROVIDER",
        "JCODE_FORCE_PROVIDER",
    ]);
    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let fake_provider = AuthUxStateSpaceProvider {
        authed: StdArc::new(AtomicBool::new(false)),
        refreshes: StdArc::new(AtomicUsize::new(0)),
        model: StdArc::new(StdMutex::new("gpt-5.5".to_string())),
        set_model_requests: StdArc::new(StdMutex::new(Vec::new())),
        provider_id: "cerebras",
        provider_label: "Cerebras",
        models: &["qwen-3-235b-a22b-instruct-2507", "llama3.1-8b"],
        include_wrong_profile_first: true,
        include_generic_profile_duplicate: true,
    };
    let refreshes = fake_provider.refreshes.clone();
    let set_model_requests = fake_provider.set_model_requests.clone();
    let provider: Arc<dyn Provider> = Arc::new(fake_provider);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new_for_test_harness(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;

    let mut bus_rx = crate::bus::Bus::global().subscribe();
    while bus_rx.try_recv().is_ok() {}

    app.start_login_provider(
        crate::provider_catalog::resolve_login_provider("cerebras")
            .expect("Cerebras login provider"),
    );

    let prompt = app.display_messages.last().expect("login prompt").content.clone();
    assert!(prompt.contains("**Cerebras API Key**"), "{prompt}");
    assert!(
        prompt.contains("Stored variable: `CEREBRAS_API_KEY`"),
        "{prompt}"
    );
    assert!(
        prompt.contains("Endpoint: `https://api.cerebras.ai/v1`"),
        "{prompt}"
    );
    assert!(
        prompt.contains("Suggested default model: `qwen-3-235b-a22b-instruct-2507`"),
        "{prompt}"
    );
    assert!(prompt.contains("**Paste your API key below**"), "{prompt}");

    let pending = app.pending_login.take().expect("pending Cerebras key login");
    let _runtime_guard = rt.enter();
    app.handle_login_input(pending, "test-cerebras-key".to_string());

    let mut saw_saved = false;
    let mut saw_catalog_started = false;
    let mut saw_activation = false;
    let mut login_success_events = 0;
    let mut login_failure_events = 0;
    let mut catalog_warning_events = 0;
    let mut activation_events = 0;
    rt.block_on(async {
        while !(saw_saved && saw_catalog_started && saw_activation) {
            match tokio::time::timeout(Duration::from_secs(2), bus_rx.recv()).await {
                Ok(Ok(crate::bus::BusEvent::LoginCompleted(login))) => {
                    if login.success {
                        login_success_events += 1;
                    } else {
                        login_failure_events += 1;
                    }
                    assert!(login.success, "unexpected failed login event: {login:?}");
                    assert_eq!(login.provider, "Cerebras");
                    assert!(login.message.contains("**Cerebras API key saved.**"));
                    assert!(
                        login
                            .message
                            .contains("Stored at `~/.config/jcode/cerebras.env`.")
                    );
                    assert!(login.message.contains("Fetching models now."));
                    assert!(!login.message.contains("did not switch models"));
                    app.handle_login_completed(login);
                    saw_saved = true;
                }
                Ok(Ok(crate::bus::BusEvent::UiActivity(activity))) => {
                    if activity.message.contains("Auth Model Catalog Warning") {
                        catalog_warning_events += 1;
                    }
                    assert!(
                        !activity.message.contains("Auth Model Catalog Warning"),
                        "unexpected warning activity: {}",
                        activity.message
                    );
                    assert!(
                        !activity.message.contains("did not switch models"),
                        "unexpected degraded activity: {}",
                        activity.message
                    );
                    if activity.message.contains("Model Discovery Started") {
                        saw_catalog_started = true;
                    }
                    super::local::handle_bus_event(
                        &mut app,
                        Ok(crate::bus::BusEvent::UiActivity(activity)),
                    );
                }
                Ok(Ok(event @ crate::bus::BusEvent::ProviderModelActivated { .. })) => {
                    activation_events += 1;
                    if let crate::bus::BusEvent::ProviderModelActivated { model, message, .. } =
                        &event
                    {
                        assert_eq!(model, "qwen-3-235b-a22b-instruct-2507");
                        assert!(message.contains("**Cerebras is ready.**"), "{message}");
                        assert!(!message.contains("wrong-profile-first"), "{message}");
                    }
                    super::local::handle_bus_event(&mut app, Ok(event));
                    saw_activation = true;
                }
                Ok(Ok(_)) => {}
                other => panic!("expected local Cerebras auth lifecycle event, got {other:?}"),
            }
        }
    });

    while let Ok(event) = bus_rx.try_recv() {
        match event {
            crate::bus::BusEvent::LoginCompleted(login) => {
                if login.success {
                    login_success_events += 1;
                } else {
                    panic!("late failed login event after successful auth: {login:?}");
                }
            }
            crate::bus::BusEvent::UiActivity(activity) => {
                if activity.message.contains("Auth Model Catalog Warning") {
                    panic!("late warning activity after successful auth: {}", activity.message);
                }
                assert!(
                    !activity.message.contains("did not switch models"),
                    "late degraded activity after successful auth: {}",
                    activity.message
                );
            }
            crate::bus::BusEvent::ProviderModelActivated { model, message, .. } => {
                activation_events += 1;
                assert_eq!(model, "qwen-3-235b-a22b-instruct-2507");
                assert!(message.contains("**Cerebras is ready.**"), "{message}");
            }
            _ => {}
        }
    }

    assert_eq!(refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(login_success_events, 1, "expected exactly one successful login event");
    assert_eq!(login_failure_events, 0, "happy auth must not publish failed login events");
    assert_eq!(catalog_warning_events, 0, "happy auth must not publish catalog warnings");
    assert_eq!(activation_events, 1, "expected exactly one provider activation event");
    assert_eq!(
        app.session.model.as_deref(),
        Some("qwen-3-235b-a22b-instruct-2507")
    );
    let transcript = app
        .display_messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    for forbidden in [
        "Auth Model Catalog Warning",
        "did not switch models",
        "contained no selectable",
        "Saved the API key and fetched the model catalog, but",
        "Login: Cerebras failed",
        "wrong-profile-first",
    ] {
        assert!(
            !transcript.contains(forbidden),
            "transcript contained forbidden degraded-success marker `{forbidden}`:\n{transcript}"
        );
    }

    set_model_requests.lock().unwrap().clear();
    app.open_model_picker();
    wait_for_model_picker_load(&mut app);
    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should open after Cerebras auth");
    let qwen_entry = picker
        .entries
        .iter()
        .find(|entry| entry.name == "qwen-3-235b-a22b-instruct-2507")
        .expect("selected Cerebras model should be visible in /model");
    assert_eq!(
        picker
            .entries
            .iter()
            .filter(|entry| entry.name == "qwen-3-235b-a22b-instruct-2507")
            .count(),
        1,
        "Cerebras model picker should not show duplicate rows for the selected model"
    );
    assert!(qwen_entry.options.iter().any(|route| {
        route.provider == "Cerebras"
            && route.api_method == "openai-compatible:cerebras"
            && route.available
    }));
    assert!(
        !qwen_entry
            .options
            .iter()
            .any(|route| route.api_method == "openai-compatible"),
        "generic direct route should be de-duplicated in favor of the Cerebras profile route"
    );
    let llama_idx = picker
        .entries
        .iter()
        .position(|entry| entry.name == "llama3.1-8b")
        .expect("alternate Cerebras model should be visible in /model");
    let llama_entry = &picker.entries[llama_idx];
    assert_eq!(
        picker
            .entries
            .iter()
            .filter(|entry| entry.name == "llama3.1-8b")
            .count(),
        1,
        "Cerebras model picker should not show duplicate rows for alternate models"
    );
    assert!(llama_entry.options.iter().any(|route| {
        route.provider == "Cerebras"
            && route.api_method == "openai-compatible:cerebras"
            && route.available
    }));
    assert!(
        !llama_entry
            .options
            .iter()
            .any(|route| route.api_method == "openai-compatible"),
        "generic direct route should be de-duplicated in favor of the Cerebras profile route"
    );
    let filtered_pos = picker
        .filtered
        .iter()
        .position(|&idx| idx == llama_idx)
        .expect("alternate Cerebras model should be selectable in filtered picker list");

    app.inline_interactive_state.as_mut().unwrap().selected = filtered_pos;
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .expect("Cerebras picker selection should switch models");

    assert_eq!(app.session.model.as_deref(), Some("llama3.1-8b"));
    assert_eq!(app.provider.model(), "llama3.1-8b");
    assert_eq!(
        set_model_requests.lock().unwrap().as_slice(),
        ["cerebras:llama3.1-8b"],
        "model picker must route post-auth switches through the authenticated Cerebras profile"
    );
}

#[test]
fn test_tui_openai_compatible_empty_catalog_does_not_switch_to_profile_default() {
    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let refreshes = StdArc::new(AtomicUsize::new(0));
    let set_model_attempts = StdArc::new(AtomicUsize::new(0));
    let provider: Arc<dyn Provider> = Arc::new(EmptyPostLoginCatalogProvider {
        refreshes: StdArc::clone(&refreshes),
        set_model_attempts: StdArc::clone(&set_model_attempts),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new_for_test_harness(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;

    let mut bus_rx = crate::bus::Bus::global().subscribe();
    while bus_rx.try_recv().is_ok() {}

    let _guard = rt.enter();
    app.start_openai_compatible_post_login_activation(
        "cerebras".to_string(),
        "Cerebras".to_string(),
    );

    let activity = rt.block_on(async {
        loop {
            match tokio::time::timeout(Duration::from_secs(2), bus_rx.recv()).await {
                Ok(Ok(crate::bus::BusEvent::ProviderModelActivated { .. })) => {
                    panic!("empty catalog must not activate a provider model")
                }
                Ok(Ok(crate::bus::BusEvent::LoginCompleted(login))) => {
                    panic!("empty local catalog must not publish final login failure: {login:?}")
                }
                Ok(Ok(crate::bus::BusEvent::UiActivity(activity)))
                    if activity.message.contains("Model Discovery Still Updating") =>
                {
                    break activity;
                }
                Ok(Ok(_)) => continue,
                other => panic!("expected pending catalog activity, got {other:?}"),
            }
        }
    });

    assert_eq!(refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(
        set_model_attempts.load(Ordering::SeqCst),
        0,
        "post-login activation must not try the metadata default when the catalog has no selectable route"
    );
    assert!(activity.message.contains("Saved credentials are active"));
    assert!(activity.message.contains("Jcode is still processing"));
    assert!(!activity.message.contains("did not switch models"));
    assert!(!activity.message.contains("documented default"));
    assert!(!activity.message.contains("qwen-3-coder-480b"));
}

#[test]
fn test_tui_openai_compatible_local_refresh_failure_is_pending_not_final_failure() {
    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let refreshes = StdArc::new(AtomicUsize::new(0));
    let set_model_attempts = StdArc::new(AtomicUsize::new(0));
    let provider: Arc<dyn Provider> = Arc::new(FailingPostLoginCatalogProvider {
        refreshes: StdArc::clone(&refreshes),
        set_model_attempts: StdArc::clone(&set_model_attempts),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new_for_test_harness(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;

    let mut bus_rx = crate::bus::Bus::global().subscribe();
    while bus_rx.try_recv().is_ok() {}

    let _guard = rt.enter();
    app.start_openai_compatible_post_login_activation(
        "cerebras".to_string(),
        "Cerebras".to_string(),
    );

    let activity = rt.block_on(async {
        loop {
            match tokio::time::timeout(Duration::from_secs(2), bus_rx.recv()).await {
                Ok(Ok(crate::bus::BusEvent::ProviderModelActivated { .. })) => {
                    panic!("failing local refresh must not activate a provider model")
                }
                Ok(Ok(crate::bus::BusEvent::LoginCompleted(login))) => {
                    panic!(
                        "local refresh failure must not publish a final login failure while server auth-change recovery can still finish: {login:?}"
                    )
                }
                Ok(Ok(crate::bus::BusEvent::UiActivity(activity)))
                    if activity.message.contains("Model Discovery Still Updating") =>
                {
                    break activity;
                }
                Ok(Ok(_)) => continue,
                other => panic!("expected pending catalog activity, got {other:?}"),
            }
        }
    });

    assert_eq!(refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(
        set_model_attempts.load(Ordering::SeqCst),
        0,
        "local refresh failure must not try to switch models from an unavailable catalog"
    );
    assert!(activity.message.contains("Saved credentials are active"));
    assert!(activity.message.contains("server auth-change catalog refresh"));
    assert!(activity.message.contains("fixture refresh failed"));
    assert!(!activity.message.contains("Login: failed"));
    assert!(!activity.message.contains("Unable to sign in"));
    assert!(!activity.message.contains("did not switch models"));
}

#[test]
fn test_model_picker_opens_loading_state_before_async_routes_complete() {
    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let calls = StdArc::new(AtomicUsize::new(0));
    let provider: Arc<dyn Provider> = Arc::new(CountingModelRoutesProvider {
        calls: StdArc::clone(&calls),
        route_count: 2,
        delay: Duration::from_millis(75),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new_for_test_harness(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;

    app.open_model_picker();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("loading picker should open immediately");
    assert_eq!(picker.entries.len(), 1);
    assert_eq!(picker.entries[0].name, "counting-a");
    assert!(
        picker.entries[0].options[0]
            .detail
            .contains("updating model list")
    );
    assert!(app.pending_model_picker_load.is_some());
    assert_eq!(
        app.status_notice(),
        Some("Updating model list…".to_string())
    );

    wait_for_model_picker_load(&mut app);
    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("hydrated picker should still be open");
    assert!(picker.entries.len() >= 2);
    assert_eq!(app.status_notice(), Some("Model list updated".to_string()));
}

#[test]
fn test_model_picker_state_space_preserves_provider_labels_after_route_hydration() {
    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let provider: Arc<dyn Provider> = Arc::new(MixedModelRoutesProvider {
        model: StdArc::new(StdMutex::new("gpt-5.5".to_string())),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new_for_test_harness(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app.recent_authenticated_provider = Some(("chutes".to_string(), Instant::now()));

    app.open_model_picker();
    wait_for_model_picker_load(&mut app);

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("hydrated mixed-provider model picker should be open");
    let mut routes_by_model = std::collections::BTreeMap::new();
    for entry in &picker.entries {
        let route = entry
            .active_option()
            .expect("model picker entry should have an active route");
        routes_by_model.insert(
            entry.name.clone(),
            (route.provider.clone(), route.api_method.clone()),
        );
    }

    assert_eq!(
        routes_by_model.get("gpt-5.5"),
        Some(&("OpenAI".to_string(), "openai-oauth".to_string()))
    );
    assert_eq!(
        routes_by_model.get("claude-opus-4-6"),
        Some(&("Anthropic".to_string(), "claude-oauth".to_string()))
    );
    assert_eq!(
        routes_by_model.get("Qwen/Qwen3-Coder-480B-A35B-Instruct"),
        Some(&(
            "Chutes".to_string(),
            "openai-compatible:chutes".to_string()
        ))
    );
    assert_eq!(
        routes_by_model.get("deepseek/deepseek-v4-pro"),
        Some(&("auto".to_string(), "openrouter".to_string()))
    );

    let chutes_rows = picker
        .entries
        .iter()
        .filter(|entry| {
            entry
                .active_option()
                .map(|route| route.provider == "Chutes")
                .unwrap_or(false)
        })
        .count();
    assert_eq!(
        chutes_rows, 1,
        "opening the model list must not collapse every route to the recently authenticated direct provider: {:?}",
        routes_by_model
    );
}

#[test]
fn test_model_picker_does_not_cache_single_model_fallback() {
    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let calls = StdArc::new(AtomicUsize::new(0));
    let provider: Arc<dyn Provider> = Arc::new(CountingModelRoutesProvider {
        calls: StdArc::clone(&calls),
        route_count: 1,
        delay: Duration::ZERO,
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new_for_test_harness(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;

    app.open_model_picker();
    wait_for_model_picker_load(&mut app);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        app.model_picker_cache.is_none(),
        "single-model fallback results should not be retained"
    );

    app.open_model_picker();
    wait_for_model_picker_load(&mut app);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "single-model fallback should be rebuilt so a later full catalog can surface"
    );
}

#[test]
fn test_local_model_picker_selection_failure_keeps_picker_open_and_shows_next_steps() {
    let mut app = create_failing_model_switch_test_app();

    app.open_model_picker();
    wait_for_model_picker_load(&mut app);
    assert!(app.inline_interactive_state.is_some());

    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .expect("enter should be handled");

    assert!(
        app.inline_interactive_state.is_some(),
        "picker should remain open so the user can choose another model"
    );
    assert_eq!(app.status_notice(), Some("Model switch failed".to_string()));

    let last = app.display_messages.last().expect("display message");
    assert_eq!(last.role, "error");
    assert!(last.content.contains("credentials expired"));
    assert!(last.content.contains("/model"));
    assert!(last.content.contains("/login"));
    assert!(last.content.contains("/account"));
}

#[test]
fn test_login_completed_spawns_auth_refresh_when_runtime_is_available() {
    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let started = StdArc::new(AtomicBool::new(false));
    let completed = StdArc::new(AtomicBool::new(false));
    let provider: Arc<dyn Provider> = Arc::new(AsyncAuthRefreshingMockProvider {
        started: StdArc::clone(&started),
        completed: StdArc::clone(&completed),
        delay: Duration::from_millis(150),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new_for_test_harness(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;

    let _guard = rt.enter();
    let start = Instant::now();
    app.handle_login_completed(crate::bus::LoginCompleted {
        provider: "openrouter".to_string(),
        success: true,
        message: "OpenRouter ready".to_string(),
    });
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_millis(100),
        "login completion should not block on auth refresh, took {:?}",
        elapsed
    );

    let wait_start = Instant::now();
    while !started.load(Ordering::SeqCst) || !completed.load(Ordering::SeqCst) {
        assert!(
            wait_start.elapsed() < Duration::from_secs(2),
            "background auth refresh did not complete"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn test_login_completed_surfaces_new_provider_models_in_local_model_picker() {
    let mut app = create_auth_refresh_test_app();

    app.handle_login_completed(crate::bus::LoginCompleted {
        provider: "copilot".to_string(),
        success: true,
        message: "Authenticated as **octocat** via GitHub Copilot.\n\nCopilot models are now available in `/model`."
            .to_string(),
    });

    app.open_model_picker();
    wait_for_model_picker_load(&mut app);

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");

    let copilot_entry = picker
        .entries
        .iter()
        .find(|entry| entry.name == "claude-opus-4.6")
        .expect("copilot model should be shown after login");

    assert!(
        picker
            .entries
            .iter()
            .any(|entry| entry.name == "grok-code-fast-1"),
        "all newly available Copilot models should appear in /model"
    );
    assert!(copilot_entry.options.iter().any(|route| {
        route.provider == "Copilot" && route.api_method == "copilot" && route.available
    }));

    assert!(
        picker.entries[0]
            .options
            .iter()
            .any(|route| route.provider == "Copilot" && route.detail.contains("recently added")),
        "recently authenticated provider should be prioritized and marked in /model"
    );
}

#[derive(Clone)]
struct AzureLoginMockProvider {
    model: StdArc<StdMutex<String>>,
    auth_changed: StdArc<AtomicUsize>,
    complete_calls: StdArc<AtomicUsize>,
}

#[async_trait::async_trait]
impl Provider for AzureLoginMockProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        self.complete_calls.fetch_add(1, Ordering::SeqCst);
        let stream = futures::stream::empty::<Result<crate::message::StreamEvent>>();
        Ok(Box::pin(stream) as crate::provider::EventStream)
    }

    fn name(&self) -> &str {
        "OpenRouter"
    }

    fn model(&self) -> String {
        self.model.lock().unwrap().clone()
    }

    fn set_model(&self, model: &str) -> Result<()> {
        let model = model
            .trim()
            .strip_prefix("openrouter:")
            .unwrap_or_else(|| model.trim())
            .trim();
        if model.is_empty() {
            anyhow::bail!("model cannot be empty");
        }
        *self.model.lock().unwrap() = model.to_string();
        Ok(())
    }

    fn available_models_display(&self) -> Vec<String> {
        vec![self.model()]
    }

    fn model_routes(&self) -> Vec<crate::provider::ModelRoute> {
        vec![crate::provider::ModelRoute {
            model: self.model(),
            provider: "Azure OpenAI".to_string(),
            api_method: "openai-compatible".to_string(),
            available: true,
            detail: String::new(),
            cheapness: None,
        }]
    }

    fn on_auth_changed(&self) {
        self.auth_changed.fetch_add(1, Ordering::SeqCst);
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

struct AzureLoginEnvGuard {
    saved: Vec<(&'static str, Option<String>)>,
}

impl AzureLoginEnvGuard {
    fn save(keys: &[&'static str]) -> Self {
        let saved = keys
            .iter()
            .map(|key| (*key, std::env::var(key).ok()))
            .collect();
        for key in keys {
            crate::env::remove_var(key);
        }
        Self { saved }
    }
}

impl Drop for AzureLoginEnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..) {
            if let Some(value) = value {
                crate::env::set_var(key, value);
            } else {
                crate::env::remove_var(key);
            }
        }
    }
}

#[test]
fn test_azure_login_completion_switches_local_model_without_completion() {
    let _env_lock = crate::storage::lock_test_env();
    let _guard = AzureLoginEnvGuard::save(&[
        "AZURE_OPENAI_ENDPOINT",
        "AZURE_OPENAI_MODEL",
        "AZURE_OPENAI_API_KEY",
        "AZURE_OPENAI_USE_ENTRA",
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_OPENROUTER_PROVIDER_FEATURES",
        "JCODE_OPENROUTER_MODEL_CATALOG",
        "JCODE_OPENROUTER_AUTH_HEADER",
        "JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER",
        "JCODE_OPENROUTER_MODEL",
        "JCODE_RUNTIME_PROVIDER",
        "JCODE_ACTIVE_PROVIDER",
        "JCODE_FORCE_PROVIDER",
    ]);
    crate::env::set_var("AZURE_OPENAI_ENDPOINT", "https://example.openai.azure.com");
    crate::env::set_var("AZURE_OPENAI_MODEL", "azure-deployment");
    crate::env::set_var("AZURE_OPENAI_API_KEY", "test-key");
    crate::env::set_var("AZURE_OPENAI_USE_ENTRA", "0");

    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let model = StdArc::new(StdMutex::new("old-model".to_string()));
    let auth_changed = StdArc::new(AtomicUsize::new(0));
    let complete_calls = StdArc::new(AtomicUsize::new(0));
    let provider: Arc<dyn Provider> = Arc::new(AzureLoginMockProvider {
        model: StdArc::clone(&model),
        auth_changed: StdArc::clone(&auth_changed),
        complete_calls: StdArc::clone(&complete_calls),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new_for_test_harness(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app.provider_session_id = Some("stale-upstream".to_string());
    app.session.provider_session_id = Some("stale-upstream".to_string());
    app.session.model = Some("old-model".to_string());

    app.handle_login_completed(crate::bus::LoginCompleted {
        provider: "Azure OpenAI".to_string(),
        success: true,
        message: "Azure OpenAI ready".to_string(),
    });

    assert_eq!(&*model.lock().unwrap(), "azure-deployment");
    assert_eq!(app.session.model.as_deref(), Some("azure-deployment"));
    assert_eq!(app.provider_session_id, None);
    assert_eq!(app.session.provider_session_id, None);
    assert_eq!(auth_changed.load(Ordering::SeqCst), 1);
    assert_eq!(complete_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        std::env::var("JCODE_RUNTIME_PROVIDER").as_deref(),
        Ok("azure-openai")
    );
    assert_eq!(
        app.status_notice(),
        Some("Login: Azure OpenAI ready (azure-deployment)".to_string())
    );
}

#[test]
fn test_local_model_picker_surfaces_antigravity_models_from_multiprovider() {
    let mut app = create_antigravity_picker_test_app();
    app.open_model_picker();
    wait_for_model_picker_load(&mut app);

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");

    let antigravity_entry = picker
        .entries
        .iter()
        .find(|entry| entry.name == "claude-sonnet-4-6")
        .expect("antigravity model should be shown after login");

    assert!(antigravity_entry.options.iter().any(|route| {
        route.provider == "Antigravity" && route.api_method == "cli" && route.available
    }));
}

#[test]
fn test_local_antigravity_model_picker_selection_preserves_antigravity_provider() {
    let mut app = create_antigravity_picker_test_app();
    app.open_model_picker();
    wait_for_model_picker_load(&mut app);

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");

    let model_idx = picker
        .entries
        .iter()
        .position(|entry| entry.name == "claude-sonnet-4-6")
        .expect("antigravity model should be in picker");
    let filtered_pos = picker
        .filtered
        .iter()
        .position(|&i| i == model_idx)
        .expect("antigravity model should be in filtered list");

    app.inline_interactive_state.as_mut().unwrap().selected = filtered_pos;
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    assert_eq!(app.provider.name(), "Antigravity");
    assert_eq!(app.provider.model(), "claude-sonnet-4-6");
    assert!(app.inline_interactive_state.is_none());
}

#[test]
fn test_local_model_picker_openrouter_bare_openai_route_uses_openai_catalog_prefix() {
    let (mut app, set_model_calls) = create_openrouter_spec_capture_test_app();
    app.open_model_picker();
    wait_for_model_picker_load(&mut app);

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");
    let model_idx = picker
        .entries
        .iter()
        .position(|entry| entry.name == "gpt-5.4 (high)")
        .expect("openrouter-backed OpenAI effort entry should be in picker");
    let filtered_pos = picker
        .filtered
        .iter()
        .position(|&i| i == model_idx)
        .expect("entry should be in filtered list");

    app.inline_interactive_state.as_mut().unwrap().selected = filtered_pos;
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .expect("model picker selection should succeed");

    assert_eq!(
        set_model_calls.lock().unwrap().as_slice(),
        ["openai/gpt-5.4@OpenAI"]
    );
}

#[test]
fn test_agent_model_picker_openrouter_bare_openai_route_saves_openai_catalog_prefix() {
    let (mut app, _set_model_calls) = create_openrouter_spec_capture_test_app();

    app.open_agent_model_picker(crate::tui::AgentModelTarget::Swarm);

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("agent model picker should be open");
    let model_idx = picker
        .entries
        .iter()
        .position(|entry| entry.name == "gpt-5.4 (high)")
        .expect("openrouter-backed OpenAI effort entry should be in picker");
    let filtered_pos = picker
        .filtered
        .iter()
        .position(|&i| i == model_idx)
        .expect("entry should be in filtered list");

    app.inline_interactive_state.as_mut().unwrap().selected = filtered_pos;
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .expect("agent model picker selection should succeed");

    let last = app.display_messages.last().expect("display message");
    assert_eq!(last.role, "system");
    assert!(
        last.content.contains("`openai/gpt-5.4@OpenAI`"),
        "message should show normalized saved spec, got: {}",
        last.content
    );
}

#[test]
fn test_local_model_picker_render_shows_antigravity_models_exactly_as_user_sees_them() {
    let mut app = create_antigravity_picker_test_app();
    let text = render_model_picker_text(&mut app, 90, 12);

    assert!(
        text.contains("MODEL") && text.contains("PROVIDER") && text.contains("METHOD"),
        "rendered /model view should include picker columns, got:
{}",
        text
    );
    assert!(
        text.contains("claude-sonnet-4-6"),
        "rendered /model view should show the Antigravity Claude row, got:
{}",
        text
    );
    assert!(
        text.contains("gpt-oss-120b-medium"),
        "rendered /model view should show the Antigravity GPT row, got:
{}",
        text
    );
    assert!(
        text.contains("Antigravity"),
        "rendered /model view should show the Antigravity provider column, got:
{}",
        text
    );
    assert!(
        text.contains("cli"),
        "rendered /model view should show the route transport column, got:
{}",
        text
    );
}

#[test]
fn test_login_smoke_model_picker_renders_unstacked_provider_rows() {
    let mut app = create_login_smoke_model_app();
    let text = render_model_picker_text(&mut app, 110, 18);

    assert!(
        text.contains("MODEL") && text.contains("PROVIDER") && text.contains("METHOD"),
        "rendered /model view should include user-visible picker columns, got:\n{}",
        text
    );
    assert!(
        text.contains("gpt-5.4")
            && text.contains("OpenAI")
            && text.contains("oauth")
            && text.contains("api key"),
        "OpenAI OAuth and API-key routes should be separately visible, got:\n{}",
        text
    );
    let glm_row = text
        .lines()
        .find(|line| line.contains("glm-51-nvfp4"))
        .unwrap_or("");
    assert!(
        glm_row.contains("Comtegra GPU Cloud") && glm_row.contains("api key") && !glm_row.contains("copilot"),
        "Comtegra GLM row should show its provider and API-key method, got row `{}` in:\n{}",
        glm_row,
        text
    );
    assert!(
        text.contains("glm-51-nvfp4")
            && text.contains("Comtegra GPU Cloud")
            && text.contains("new"),
        "Comtegra login route should be visible and marked new, got:\n{}",
        text
    );
    assert!(
        text.contains("claude-opus-4.6") && text.contains("Copilot"),
        "Copilot route should be visible, got:\n{}",
        text
    );
    assert!(
        text.contains("deepseek/deepseek-v4-pro") && text.contains("openrouter"),
        "OpenRouter route should be visible, got:\n{}",
        text
    );
    let deepseek_auto_row = text
        .lines()
        .find(|line| line.contains("deepseek/deepseek-v4-pro") && line.contains("auto"))
        .unwrap_or("");
    let deepseek_provider_row = text
        .lines()
        .find(|line| line.contains("deepseek/deepseek-v4-pro") && line.contains("DeepSeek"))
        .unwrap_or("");
    assert!(
        deepseek_auto_row.contains('★'),
        "OpenRouter auto route should carry the recommended marker, got row `{}` in:\n{}",
        deepseek_auto_row,
        text
    );
    assert!(
        !deepseek_provider_row.contains('★'),
        "OpenRouter provider-specific routes should not carry the recommended marker, got row `{}` in:\n{}",
        deepseek_provider_row,
        text
    );
    let kimi25_row = text
        .lines()
        .find(|line| line.contains("moonshotai/kimi-k2.5"))
        .unwrap_or("");
    assert!(
        !kimi25_row.contains('★'),
        "Kimi K2.5 should not be recommended, got row `{}` in:\n{}",
        kimi25_row,
        text
    );
    assert!(
        text.contains("openai/gpt-5.5") && text.contains("OpenRouter/OpenAI"),
        "OpenRouter endpoint routes should not look like native OpenAI API-key rows, got:\n{}",
        text
    );
    assert!(
        !text.contains("(2)"),
        "provider routes should not be hidden behind stacked option counts, got:\n{}",
        text
    );
}

#[test]
fn test_model_picker_filter_text_includes_provider_and_method() {
    let entry = crate::tui::PickerEntry {
        name: "glm-51-nvfp4".to_string(),
        options: vec![crate::tui::PickerOption {
            provider: "Comtegra GPU Cloud".to_string(),
            api_method: "openai-compatible:comtegra".to_string(),
            available: true,
            detail: "https://llm.comtegra.cloud/v1".to_string(),
            estimated_reference_cost_micros: None,
        }],
        action: crate::tui::PickerAction::Model,
        selected_option: 0,
        is_current: false,
        is_default: false,
        recommended: false,
        recommendation_rank: usize::MAX,
        old: false,
        created_date: None,
        effort: None,
    };

    let filter_text = crate::tui::PickerKind::Model.filter_text(&entry);
    assert!(filter_text.contains("glm-51-nvfp4"));
    assert!(filter_text.contains("Comtegra GPU Cloud"));
    assert!(filter_text.contains("openai-compatible:comtegra"));
}

#[test]
fn test_login_picker_preview_stays_open_and_updates_filter() {
    let mut app = create_test_app();

    for c in "/login za".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("login picker preview should be open");
    assert!(picker.preview);
    assert_eq!(picker.kind, crate::tui::PickerKind::Login);
    assert_eq!(picker.filter, "za");
    assert!(
        picker
            .filtered
            .iter()
            .any(|&i| picker.entries[i].name == "Z.AI")
    );
    assert_eq!(app.input(), "/login za");
}

#[test]
fn test_login_picker_preview_enter_starts_login_flow() {
    let mut app = create_test_app();

    for c in "/login zai".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    assert!(app.inline_interactive_state.is_none());
    match app.pending_login {
        Some(crate::tui::app::auth::PendingLogin::ApiKeyProfile {
            provider,
            openai_compatible_profile: Some(profile),
            ..
        }) => {
            assert_eq!(provider, "Z.AI");
            assert_eq!(profile.id, crate::provider_catalog::ZAI_PROFILE.id);
        }
        ref other => panic!("unexpected pending login state: {other:?}"),
    }
}

#[test]
fn test_subagent_model_command_sets_and_resets_session_preference() {
    let mut app = create_test_app();

    assert!(super::commands::handle_session_command(
        &mut app,
        "/subagent-model gpt-5.4"
    ));
    assert_eq!(app.session.subagent_model.as_deref(), Some("gpt-5.4"));

    assert!(super::commands::handle_session_command(
        &mut app,
        "/subagent-model inherit"
    ));
    assert_eq!(app.session.subagent_model, None);
}

#[test]
fn test_autoreview_command_toggles_session_preference() {
    let mut app = create_test_app();

    assert!(super::commands::handle_session_command(
        &mut app,
        "/autoreview on"
    ));
    assert_eq!(app.session.autoreview_enabled, Some(true));
    assert!(app.autoreview_enabled);

    assert!(super::commands::handle_session_command(
        &mut app,
        "/autoreview off"
    ));
    assert_eq!(app.session.autoreview_enabled, Some(false));
    assert!(!app.autoreview_enabled);
}

#[test]
fn test_autojudge_command_toggles_session_preference() {
    let mut app = create_test_app();

    assert!(super::commands::handle_session_command(
        &mut app,
        "/autojudge on"
    ));
    assert_eq!(app.session.autojudge_enabled, Some(true));
    assert!(app.autojudge_enabled);

    assert!(super::commands::handle_session_command(
        &mut app,
        "/autojudge off"
    ));
    assert_eq!(app.session.autojudge_enabled, Some(false));
    assert!(!app.autojudge_enabled);
}

#[test]
fn test_transcript_path_command_reports_current_session_file() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        let expected = crate::session::session_path(&app.session.id).expect("session path");

        assert!(super::commands::handle_session_command(
            &mut app,
            "/transcript path"
        ));

        assert!(app.display_messages().iter().any(|msg| {
            msg.content.contains("Transcript file:")
                && msg.content.contains(&expected.display().to_string())
        }));
    });
}

#[test]
fn test_poke_arms_auto_poke_until_todos_are_done() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "todo-1".to_string(),
                content: "Finish the remaining task".to_string(),
                status: "pending".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
            }],
        )
        .expect("save todos");

        assert!(super::commands::handle_session_command(&mut app, "/poke"));

        assert!(app.auto_poke_incomplete_todos);
        assert!(app.pending_turn);
        assert!(app.display_messages().iter().any(|msg| {
            msg.content.contains("Poking model: 1 incomplete todo")
                && msg.content.contains("/poke off")
        }));
    });
}

#[test]
fn test_poke_status_reports_current_state() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "todo-1".to_string(),
                content: "Finish the remaining task".to_string(),
                status: "pending".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
            }],
        )
        .expect("save todos");

        assert!(super::commands::handle_session_command(
            &mut app,
            "/poke status"
        ));
        assert!(app.display_messages().iter().any(|msg| {
            msg.content
                .contains("Auto-poke: **ON**. 1 incomplete todo.")
        }));

        app.auto_poke_incomplete_todos = true;
        app.is_processing = true;
        app.queued_messages
            .push(super::commands::build_poke_message(
                &super::commands::incomplete_poke_todos(&app),
            ));

        assert!(super::commands::handle_session_command(
            &mut app,
            "/poke status"
        ));
        assert!(app.display_messages().iter().any(|msg| {
            msg.content
                .contains("Auto-poke: **ON**. 1 incomplete todo.")
                && msg.content.contains("A follow-up poke is queued.")
                && msg.content.contains("A turn is currently running.")
        }));
    });
}

#[test]
fn test_poke_off_disarms_and_clears_queued_followup() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "todo-1".to_string(),
                content: "Keep going".to_string(),
                status: "pending".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
            }],
        )
        .expect("save todos");

        app.auto_poke_incomplete_todos = true;
        app.pending_queued_dispatch = true;
        app.queued_messages
            .push(super::commands::build_poke_message(
                &super::commands::incomplete_poke_todos(&app),
            ));

        assert!(super::commands::handle_session_command(
            &mut app,
            "/poke off"
        ));

        assert!(!app.auto_poke_incomplete_todos);
        assert!(!app.pending_queued_dispatch);
        assert!(app.queued_messages().is_empty());
        assert_eq!(app.status_notice(), Some("Poke: OFF".to_string()));
        assert!(app.display_messages().iter().any(|msg| {
            msg.content.contains("Auto-poke disabled.")
                && msg.content.contains("Cleared 1 queued poke follow-up")
        }));
    });
}

#[test]
fn test_poke_queues_when_turn_is_in_progress() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "todo-1".to_string(),
                content: "Finish the remaining task".to_string(),
                status: "pending".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
            }],
        )
        .expect("save todos");

        app.is_processing = true;

        assert!(super::commands::handle_session_command(&mut app, "/poke"));

        assert!(app.auto_poke_incomplete_todos);
        assert!(app.is_processing);
        assert!(!app.cancel_requested);
        assert!(!app.pending_turn);
        assert_eq!(
            app.status_notice(),
            Some("Poke queued after current turn".to_string())
        );
        assert!(app.queued_messages().is_empty());
        assert!(app.display_messages().iter().any(|msg| {
            msg.content
                .contains("/poke queued. Re-checking incomplete todos after this turn")
        }));

        crate::todo::save_todos(
            &app.session.id,
            &[
                crate::todo::TodoItem {
                    id: "todo-1".to_string(),
                    content: "Finish the remaining task".to_string(),
                    status: "pending".to_string(),
                    priority: "high".to_string(),
                    blocked_by: Vec::new(),
                    assigned_to: None,
                },
                crate::todo::TodoItem {
                    id: "todo-2".to_string(),
                    content: "Pick up the newly discovered task".to_string(),
                    status: "pending".to_string(),
                    priority: "medium".to_string(),
                    blocked_by: Vec::new(),
                    assigned_to: None,
                },
            ],
        )
        .expect("save updated todos");

        super::local::finish_turn(&mut app);

        assert!(app.pending_queued_dispatch);
        assert_eq!(app.queued_messages().len(), 1);
        assert!(app.queued_messages()[0].contains("You have 2 incomplete todos"));
        assert!(!app.queued_messages()[0].contains("Pick up the newly discovered task"));
        assert!(!app.queued_messages()[0].contains("/poke off"));
    });
}

#[test]
fn test_finish_turn_auto_pokes_again_when_todos_remain() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "todo-1".to_string(),
                content: "Keep going".to_string(),
                status: "in_progress".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
            }],
        )
        .expect("save todos");

        app.auto_poke_incomplete_todos = true;
        app.is_processing = true;
        super::local::finish_turn(&mut app);

        assert!(app.pending_queued_dispatch);
        assert_eq!(app.queued_messages().len(), 1);
        assert!(app.queued_messages()[0].contains("Continue working, or update the todo tool."));
    });
}

#[test]
fn test_finish_turn_auto_poke_preserves_visible_turn_started() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "todo-1".to_string(),
                content: "Keep going".to_string(),
                status: "in_progress".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
            }],
        )
        .expect("save todos");

        let started = Instant::now() - Duration::from_secs(45);
        app.auto_poke_incomplete_todos = true;
        app.is_processing = true;
        app.visible_turn_started = Some(started);

        super::local::finish_turn(&mut app);

        assert_eq!(app.visible_turn_started, Some(started));
        assert!(app.pending_queued_dispatch);
    });
}

#[test]
fn test_help_topic_shows_overnight_command_details() {
    let mut app = create_test_app();
    app.input = "/help overnight".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("`/overnight <hours>[h|m] [mission]`"));
    assert!(msg.content.contains("review HTML page"));
    assert!(msg.content.contains("`/overnight status`"));
}

#[test]
fn test_overnight_status_without_runs_is_handled() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        assert!(super::commands::handle_session_command(
            &mut app,
            "/overnight status"
        ));

        let msg = app
            .display_messages()
            .last()
            .expect("missing overnight status response");
        assert_eq!(msg.role, "system");
        assert!(msg.content.contains("No overnight runs found"));
    });
}

#[test]
fn test_overnight_help_command_is_handled() {
    let mut app = create_test_app();
    assert!(super::commands::handle_session_command(
        &mut app,
        "/overnight help"
    ));

    let msg = app
        .display_messages()
        .last()
        .expect("missing overnight help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("`/overnight <hours>[h|m] [mission]`"));
    assert!(msg.content.contains("`/overnight review`"));
}

#[test]
fn test_overnight_start_runs_as_visible_local_turn() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        assert!(super::commands::handle_session_command(
            &mut app,
            "/overnight 1m hi"
        ));

        assert!(app.pending_turn, "local overnight should start a visible turn");
        assert!(app.is_processing, "local overnight should enter processing state");
        assert!(app.queued_messages.is_empty(), "local overnight should not use remote queue");
        let last_message = app.session.messages.last().expect("overnight prompt message");
        assert!(last_message.content.iter().any(|block| matches!(
            block,
            crate::message::ContentBlock::Text { text, .. }
                if text.contains("visible Overnight Coordinator")
        )));
    });
}

#[test]
fn test_overnight_start_queues_remote_turn_without_stuck_sending() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.is_remote = true;
        assert!(super::commands::handle_session_command(
            &mut app,
            "/overnight 1m hi"
        ));

        assert!(!app.pending_turn, "remote overnight should not set local pending_turn");
        assert!(!app.is_processing, "remote overnight should not get stuck in local Sending");
        assert_eq!(app.queued_messages.len(), 1);
        assert!(app.queued_messages[0].contains("visible Overnight Coordinator"));
    });
}
