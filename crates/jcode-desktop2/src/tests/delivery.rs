//! Delivery marks end to end: submit, acknowledge, and the wiggle's effect on
//! scheduling and pixels.
//!
//! The mechanism is only worth having if it is *honest*. These tests hold the
//! two ways it could lie: a message reported as acknowledged before the agent
//! confirmed it, and an acknowledgement that never shows up because nothing
//! asked for a frame. The visual half checks the card actually moves, because a
//! wiggle that is only in the model is not a wiggle.

use crate::ack::{Delivery, WIGGLE};
use crate::keymap::Action;
use crate::transcript::Role;
use crate::{App, harness};
use std::time::Instant;

fn app_with_session() -> App {
    let mut app = App::default();
    app.model.session_id = Some("session_test".into());
    app
}

fn deliveries(app: &App) -> Vec<Option<Delivery>> {
    app.model
        .transcript
        .messages()
        .iter()
        .filter(|message| message.role == Role::User)
        .map(|message| message.delivery)
        .collect()
}

/// Submitting marks the message as sent and nothing more: at that moment the
/// app has written to a socket, which is not the same as the agent having it.
#[test]
fn a_submitted_message_starts_pending() {
    let mut app = app_with_session();
    app.apply(Action::Insert, Some("hello"));
    app.apply(Action::Submit, None);
    assert_eq!(deliveries(&app), vec![Some(Delivery::Sent)]);
}

/// The acceptance event is what promotes it, and it promotes the *oldest*
/// pending message: the session's queue is a queue.
#[test]
fn acknowledgement_lands_on_the_oldest_pending_message() {
    let mut app = app_with_session();
    for text in ["first", "second"] {
        // Settle the turn between submits: a mid-turn submit queues rather
        // than sends now, and this test is about acks landing on *sent*
        // messages in order.
        app.model.busy = false;
        app.apply(Action::Insert, Some(text));
        app.apply(Action::Submit, None);
    }
    let now = Instant::now();
    assert!(app.model.transcript.acknowledge_oldest_pending(now));
    let marks = deliveries(&app);
    assert!(marks[0].is_some_and(Delivery::is_acked), "{marks:?}");
    assert_eq!(marks[1], Some(Delivery::Sent), "{marks:?}");

    assert!(app.model.transcript.acknowledge_oldest_pending(now));
    assert!(
        deliveries(&app)
            .iter()
            .all(|mark| mark.is_some_and(Delivery::is_acked))
    );
    // A third ack has nothing left to promote; reporting `false` is what lets
    // the caller skip a redraw for another client's message.
    assert!(!app.model.transcript.acknowledge_oldest_pending(now));
}

/// A user message replayed from history carries no mark at all. Marking it
/// "sent" would be a claim about a conversation this window never watched.
#[test]
fn a_history_message_carries_no_delivery_mark() {
    let mut app = app_with_session();
    app.model
        .transcript
        .push(crate::transcript::Message::user("from history"));
    assert_eq!(deliveries(&app), vec![None]);
    assert!(
        !app.model
            .transcript
            .acknowledge_oldest_pending(Instant::now())
    );
}

/// The whole path: the harness update the bridge produces has to reach the
/// transcript, or the UI would sit on "sent" forever with a working backend.
#[test]
fn the_harness_acceptance_update_promotes_the_message() {
    let mut app = app_with_session();
    let (updates, update_rx) = std::sync::mpsc::channel();
    let (commands, _command_rx) = std::sync::mpsc::channel();
    app.harness = Some((update_rx, commands));
    app.apply(Action::Insert, Some("hello"));
    app.apply(Action::Submit, None);
    updates
        .send(harness::HarnessUpdate::MessageAccepted)
        .expect("queue the acceptance");
    app.drain_harness_updates();
    assert!(
        deliveries(&app)[0].is_some_and(Delivery::is_acked),
        "the acceptance event did not reach the transcript"
    );
}

/// A pending message must not animate, and an acknowledged one must ask for
/// frames until its wiggle is over. Without the second half the nod would be
/// invisible on an otherwise idle window, which is the case it exists for.
#[test]
fn the_wiggle_drives_the_animation_deadline() {
    let mut app = app_with_session();
    app.model.focused = true;
    app.apply(Action::Insert, Some("hello"));
    app.apply(Action::Submit, None);
    let now = Instant::now();
    // A submitted turn is busy, which animates the spinner; the interesting
    // question is whether the *ack* alone keeps frames coming, so settle
    // everything else first.
    app.model.busy = false;
    app.model.activity.finish();
    app.model.stream.reveal_all();
    app.model.caret = crate::caret::Caret::pinned(true);
    assert_eq!(
        app.animation_deadline(now),
        None,
        "a pending message animated something"
    );
    app.model.transcript.acknowledge_oldest_pending(now);
    assert!(
        app.animation_deadline(now).is_some(),
        "an acknowledged message asked for no frames"
    );
    assert_eq!(
        app.animation_deadline(now + WIGGLE * 2),
        None,
        "the wiggle never finished"
    );
}

