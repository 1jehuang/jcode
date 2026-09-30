// Per-window accounts in a local (standalone) TUI session: the provider must
// act on what the window shows (failover toggle, pins restored on resume).

/// Records `set_account_failover` and keeps its own pins, like the real
/// multi-provider.
#[derive(Clone, Default)]
struct AccountRecordingProvider {
    failover_calls: StdArc<StdMutex<Vec<Option<bool>>>>,
    pins: StdArc<
        StdMutex<
            std::collections::BTreeMap<
                jcode_provider_core::AccountProviderKind,
                jcode_provider_core::AccountPin,
            >,
        >,
    >,
}

#[async_trait::async_trait]
impl Provider for AccountRecordingProvider {
    async fn complete(
        &self,
        _messages: &[crate::message::Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        unimplemented!("AccountRecordingProvider")
    }
    fn name(&self) -> &str {
        "claude"
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
    fn account_pin(
        &self,
        kind: jcode_provider_core::AccountProviderKind,
    ) -> Option<jcode_provider_core::AccountPin> {
        self.pins.lock().unwrap().get(&kind).cloned()
    }
    fn set_account_pin(
        &self,
        kind: jcode_provider_core::AccountProviderKind,
        pin: Option<jcode_provider_core::AccountPin>,
    ) -> Result<()> {
        let mut pins = self.pins.lock().unwrap();
        match pin {
            Some(pin) => pins.insert(kind, pin),
            None => pins.remove(&kind),
        };
        Ok(())
    }
    fn set_account_failover(&self, enabled: Option<bool>) {
        self.failover_calls.lock().unwrap().push(enabled);
    }
}

fn account_recording_app() -> (App, AccountRecordingProvider) {
    let provider = AccountRecordingProvider::default();
    let provider_dyn: Arc<dyn Provider> = Arc::new(provider.clone());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider_dyn.clone()));
    let mut app = App::new_for_test_harness(provider_dyn, registry);
    app.queue_mode = false;
    (app, provider)
}

/// Greptile "Local toggle misses provider": `/account failover on|off` in a
/// local session must reach the provider, not only the display.
#[test]
fn local_account_failover_toggle_reaches_provider() {
    with_temp_jcode_home(|| {
        let (mut app, provider) = account_recording_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            app.input = "/account failover off".to_string();
            app.submit_input();
            app.input = "/account failover on".to_string();
            app.submit_input();
            app.input = "/account failover default".to_string();
            app.submit_input();
        });
        assert_eq!(
            *provider.failover_calls.lock().unwrap(),
            vec![Some(false), Some(true), None],
            "every toggle must be applied to the provider"
        );
        assert_eq!(app.session.account_failover, None);
    });
}

/// Resuming a local session applies its saved failover toggle and pin to the
/// provider.
#[test]
fn local_resume_applies_account_failover_and_pin() {
    with_temp_jcode_home(|| {
        crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
            label: String::new(),
            access: "access-a".to_string(),
            refresh: "refresh-a".to_string(),
            expires: chrono::Utc::now().timestamp_millis() + 3_600_000,
            email: Some("a@example.com".to_string()),
            subscription_type: None,
            scopes: Vec::new(),
        })
        .unwrap();
        let second = crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
            label: String::new(),
            access: "access-b".to_string(),
            refresh: "refresh-b".to_string(),
            expires: chrono::Utc::now().timestamp_millis() + 3_600_000,
            email: Some("b@example.com".to_string()),
            subscription_type: None,
            scopes: Vec::new(),
        })
        .unwrap();
        let mut saved = crate::session::Session::create(None, None);
        saved.account_failover = Some(false);
        saved.account_pins.insert(
            "claude".to_string(),
            jcode_provider_core::AccountPin::new(&second, Some("b@example.com".to_string())),
        );
        saved.save_prepared().unwrap();

        let (mut app, provider) = account_recording_app();
        app.restore_session(&saved.id);
        assert_eq!(
            provider.failover_calls.lock().unwrap().last().copied(),
            Some(Some(false)),
            "the saved toggle must reach the provider"
        );
        assert_eq!(
            provider
                .pins
                .lock()
                .unwrap()
                .get(&jcode_provider_core::AccountProviderKind::Claude)
                .map(|pin| pin.label.clone()),
            Some(second.clone())
        );
        cleanup_reload_context_file(&saved.id);
    });
}
