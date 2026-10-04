// Claude Code CLI mode wiring: a stub runtime in the dedicated slot proves
// MultiProvider dispatch, the `claude-code:` prefix intercept, tool-ownership
// delegation, instance pins and route catalog shape, without spawning `claude`.

/// Stub for the Claude Code runtime: records the model and instance pin and
/// owns tool execution like the real runtime.
struct ClaudeCodeStubRuntime {
    model: std::sync::Mutex<String>,
    pin: jcode_provider_core::AccountPinSlot,
    tx: NativeToolResultSender,
}

impl ClaudeCodeStubRuntime {
    fn new(tx: NativeToolResultSender) -> Self {
        Self {
            model: std::sync::Mutex::new("claude-opus-4-8".to_string()),
            pin: Default::default(),
            tx,
        }
    }
}

#[async_trait::async_trait]
impl Provider for ClaudeCodeStubRuntime {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> anyhow::Result<EventStream> {
        anyhow::bail!("claude code stub: complete is not exercised")
    }
    fn name(&self) -> &'static str {
        "Claude Code"
    }
    fn model(&self) -> String {
        self.model.lock().unwrap().clone()
    }
    fn set_model(&self, model: &str) -> anyhow::Result<()> {
        *self.model.lock().unwrap() = model.to_string();
        Ok(())
    }
    fn available_models_display(&self) -> Vec<String> {
        vec![
            "claude-opus-4-8".to_string(),
            "claude-sonnet-4-6".to_string(),
        ]
    }
    fn handles_tools_internally(&self) -> bool {
        true
    }
    fn native_result_sender(&self) -> Option<NativeToolResultSender> {
        Some(self.tx.clone())
    }
    fn account_pin(&self, kind: AccountProviderKind) -> Option<AccountPin> {
        (kind == AccountProviderKind::ClaudeCode)
            .then(|| self.pin.get())
            .flatten()
    }
    fn set_account_pin(
        &self,
        kind: AccountProviderKind,
        pin: Option<AccountPin>,
    ) -> anyhow::Result<()> {
        if kind == AccountProviderKind::ClaudeCode {
            self.pin.set(pin);
        }
        Ok(())
    }
    fn fork(&self) -> Arc<dyn Provider> {
        let fork = Self::new(self.tx.clone());
        *fork.model.lock().unwrap() = self.model();
        fork.pin.set(self.pin.get());
        Arc::new(fork)
    }
}

fn multi_provider_with_claude_code(tx: NativeToolResultSender) -> MultiProvider {
    MultiProvider {
        anthropic: RwLock::new(Some(test_anthropic_runtime() as Arc<dyn Provider>)),
        openai: RwLock::new(None),
        copilot_api: RwLock::new(None),
        antigravity: RwLock::new(None),
        gemini: RwLock::new(None),
        cursor: RwLock::new(None),
        claude_code: RwLock::new(Some(
            Arc::new(ClaudeCodeStubRuntime::new(tx)) as Arc<dyn Provider>
        )),
        bedrock: RwLock::new(None),
        openrouter: RwLock::new(None),
        openai_compatible_profiles: RwLock::new(std::collections::HashMap::new()),
        active_openai_compatible_profile: RwLock::new(None),
        active: RwLock::new(ActiveProvider::Claude),
        startup_notices: RwLock::new(Vec::new()),
        initial_provider: None,
        routes_memo: std::sync::Mutex::new(None),
        post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        account_failover: Default::default(),
    }
}

#[test]
fn claude_code_prefix_routes_to_the_claude_code_slot_not_native_claude() {
    with_clean_provider_test_env(|| {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let provider = multi_provider_with_claude_code(tx);

        // Native Claude stays the native runtime: no tool ownership change.
        assert!(!provider.handles_tools_internally());
        assert!(provider.native_result_sender().is_none());

        provider
            .set_model("claude-code:claude-sonnet-4-6")
            .expect("claude-code prefix switches to the Claude Code runtime");
        assert_eq!(provider.active_provider(), ActiveProvider::ClaudeCode);
        assert_eq!(provider.model(), "claude-sonnet-4-6");
        assert_eq!(provider.name(), "Claude Code");
        assert!(provider.handles_tools_internally());

        // A plain native model switches back to the native runtime.
        provider
            .set_model("claude:claude-opus-4-8")
            .expect("native claude switch");
        assert_eq!(provider.active_provider(), ActiveProvider::Claude);
        assert!(!provider.handles_tools_internally());
    });
}

#[test]
fn claude_code_native_result_sender_reaches_the_runtime() {
    let runtime = enter_test_runtime();
    runtime.block_on(async {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let provider = multi_provider_with_claude_code(tx);
        provider.set_active_provider(ActiveProvider::ClaudeCode);
        let sender = provider
            .native_result_sender()
            .expect("Claude Code must expose its MCP tool result bridge");
        sender
            .send(NativeToolResult::success(
                "mcp:1".to_string(),
                "ok".to_string(),
            ))
            .await
            .expect("send native result");
        assert_eq!(rx.recv().await.expect("result").request_id, "mcp:1");
    });
}

