use super::*;
use crate::auth::claude_cli_completion::{self as completion, LoginFlow, LoginSource};
use crate::tui::app::auth::claude_cli_wait::wait_for_login;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

fn save_login(access: &str) -> String {
    crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
        record_id: None,
        label: access.into(),
        access: access.into(),
        refresh: format!("{access}-refresh"),
        expires: i64::MAX,
        email: Some(format!("{access}@example.test")),
        subscription_type: None,
        scopes: vec![],
    })
    .unwrap()
}

fn finish(flow: &LoginFlow, label: &str) {
    let attempt = completion::claim(&flow.path).unwrap();
    crate::auth::claude::set_active_account(label).unwrap();
    let credentials = crate::auth::claude::load_credentials_for_account(label).unwrap();
    let completed = completion::capture(&credentials, LoginSource::Native, None).unwrap();
    completion::finish(&flow.path, &attempt, Some(completed)).unwrap();
}

fn flow(app: &App) -> Arc<LoginFlow> {
    match app.pending_login.as_ref().unwrap() {
        PendingLogin::ClaudeCli { flow, .. } => Arc::clone(flow),
        _ => panic!("expected tracked CLI login"),
    }
}
fn event(flow: &LoginFlow) -> LoginCompleted {
    LoginCompleted {
        provider: format!("claude_cli#{}", flow.attempt),
        success: true,
        message: "completed".into(),
    }
}

#[tokio::test]
async fn ordinary_refresh_and_enter_cannot_complete_a_pending_cli_attempt() {
    let _home = TestHome::new();
    let label = save_login("before-refresh");
    crate::auth::claude::set_active_account(&label).unwrap();
    let receipt = Arc::new(LoginFlow::new().unwrap());
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut waiting = tokio::spawn(wait_for_login(
        Arc::downgrade(&receipt),
        Arc::downgrade(&cancelled),
        Duration::from_secs(3),
    ));
    tokio::task::yield_now().await;
    crate::auth::claude::update_account_tokens(
        &label,
        "rotated-access",
        "rotated-refresh",
        i64::MAX,
    )
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut waiting)
            .await
            .is_err(),
        "background refresh must leave the real wait pending"
    );
    cancelled.store(true, Ordering::Release);
    assert!(
        tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .unwrap()
            .unwrap()
            .is_none()
    );
    assert!(receipt.result().unwrap().is_none());
}

#[tokio::test]
async fn cli_wait_stops_when_cancelled_or_its_pending_attempt_is_dropped() {
    let _home = TestHome::new();
    let receipt = Arc::new(LoginFlow::new().unwrap());
    let cancelled = Arc::new(AtomicBool::new(true));
    assert!(
        wait_for_login(
            Arc::downgrade(&receipt),
            Arc::downgrade(&cancelled),
            Duration::ZERO
        )
        .await
        .is_none()
    );
    let weak = Arc::downgrade(&receipt);
    drop(receipt);
    cancelled.store(false, Ordering::Release);
    assert!(
        wait_for_login(weak, Arc::downgrade(&cancelled), Duration::ZERO)
            .await
            .is_none()
    );
}

#[test]
fn cli_completion_process_helper() {
    let Ok(path) = std::env::var("JCODE_TEST_CLAUDE_COMPLETION") else {
        return;
    };
    let root = std::env::var_os("JCODE_TEST_COMPLETION_ROOT").unwrap();
    assert_eq!(std::env::var_os("JCODE_HOME"), Some(root));
    assert_eq!(std::env::var("JCODE_TEST_SESSION").unwrap(), "1");
    let label = std::env::var("JCODE_TEST_COMPLETION_ACCOUNT").unwrap();
    let attempt = completion::claim(std::path::Path::new(&path)).unwrap();
    crate::auth::claude::set_active_account(&label).unwrap();
    let selected = crate::auth::claude::load_credentials_for_account(&label).unwrap();
    let result = completion::capture(&selected, LoginSource::Native, None).unwrap();
    completion::finish(std::path::Path::new(&path), &attempt, Some(result)).unwrap();
}

