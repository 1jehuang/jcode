//! Event-loop scheduling: whether the app wakes to draw the frames its own
//! model says are animating.
//!
//! This exists because the caret did not blink. `Caret::next_toggle_at` was
//! correct and unit-tested, and the loop even set a `WaitUntil` deadline, but
//! nothing repainted when the deadline fired and the deadline was only ever set
//! from inside the redraw handler. Testing the caret's arithmetic could never
//! catch that: the bug lived in the wiring, so the wiring is what these tests
//! pin down.

use crate::{App, caret::Caret};
use std::time::{Duration, Instant};

fn focused_app() -> App {
    let mut app = App::default();
    app.model.focused = true;
    app.model.busy = false;
    // Default nodes may enable the donut, which animates unconditionally and
    // would mask whether the caret contributed a deadline of its own.
    app.model.donut = None;
    app
}

/// The whole point: a focused, idle app must schedule a wake, or the caret can
/// never toggle no matter how correct its arithmetic is.
#[test]
fn a_focused_app_schedules_a_wake_for_the_blink() {
    let app = focused_app();
    let now = Instant::now();
    assert!(
        app.animation_deadline(now).is_some(),
        "no wake scheduled, so the caret would never blink"
    );
}

/// The scheduled wake must land at a moment when the caret's visibility has
/// actually changed. A deadline that fires early would redraw forever while the
/// caret appeared frozen, which looks identical to the original bug.
#[test]
fn the_scheduled_wake_lands_on_a_visibility_change() {
    let mut app = focused_app();
    let start = Instant::now();
    // Start past the solid-after-typing grace period, where it truly blinks.
    app.model.caret = Caret::with_activity(start);
    let mut now = start + crate::caret::SOLID_AFTER_INPUT;
    let mut visible = app.model.caret.visible_at(now);
    for step in 0..4 {
        let at = app
            .animation_deadline(now)
            .unwrap_or_else(|| panic!("no wake scheduled at step {step}"));
        let next = app.model.caret.visible_at(at);
        assert_ne!(
            visible, next,
            "the caret did not change state at its own wake time (step {step})"
        );
        visible = next;
        now = at;
    }
}

/// An idle background window must sleep. Without this, the fix for a static
/// caret would be an app that repaints twice a second forever.
#[test]
fn an_unfocused_window_sleeps() {
    let mut app = focused_app();
    app.model.focused = false;
    assert_eq!(
        app.animation_deadline(Instant::now()),
        None,
        "an unfocused window still scheduled wakes"
    );
}

/// An unfocused window with a turn running must keep waking: the spinner is
/// the proof the agent is alive, and a user watching from another window
/// reads a frozen spinner as a hang.
#[test]
fn an_unfocused_running_turn_still_animates() {
    let mut app = focused_app();
    app.model.focused = false;
    app.model.busy = true;
    let now = Instant::now();
    app.model.activity.start(now);
    let deadline = app
        .animation_deadline(now)
        .expect("an unfocused turn scheduled no frames, so the spinner froze");
    assert!(
        deadline <= now + Duration::from_millis(250),
        "the background spinner cadence is too slow to read as motion"
    );
}

/// A reply streaming into an unfocused window must keep revealing: the text
/// arriving is the information, and freezing it mid-sweep makes the reply
/// look stalled from across the desktop.
#[test]
fn an_unfocused_streaming_reveal_still_animates() {
    let mut app = focused_app();
    app.model.focused = false;
    let now = Instant::now();
    app.model.stream.extend_to(400, now);
    assert!(
        app.animation_deadline(now).is_some(),
        "a reply streaming into a background window froze mid-reveal"
    );
}

/// Background wakes must stay on the timer path, not chain display-paced
/// frames: 10fps is the point of the background cadence, and chaining would
/// silently promote it back to the display rate.
#[test]
fn background_animation_stays_on_the_timer() {
    let mut app = focused_app();
    app.model.focused = false;
    app.model.busy = true;
    let now = Instant::now();
    app.model.activity.start(now);
    app.model.stream.extend_to(400, now);
    assert!(
        !app.wants_display_paced_frame(now),
        "an unfocused window chained display-rate frames"
    );
}

