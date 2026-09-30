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

fn claude_window(label: &str, pinned: bool) -> crate::protocol::SessionAccountInfo {
    crate::protocol::SessionAccountInfo {
        provider: "claude".to_string(),
        label: Some(label.to_string()),
        pinned,
        is_default: !pinned,
    }
}

fn store_two_claude_accounts() -> (String, String) {
    let account = |n: &str| crate::auth::claude::AnthropicAccount {
        label: String::new(),
        access: format!("access-{n}"),
        refresh: format!("refresh-{n}"),
        expires: chrono::Utc::now().timestamp_millis() + 3_600_000,
        email: Some(format!("{n}@example.com")),
        subscription_type: None,
        scopes: Vec::new(),
    };
    let first = crate::auth::claude::upsert_account(account("first")).unwrap();
    let second = crate::auth::claude::upsert_account(account("second")).unwrap();
    (first, second)
}

/// Greptile "Remote rejection looks successful": remote /account switch,
/// default and failover change nothing until the server answers that request
/// id. An Error keeps the old display and is shown.
#[test]
fn remote_account_request_rejection_keeps_old_display() {
    with_temp_jcode_home(|| {
        let (first, second) = store_two_claude_accounts();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut app = create_test_app();
        app.is_remote = true;
        app.replace_window_accounts(vec![claude_window(&first, false)]);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        let window_label =
            |app: &App| app.window_account("claude").and_then(|w| w.label);

        // Switch rejected by the server (e.g. the daemon's store differs).
        let switch_id = remote.next_request_id_for_test();
        rt.block_on(app.execute_window_account_command_remote(
            super::auth::AccountCommand::UseInWindow {
                provider_id: "claude".to_string(),
                label: second.clone(),
            },
            &mut remote,
        ))
        .unwrap();
        assert_eq!(window_label(&app), Some(first.clone()), "not before the reply");
        assert!(
            !app.display_messages()
                .iter()
                .any(|m| m.content.contains(&format!("This window now uses {second}"))),
            "no success announcement on send"
        );
        app.handle_server_event(
            crate::protocol::ServerEvent::Error {
                id: switch_id,
                message: "pin refused".to_string(),
                retry_after_secs: None,
            },
            &mut remote,
        );
        assert_eq!(window_label(&app), Some(first.clone()), "rejected: unchanged");
        let last = app.display_messages().last().unwrap().content.clone();
        assert!(
            last.contains(&format!("Could not switch this window to {second}: pin refused")),
            "{last}"
        );
        assert!(
            !app.display_messages()
                .iter()
                .any(|m| m.content.contains(&format!("This window now uses {second}"))),
            "a rejected switch must not be announced"
        );

        // Failover toggle rejected, then accepted.
        let failover_id = remote.next_request_id_for_test();
        rt.block_on(app.execute_window_account_command_remote(
            super::auth::AccountCommand::Failover(super::auth::AccountFailoverMode::Off),
            &mut remote,
        ))
        .unwrap();
        assert_eq!(app.window_account_failover, None, "not before the reply");
        app.handle_server_event(
            crate::protocol::ServerEvent::Error {
                id: failover_id,
                message: "session busy".to_string(),
                retry_after_secs: None,
            },
            &mut remote,
        );
        assert_eq!(app.window_account_failover, None, "rejected: unchanged");
        assert!(app
            .display_messages()
            .last()
            .unwrap()
            .content
            .contains("Could not change account failover: session busy"));
        let failover_id = remote.next_request_id_for_test();
        rt.block_on(app.execute_window_account_command_remote(
            super::auth::AccountCommand::Failover(super::auth::AccountFailoverMode::Off),
            &mut remote,
        ))
        .unwrap();
        app.handle_server_event(
            crate::protocol::ServerEvent::Done { id: failover_id },
            &mut remote,
        );
        assert_eq!(app.window_account_failover, Some(false), "confirmed");

        // Default rejected.
        let default_id = remote.next_request_id_for_test();
        rt.block_on(app.execute_window_account_command_remote(
            super::auth::AccountCommand::SetDefault {
                provider_id: "claude".to_string(),
                label: second.clone(),
            },
            &mut remote,
        ))
        .unwrap();
        app.handle_server_event(
            crate::protocol::ServerEvent::Error {
                id: default_id,
                message: "store locked".to_string(),
                retry_after_secs: None,
            },
            &mut remote,
        );
        assert_eq!(window_label(&app), Some(first.clone()));
        assert!(
            !app.display_messages()
                .iter()
                .any(|m| m.content.contains(&format!("now use {second}"))),
            "a rejected default must not be announced"
        );
        assert!(app
            .display_messages()
            .last()
            .unwrap()
            .content
            .contains(&format!("Could not make {second} the default account: store locked")));
    });
}

