use super::*;

#[test]
fn stall_timeout_never_below_two_minutes() {
    // Even with the default 180s provider idle timeout, the client stall
    // guard must give the server-side timeout room to fire first.
    let timeout = stall_timeout();
    assert!(
        timeout >= Duration::from_secs(2 * 60),
        "stall timeout regressed below 2 minutes: {timeout:?}"
    );
    // And it must exceed the max provider idle budget so a healthy silent
    // reasoning stretch is cancelled server-side (visible error + retry)
    // rather than by the client watchdog (issue #434).
    let provider_idle = crate::provider::max_stream_idle_timeout();
    assert!(
        timeout > provider_idle,
        "stall timeout {timeout:?} must exceed provider idle timeout {provider_idle:?}"
    );
}

#[test]
fn format_stall_duration_is_human_readable() {
    assert_eq!(format_stall_duration(Duration::from_secs(90)), "90 seconds");
    assert_eq!(format_stall_duration(Duration::from_secs(120)), "2 minutes");
    assert_eq!(
        format_stall_duration(Duration::from_secs(210)),
        "3.5 minutes"
    );
    assert_eq!(
        format_stall_duration(Duration::from_secs(430)),
        "7.2 minutes"
    );
}

/// The stranded-auto-poke bug: a continuation sits in `queued_messages`
/// with `pending_queued_dispatch` already consumed, so nothing ever sends
/// it while `is_processing()` (queue-aware) keeps the spinner up.
#[test]
fn starved_queued_followup_is_rearmed_after_timeout() {
    let mut app = App::new_for_remote(None);
    app.is_processing = false;
    app.pending_queued_dispatch = false;
    app.queued_messages
        .push(crate::todo::build_auto_poke_message(2));

    // First observation only arms the timer; it must not re-dispatch yet.
    assert!(!detect_starved_queued_followup(&mut app));
    assert!(app.queued_followup_starved_since.is_some());
    assert!(!app.pending_queued_dispatch);

    // Backdate past the timeout to simulate a stranded follow-up.
    app.queued_followup_starved_since =
        Some(Instant::now() - QUEUED_FOLLOWUP_STARVATION_TIMEOUT - Duration::from_secs(1));
    assert!(detect_starved_queued_followup(&mut app));
    assert!(
        app.pending_queued_dispatch,
        "watchdog must re-arm dispatch so the queued poke is actually sent"
    );
    assert!(app.queued_followup_starved_since.is_none());
    assert_eq!(
        app.queued_messages.len(),
        1,
        "watchdog must not drop the queued continuation"
    );
}

/// A live turn (or an already-armed dispatch) is normal, not starvation.
#[test]
fn starvation_watchdog_ignores_healthy_states() {
    let mut app = App::new_for_remote(None);

    // Empty queue: nothing to starve.
    assert!(!detect_starved_queued_followup(&mut app));
    assert!(app.queued_followup_starved_since.is_none());

    // Queued but a turn is in flight: the queue drains at turn end.
    app.queued_messages.push("poke".to_string());
    app.is_processing = true;
    app.queued_followup_starved_since =
        Some(Instant::now() - QUEUED_FOLLOWUP_STARVATION_TIMEOUT - Duration::from_secs(1));
    assert!(!detect_starved_queued_followup(&mut app));
    assert!(
        app.queued_followup_starved_since.is_none(),
        "timer must reset once the state is healthy again"
    );

    // Dispatch already armed: the event loop will send on the next pass.
    app.is_processing = false;
    app.pending_queued_dispatch = true;
    app.queued_followup_starved_since =
        Some(Instant::now() - QUEUED_FOLLOWUP_STARVATION_TIMEOUT - Duration::from_secs(1));
    assert!(!detect_starved_queued_followup(&mut app));
    assert!(app.queued_followup_starved_since.is_none());
}
