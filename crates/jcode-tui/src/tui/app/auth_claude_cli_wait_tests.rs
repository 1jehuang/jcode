use super::*;
use crate::tui::app::auth::claude_cli_wait::{selected_login, wait_for_login};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

fn save_login(access: &str, expires: i64) {
    let label = crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
        label: "fresh-login".into(),
        access: access.into(),
        refresh: format!("{access}-refresh"),
        expires,
        email: Some("fixture@example.test".into()),
        subscription_type: None,
        scopes: vec![],
    })
    .unwrap();
    crate::auth::claude::set_active_account(&label).unwrap();
}

#[tokio::test]
async fn cli_wait_requires_a_changed_usable_login_and_bounds_unchanged_waits() {
    let _home = TestHome::new();
    let cancelled = Arc::new(AtomicBool::new(false));
    assert!(selected_login().is_none());
    save_login("expired", 1);
    assert_eq!(
        wait_for_login(None, Arc::downgrade(&cancelled), Duration::ZERO).await,
        Some(false)
    );
    save_login("fresh", i64::MAX);
    assert_eq!(
        wait_for_login(None, Arc::downgrade(&cancelled), Duration::ZERO).await,
        Some(true)
    );
    let before = selected_login();
    assert_eq!(
        wait_for_login(before, Arc::downgrade(&cancelled), Duration::ZERO).await,
        Some(false)
    );
}

#[tokio::test]
async fn cli_wait_stops_on_cancellation_or_dropped_pending_attempt() {
    let _home = TestHome::new();
    save_login("fresh", i64::MAX);
    let cancelled = Arc::new(AtomicBool::new(true));
    assert_eq!(
        wait_for_login(None, Arc::downgrade(&cancelled), Duration::ZERO).await,
        None
    );
    let weak = Arc::downgrade(&cancelled);
    drop(cancelled);
    assert_eq!(wait_for_login(None, weak, Duration::ZERO).await, None);
}

#[test]
fn cli_login_completion_reaches_onboarding_through_the_real_bus_handler() {
    let _home = TestHome::new();
    let mut app = crate::tui::app::tests::create_test_app();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        app.begin_onboarding_flow_at_login();
        let mut receiver = Bus::global().subscribe();
        app.start_claude_code_cli_login();
        assert!(matches!(
            app.pending_login,
            Some(PendingLogin::ClaudeCli { .. })
        ));
        save_login("completed-in-second-terminal", i64::MAX);
        let login = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(BusEvent::LoginCompleted(login)) = receiver.recv().await
                    && login.provider.starts_with("claude_cli#")
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
fn cli_login_ignores_stale_and_cancelled_completion_events() {
    let _home = TestHome::new();
    let mut app = crate::tui::app::tests::create_test_app();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _runtime = runtime.enter();
    app.start_claude_code_cli_login();
    let event_provider = match app.pending_login.as_ref().unwrap() {
        PendingLogin::ClaudeCli { attempt, .. } => format!("claude_cli#{attempt}"),
        _ => panic!("expected tracked CLI login"),
    };
    app.handle_login_completed(LoginCompleted {
        provider: "claude_cli#old-attempt".into(),
        success: true,
        message: "old".into(),
    });
    assert!(app.recent_authenticated_provider.is_none());
    let pending = app.pending_login.take().unwrap();
    let cancelled = match &pending {
        PendingLogin::ClaudeCli { cancelled, .. } => Arc::clone(cancelled),
        _ => unreachable!(),
    };
    app.handle_login_input(pending, "/cancel".into());
    assert!(cancelled.load(Ordering::Acquire));
    app.handle_login_completed(LoginCompleted {
        provider: event_provider,
        success: true,
        message: "late".into(),
    });
    assert!(app.pending_login.is_none());
    assert!(app.recent_authenticated_provider.is_none());
}

#[test]
fn cli_login_can_confirm_an_unchanged_login_after_returning_to_the_tui() {
    let _home = TestHome::new();
    save_login("already-current", i64::MAX);
    let mut app = crate::tui::app::tests::create_test_app();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _runtime = runtime.enter();
    app.start_claude_code_cli_login();
    let pending = app.pending_login.take().unwrap();
    app.handle_login_input(pending, String::new());
    assert!(app.pending_login.is_none());
    assert_eq!(
        app.recent_authenticated_provider
            .as_ref()
            .map(|(provider, _)| provider.as_str()),
        Some("claude")
    );
}
