// Retained saved-login pins must not change an explicit API billing route.
struct RetainedPinBillingRuntime {
    kind: AccountProviderKind,
    mode: Mutex<CredentialMode>,
    pin: Mutex<Option<AccountPin>>,
    model: Mutex<String>,
    attempts: Arc<Mutex<Vec<String>>>,
}

impl RetainedPinBillingRuntime {
    fn new(
        kind: AccountProviderKind,
        mode: CredentialMode,
        pin: Option<AccountPin>,
        attempts: Arc<Mutex<Vec<String>>>,
    ) -> Self {
        Self {
            kind,
            mode: Mutex::new(mode),
            pin: Mutex::new(pin),
            model: Mutex::new(match kind {
                AccountProviderKind::Claude => "claude-opus-4-6".into(),
                AccountProviderKind::OpenAi => "gpt-5.4".into(),
                AccountProviderKind::ClaudeCode => unreachable!("Claude Code has no API-key credential route"),
            }),
            attempts,
        }
    }
}

#[async_trait::async_trait]
impl Provider for RetainedPinBillingRuntime {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        self.attempts.lock().unwrap().push("api request".into());
        Ok(Box::pin(futures::stream::iter(vec![Ok(
            crate::message::StreamEvent::TextDelta("api request".into()),
        )])))
    }
    fn name(&self) -> &str {
        match self.kind {
            AccountProviderKind::Claude => "Anthropic",
            AccountProviderKind::OpenAi => "OpenAI",
            AccountProviderKind::ClaudeCode => unreachable!("Claude Code has no API-key credential route"),
        }
    }
    fn model(&self) -> String {
        self.model.lock().unwrap().clone()
    }
    fn set_model(&self, model: &str) -> Result<()> {
        *self.model.lock().unwrap() = model.to_string();
        Ok(())
    }
    fn credential_mode(&self) -> CredentialMode {
        *self.mode.lock().unwrap()
    }
    fn set_credential_mode(&self, mode: CredentialMode) -> Result<()> {
        *self.mode.lock().unwrap() = mode;
        Ok(())
    }
    fn active_resolved_credential(&self) -> Option<jcode_provider_core::ResolvedCredential> {
        match self.credential_mode() {
            CredentialMode::ApiKey => Some(jcode_provider_core::ResolvedCredential::ApiKey),
            CredentialMode::OAuth => Some(jcode_provider_core::ResolvedCredential::Oauth),
            CredentialMode::Auto => None,
        }
    }
    fn account_pin(&self, kind: AccountProviderKind) -> Option<AccountPin> {
        (kind == self.kind)
            .then(|| self.pin.lock().unwrap().clone())
            .flatten()
    }
    fn set_account_pin(&self, kind: AccountProviderKind, pin: Option<AccountPin>) -> Result<()> {
        if kind == self.kind {
            *self.pin.lock().unwrap() = pin;
        }
        Ok(())
    }
    fn oauth_account_label(&self) -> Option<String> {
        self.pin
            .lock()
            .unwrap()
            .as_ref()
            .map(|pin| pin.label.clone())
    }
    fn set_oauth_account(&self, label: Option<&str>) -> Result<()> {
        *self.pin.lock().unwrap() = label.map(|label| AccountPin::new(label, None));
        Ok(())
    }
    fn fork(&self) -> Arc<dyn Provider> {
        let fork = Self::new(
            self.kind,
            self.credential_mode(),
            self.account_pin(self.kind),
            Arc::clone(&self.attempts),
        );
        fork.set_model(&self.model()).unwrap();
        Arc::new(fork)
    }
}

fn retained_pin_billing_fixture(
    kind: AccountProviderKind,
    mode: CredentialMode,
    pin: AccountPin,
) -> (MultiProvider, Arc<Mutex<Vec<String>>>) {
    let provider = test_multi_provider_with_openai();
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let runtime: Arc<dyn Provider> = Arc::new(RetainedPinBillingRuntime::new(
        kind,
        mode,
        Some(pin),
        Arc::clone(&attempts),
    ));
    *provider.anthropic.write().unwrap() = None;
    *provider.openai.write().unwrap() = None;
    match kind {
        AccountProviderKind::Claude => {
            *provider.anthropic.write().unwrap() = Some(runtime);
            provider.set_active_provider(ActiveProvider::Claude);
        }
        AccountProviderKind::OpenAi => {
            *provider.openai.write().unwrap() = Some(runtime);
            provider.set_active_provider(ActiveProvider::OpenAI);
        }
        AccountProviderKind::ClaudeCode => unreachable!("Claude Code has no API-key credential route"),
    }
    provider.set_account_failover(Some(false));
    (provider, attempts)
}

