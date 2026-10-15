use super::*;
use crate::server::usage_limit_resume::{has_pending_resume, register_pending_resume};

async fn cancel_idle_session(
    control_session_id: &str,
    processing_session_id: Option<&str>,
) -> (InterruptSignal, Vec<ServerEvent>) {
    let stop_signal = InterruptSignal::new();
    let control = SessionControlHandle::cancel_only(
        control_session_id,
        Arc::new(std::sync::Mutex::new(Vec::new())),
        stop_signal.clone(),
    );
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();
    let members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::new()));
    let event_history = Arc::new(RwLock::new(std::collections::VecDeque::new()));
    let event_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (event_tx, _) = broadcast::channel(8);
    let mut processing = false;
    let mut message_id = None;
    let mut session_id = processing_session_id.map(str::to_string);
    let mut task = None;

    cancel_processing_message(
        &mut ProcessingState {
            client_is_processing: &mut processing,
            message_id: &mut message_id,
            session_id: &mut session_id,
            task: &mut task,
        },
        &control,
        &client_event_tx,
        &SwarmStatusRefs {
            members: &members,
            swarms_by_id: &swarms_by_id,
            event_history: &event_history,
            event_counter: &event_counter,
            event_tx: &event_tx,
        },
        Some(1),
        None,
    )
    .await;

    let events = std::iter::from_fn(|| client_event_rx.try_recv().ok()).collect();
    (stop_signal, events)
}

#[tokio::test]
async fn idle_cancel_prevents_the_waiting_turn_from_resuming_after_its_reset() {
    let session_id = "cancel_waiting_turn_after_reset";
    let ticket = register_pending_resume(session_id);
    assert!(!crate::turn_cancel_registry::has_active_turn(session_id));

    let (_, events) = cancel_idle_session(session_id, None).await;

    assert!(matches!(events.as_slice(), [ServerEvent::Interrupted]));
    assert!(
        !ticket.wait(Duration::ZERO).await,
        "a turn acknowledged as interrupted must not resume when its reset arrives"
    );
    assert!(!has_pending_resume(session_id));
}

#[tokio::test]
async fn idle_cancel_uses_the_processing_session_for_a_pending_resume() {
    let session_id = "cancel_waiting_processing_session";
    let ticket = register_pending_resume(session_id);

    cancel_idle_session("cancel_waiting_control_session", Some(session_id)).await;

    assert!(!ticket.wait(Duration::ZERO).await);
}

#[tokio::test]
async fn cancelling_a_pending_resume_preserves_another_sessions_ticket() {
    let session_id = "cancel_waiting_session_only";
    let cancelled = register_pending_resume(session_id);
    let unrelated_id = "cancel_waiting_unrelated_session";
    let unrelated = register_pending_resume(unrelated_id);

    cancel_idle_session(session_id, None).await;

    assert!(!cancelled.wait(Duration::ZERO).await);
    assert!(has_pending_resume(unrelated_id));
    assert!(unrelated.wait(Duration::ZERO).await);
}

#[tokio::test]
async fn cancelling_a_pending_resume_does_not_poison_the_next_turn() {
    let session_id = "cancel_waiting_then_new_turn";
    let ticket = register_pending_resume(session_id);

    let (stop_signal, _) = cancel_idle_session(session_id, None).await;

    assert!(!ticket.wait(Duration::ZERO).await);
    assert!(
        !stop_signal.is_set(),
        "the cancelled wait must not leave a stop signal for the next turn"
    );
}
