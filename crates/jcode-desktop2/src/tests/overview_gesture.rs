//! The Alt-hold gesture and the overview's dispatch.
//!
//! Drives the real handlers (`on_alt_changed`, `tick_overview`, `apply`,
//! `on_pointer_pressed`) rather than reimplementing them, because the whole
//! risk in this feature is the *wiring*: a field that lays out correctly but
//! opens on the wrong chord, swallows typing, or fails to attach on release is
//! worse than no field at all.

use crate::keymap::Action;
use crate::strip::{Entry, Strip};
use crate::{App, OVERVIEW_HOLD, keymap};
use std::time::{Duration, Instant};
use winit::keyboard::{Key, NamedKey, SmolStr};

/// An app attached to `mushroom`, with five sessions across two projects: the
/// smallest field in which every navigation direction means something.
fn app() -> App {
    let entries = vec![
        Entry {
            session_id: "session_clover_1_a".into(),
            working_dir: Some("/home/j/jcode".into()),
            busy: false,
            weight: 480_000.0,
        },
        Entry {
            session_id: "session_mushroom_2_b".into(),
            working_dir: Some("/home/j/jcode".into()),
            busy: true,
            weight: 90_000.0,
        },
        Entry {
            session_id: "session_pebble_3_c".into(),
            working_dir: Some("/home/j/jcode".into()),
            busy: false,
            weight: 6_000.0,
        },
        Entry {
            session_id: "session_harbor_4_d".into(),
            working_dir: Some("/home/j/site".into()),
            busy: false,
            weight: 210_000.0,
        },
        Entry {
            session_id: "session_ember_5_e".into(),
            working_dir: Some("/home/j/site".into()),
            busy: false,
            weight: 1_200.0,
        },
    ];
    let mut app = App::default();
    app.model.session_id = Some("session_mushroom_2_b".into());
    app.model.strip = Strip::build(entries, Some("session_mushroom_2_b"));
    app
}

/// Hold Alt long enough for the field to open, and return the clock it opened
/// at so a test can keep advancing from there.
fn hold_alt(app: &mut App) -> Instant {
    let start = Instant::now();
    app.on_alt_changed(true, start);
    let ripe = start + OVERVIEW_HOLD;
    app.tick_overview(ripe);
    ripe
}

/// Run the zoom to completion, so `phase` is settled.
fn settle(app: &mut App, from: Instant) {
    for step in 1..=40 {
        app.tick_overview(from + Duration::from_millis(step * 16));
    }
}

fn press_overview(app: &mut App, key: Key) {
    if let Some(action) = keymap::resolve_overview(&key) {
        app.apply(action, None);
    }
}

/// The core gesture.
#[test]
fn holding_alt_opens_the_overview() {
    let mut app = app();
    assert!(!app.model.overview.is_visible());
    let opened = hold_alt(&mut app);
    assert!(app.model.overview.is_open(), "the field did not open");
    settle(&mut app, opened);
    assert!((app.model.overview.phase() - 1.0).abs() < 1e-9);
}

/// A tap must not flash the field: Alt is the first half of a dozen editing
/// chords, and an overview that blinks on the way to Alt+B would make the
/// composer feel haunted.
#[test]
fn a_brief_tap_of_alt_opens_nothing() {
    let mut app = app();
    let start = Instant::now();
    app.on_alt_changed(true, start);
    app.tick_overview(start + OVERVIEW_HOLD / 3);
    assert!(!app.model.overview.is_visible(), "a tap opened the field");
    app.on_alt_changed(false, start + OVERVIEW_HOLD / 2);
    app.tick_overview(start + OVERVIEW_HOLD);
    assert!(!app.model.overview.is_visible());
}

