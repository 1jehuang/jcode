use super::*;
use crate::tui::app::PendingRemoteMessage;
use crate::tui::app::commands::handle_cancel_command;
use crate::tui::app::terminal_title::{TurnOutcome, program_state};
use crate::tui::app::tests::create_test_app;
use crate::tui::backend::RemoteConnection;
use crate::tui::program_status::ProgramState;
use std::time::{Duration, Instant};

fn pending_user_message(content: &str) -> PendingRemoteMessage {
    PendingRemoteMessage {
        content: content.into(),
        images: Vec::new(),
        is_system: false,
        system_reminder: None,
        auto_retry: true,
        retry_attempts: 0,
        retry_at: None,
    }
}

/// A guardrail stop is captured when the turn finishes, before the turn-end
/// follow-up scheduler consumes the flag that drives its circuit breaker.
/// Even though `guardrail_stops_exhausted_at_turn_end` clears
/// `turn_guardrail_stopped`, herdr must still see `error`, not `done`.
#[test]
fn guardrail_stop_survives_turn_end_flag_consumption() {
    let mut app = create_test_app();
    app.suppress_terminal_title_updates = true;
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = runtime.enter();
    let mut remote = RemoteConnection::dummy();
    app.is_processing = true;
    app.current_message_id = Some(7);
    app.status = ProcessingStatus::Streaming;
    app.turn_guardrail_stopped = true;

    handle_server_event(&mut app, ServerEvent::Done { id: 7 }, &mut remote);

    // The scheduler took the flag for its consecutive-stop counter.
    assert!(!app.turn_guardrail_stopped);
    let state = app.terminal_title.borrow();
    assert_eq!(state.turn_outcome, TurnOutcome::Error);
    assert_eq!(
        state.turn_error_message.as_deref(),
        Some("provider guardrail stop")
    );
    assert_eq!(
        program_state(app.is_processing(), state.turn_outcome),
        ProgramState::Error
    );
}

/// A rate-limit hold arrives after a turn already looked done. The hold is
/// still live work: herdr must see `working` (retrying), never `done`.
#[test]
fn rate_limit_hold_after_done_reports_working() {
    let mut app = create_test_app();
    app.suppress_terminal_title_updates = true;
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = runtime.enter();
    let mut remote = RemoteConnection::dummy();
    app.is_processing = true;
    app.current_message_id = Some(7);
    app.status = ProcessingStatus::Streaming;
    app.note_turn_done(false);
    app.rate_limit_pending_message = Some(pending_user_message("hello"));

    let message = "429 rate limit exceeded; retry in 60 seconds";
    handle_server_event(
        &mut app,
        ServerEvent::Error {
            id: 7,
            message: message.into(),
            retry_after_secs: None,
        },
        &mut remote,
    );

    assert!(!app.is_processing);
    let state = app.terminal_title.borrow();
    assert_eq!(state.turn_outcome, TurnOutcome::Retrying);
    assert_eq!(state.turn_error_message.as_deref(), Some(message));
    assert_eq!(
        program_state(app.is_processing(), state.turn_outcome),
        ProgramState::Working
    );
}

/// On SSH the turn-end follow-up scheduler returns before consuming the
/// guardrail flag, so it can survive into a later send. A new send must clear
/// both the flag and the stale outcome.
#[test]
fn new_send_clears_stale_guardrail_stop() {
    let mut app = create_test_app();
    app.suppress_terminal_title_updates = true;
    app.turn_guardrail_stopped = true;
    app.note_turn_done(true);

    app.note_turn_started();

    assert!(!app.turn_guardrail_stopped);
    assert_eq!(app.terminal_title.borrow().turn_outcome, TurnOutcome::None);

    app.note_turn_done(app.turn_guardrail_stopped);
    assert_eq!(app.terminal_title.borrow().turn_outcome, TurnOutcome::Done);
}

/// `/cancel` and `/stop` drop a held rate-limit retry. The turn is no longer
/// working, so herdr must see `idle` instead of the stale `retrying` state.
#[test]
fn canceling_a_retry_hold_reports_idle() {
    let mut app = create_test_app();
    app.suppress_terminal_title_updates = true;
    app.rate_limit_reset = Some(Instant::now() + Duration::from_secs(30));
    app.rate_limit_pending_message = Some(pending_user_message("hello"));
    app.note_turn_retrying("rate limited");
    assert!(!app.is_processing);

    handle_cancel_command(&mut app, "/cancel");

    assert!(app.rate_limit_pending_message.is_none());
    let state = app.terminal_title.borrow();
    assert_eq!(state.turn_outcome, TurnOutcome::Interrupted);
    assert_eq!(
        program_state(app.is_processing(), state.turn_outcome),
        ProgramState::Idle
    );
}