/// The decorative animations stay off in the background: a blinking caret in
/// a window that cannot receive typing lies, and the donut is not worth the
/// GPU while nobody is looking at it.
#[test]
fn an_unfocused_idle_window_still_skips_the_caret_and_donut() {
    let mut app = focused_app();
    app.model.focused = false;
    app.model.donut = Some(crate::donut::Donut::new(8));
    // A fresh caret would blink soon if it were being scheduled.
    app.model.caret = Caret::with_activity(Instant::now());
    assert_eq!(
        app.animation_deadline(Instant::now()),
        None,
        "an unfocused window scheduled wakes for decorative motion"
    );
}

/// While a turn is streaming the composer shows the activity line, so there is
/// no caret to blink and no reason to wake for one. A busy turn with no running
/// activity (the state right after attaching) must still sleep.
#[test]
fn a_busy_turn_does_not_schedule_caret_wakes() {
    let mut app = focused_app();
    app.model.busy = true;
    assert_eq!(
        app.animation_deadline(Instant::now()),
        None,
        "a busy turn scheduled a caret wake for a caret it does not draw"
    );
}

/// A running turn must animate: the whole point of the spinner is that the
/// window does not look frozen while the agent works, which needs frames.
#[test]
fn a_running_turn_schedules_spinner_frames() {
    let mut app = focused_app();
    app.model.donut = None;
    app.model.busy = true;
    let now = Instant::now();
    app.model.activity.start(now);
    let deadline = app
        .animation_deadline(now)
        .expect("a running turn scheduled no frames, so the spinner would freeze");
    assert!(
        deadline <= now + Duration::from_millis(200),
        "the spinner's next frame is too far out to read as motion"
    );
}

/// The spinner stops asking for frames the moment the turn ends, so a finished
/// session goes back to sleep.
#[test]
fn a_finished_turn_stops_the_spinner_wakes() {
    let mut app = focused_app();
    app.model.donut = None;
    app.model.busy = true;
    let now = Instant::now();
    app.model.activity.start(now);
    app.model.activity.finish();
    app.model.busy = false;
    app.model.caret = Caret::pinned(true);
    assert_eq!(
        app.animation_deadline(now),
        None,
        "the app kept waking after the turn ended"
    );
}

/// The donut and the caret share one deadline, and the sooner of the two must
/// win: whichever animation is starved is the one the user sees stutter.
#[test]
fn the_earliest_animation_deadline_wins() {
    let mut app = focused_app();
    let now = Instant::now();
    // Park the caret so its next toggle is far away.
    app.model.caret = Caret::with_activity(now);
    let caret_only = app.animation_deadline(now).expect("no caret wake");
    app.model.donut = Some(crate::donut::Donut::new(8));
    let both = app.animation_deadline(now).expect("no wake with a donut");
    assert!(
        both <= caret_only,
        "adding the donut pushed the next frame later ({both:?} vs {caret_only:?}), \
         so its animation would stutter"
    );
    assert!(
        both <= now + Duration::from_millis(200),
        "the donut's frame deadline is too far out to look like motion"
    );
}

/// A pinned caret is used by capture nodes, which must render deterministically.
/// If a pinned caret still asked for wakes, captures would race the clock.
#[test]
fn a_pinned_caret_needs_no_wakes() {
    let mut app = focused_app();
    app.model.caret = Caret::pinned(true);
    assert_eq!(
        app.animation_deadline(Instant::now()),
        None,
        "a pinned caret scheduled a wake it can never use"
    );
}

/// Continuous animations are paced by the display: the redraw handler chains
/// the next frame immediately instead of waiting for a timer, because a timer
/// beating against the refresh interval skips frames and reads as a hitch.
mod display_pacing {
    use super::*;

    /// The donut is the archetypal continuous animation: while it spins, every
    /// drawn frame must ask for the next one.
    #[test]
    fn a_spinning_donut_chains_frames() {
        let mut app = focused_app();
        app.model.donut = Some(crate::donut::Donut::new(8));
        assert!(
            app.wants_display_paced_frame(Instant::now()),
            "the donut was left to the timer, so its spin will hitch"
        );
    }

    /// A streaming reveal in flight must also chain, or a reply sweeps in at
    /// the timer's beat rather than the display's.
    #[test]
    fn a_streaming_reveal_chains_frames() {
        let mut app = focused_app();
        app.model.donut = None;
        app.model.caret = Caret::pinned(true);
        let now = Instant::now();
        app.model.stream.extend_to(400, now);
        assert!(
            app.wants_display_paced_frame(now),
            "a mid-reveal frame did not chain the next one"
        );
    }