#[test]
fn claude_code_route_selection_pins_the_instance() {
    with_clean_provider_test_env(|| {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let provider = multi_provider_with_claude_code(tx);
        let selection = RouteSelection::from_model_route(&ModelRoute {
            model: "claude-opus-4-8".to_string(),
            provider: "Claude Code".to_string(),
            api_method: "claude-code".to_string(),
            available: true,
            detail: String::new(),
            usage: None,
            cheapness: None,
        });
        provider
            .set_route_selection(&selection)
            .expect("default instance route");
        assert_eq!(provider.active_provider(), ActiveProvider::ClaudeCode);
        // Default instance: no pin needed.
        assert_eq!(provider.account_pin(AccountProviderKind::ClaudeCode), None);

        // Unknown instance ids are rejected rather than silently using another login.
        let unknown = RouteSelection::from_model_route(&ModelRoute {
            model: "claude-opus-4-8".to_string(),
            provider: "Claude Code (ghost)".to_string(),
            api_method: "claude-code:ghost".to_string(),
            available: true,
            detail: String::new(),
            usage: None,
            cheapness: None,
        });
        assert!(provider.set_route_selection(&unknown).is_err());

        // Pins forward to the runtime and survive fork.
        provider
            .set_account_pin(
                AccountProviderKind::ClaudeCode,
                Some(AccountPin::new("default", None)),
            )
            .expect("pin");
        let fork = provider.fork();
        assert_eq!(
            fork.account_pin(AccountProviderKind::ClaudeCode)
                .map(|pin| pin.label),
            Some("default".to_string())
        );
    });
}

#[test]
fn claude_code_registry_key_instantiates_the_registered_runtime() {
    with_clean_provider_test_env(|| {
        external::register_external_provider(external::CLAUDE_CODE_RUNTIME, || {
            let (tx, _rx) = tokio::sync::mpsc::channel(1);
            Arc::new(ClaudeCodeStubRuntime::new(tx)) as Arc<dyn Provider>
        });
        let runtime = external::instantiate_external_provider(external::CLAUDE_CODE_RUNTIME)
            .expect("registered");
        assert_eq!(runtime.name(), "Claude Code");
        assert!(runtime.handles_tools_internally());
    });
}

#[test]
fn claude_code_routes_cover_every_instance() {
    let settings = crate::config::ClaudeCodeConfig {
        default_instance: "work".to_string(),
        instances: vec![
            crate::config::ClaudeCodeInstanceConfig {
                id: "work".to_string(),
                display_name: Some("Claude Work".to_string()),
                ..Default::default()
            },
            crate::config::ClaudeCodeInstanceConfig {
                id: "personal".to_string(),
                home: Some("/tmp/claude-personal".to_string()),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let routes = super::catalog_routes::claude_code_routes_for(
        &settings,
        &[
            "claude-opus-4-8".to_string(),
            "claude-sonnet-4-6".to_string(),
        ],
    );
    assert_eq!(routes.len(), 4);
    let work: Vec<_> = routes
        .iter()
        .filter(|route| route.api_method == "claude-code")
        .collect();
    assert_eq!(work.len(), 2);
    assert!(work.iter().all(|route| route.provider == "Claude Work"));
    let personal: Vec<_> = routes
        .iter()
        .filter(|route| route.api_method == "claude-code:personal")
        .collect();
    assert_eq!(personal.len(), 2);
    assert!(
        personal
            .iter()
            .all(|route| route.provider == "Claude Code (personal)"
                && route.detail.contains("/tmp/claude-personal"))
    );
    // Every route round-trips to a claude-code: model spec and its instance.
    for route in &routes {
        let selection = RouteSelection::from_model_route(route);
        assert!(selection.routed_model_spec().starts_with("claude-code:"));
        assert_eq!(selection.runtime_key.stable_id(), route.api_method);
    }
}

#[test]
fn claude_code_session_route_restore_round_trips() {
    with_clean_provider_test_env(|| {
        assert_eq!(
            MultiProvider::model_switch_request_for_session_route(
                "claude-opus-4-8",
                Some("claude-code"),
                Some("claude-code:personal"),
            ),
            "claude-code:claude-opus-4-8"
        );
        assert_eq!(
            MultiProvider::model_switch_request_for_session_model(
                "claude-opus-4-8",
                Some("claude-code")
            ),
            "claude-code:claude-opus-4-8"
        );
        let selection = MultiProvider::default_model_selection_from_route(
            "claude-opus-4-8",
            "claude-code:personal",
            "Claude Code (personal)",
        );
        assert_eq!(selection.model_spec, "claude-code:claude-opus-4-8");
        assert_eq!(selection.provider_key.as_deref(), Some("claude-code"));
        // Native Claude restores are unchanged.
        assert_eq!(
            MultiProvider::model_switch_request_for_session_route(
                "claude-opus-4-8",
                Some("claude-oauth"),
                Some("claude-oauth"),
            ),
            "claude-oauth:claude-opus-4-8"
        );
    });
}
