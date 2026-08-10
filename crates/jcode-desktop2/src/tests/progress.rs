//! Background-task progress: from a harness event to a bar on the page.
//!
//! The failure these pin down: the agent backgrounds a long command, waits on
//! it, and the window shows a spinner and a phase label for minutes. The daemon
//! knew the percentage the whole time, so the app was throwing away the one
//! fact the user was waiting for. These tests follow that fact all the way
//! through the fold: an event arrives, a card appears, ticks refine it in
//! place, completion retires it, and the loop wakes often enough to animate a
//! bar that cannot report a number.

use crate::{App, harness, transcript};
use std::time::{Duration, Instant};

fn app_with_harness() -> (App, std::sync::mpsc::Sender<harness::HarnessUpdate>) {
    let (update_tx, update_rx) = std::sync::mpsc::channel();
    let (outgoing_tx, _outgoing_rx) = std::sync::mpsc::channel();
    let mut app = App::default();
    app.model.session_id = Some("session_test".into());
    app.model.donut = None;
    app.harness = Some((update_rx, harness::CommandSender::for_test(outgoing_tx)));
    (app, update_tx)
}

fn tick(task_id: &str, summary: &str, percent: Option<f32>) -> harness::HarnessUpdate {
    harness::HarnessUpdate::Progress {
        task_id: task_id.into(),
        label: "bash".into(),
        summary: summary.into(),
        percent,
        done: false,
    }
}

fn finished(task_id: &str) -> harness::HarnessUpdate {
    harness::HarnessUpdate::Progress {
        task_id: task_id.into(),
        label: "bash".into(),
        summary: "✓ completed · 12.5s".into(),
        percent: None,
        done: true,
    }
}

fn cards(app: &App) -> Vec<&transcript::Message> {
    app.model
        .transcript
        .messages()
        .iter()
        .filter(|message| message.role == transcript::Role::Progress)
        .collect()
}

/// The whole point: a reported percentage reaches the page as a card with a
/// fraction to draw, not as prose the user has to read.
#[test]
fn a_progress_event_puts_a_bar_on_the_page() {
    let (mut app, updates) = app_with_harness();
    updates
        .send(tick("t1", "42% · Running tests", Some(42.0)))
        .expect("queue the tick");
    app.drain_harness_updates();

    let cards = cards(&app);
    assert_eq!(cards.len(), 1, "no progress card reached the transcript");
    assert_eq!(cards[0].fraction(), Some(0.42));
    assert!(
        cards[0].source.contains("Running tests"),
        "the card lost its status line: {}",
        cards[0].source
    );
}

/// A hundred ticks is one card. A row per tick would bury the conversation
/// under the progress of the thing it is waiting for.
#[test]
fn ticks_refine_the_same_card() {
    let (mut app, updates) = app_with_harness();
    for percent in [5.0, 25.0, 75.0, 99.0] {
        updates
            .send(tick("t1", &format!("{percent}%"), Some(percent)))
            .expect("queue the tick");
    }
    app.drain_harness_updates();

    let cards = cards(&app);
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].fraction(), Some(0.99));
}

/// A task that finishes takes its bar with it: a bar left behind claims work is
/// still running, which is exactly the lie the spinner used to tell.
#[test]
fn a_finished_task_retires_its_bar() {
    let (mut app, updates) = app_with_harness();
    updates.send(tick("t1", "50%", Some(50.0))).expect("tick");
    updates.send(tick("t2", "10%", Some(10.0))).expect("tick");
    updates.send(finished("t1")).expect("completion");
    app.drain_harness_updates();

    let ids: Vec<&str> = cards(&app)
        .iter()
        .map(|card| card.call_id.as_deref().expect("a task id"))
        .collect();
    assert_eq!(ids, vec!["t2"], "the finished task kept its bar");
}

/// A backgrounded task outlives the turn that started it, so the turn ending
/// must not wipe the bar: `bg` tasks keep running, and their own completion
/// event is what retires the card.
#[test]
fn a_bar_survives_the_turn_that_started_it() {
    let (mut app, updates) = app_with_harness();
    updates.send(tick("t1", "30%", Some(30.0))).expect("tick");
    updates
        .send(harness::HarnessUpdate::TurnDone)
        .expect("queue the turn end");
    app.drain_harness_updates();

    assert_eq!(
        cards(&app).len(),
        1,
        "the turn ending retired a task that is still running"
    );
    assert!(!app.model.busy, "the turn did not end");
}

/// An indeterminate bar sweeps, so it must pull the loop awake; a page of
/// determinate bars is a still image between ticks and must not.
#[test]
fn an_indeterminate_bar_schedules_wakes_and_a_determinate_one_does_not() {
    let (mut app, updates) = app_with_harness();
    app.model.focused = true;
    app.model.caret = crate::caret::Caret::pinned(true);
    updates
        .send(tick("t1", "working", None))
        .expect("queue the tick");
    app.drain_harness_updates();
    let now = Instant::now();
    let sweeping = app
        .animation_deadline(now)
        .expect("an indeterminate bar scheduled no wake, so it would freeze");
    assert!(
        sweeping <= now + Duration::from_millis(100),
        "the sweep's wake was too far out to look like motion"
    );

    // The same task reporting a number stops needing frames.
    updates
        .send(tick("t1", "60%", Some(60.0)))
        .expect("queue the tick");
    app.drain_harness_updates();
    assert_eq!(
        app.animation_deadline(now),
        None,
        "a determinate bar kept the window awake for nothing"
    );
}

/// Switching sessions must not carry another conversation's bars across: a
/// build belonging to the session being left would be reported as this one's.
#[test]
fn switching_sessions_drops_the_bars() {
    let (mut app, updates) = app_with_harness();
    updates.send(tick("t1", "40%", Some(40.0))).expect("tick");
    app.drain_harness_updates();
    assert_eq!(cards(&app).len(), 1);

    app.model.strips = crate::strip::Strips::build(
        vec![
            crate::strip::Panel::new("session_test", Some("/tmp/a")),
            crate::strip::Panel::new("session_other", Some("/tmp/b")),
        ],
        Some("session_other"),
    );
    app.attach_focused_session();

    assert!(cards(&app).is_empty(), "a bar followed the session switch");
    assert_eq!(
        app.model.progress_clock, None,
        "the bars' clock kept running"
    );
}