/// The wiggle has to be visible, not merely modelled: render the same message
/// pending and mid-nod and require the card's left edge to have moved. A model
/// field nothing draws is the failure mode this catches.
#[test]
#[ignore = "requires a GPU"]
fn an_acknowledged_card_visibly_moves() {
    use crate::tests::visual::Rendered;

    // Start from the attached node, so the page is a live conversation rather
    // than the boot reveal (which fades the whole transcript in and would
    // dominate the measurement).
    let mut model = crate::states::by_name("attached_empty").expect("attached_empty node");
    model.transcript = crate::transcript::Transcript::default();
    model
        .transcript
        .push(crate::transcript::Message::sent("acknowledge me"));
    model.donut = None;

    let pending = Rendered::new(&model).expect("render the pending card");
    // A quarter through the wiggle is near its first peak, so the card is at
    // its most displaced and the comparison is not measuring a zero crossing.
    let at = Instant::now() - WIGGLE.mul_f64(0.25);
    assert!(model.transcript.acknowledge_oldest_pending(at));
    let acked = Rendered::new(&model).expect("render the acknowledged card");

    // The card is a wash on paper, so its left edge is the first column near
    // the measure that is darker than the page. Sampling a band around
    // `frame.left` keeps the window's own furniture (borders, scrollbar) out of
    // the measurement.
    // The card is a wash on paper, so its left edge is the first column that
    // is darker than the page. Scan the whole transcript region and take the
    // topmost row that inks near the measure, so this does not depend on where
    // the single message happens to be placed.
    let left_edge = |rendered: &Rendered| {
        let frame = rendered.frame;
        let s = frame.scale;
        let from = ((frame.left - 14.0) * s).round().max(0.0) as u32;
        let to = ((frame.left + 14.0) * s).round() as u32;
        let top = (frame.body_top * s).round() as u32;
        let bottom = (frame.body_bottom * s)
            .round()
            .min(f64::from(rendered.height - 1)) as u32;
        for y in top..=bottom {
            let page = rendered.luma(from, y);
            if let Some(x) = (from..to).find(|x| rendered.luma(*x, y) < page - 0.01) {
                return Some(x);
            }
        }
        None
    };
    let (before, after) = (left_edge(&pending), left_edge(&acked));
    assert!(
        before.is_some() && after.is_some(),
        "the user card did not ink at all: {before:?} {after:?}"
    );
    assert_ne!(
        before, after,
        "the acknowledgement wiggle drew nothing: card edge stayed at {before:?}"
    );
}

/// A message typed while the agent is mid-turn must not be sent: the daemon
/// would answer "already processing" and the text would be lost to an error.
/// It waits in the transcript as `Queued`, and nothing goes down the wire.
#[test]
fn a_message_typed_mid_turn_is_queued_not_sent() {
    let mut app = app_with_session();
    let (_updates, update_rx) = std::sync::mpsc::channel();
    let (commands, command_rx) = std::sync::mpsc::channel();
    app.harness = Some((update_rx, commands));

    app.apply(Action::Insert, Some("first"));
    app.apply(Action::Submit, None);
    assert!(matches!(
        command_rx.try_recv(),
        Ok(harness::Command::Send(_))
    ));

    // The turn is running; a second message waits its turn.
    app.apply(Action::Insert, Some("second"));
    app.apply(Action::Submit, None);
    assert_eq!(
        deliveries(&app),
        vec![Some(Delivery::Sent), Some(Delivery::Queued)]
    );
    assert!(
        command_rx.try_recv().is_err(),
        "a queued message was sent into a busy turn"
    );
}