#[test]
fn retained_api_pin_public_requests_ignore_missing_login_and_preserve_oauth_cooldown() {
    with_account_failover_env(|| {
        let rt = enter_test_runtime();
        rt.block_on(async {
            for kind in [AccountProviderKind::Claude, AccountProviderKind::OpenAi] {
                let label = match kind {
                    AccountProviderKind::Claude => store_claude_accounts(1).remove(0),
                    AccountProviderKind::OpenAi => store_openai_accounts(1).remove(0),
                    AccountProviderKind::ClaudeCode => unreachable!("Claude Code has no API-key credential route"),
                };
                for missing in [false, true] {
                    for split in [false, true] {
                        let pin =
                            AccountPin::new(if missing { "deleted-login" } else { &label }, None);
                        let (provider, attempts) =
                            retained_pin_billing_fixture(kind, CredentialMode::ApiKey, pin.clone());
                        record_provider_unavailable_for_account_label(
                            kind.key(),
                            Some(&pin.label),
                            "saved OAuth quota exhausted",
                        );
                        let stream = if split {
                            provider
                                .complete_split(&[], &[], "static", "dynamic", None)
                                .await
                        } else {
                            provider.complete(&[], &[], "system", None).await
                        }
                        .expect("retained OAuth pin must not block API billing");
                        assert_eq!(drain_text(stream).await.unwrap(), "api request");
                        assert_eq!(*attempts.lock().unwrap(), ["api request"]);
                        assert_eq!(provider.account_pin(kind), Some(pin.clone()));
                        assert_eq!(
                            provider.active_resolved_credential(),
                            Some(jcode_provider_core::ResolvedCredential::ApiKey)
                        );
                        assert!(
                            provider_unavailability_detail_for_account_label(
                                kind.key(),
                                Some(&pin.label)
                            )
                            .is_some(),
                            "API success must not clear saved OAuth cooldown"
                        );
                        clear_provider_unavailable_for_label(kind.key(), &pin.label);
                    }
                }
            }
        });
    });
}

#[test]
fn retained_oauth_pin_still_fails_closed_on_missing_login_or_cooldown() {
    with_account_failover_env(|| {
        let rt = enter_test_runtime();
        rt.block_on(async {
            for kind in [AccountProviderKind::Claude, AccountProviderKind::OpenAi] {
                let label = match kind {
                    AccountProviderKind::Claude => store_claude_accounts(1).remove(0),
                    AccountProviderKind::OpenAi => store_openai_accounts(1).remove(0),
                    AccountProviderKind::ClaudeCode => unreachable!("Claude Code has no API-key credential route"),
                };
                for missing in [false, true] {
                    let pin = AccountPin::new(if missing { "deleted-login" } else { &label }, None);
                    let (provider, attempts) =
                        retained_pin_billing_fixture(kind, CredentialMode::OAuth, pin.clone());
                    if !missing {
                        record_provider_unavailable_for_account_label(
                            kind.key(),
                            Some(&pin.label),
                            "saved OAuth quota exhausted",
                        );
                    }
                    let error = provider
                        .complete(&[], &[], "system", None)
                        .await
                        .err()
                        .expect("OAuth pin must remain fail closed");
                    assert!(error.to_string().contains(if missing {
                        "no longer exists"
                    } else {
                        "unavailable"
                    }));
                    assert!(attempts.lock().unwrap().is_empty());
                    assert_eq!(provider.account_pin(kind), Some(pin.clone()));
                    if !missing {
                        assert!(
                            provider_unavailability_detail_for_account_label(
                                kind.key(),
                                Some(&pin.label)
                            )
                            .is_some()
                        );
                        clear_provider_unavailable_for_label(kind.key(), &pin.label);
                    }
                }
            }
        });
    });
}

#[test]
fn retained_api_pin_public_fork_preserves_billing_mode_and_account_pin() {
    with_account_failover_env(|| {
        let rt = enter_test_runtime();
        let _entered = rt.enter();
        for kind in [AccountProviderKind::Claude, AccountProviderKind::OpenAi] {
            let label = match kind {
                AccountProviderKind::Claude => store_claude_accounts(1).remove(0),
                AccountProviderKind::OpenAi => store_openai_accounts(1).remove(0),
                    AccountProviderKind::ClaudeCode => unreachable!("Claude Code has no API-key credential route"),
            };
            let pin = AccountPin::new(label, None);
            let (provider, attempts) =
                retained_pin_billing_fixture(kind, CredentialMode::ApiKey, pin.clone());
            let key = match kind {
                AccountProviderKind::Claude => external::ANTHROPIC_RUNTIME,
                AccountProviderKind::OpenAi => external::OPENAI_RUNTIME,
                AccountProviderKind::ClaudeCode => unreachable!("Claude Code has no API-key credential route"),
            };
            struct RestoreFactory(&'static str, Option<Arc<dyn Provider>>);
            impl Drop for RestoreFactory {
                fn drop(&mut self) {
                    let previous = self.1.take();
                    external::register_external_provider_fallible(self.0, move || previous.clone());
                }
            }
            let _restore = RestoreFactory(key, external::instantiate_external_provider(key));
            let factory_attempts = Arc::clone(&attempts);
            external::register_external_provider(key, move || {
                Arc::new(RetainedPinBillingRuntime::new(
                    kind,
                    CredentialMode::ApiKey,
                    None,
                    Arc::clone(&factory_attempts),
                ))
            });
            let fork = provider.fork();
            assert_eq!(
                fork.active_resolved_credential(),
                Some(jcode_provider_core::ResolvedCredential::ApiKey)
            );
            assert_eq!(fork.account_pin(kind), Some(pin.clone()));
            assert_eq!(fork.model(), provider.model());
            assert_eq!(
                provider.active_resolved_credential(),
                Some(jcode_provider_core::ResolvedCredential::ApiKey)
            );
            assert_eq!(provider.account_pin(kind), Some(pin));
            assert!(attempts.lock().unwrap().is_empty());
        }
    });
}
