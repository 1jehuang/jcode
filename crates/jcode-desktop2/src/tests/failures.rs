//! What the user sees when things go wrong.
//!
//! The failure that motivated these: with no network connection, a turn simply
//! never produced anything. The activity spinner kept turning, the status line
//! was suppressed because a session *was* attached, and the app looked like it
//! had ignored the message. Every test here is a variant of "a failure must be
//! visible where the user is already looking, and the turn must end".

use crate::{App, harness, transcript};

/// An app with a live update channel, so real `HarnessUpdate`s can be folded in
/// through the same path the window uses.
fn app_with_harness() -> (App, std::sync::mpsc::Sender<harness::HarnessUpdate>) {
    let (update_tx, update_rx) = std::sync::mpsc::channel();
    let (outgoing_tx, _outgoing_rx) = std::sync::mpsc::channel();
    let mut app = App::default();
    app.model.session_id = Some("session_test".into());
    app.harness = Some((update_rx, outgoing_tx));
    (app, update_tx)
}

/// The whole bug: a turn that cannot run must say so *in the conversation*.
/// The status line is deliberately hidden once a session is attached, so a
/// status-only report is invisible in exactly the case that matters.
#[test]
fn a_failed_turn_is_visible_in_the_conversation() {
    let (mut app, updates) = app_with_harness();
    app.model
        .transcript
        .push(transcript::Message::user("summarise the file"));
    updates
        .send(harness::HarnessUpdate::Activity("thinking".into()))
        .expect("queue the activity");
    updates
        .send(harness::HarnessUpdate::Failed(
            "no network connection: dns error".into(),
        ))
        .expect("queue the failure");
    app.drain_harness_updates();

    assert!(
        app.model.transcript.plain_text().contains("no network"),
        "the failure never reached the transcript: {}",
        app.model.transcript.plain_text()
    );
    // The footnote is the second place a user looks, and it must agree.
    assert!(
        app.model.footnote().is_some(),
        "a failure left no footnote either"
    );
}

/// A failure ends the turn. Without this the spinner keeps turning against a
/// turn that already died, which is the "it just hung" half of the report.
#[test]
fn a_failure_stops_the_turn() {
    let (mut app, updates) = app_with_harness();
    updates
        .send(harness::HarnessUpdate::Activity("bash".into()))
        .expect("queue the activity");
    app.drain_harness_updates();
    assert!(app.model.busy, "the turn never started");

    updates
        .send(harness::HarnessUpdate::Failed("provider is down".into()))
        .expect("queue the failure");
    app.drain_harness_updates();
    assert!(
        !app.model.busy,
        "a failed turn is still reported as running"
    );
    assert!(
        app.model.activity.line(std::time::Instant::now()).is_none(),
        "the activity line outlived the turn it described"
    );
}

/// The live tool card claims work is happening right now. A call that failed is
/// not running, so the card must go with the failure rather than being left
/// behind as a permanent lie.
#[test]
fn a_failure_retires_the_live_tool_card() {
    let (mut app, updates) = app_with_harness();
    updates
        .send(harness::HarnessUpdate::Tool {
            call_id: "call_1".into(),
            label: "read the config".into(),
        })
        .expect("queue the tool");
    updates
        .send(harness::HarnessUpdate::Failed(
            "no network connection: dns error".into(),
        ))
        .expect("queue the failure");
    app.drain_harness_updates();

    assert!(
        !app.model
            .transcript
            .messages()
            .iter()
            .any(|message| message.role == transcript::Role::Tool),
        "a dead call's card survived the failure"
    );
}

/// A retrying provider fails once per attempt. The transcript must not become
/// a wall of the same sentence, and the reveal must not be left mid-sweep with
/// nothing arriving.
#[test]
fn repeated_failures_do_not_flood_the_page() {
    let (mut app, updates) = app_with_harness();
    for _ in 0..8 {
        updates
            .send(harness::HarnessUpdate::Failed(
                "no network connection: dns error".into(),
            ))
            .expect("queue the failure");
    }
    app.drain_harness_updates();
    assert_eq!(
        app.model.transcript.messages().len(),
        1,
        "identical failures stacked up"
    );
    assert!(
        !app.model.stream.is_revealing(),
        "a notice left the reveal running with nothing to reveal"
    );
}