/// A confirmed remote switch updates the window once the server answers.
#[test]
fn remote_account_switch_applies_on_done() {
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut app = create_test_app();
        app.is_remote = true;
        app.replace_window_accounts(vec![claude_window("claude-otter", false)]);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        let id = rt
            .block_on(remote.set_session_account("claude", Some("claude-fox")))
            .unwrap();
        app.pending_account_requests.insert(
            id,
            super::window_account::PendingAccountRequest::UseInWindow {
                family: "claude".to_string(),
                label: "claude-fox".to_string(),
            },
        );
        app.handle_server_event(
            crate::protocol::ServerEvent::SessionAccountChanged {
                provider: "claude".to_string(),
                label: Some("claude-fox".to_string()),
                pinned: true,
                is_default: false,
                reason: Some("this window now uses claude-fox".to_string()),
            },
            &mut remote,
        );
        app.handle_server_event(crate::protocol::ServerEvent::Done { id }, &mut remote);
        let window = app.window_account("claude").unwrap();
        assert_eq!(window.label.as_deref(), Some("claude-fox"));
        assert!(window.pinned);
        let announcements = app
            .display_messages()
            .iter()
            .filter(|m| m.content.contains("claude-fox"))
            .count();
        assert_eq!(announcements, 1, "announced once, on Done");
    });
}

fn pending_use(app: &mut App, id: u64, label: &str) {
    app.pending_account_requests.insert(
        id,
        super::window_account::PendingAccountRequest::UseInWindow {
            family: "claude".to_string(),
            label: label.to_string(),
        },
    );
}

fn account_changed(label: &str, reason: Option<&str>) -> crate::protocol::ServerEvent {
    crate::protocol::ServerEvent::SessionAccountChanged {
        provider: "claude".to_string(),
        label: Some(label.to_string()),
        pinned: true,
        is_default: false,
        reason: reason.map(str::to_string),
    }
}

/// Switch to fox then otter: a late Done for fox must not put fox back.
#[test]
fn stale_account_done_does_not_regress_display() {
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut app = create_test_app();
        app.is_remote = true;
        app.replace_window_accounts(vec![claude_window("claude-owl", false)]);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        pending_use(&mut app, 1, "claude-fox");
        pending_use(&mut app, 2, "claude-otter");
        app.handle_server_event(account_changed("claude-fox", None), &mut remote);
        app.handle_server_event(account_changed("claude-otter", None), &mut remote);
        app.handle_server_event(crate::protocol::ServerEvent::Done { id: 2 }, &mut remote);
        app.handle_server_event(crate::protocol::ServerEvent::Done { id: 1 }, &mut remote);
        assert_eq!(
            app.window_account_label("claude").as_deref(),
            Some("claude-otter"),
            "stale Done regressed the badge"
        );
        let last = app.display_messages().last().unwrap().content.clone();
        assert!(!last.contains("now uses claude-fox"), "stale Done announced: {last}");
    });
}

/// A pending request for fox must not hide an automatic move to otter.
#[test]
fn unrelated_pending_request_does_not_silence_failover() {
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut app = create_test_app();
        app.is_remote = true;
        app.replace_window_accounts(vec![claude_window("claude-owl", true)]);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        pending_use(&mut app, 7, "claude-fox");
        app.handle_server_event(
            account_changed("claude-otter", Some("claude-owl is out of usage")),
            &mut remote,
        );
        assert!(
            app.display_messages()
                .iter()
                .any(|m| m.content.contains("moved from claude-owl to claude-otter")),
            "failover notice was suppressed"
        );
    });
}

/// A new connection drops requests the old one never answered.
#[test]
fn reconnect_clears_pending_account_requests() {
    let mut app = create_test_app();
    pending_use(&mut app, 3, "claude-fox");
    app.reset_account_requests_for_new_connection();
    assert!(app.pending_account_requests.is_empty());
}
