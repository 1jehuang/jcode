/// Minimal runtime that stores an account pin like the real Anthropic/OpenAI
/// runtimes do, so MultiProvider pin forwarding and fork semantics can be
/// tested from base.
struct PinAwareStubRuntime {
    inner: StubExternalRuntime,
    kind: AccountProviderKind,
    pin: jcode_provider_core::AccountPinSlot,
}

impl PinAwareStubRuntime {
    fn anthropic() -> Self {
        Self {
            inner: StubExternalRuntime::anthropic(),
            kind: AccountProviderKind::Claude,
            pin: Default::default(),
        }
    }

    fn openai() -> Self {
        Self {
            inner: StubExternalRuntime::openai(),
            kind: AccountProviderKind::OpenAi,
            pin: Default::default(),
        }
    }
}

#[async_trait::async_trait]
impl Provider for PinAwareStubRuntime {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
    ) -> anyhow::Result<EventStream> {
        self.inner
            .complete(messages, tools, system, resume_session_id)
            .await
    }
    fn name(&self) -> &'static str {
        self.inner.name
    }
    fn model(&self) -> String {
        self.inner.model()
    }
    fn set_model(&self, model: &str) -> anyhow::Result<()> {
        self.inner.set_model(model)
    }
    fn available_models(&self) -> Vec<&'static str> {
        self.inner.available_models()
    }
    fn available_models_display(&self) -> Vec<String> {
        self.inner.available_models_display()
    }
    fn model_routes(&self) -> Vec<ModelRoute> {
        self.inner.model_routes()
    }
    fn account_pin(&self, kind: AccountProviderKind) -> Option<AccountPin> {
        (kind == self.kind).then(|| self.pin.get()).flatten()
    }
    fn set_account_pin(
        &self,
        kind: AccountProviderKind,
        pin: Option<AccountPin>,
    ) -> anyhow::Result<()> {
        if kind == self.kind {
            self.pin.set(pin);
        }
        Ok(())
    }
    fn resolved_account_label(&self, kind: AccountProviderKind) -> Option<String> {
        (kind == self.kind)
            .then(|| self.pin.get().map(|pin| pin.label))
            .flatten()
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            inner: StubExternalRuntime::new(
                self.inner.name,
                self.inner.provider_label,
                self.inner.api_method,
                self.inner.models,
            ),
            kind: self.kind,
            pin: self.pin.fork(),
        })
    }
}

fn multi_provider_with_pin_aware_runtimes() -> MultiProvider {
    MultiProvider {
        anthropic: RwLock::new(Some(
            Arc::new(PinAwareStubRuntime::anthropic()) as Arc<dyn Provider>
        )),
        openai: RwLock::new(Some(
            Arc::new(PinAwareStubRuntime::openai()) as Arc<dyn Provider>
        )),
        copilot_api: RwLock::new(None),
        antigravity: RwLock::new(None),
        gemini: RwLock::new(None),
        cursor: RwLock::new(None),
        claude_code: RwLock::new(None),
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

/// `fork()` continues a session (compaction, resumed turn, helper work) and
/// keeps its accounts. `fork_for_new_session()` is a new window and starts on
/// the default account.
#[test]
fn multiprovider_fork_copies_pins_new_session_fork_does_not() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _guard = runtime.enter();
        external::register_external_provider(external::ANTHROPIC_RUNTIME, || {
            Arc::new(PinAwareStubRuntime::anthropic()) as Arc<dyn Provider>
        });
        external::register_external_provider(external::OPENAI_RUNTIME, || {
            Arc::new(PinAwareStubRuntime::openai()) as Arc<dyn Provider>
        });
        crate::env::set_var("ANTHROPIC_API_KEY", "sk-ant-test");
        crate::env::set_var("OPENAI_API_KEY", "sk-openai-test");

        let session = multi_provider_with_pin_aware_runtimes();
        let fox = AccountPin::new("claude-fox", Some("fox@example.com".to_string()));
        let ofox = AccountPin::new("openai-fox", Some("id:acct-fox".to_string()));
        session
            .set_account_pin(AccountProviderKind::Claude, Some(fox.clone()))
            .unwrap();
        session
            .set_account_pin(AccountProviderKind::OpenAi, Some(ofox.clone()))
            .unwrap();
        assert_eq!(
            session.account_pin(AccountProviderKind::Claude),
            Some(fox.clone())
        );
        assert_eq!(
            session
                .resolved_account_label(AccountProviderKind::OpenAi)
                .as_deref(),
            Some("openai-fox")
        );

        let continued = session.fork();
        assert_eq!(
            continued.account_pin(AccountProviderKind::Claude),
            Some(fox.clone())
        );
        assert_eq!(
            continued.account_pin(AccountProviderKind::OpenAi),
            Some(ofox.clone())
        );

        // The copy is a value: unpinning the fork leaves the session pinned.
        continued
            .set_account_pin(AccountProviderKind::Claude, None)
            .unwrap();
        assert_eq!(session.account_pin(AccountProviderKind::Claude), Some(fox));

        let new_window = session.fork_for_new_session();
        assert_eq!(new_window.account_pin(AccountProviderKind::Claude), None);
        assert_eq!(new_window.account_pin(AccountProviderKind::OpenAi), None);
    });
}
