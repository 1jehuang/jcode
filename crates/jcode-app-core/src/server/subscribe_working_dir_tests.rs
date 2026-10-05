//! Tests for the client-send helpers in `subscribe_working_dir.rs`.

use super::*;
use tokio::sync::mpsc;

/// `notify_client` reports delivery, and delivers the event unchanged when it can.
#[tokio::test]
async fn notify_client_delivers_and_reports_true() {
    let (tx, mut rx) = mpsc::unbounded_channel::<&'static str>();

    assert!(notify_client(&tx, "SessionId", "SessionId"));

    assert_eq!(rx.recv().await, Some("SessionId"));
}

/// A disconnected client is not an error to propagate, but it must be reported as a
/// failed delivery. Before the helper existed these sends were `let _ = ...`, so an
/// undeliverable event was indistinguishable from a delivered one at every call site.
#[tokio::test]
async fn notify_client_reports_false_when_the_client_is_gone() {
    let (tx, rx) = mpsc::unbounded_channel::<&'static str>();
    drop(rx);

    assert!(!notify_client(&tx, "Done", "Done"));
}

/// The helper is generic, so it works for a swarm member's event channel as well as the
/// per-client one, and it does not swallow the event on the way through.
#[tokio::test]
async fn notify_client_is_generic_over_the_event_type() {
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerEvent>();

    assert!(notify_client(&tx, ServerEvent::Done { id: 7 }, "Done"));

    match rx.recv().await {
        Some(ServerEvent::Done { id }) => assert_eq!(id, 7),
        other => panic!("expected Done, got {other:?}"),
    }
}