    /// The scroll ease is continuous while the lag decays.
    #[test]
    fn a_scroll_ease_chains_frames() {
        let mut app = focused_app();
        app.model.donut = None;
        app.model.caret = Caret::pinned(true);
        let now = Instant::now();
        app.model.smooth.nudge(80.0, now);
        assert!(
            app.wants_display_paced_frame(now),
            "a scroll mid-ease did not chain the next frame"
        );
    }

    /// The caret's blink is sparse. It must stay on the timer, or an idle
    /// focused window would repaint at the display rate forever.
    #[test]
    fn an_idle_blink_does_not_chain_frames() {
        let mut app = focused_app();
        app.model.donut = None;
        // Fresh activity: the next toggle is a full grace period away.
        app.model.caret = Caret::with_activity(Instant::now());
        assert!(
            !app.wants_display_paced_frame(Instant::now()),
            "an idle blinking caret chained display-rate frames"
        );
    }

    /// Nothing animating, nothing chained: the resting state must sleep.
    #[test]
    fn an_idle_window_does_not_chain_frames() {
        let mut app = focused_app();
        app.model.donut = None;
        app.model.caret = Caret::pinned(true);
        assert!(
            !app.wants_display_paced_frame(Instant::now()),
            "an idle window kept drawing"
        );
    }
}

/// The footnote is the app's only remaining chrome row, so what wins it matters:
/// a single row that shows the wrong one of four possible messages is worse
/// than no row at all.
mod footnote {
    use crate::Model;

    fn attached() -> Model {
        let mut model = Model {
            session_id: Some("session_test".into()),
            status: "attached: session_test".into(),
            ..Model::default()
        };
        // A conversation in progress: the empty page is its own case, since it
        // is where the build version is shown.
        model
            .transcript
            .push(crate::transcript::Message::user("hello"));
        model
    }

    /// The steady state must be silent. This is the whole reason the masthead
    /// went away: "attached: session_..." on every frame forever is noise.
    #[test]
    fn a_healthy_attached_session_shows_no_footnote() {
        let mut model = attached();
        model.meta = crate::meta::Meta {
            version: "v1.2.3".into(),
            update: crate::meta::UpdateState::Current,
            account: Some("someone@example.dev (anthropic)".into()),
        };
        assert_eq!(
            model.footnote(),
            None,
            "an idle healthy session still drew a footnote"
        );
    }

    /// The version is the one fact about the client the window cannot
    /// otherwise answer, so an empty page carries it. It yields to every
    /// actionable message, and disappears as soon as there is a conversation.
    #[test]
    fn an_empty_page_shows_the_client_version() {
        let mut model = attached();
        model.transcript = crate::transcript::Transcript::default();
        model.meta = crate::meta::Meta {
            version: "v1.2.3".into(),
            update: crate::meta::UpdateState::Current,
            account: Some("someone@example.dev (anthropic)".into()),
        };
        assert_eq!(model.footnote().as_deref(), Some("v1.2.3"));
    }

    /// A failure to connect must be visible, or a dead runtime is
    /// indistinguishable from an app that ignores your input.
    #[test]
    fn a_connection_failure_is_reported() {
        let mut model = Model {
            session_id: None,
            status: "disconnected: no such file or directory".into(),
            ..Model::default()
        };
        model.notice = None;
        assert!(
            model
                .footnote()
                .is_some_and(|line| line.contains("disconnected")),
            "a disconnected app said nothing"
        );
    }

    #[test]
    fn a_notice_outranks_everything_else() {
        let mut model = Model {
            session_id: None,
            status: "disconnected: gone".into(),
            scroll: 50.0,
            ..Model::default()
        };
        model.set_notice("nothing to undo");
        assert_eq!(model.footnote().as_deref(), Some("nothing to undo"));
    }

    #[test]
    fn scrollback_outranks_status_and_build_alerts() {
        let mut model = attached();
        model.scroll = 120.0;
        assert!(
            model
                .footnote()
                .is_some_and(|line| line.contains("scrolled back")),
            "scrolling back was not reported"
        );
    }

    /// A build alert is the lowest priority, but must still surface when
    /// nothing else is competing: it is the only signal that a restart or a
    /// login is needed.
    #[test]
    fn a_build_alert_surfaces_when_nothing_else_competes() {
        let mut model = attached();
        model.meta = crate::meta::Meta {
            version: "v1.2.3".into(),
            update: crate::meta::UpdateState::Available,
            account: Some("someone@example.dev (anthropic)".into()),
        };
        assert!(
            model.footnote().is_some_and(|line| line.contains("update")),
            "a pending update was never mentioned anywhere"
        );
    }
}
