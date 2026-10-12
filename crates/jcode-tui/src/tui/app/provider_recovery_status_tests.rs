//! F3: relayed provider statuses must drive the real remote error handler.
use super::is_provider_overload_error;
use crate::protocol::ServerEvent;
use crate::tui::app::{App, PendingRemoteMessage};
use crate::tui::backend::RemoteConnection;

fn pending_turn() -> PendingRemoteMessage {
    PendingRemoteMessage {
        content: "continue provider recovery".to_string(),
        images: vec![],
        is_system: false,
        system_reminder: None,
        auto_retry: false,
        retry_attempts: 0,
        overload_attempts: 0,
        retry_at: None,
    }
}

fn handle_error(message: &str) -> App {
    let mut app = crate::tui::app::tests::create_test_app();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    let mut remote = RemoteConnection::dummy();
    app.rate_limit_pending_message = Some(pending_turn());
    app.current_message_id = Some(41);
    app.is_processing = true;
    app.handle_server_event(
        ServerEvent::Error {
            id: 41,
            message: message.to_string(),
            retry_after_secs: None,
            server_resumes: false,
        },
        &mut remote,
    );
    app
}

#[test]
fn relayed_http_529_remote_error_schedules_overload_hold() {
    let app = handle_error("HTTP 529: upstream busy");
    let pending = app.rate_limit_pending_message.as_ref().expect("turn held");
    assert!(
        pending.auto_retry
            && pending.overload_attempts == 1
            && pending.retry_attempts == 0
            && pending.retry_at.is_some()
            && pending.retry_at == app.rate_limit_reset,
        "the user turn must be held on the overload budget"
    );
}

#[test]
fn relayed_http_401_remote_error_does_not_schedule_overload_hold() {
    let app = handle_error("HTTP 401: temporarily unavailable");
    assert!(
        app.rate_limit_reset.is_none()
            && !app
                .rate_limit_pending_message
                .as_ref()
                .is_some_and(|pending| {
                    pending.overload_attempts > 0 || pending.retry_at.is_some()
                }),
        "an explicit authentication failure must not schedule a resend"
    );
}

#[test]
fn permanent_http_statuses_outrank_overload_wording() {
    for status in [400, 401, 402, 403, 404, 405, 406, 422] {
        assert!(
            !is_provider_overload_error(&format!("HTTP {status}: temporarily unavailable")),
            "HTTP {status} must not become an overload hold"
        );
    }
}

#[test]
fn request_metadata_does_not_override_permanent_status() {
    assert!(!is_provider_overload_error(
        "endpoint: HTTP 529\nmodel: status: 503\nauth: HTTP 402\nstatus: 401 temporarily unavailable"
    ));
}

#[test]
fn embedded_status_digits_do_not_schedule_overload() {
    for message in ["status: 5290 upstream busy", "HTTP 529model upstream busy"] {
        assert!(!is_provider_overload_error(message), "{message}");
    }
}