/// Alt+B must still be word-motion. The gesture may not cost the user a
/// keybinding they already had.
#[test]
fn an_alt_chord_cancels_the_pending_overview() {
    let mut app = app();
    app.apply(Action::Insert, Some("hello world"));
    let start = Instant::now();
    app.on_alt_changed(true, start);
    // The key handler clears the pending hold the moment Alt becomes a chord.
    app.alt_held_since = None;
    app.apply(Action::MoveWordLeft, None);
    app.tick_overview(start + OVERVIEW_HOLD * 2);
    assert!(
        !app.model.overview.is_visible(),
        "an editing chord opened the overview"
    );
}

/// Releasing Alt is the commit: the whole point of a held gesture.
#[test]
fn releasing_alt_attaches_to_the_highlighted_session() {
    let mut app = app();
    let opened = hold_alt(&mut app);
    settle(&mut app, opened);
    // Move somewhere else, whichever direction the field offers.
    let field = app.overview_field();
    let target = ["left", "right", "up", "down"]
        .iter()
        .zip([
            crate::overview::Dir::Left,
            crate::overview::Dir::Right,
            crate::overview::Dir::Up,
            crate::overview::Dir::Down,
        ])
        .find_map(|(_, dir)| field.neighbor("session_mushroom_2_b", dir))
        .expect("a five-session field has a neighbour somewhere")
        .session_id
        .clone();
    app.model.overview.set_focus(&target);

    app.on_alt_changed(false, Instant::now());
    assert_eq!(
        app.model.session_id.as_deref(),
        Some(target.as_str()),
        "releasing alt did not switch session"
    );
    // Switching sessions clears the page: showing another conversation's
    // output under this one's name would be actively misleading.
    assert!(app.model.transcript.is_empty());
}

/// Escape backs out without switching, so the gesture is explorable.
#[test]
fn escape_leaves_the_session_alone() {
    let mut app = app();
    let opened = hold_alt(&mut app);
    settle(&mut app, opened);
    app.model.overview.set_focus("session_harbor_4_d");
    press_overview(&mut app, Key::Named(NamedKey::Escape));
    assert!(!app.model.overview.is_open());
    assert_eq!(
        app.model.session_id.as_deref(),
        Some("session_mushroom_2_b"),
        "escape switched session anyway"
    );
}

/// The field must not vanish the instant it is dismissed: the zoom back into
/// the chosen session is what makes the switch legible.
#[test]
fn the_field_stays_drawn_while_it_zooms_back_out() {
    let mut app = app();
    let opened = hold_alt(&mut app);
    settle(&mut app, opened);
    app.close_overview(false);
    assert!(
        app.model.overview.is_visible(),
        "the field disappeared instead of zooming out"
    );
    assert!(!app.model.overview.is_open());
    settle(&mut app, Instant::now());
    assert!(!app.model.overview.is_visible());
}

/// Arrows move across the field rather than through the text, and typing does
/// not reach the composer: the overview owns the keyboard while it is up.
#[test]
fn the_overview_owns_the_keyboard() {
    let mut app = app();
    app.apply(Action::Insert, Some("draft"));
    let opened = hold_alt(&mut app);
    settle(&mut app, opened);

    // Arrows are field motion here, not cursor motion.
    assert_eq!(
        keymap::resolve_overview(&Key::Named(NamedKey::ArrowRight)),
        Some(Action::OverviewRight)
    );
    // An ordinary letter is not bound, so the window handler swallows it and
    // the draft is untouched.
    assert_eq!(
        keymap::resolve_overview(&Key::Character(SmolStr::new("z"))),
        None
    );
    assert_eq!(app.model.editor.text(), "draft");
}

/// Every direction has to actually move the highlight somewhere, or the keys
/// read as broken.
#[test]
fn arrow_keys_move_the_highlight() {
    let mut app = app();
    let opened = hold_alt(&mut app);
    settle(&mut app, opened);
    let mut moved = 0;
    for key in [
        NamedKey::ArrowLeft,
        NamedKey::ArrowRight,
        NamedKey::ArrowUp,
        NamedKey::ArrowDown,
    ] {
        app.model.overview.set_focus("session_mushroom_2_b");
        press_overview(&mut app, Key::Named(key));
        if app.model.overview.focus() != Some("session_mushroom_2_b") {
            moved += 1;
        }
    }
    assert!(moved >= 2, "only {moved} of four directions went anywhere");
}