/// The turn ending is what sends a queued message: it is promoted to `Sent`,
/// goes down the wire, and a new turn starts. One per boundary, because the
/// daemon takes one message per turn.
#[test]
fn the_turn_ending_sends_the_oldest_queued_message() {
    let mut app = app_with_session();
    let (updates, update_rx) = std::sync::mpsc::channel();
    let (commands, command_rx) = std::sync::mpsc::channel();
    app.harness = Some((update_rx, commands));

    app.apply(Action::Insert, Some("first"));
    app.apply(Action::Submit, None);
    let _ = command_rx.try_recv();
    for text in ["second", "third"] {
        app.apply(Action::Insert, Some(text));
        app.apply(Action::Submit, None);
    }

    updates
        .send(harness::HarnessUpdate::TurnDone)
        .expect("queue the boundary");
    app.drain_harness_updates();

    // The oldest queued message went, the younger one still waits.
    assert_eq!(
        deliveries(&app),
        vec![
            Some(Delivery::Sent),
            Some(Delivery::Sent),
            Some(Delivery::Queued)
        ]
    );
    match command_rx.try_recv() {
        Ok(harness::Command::Send(content)) => assert_eq!(content, "second"),
        other => panic!("the queued message was not sent: {other:?}"),
    }
    assert!(app.model.busy, "the flushed message did not start a turn");
    assert!(
        command_rx.try_recv().is_err(),
        "more than one queued message was sent at one boundary"
    );
}

/// A failed turn is a turn boundary too: a message queued behind it must not
/// wait forever for a `TurnDone` that will never come.
#[test]
fn a_failure_also_flushes_the_queue() {
    let mut app = app_with_session();
    let (updates, update_rx) = std::sync::mpsc::channel();
    let (commands, command_rx) = std::sync::mpsc::channel();
    app.harness = Some((update_rx, commands));

    app.apply(Action::Insert, Some("first"));
    app.apply(Action::Submit, None);
    let _ = command_rx.try_recv();
    app.apply(Action::Insert, Some("second"));
    app.apply(Action::Submit, None);

    updates
        .send(harness::HarnessUpdate::Failed("provider fell over".into()))
        .expect("queue the failure");
    updates
        .send(harness::HarnessUpdate::TurnDone)
        .expect("queue the boundary");
    app.drain_harness_updates();

    match command_rx.try_recv() {
        Ok(harness::Command::Send(content)) => assert_eq!(content, "second"),
        other => panic!("the queued message was not sent after a failure: {other:?}"),
    }
}

/// Streamed reply text lands *above* the queued messages: they are the future
/// of the conversation, and the current turn's output is its past.
#[test]
fn streamed_text_lands_above_queued_messages() {
    let mut app = app_with_session();
    let (updates, update_rx) = std::sync::mpsc::channel();
    let (commands, _command_rx) = std::sync::mpsc::channel();
    app.harness = Some((update_rx, commands));

    app.apply(Action::Insert, Some("first"));
    app.apply(Action::Submit, None);
    app.apply(Action::Insert, Some("second"));
    app.apply(Action::Submit, None);

    updates
        .send(harness::HarnessUpdate::Text("the reply".into()))
        .expect("queue the delta");
    app.drain_harness_updates();

    let roles: Vec<_> = app
        .model
        .transcript
        .messages()
        .iter()
        .map(|message| (message.role, message.delivery))
        .collect();
    assert_eq!(
        roles,
        vec![
            (Role::User, Some(Delivery::Sent)),
            (Role::Assistant, None),
            (Role::User, Some(Delivery::Queued)),
        ],
        "the reply did not stream in above the queued message"
    );
}

/// An ack can only belong to a message that was actually sent. A queued
/// message must never be promoted straight to `Acked` by someone else's ack.
#[test]
fn an_ack_skips_queued_messages() {
    let mut app = app_with_session();
    app.model
        .transcript
        .push(crate::transcript::Message::queued("waiting"));
    assert!(
        !app.model
            .transcript
            .acknowledge_oldest_pending(Instant::now()),
        "an ack landed on a message that was never sent"
    );
    assert_eq!(deliveries(&app), vec![Some(Delivery::Queued)]);
}

/// The queued tone: fainter than sent, and the acknowledgement ramps to full
/// ink over the wiggle so the nod and the tone change are one event.
#[test]
fn the_delivery_tone_ramps_with_the_acknowledgement() {
    let now = Instant::now();
    assert_eq!(Delivery::Queued.tone(now), crate::ack::PENDING_TONE);
    assert_eq!(Delivery::Sent.tone(now), crate::ack::PENDING_TONE);
    let acked = Delivery::Acked { at: now };
    assert_eq!(acked.tone(now), crate::ack::PENDING_TONE);
    let mid = acked.tone(now + WIGGLE / 2);
    assert!(
        mid > crate::ack::PENDING_TONE && mid < 1.0,
        "mid-wiggle tone did not ramp: {mid}"
    );
    assert_eq!(acked.tone(now + WIGGLE), 1.0);
    assert_eq!(acked.tone(now + WIGGLE * 3), 1.0);
}
