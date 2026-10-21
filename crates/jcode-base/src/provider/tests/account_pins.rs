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
    fn credential_mode(&self) -> jcode_provider_core::CredentialMode {
        self.inner.credential_mode()
    }
    fn set_credential_mode(&self, mode: jcode_provider_core::CredentialMode) -> anyhow::Result<()> {
        self.inner.set_credential_mode(mode)
    }
    fn oauth_account_label(&self) -> Option<String> {
        self.pin.get().map(|pin| pin.label)
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
        catalog_scope: next_catalog_scope(),
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

#[test]
fn running_instances_follow_account_rename_and_reject_reused_labels() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _runtime = rt.enter();
        for (kind, active) in [
            (AccountProviderKind::Claude, ActiveProvider::Claude),
            (AccountProviderKind::OpenAi, ActiveProvider::OpenAI),
        ] {
            let save_account = |label: &str, email: &str| match kind {
                AccountProviderKind::Claude => {
                    let actual = crate::auth::claude::upsert_account(
                        crate::auth::claude::AnthropicAccount {
                            label: label.into(),
                            access: "synthetic-access".into(),
                            refresh: "synthetic-refresh".into(),
                            expires: i64::MAX,
                            email: Some(email.into()),
                            subscription_type: None,
                            scopes: vec!["user:inference".into()],
                        },
                    )
                    .unwrap();
                    if actual != label {
                        crate::auth::claude::rename_account(&actual, label).unwrap();
                    }
                }
                AccountProviderKind::OpenAi => {
                    let actual = crate::auth::codex::upsert_account_from_tokens(
                        label,
                        "synthetic-access",
                        "synthetic-refresh",
                        None,
                        Some(i64::MAX),
                    )
                    .unwrap();
                    crate::auth::codex::update_account_profile(&actual, Some(email.into()))
                        .unwrap();
                    if actual != label {
                        crate::auth::codex::rename_account(&actual, label).unwrap();
                    }
                }
                _ => unreachable!(),
            };
            let rename = |old, new| match kind {
                AccountProviderKind::Claude => crate::auth::claude::rename_account(old, new),
                AccountProviderKind::OpenAi => crate::auth::codex::rename_account(old, new),
                _ => unreachable!(),
            };
            save_account("work", "original@example.invalid");
            let pin = match kind {
                AccountProviderKind::Claude => crate::auth::claude::pin_for_label("work"),
                AccountProviderKind::OpenAi => crate::auth::codex::pin_for_label("work"),
                _ => unreachable!(),
            }
            .unwrap();
            let instances = [
                multi_provider_with_pin_aware_runtimes(),
                multi_provider_with_pin_aware_runtimes(),
            ];
            for instance in &instances {
                instance.set_active_provider(active);
                instance.set_account_pin(kind, Some(pin.clone())).unwrap();
                instance.set_credential_mode(CredentialMode::OAuth).unwrap();
                assert_eq!(instance.oauth_account_label().as_deref(), Some("work"));
            }
            // Neither instance receives the UI's rename action or any IPC update.
            rename("work", "Research team").unwrap();
            rename("Research team", "Final name").unwrap();
            for instance in &instances {
                assert_eq!(
                    instance.oauth_account_label().as_deref(),
                    Some("Final name")
                );
                assert!(
                    instance
                        .saved_account_binding_exists(active, "Final name")
                        .unwrap()
                );
                assert_eq!(instance.account_pin(kind), Some(pin.clone()));
                let error = rt
                    .block_on(instance.complete(&[], &[], "", None))
                    .err()
                    .expect("the downstream test runtime deliberately rejects streaming");
                assert!(
                    error.to_string().contains("runtime does not stream"),
                    "renamed binding must pass the real request gate: {error}"
                );
            }
            // A new login reusing the previous label cannot capture these sessions.
            save_account("work", "different@example.invalid");
            for instance in &instances {
                assert_eq!(
                    instance.oauth_account_label().as_deref(),
                    Some("Final name")
                );
                assert!(
                    !instance
                        .saved_account_binding_exists(active, "work")
                        .unwrap()
                );
            }
            match kind {
                AccountProviderKind::Claude => crate::auth::claude::remove_account("Final name"),
                AccountProviderKind::OpenAi => crate::auth::codex::remove_account("Final name"),
                _ => unreachable!(),
            }
            .unwrap();
            for instance in &instances {
                assert!(
                    !instance
                        .saved_account_binding_exists(active, "work")
                        .unwrap()
                );
                let error = rt
                    .block_on(instance.complete(&[], &[], "", None))
                    .err()
                    .expect("removed account must reject the request");
                assert!(error.to_string().contains("no longer exists"), "{error}");
            }
        }
    });
}