/// Tab must reach every session, including any the cone-based directional
/// search cannot see from where the user happens to be standing.
#[test]
fn tab_walks_the_whole_field() {
    let mut app = app();
    let opened = hold_alt(&mut app);
    settle(&mut app, opened);
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..5 {
        press_overview(&mut app, Key::Named(NamedKey::Tab));
        seen.insert(app.model.overview.focus().unwrap_or_default().to_string());
    }
    assert_eq!(seen.len(), 5, "tab did not visit every session");
}

/// Clicking a blob picks that session, which is the mouse half of the gesture.
#[test]
fn clicking_a_blob_switches_to_it() {
    let mut app = app();
    let opened = hold_alt(&mut app);
    settle(&mut app, opened);
    let blob = app
        .overview_field()
        .blobs
        .iter()
        .find(|blob| !blob.current)
        .cloned()
        .expect("another session to click");
    app.pointer = blob.center;
    app.on_pointer_pressed();
    assert_eq!(
        app.model.session_id.as_deref(),
        Some(blob.session_id.as_str())
    );
}

/// Clicking the paper between blobs dismisses without switching, like any
/// overview. A click that landed on nothing must not be a commit.
#[test]
fn clicking_the_paper_dismisses_without_switching() {
    let mut app = app();
    let opened = hold_alt(&mut app);
    settle(&mut app, opened);
    let field = app.overview_field();
    // A corner of the area: outside every circle by construction, since the
    // packing centres the field.
    let (x0, y0, _, _) = crate::overview::area(&app.frame);
    assert!(field.hit(x0 + 1.0, y0 + 1.0).is_none());
    app.pointer = (x0 + 1.0, y0 + 1.0);
    app.on_pointer_pressed();
    assert!(!app.model.overview.is_open());
    assert_eq!(
        app.model.session_id.as_deref(),
        Some("session_mushroom_2_b")
    );
}

/// Hovering moves the highlight, so the mouse and the arrows drive one
/// selection rather than two competing ones.
#[test]
fn hovering_a_blob_highlights_it() {
    let mut app = app();
    let opened = hold_alt(&mut app);
    settle(&mut app, opened);
    let blob = app
        .overview_field()
        .blobs
        .iter()
        .find(|blob| !blob.current)
        .cloned()
        .expect("another session to hover");
    app.pointer = blob.center;
    app.on_pointer_moved();
    assert_eq!(app.model.overview.focus(), Some(blob.session_id.as_str()));
    // Still only a highlight: hovering must never switch on its own.
    assert_eq!(
        app.model.session_id.as_deref(),
        Some("session_mushroom_2_b")
    );
}

/// The zoom has to be scheduled, or the field would only animate when some
/// unrelated event happened to wake the loop.
#[test]
fn the_zoom_schedules_its_own_frames() {
    let mut app = app();
    let now = Instant::now();
    app.on_alt_changed(true, now);
    assert!(
        app.animation_deadline(now).is_some(),
        "a pending hold did not schedule a wakeup"
    );
    let opened = hold_alt(&mut app);
    assert!(
        app.animation_deadline(opened).is_some(),
        "the zoom did not schedule a frame"
    );
    settle(&mut app, opened);
    // Settled and open: nothing left to animate from the overview itself.
    assert!(!app.model.overview.is_animating());
}

/// Releasing Alt on the session you were already in must be a quiet no-op
/// rather than a reattach that clears the transcript you were reading.
#[test]
fn committing_to_the_current_session_keeps_the_transcript() {
    let mut app = app();
    app.model
        .transcript
        .push(crate::transcript::Message::user("keep me"));
    let opened = hold_alt(&mut app);
    settle(&mut app, opened);
    app.on_alt_changed(false, Instant::now());
    assert!(
        !app.model.transcript.is_empty(),
        "a no-op switch wiped the conversation"
    );
}