#[test]
fn separate_cli_process_completes_onboarding_and_replaces_the_stale_tui_override() {
    let home = TestHome::new();
    let previous = save_login("previous");
    let selected = save_login("selected-by-cli");
    crate::auth::claude::set_active_account(&previous).unwrap();
    crate::auth::claude::trust_native_source().unwrap();
    let mut app = crate::tui::app::tests::create_test_app();
    app.begin_onboarding_flow_at_login();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut receiver = Bus::global().subscribe();
        app.start_claude_code_cli_login();
        let receipt = flow(&app);
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tui::app::auth::claude_login::tests::completion::cli_completion_process_helper",
                "--test-threads=1",
            ])
            .env("JCODE_TEST_CLAUDE_COMPLETION", &receipt.path)
            .env("JCODE_TEST_COMPLETION_ROOT", home._sandbox.root())
            .env("JCODE_TEST_COMPLETION_ACCOUNT", &selected)
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(
            crate::auth::claude::get_active_account_override().as_deref(),
            Some(previous.as_str())
        );
        let login = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(BusEvent::LoginCompleted(login)) = receiver.recv().await
                    && login.provider == format!("claude_cli#{}", receipt.attempt)
                {
                    break login;
                }
            }
        })
        .await
        .unwrap();
        app.handle_login_completed(login);
        assert!(app.pending_login.is_none());
        assert_eq!(
            crate::auth::claude::get_active_account_override().as_deref(),
            Some(selected.as_str())
        );
        assert_eq!(
            crate::auth::claude::load_credentials()
                .unwrap()
                .access_token,
            "selected-by-cli"
        );
        assert_eq!(
            app.recent_authenticated_provider
                .as_ref()
                .map(|(provider, _)| provider.as_str()),
            Some("claude")
        );
        assert!(!matches!(
            app.onboarding_phase(),
            Some(crate::tui::app::onboarding_flow::OnboardingPhase::Login { .. })
        ));
    });
}

#[test]
fn enter_stale_and_cancelled_events_never_claim_an_uncompleted_login() {
    let _home = TestHome::new();
    save_login("already-current");
    let mut app = crate::tui::app::tests::create_test_app();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _runtime = runtime.enter();
    app.start_claude_code_cli_login();
    let receipt = flow(&app);
    let pending = app.pending_login.take().unwrap();
    app.handle_login_input(pending, String::new());
    assert!(app.recent_authenticated_provider.is_none());
    assert!(app.pending_login.is_some());
    app.handle_login_completed(LoginCompleted {
        provider: "claude_cli#old-attempt".into(),
        success: true,
        message: "stale".into(),
    });
    let pending = app.pending_login.take().unwrap();
    app.handle_login_input(pending, "/cancel".into());
    app.handle_login_completed(event(&receipt));
    assert!(app.pending_login.is_none());
    assert!(app.recent_authenticated_provider.is_none());
}

#[test]
fn later_manual_override_or_revoked_approval_prevents_receipt_activation() {
    let _home = TestHome::new();
    let previous = save_login("previous");
    let selected = save_login("selected");
    let manual = save_login("manual-choice");
    crate::auth::claude::set_active_account(&previous).unwrap();
    crate::auth::claude::trust_native_source().unwrap();
    let mut app = crate::tui::app::tests::create_test_app();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _runtime = runtime.enter();
    app.start_claude_code_cli_login();
    let receipt = flow(&app);
    finish(&receipt, &selected);
    crate::auth::claude::set_active_account_override(Some(manual.clone()));
    app.handle_login_completed(event(&receipt));
    assert!(app.recent_authenticated_provider.is_none());
    assert_eq!(
        crate::auth::claude::get_active_account_override().as_deref(),
        Some(manual.as_str())
    );
    app.start_claude_code_cli_login();
    let receipt = flow(&app);
    finish(&receipt, &selected);
    crate::auth::claude::set_active_account_override(Some(manual.clone()));
    crate::config::Config::revoke_external_auth_source(
        crate::auth::claude::CLAUDE_CODE_NATIVE_AUTH_SOURCE_ID,
    )
    .unwrap();
    app.handle_login_completed(event(&receipt));
    assert!(app.recent_authenticated_provider.is_none());
    assert_eq!(
        crate::auth::claude::get_active_account_override().as_deref(),
        Some(manual.as_str())
    );
    assert!(!crate::auth::claude::native_source_allowed());
}

#[test]
fn receipt_is_one_use_across_competing_claimers_and_contains_no_credentials() {
    let _home = TestHome::new();
    let label = save_login("private-receipt-access");
    crate::auth::claude::set_active_account(&label).unwrap();
    crate::auth::claude::trust_native_source().unwrap();
    let receipt = Arc::new(LoginFlow::new().unwrap());
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let workers: Vec<_> = (0..3)
        .map(|_| {
            let receipt = Arc::clone(&receipt);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                completion::claim(&receipt.path).is_ok()
            })
        })
        .collect();
    assert_eq!(
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .filter(|success| *success)
            .count(),
        1
    );
    let selected = crate::auth::claude::load_credentials_for_account(&label).unwrap();
    let completed = completion::capture(&selected, LoginSource::Native, None).unwrap();
    completion::finish(&receipt.path, &receipt.attempt, Some(completed)).unwrap();
    let contents = std::fs::read_to_string(&receipt.path).unwrap();
    assert!(!contents.contains("private-receipt-access"));
    assert!(receipt.result().unwrap().flatten().is_some());
    assert!(completion::finish(&receipt.path, &receipt.attempt, None).is_err());
    assert!(completion::claim(&receipt.path).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&receipt.path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn changed_grant_and_newer_shared_default_reject_a_completed_receipt() {
    let _home = TestHome::new();
    let previous = save_login("previous");
    let selected = save_login("selected");
    let manual = save_login("manual");
    crate::auth::claude::trust_native_source().unwrap();
    let receipt = LoginFlow::new().unwrap();
    finish(&receipt, &selected);
    let completed = receipt.result().unwrap().flatten().unwrap();
    crate::auth::claude::set_active_account_override(Some(previous.clone()));
    crate::auth::claude::update_account_tokens(
        &selected,
        "replacement",
        "replacement-refresh",
        i64::MAX,
    )
    .unwrap();
    assert!(completion::apply(&completed, Some(&previous)).is_err());
    assert_eq!(
        crate::auth::claude::get_active_account_override().as_deref(),
        Some(previous.as_str())
    );
    crate::auth::claude::set_active_account(&manual).unwrap();
    crate::auth::claude::set_active_account_override(Some(previous.clone()));
    assert!(completion::apply(&completed, Some(&previous)).is_err());
    assert_eq!(
        crate::auth::claude::get_active_account_override().as_deref(),
        Some(previous.as_str())
    );
}

struct ChoiceProvider {
    model: std::sync::RwLock<String>,
    api: bool,
    retained_pin: Option<jcode_provider_core::AccountPin>,
}
#[async_trait::async_trait]
impl Provider for ChoiceProvider {
    async fn complete(
        &self,
        _: &[crate::message::Message],
        _: &[crate::message::ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> anyhow::Result<jcode_provider_core::EventStream> {
        anyhow::bail!("receipt tests never make provider requests")
    }
    fn name(&self) -> &str {
        "Claude"
    }
    fn model(&self) -> String {
        self.model.read().unwrap().clone()
    }
    fn set_model(&self, model: &str) -> anyhow::Result<()> {
        *self.model.write().unwrap() = model.into();
        Ok(())
    }
    fn active_explicit_credential(&self) -> Option<jcode_provider_core::ResolvedCredential> {
        Some(if self.api {
            jcode_provider_core::ResolvedCredential::ApiKey
        } else {
            jcode_provider_core::ResolvedCredential::Oauth
        })
    }
    fn active_resolved_credential(&self) -> Option<jcode_provider_core::ResolvedCredential> {
        self.active_explicit_credential()
    }
    fn oauth_account_label(&self) -> Option<String> {
        self.retained_pin.as_ref().map(|pin| pin.label.clone())
    }
    fn oauth_account_pin(&self) -> Option<jcode_provider_core::AccountPin> {
        self.retained_pin.clone()
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            model: std::sync::RwLock::new(self.model()),
            api: self.api,
            retained_pin: self.retained_pin.clone(),
        })
    }
}

#[test]
fn later_model_choice_and_explicit_api_billing_are_preserved() {
    let _home = TestHome::new();
    let previous = save_login("previous");
    let selected = save_login("selected");
    crate::auth::claude::set_active_account(&previous).unwrap();
    crate::auth::claude::trust_native_source().unwrap();
    let mut app = crate::tui::app::tests::create_test_app();
    app.provider = Arc::new(ChoiceProvider {
        model: std::sync::RwLock::new("original".into()),
        api: false,
        retained_pin: None,
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _runtime = runtime.enter();
    app.start_claude_code_cli_login();
    let receipt = flow(&app);
    finish(&receipt, &selected);
    crate::auth::claude::set_active_account_override(Some(previous.clone()));
    app.provider.set_model("manual-new-model").unwrap();
    app.handle_login_completed(event(&receipt));
    assert!(app.recent_authenticated_provider.is_none());
    assert_eq!(app.provider.model(), "manual-new-model");
    assert_eq!(
        crate::auth::claude::get_active_account_override().as_deref(),
        Some(previous.as_str())
    );
    app.provider = Arc::new(ChoiceProvider {
        model: std::sync::RwLock::new("keep-api".into()),
        api: true,
        retained_pin: None,
    });
    app.start_claude_code_cli_login();
    let receipt = flow(&app);
    finish(&receipt, &selected);
    crate::auth::claude::set_active_account_override(Some(previous));
    app.handle_login_completed(event(&receipt));
    assert_eq!(app.provider.model(), "keep-api");
    assert_eq!(
        app.provider.active_resolved_credential(),
        Some(jcode_provider_core::ResolvedCredential::ApiKey)
    );
    assert_eq!(
        app.recent_authenticated_provider
            .as_ref()
            .map(|(provider, _)| provider.as_str()),
        Some("claude")
    );
}

fn stage_remote_login() -> (App, String, String) {
    let previous = save_login("previous-daemon");
    let selected = save_login("selected-daemon");
    crate::auth::claude::set_active_account(&previous).unwrap();
    crate::auth::claude::trust_native_source().unwrap();
    let mut app = crate::tui::app::tests::create_test_app();
    app.is_remote = true;
    app.remote_provider_name = Some("Anthropic".into());
    app.remote_provider_model = Some("claude-sonnet-4-6".into());
    app.remote_provider_oauth_account_label = Some(previous.clone());
    app.remote_resolved_credential = Some(jcode_provider_core::ResolvedCredential::Oauth);
    (app, previous, selected)
}

#[test]
fn daemon_login_requires_both_exact_request_ack_and_explicit_grant_proof() {
    for proof_first in [false, true] {
        let _home = TestHome::new();
        let (mut app, previous, selected) = stage_remote_login();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _runtime = runtime.enter();
        app.start_claude_code_cli_login();
        let receipt = flow(&app);
        finish(&receipt, &selected);
        crate::auth::claude::set_active_account_override(Some(previous));
        app.handle_login_completed(event(&receipt));
        assert!(app.claude_cli_route_queued());
        let proof = app.claude_cli_route_verification().unwrap().unwrap();
        assert_eq!(proof.attempt, receipt.attempt);
        app.pending_route_selection.take().unwrap();
        app.note_claude_cli_route_sent(71);
        app.confirm_claude_cli_grant(70, true);
        app.note_claude_cli_model_ack(70);
        assert!(app.recent_authenticated_provider.is_none());
        app.remote_provider_oauth_account_label = Some(selected.clone());
        app.remote_model_switch_in_flight = false;
        if proof_first {
            app.confirm_claude_cli_grant(71, true);
        } else {
            app.note_claude_cli_model_ack(71);
        }
        app.handle_login_completed(event(&receipt));
        assert!(
            app.recent_authenticated_provider.is_none(),
            "one acknowledgement is insufficient"
        );
        if proof_first {
            app.note_claude_cli_model_ack(71);
        } else {
            app.confirm_claude_cli_grant(71, true);
        }
        app.handle_login_completed(event(&receipt));
        assert!(app.pending_login.is_none());
        assert_eq!(
            app.recent_authenticated_provider
                .as_ref()
                .map(|(provider, _)| provider.as_str()),
            Some("claude")
        );
        assert_eq!(
            crate::auth::claude::get_active_account_override().as_deref(),
            Some(selected.as_str())
        );
    }
}

#[test]
fn daemon_failure_timeout_and_cancel_restore_only_this_attempts_override() {
    for reason in ["proof", "timeout", "cancel", "manual"] {
        let _home = TestHome::new();
        let (mut app, previous, selected) = stage_remote_login();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _runtime = runtime.enter();
        app.start_claude_code_cli_login();
        let receipt = flow(&app);
        finish(&receipt, &selected);
        crate::auth::claude::set_active_account_override(Some(previous.clone()));
        app.handle_login_completed(event(&receipt));
        app.pending_route_selection.take().unwrap();
        app.note_claude_cli_route_sent(81);
        if reason == "manual" {
            crate::auth::claude::set_active_account_override(Some("later-manual".into()));
        }
        if reason == "cancel" {
            let pending = app.pending_login.take().unwrap();
            app.handle_login_input(pending, "/cancel".into());
        } else {
            if reason == "proof" {
                app.confirm_claude_cli_grant(81, false);
            }
            let mut failed = event(&receipt);
            failed.success = false;
            app.handle_login_completed(failed);
        }
        assert!(app.recent_authenticated_provider.is_none());
        assert_eq!(
            crate::auth::claude::get_active_account_override().as_deref(),
            Some(if reason == "manual" {
                "later-manual"
            } else {
                previous.as_str()
            })
        );
        assert!(
            !receipt.path.exists(),
            "late daemon mutations must lose the receipt even while another Arc is held"
        );
        app.confirm_claude_cli_grant(81, true);
        app.note_claude_cli_model_ack(81);
        app.handle_login_completed(event(&receipt));
        assert!(app.recent_authenticated_provider.is_none());
    }
}

#[test]
fn completed_file_login_is_rejected_in_a_different_or_revoked_daemon_source() {
    let _home = TestHome::new();
    let source = crate::auth::claude::ExternalClaudeAuthSource::ClaudeCode;
    let path = source.path().unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    crate::storage::write_json_secret(&path, &serde_json::json!({ "claudeAiOauth": {
        "accessToken": "completed-file-access", "refreshToken": "completed-file-refresh", "expiresAt": i64::MAX,
    }})).unwrap();
    crate::auth::claude::trust_external_auth_source(source).unwrap();
    let credentials = crate::auth::claude::load_claude_code_credentials().unwrap();
    let completed =
        completion::capture(&credentials, LoginSource::File(path.clone()), None).unwrap();
    let receipt = LoginFlow::new().unwrap();
    completion::claim(&receipt.path).unwrap();
    completion::finish(&receipt.path, &receipt.attempt, Some(completed.clone())).unwrap();
    completion::with_verified_receipt(
        &receipt.path,
        &receipt.attempt,
        &completed.fingerprint,
        || Ok(()),
    )
    .unwrap();
    crate::env::set_var(
        "CLAUDE_CONFIG_DIR",
        path.parent().unwrap().join("other-daemon-profile"),
    );
    assert!(
        completion::with_verified_receipt(
            &receipt.path,
            &receipt.attempt,
            &completed.fingerprint,
            || Ok(())
        )
        .is_err()
    );
    crate::env::set_var("CLAUDE_CONFIG_DIR", path.parent().unwrap());
    crate::config::Config::revoke_external_auth_source_for_path(source.source_id(), &path).unwrap();
    assert!(
        completion::with_verified_receipt(
            &receipt.path,
            &receipt.attempt,
            &completed.fingerprint,
            || Ok(())
        )
        .is_err()
    );
}

#[test]
fn receipt_cancellation_serializes_with_the_daemon_activation_commit() {
    let _home = TestHome::new();
    let selected = save_login("serialized-selected");
    crate::auth::claude::trust_native_source().unwrap();
    let receipt = Arc::new(LoginFlow::new().unwrap());
    finish(&receipt, &selected);
    let completed = receipt.result().unwrap().flatten().unwrap();
    let (entered, entry) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let (cancelled, cancellation) = std::sync::mpsc::channel();
    let active = Arc::clone(&receipt);
    let applying = std::thread::spawn(move || {
        completion::with_verified_receipt(
            &active.path,
            &active.attempt,
            &completed.fingerprint,
            || {
                entered.send(()).unwrap();
                released.recv_timeout(Duration::from_secs(3)).unwrap();
                Ok(())
            },
        )
        .unwrap();
    });
    entry.recv_timeout(Duration::from_secs(3)).unwrap();
    let cancel = Arc::clone(&receipt);
    let cancelling = std::thread::spawn(move || {
        cancel.cancel().unwrap();
        cancelled.send(()).unwrap();
    });
    assert!(matches!(
        cancellation.recv_timeout(Duration::from_millis(50)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    release.send(()).unwrap();
    applying.join().unwrap();
    cancelling.join().unwrap();
    cancellation.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(!receipt.path.exists());
}

#[test]
fn identity_pin_tui_model_bookkeeping_preserves_api_billing_with_a_removed_pin() {
    let _home = TestHome::new();
    let label = save_login("inactive-pin");
    let pin = crate::auth::account_pins::claude_pin(&label).unwrap();
    let mut app = crate::tui::app::tests::create_test_app();
    app.provider = Arc::new(ChoiceProvider {
        model: std::sync::RwLock::new("claude-future-identity".into()),
        api: true,
        retained_pin: Some(pin.clone()),
    });
    app.session
        .account_pins
        .insert("claude".into(), pin.clone());
    crate::auth::claude::remove_account(&label).unwrap();
    app.session.route_api_method = Some("claude-api".into());
    app.finalize_model_switch("claude-future-identity");
    assert_eq!(app.session.route_api_method.as_deref(), Some("claude-api"));
    assert_eq!(app.session.account_pins["claude"], pin);
    for stale in ["claude-oauth", "claude-oauth@removed"] {
        app.session.route_api_method = Some(stale.into());
        app.finalize_model_switch("claude-future-identity");
        assert!(app.session.route_api_method.is_none());
    }
    let saved = crate::session::Session::load(&app.session.id).unwrap();
    assert_eq!(saved.account_pins, app.session.account_pins);
}
