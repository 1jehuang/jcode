//! The Super-hold gesture and the overview's dispatch.
//!
//! Drives the real handlers (`on_super_changed`, `tick_overview`, `apply`,
//! `overview_keydown`, `on_pointer_pressed`) rather than reimplementing them,
//! because the whole risk in this feature is the *wiring*: a field that lays
//! out correctly but opens on the wrong chord, swallows typing, or fails to
//! attach on release is worse than no field at all.

use crate::keymap::Action;
use crate::strip::{Entry, Strip};
use crate::{App, SUPER_TAP, keymap};
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

/// The gesture ships on: a default app must open the field when Super goes
/// down, because the whole point is that it costs zero configuration.
#[test]
fn the_super_overview_is_enabled_by_default() {
    let mut app = App::default();
    app.on_super_changed(true, Instant::now());
    assert!(
        app.model.overview.is_visible(),
        "the default app did not open the field on Super"
    );
}

/// Benching the flag must turn Super back into a plain chord modifier.
#[test]
fn the_benched_gesture_never_opens() {
    let mut app = App {
        super_overview: false,
        ..App::default()
    };
    app.on_super_changed(true, Instant::now());
    assert!(
        !app.model.overview.is_visible(),
        "the benched overview opened anyway"
    );
}

/// Press Super, which opens the field at once, and return the clock it opened
/// at so a test can keep advancing from there.
fn hold_super(app: &mut App) -> Instant {
    let start = Instant::now();
    app.on_super_changed(true, start);
    app.tick_overview(start);
    start
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
fn holding_super_opens_the_overview() {
    let mut app = app();
    assert!(!app.model.overview.is_visible());
    let opened = hold_super(&mut app);
    assert!(app.model.overview.is_open(), "the field did not open");
    settle(&mut app, opened);
    assert!((app.model.overview.phase() - 1.0).abs() < 1e-9);
}

/// The field is up on the keydown: the gesture may not make the user wait out
/// a threshold before the app admits it heard them.
#[test]
fn super_opens_the_field_on_the_keydown() {
    let mut app = app();
    let start = Instant::now();
    app.on_super_changed(true, start);
    assert!(
        app.model.overview.is_open(),
        "super did not open the field immediately"
    );
}

/// A bare tap leaves everything as it was, and takes the field off screen at
/// once rather than zooming it out: the user was reaching for a chord.
#[test]
fn a_brief_tap_of_super_changes_nothing() {
    let mut app = app();
    let start = Instant::now();
    app.on_super_changed(true, start);
    app.on_super_changed(false, start + SUPER_TAP / 2);
    assert!(!app.model.overview.is_visible(), "a tap left the field up");
    assert_eq!(
        app.model.session_id.as_deref(),
        Some("session_mushroom_2_b"),
        "a tap switched session"
    );
}

/// Super+V must still paste. The gesture may not cost the user a keybinding
/// they already had: a chord key aborts the field and does its normal work.
#[test]
fn a_super_chord_cancels_the_pending_overview() {
    let mut app = app();
    let start = Instant::now();
    app.on_super_changed(true, start);
    app.modifiers = winit::keyboard::ModifiersState::SUPER;
    // 'v' is not an overview key, so the handler aborts the field and runs
    // the chord through the ordinary keymap.
    app.overview_keydown(&Key::Character(SmolStr::new("v")), None);
    app.tick_overview(start + SUPER_TAP * 2);
    assert!(
        !app.model.overview.is_visible(),
        "an editing chord left the overview up"
    );
    // The release that follows must not commit either.
    app.on_super_changed(false, start + SUPER_TAP * 2);
    assert_eq!(
        app.model.session_id.as_deref(),
        Some("session_mushroom_2_b"),
        "the aborted gesture switched session on release"
    );
}

/// Releasing Super is the commit: the whole point of a held gesture.
#[test]
fn releasing_super_attaches_to_the_highlighted_session() {
    let mut app = app();
    let opened = hold_super(&mut app);
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

    app.on_super_changed(false, Instant::now());
    assert_eq!(
        app.model.session_id.as_deref(),
        Some(target.as_str()),
        "releasing super did not switch session"
    );
    // Switching sessions clears the page: showing another conversation's
    // output under this one's name would be actively misleading.
    assert!(app.model.transcript.is_empty());
}

/// Escape backs out without switching, so the gesture is explorable. The
/// Super release that inevitably follows must stay a no-op.
#[test]
fn escape_leaves_the_session_alone() {
    let mut app = app();
    let opened = hold_super(&mut app);
    settle(&mut app, opened);
    app.model.overview.set_focus("session_harbor_4_d");
    press_overview(&mut app, Key::Named(NamedKey::Escape));
    assert!(!app.model.overview.is_open());
    app.on_super_changed(false, Instant::now() + SUPER_TAP * 2);
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
    let opened = hold_super(&mut app);
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
    let opened = hold_super(&mut app);
    settle(&mut app, opened);

    // Arrows are field motion here, not cursor motion.
    assert_eq!(
        keymap::resolve_overview(&Key::Named(NamedKey::ArrowRight)),
        Some(Action::OverviewRight)
    );
    // An ordinary letter is not bound in the field, so the handler treats it
    // as a chord: the field aborts and the draft is untouched.
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
    let opened = hold_super(&mut app);
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

/// Super+HJKL while the field is up must drive the field, not the session
/// strip: the same muscle memory, pointed at what is on screen.
#[test]
fn hjkl_move_the_highlight_while_super_is_held() {
    let mut app = app();
    let opened = hold_super(&mut app);
    settle(&mut app, opened);
    app.modifiers = winit::keyboard::ModifiersState::SUPER;
    let mut moved = 0;
    for letter in ["h", "l", "k", "j"] {
        app.model.overview.set_focus("session_mushroom_2_b");
        app.overview_keydown(&Key::Character(SmolStr::new(letter)), None);
        if app.model.overview.focus() != Some("session_mushroom_2_b") {
            moved += 1;
        }
    }
    assert!(moved >= 2, "only {moved} of hjkl went anywhere");
    assert!(
        app.model.overview.is_open(),
        "field motion keys dismissed the field"
    );
}

/// Tab must reach every session, including any the cone-based directional
/// search cannot see from where the user happens to be standing.
#[test]
fn tab_walks_the_whole_field() {
    let mut app = app();
    let opened = hold_super(&mut app);
    settle(&mut app, opened);
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..5 {
        press_overview(&mut app, Key::Named(NamedKey::Tab));
        seen.insert(app.model.overview.focus().unwrap_or_default().to_string());
    }
    assert_eq!(seen.len(), 5, "tab did not visit every session");
}

/// Clicking a card picks that session, which is the mouse half of the gesture.
#[test]
fn clicking_a_card_switches_to_it() {
    let mut app = app();
    let opened = hold_super(&mut app);
    settle(&mut app, opened);
    let card = app
        .overview_field()
        .cards
        .iter()
        .find(|card| !card.current)
        .cloned()
        .expect("another session to click");
    app.pointer = card.center();
    app.on_pointer_pressed();
    assert_eq!(
        app.model.session_id.as_deref(),
        Some(card.session_id.as_str())
    );
}

/// Clicking the paper between cards dismisses without switching, like any
/// overview. A click that landed on nothing must not be a commit.
#[test]
fn clicking_the_paper_dismisses_without_switching() {
    let mut app = app();
    let opened = hold_super(&mut app);
    settle(&mut app, opened);
    let field = app.overview_field();
    // A corner of the area: outside every card by construction, since the
    // layout centres the rows.
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
fn hovering_a_card_highlights_it() {
    let mut app = app();
    let opened = hold_super(&mut app);
    settle(&mut app, opened);
    let card = app
        .overview_field()
        .cards
        .iter()
        .find(|card| !card.current)
        .cloned()
        .expect("another session to hover");
    app.pointer = card.center();
    app.on_pointer_moved();
    assert_eq!(app.model.overview.focus(), Some(card.session_id.as_str()));
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
    app.on_super_changed(true, now);
    assert!(
        app.animation_deadline(now).is_some(),
        "a pending hold did not schedule a wakeup"
    );
    let opened = hold_super(&mut app);
    assert!(
        app.animation_deadline(opened).is_some(),
        "the zoom did not schedule a frame"
    );
    settle(&mut app, opened);
    // Settled and open: nothing left to animate from the overview itself.
    assert!(!app.model.overview.is_animating());
}

/// Releasing Super on the session you were already in must be a quiet no-op
/// rather than a reattach that clears the transcript you were reading.
#[test]
fn committing_to_the_current_session_keeps_the_transcript() {
    let mut app = app();
    app.model
        .transcript
        .push(crate::transcript::Message::user("keep me"));
    let opened = hold_super(&mut app);
    settle(&mut app, opened);
    app.on_super_changed(false, Instant::now());
    assert!(
        !app.model.transcript.is_empty(),
        "a no-op switch wiped the conversation"
    );
}

/// The whole gesture through the window's own dispatch: hold Super, press one
/// of hjkl, release. The previous tests reach into `overview_keydown` and
/// `apply` directly, so a regression in `key_pressed` (the arm the compositor
/// actually calls) could not fail any of them.
#[test]
fn the_full_super_hjkl_gesture_switches_session_through_key_pressed() {
    for letter in ["h", "l", "k", "j"] {
        let mut app = app();
        app.modifiers = winit::keyboard::ModifiersState::SUPER;
        let opened = hold_super(&mut app);
        settle(&mut app, opened);
        assert!(
            app.model.overview.is_open(),
            "{letter}: holding super did not open the field"
        );
        app.key_pressed(&Key::Character(SmolStr::new(letter)), Some(letter));
        assert!(
            app.model.overview.is_open(),
            "{letter}: motion dismissed the field"
        );
        assert!(
            app.model.editor.text().is_empty(),
            "{letter}: the motion key was typed into the composer"
        );
        let target = app
            .model
            .overview
            .focus()
            .expect("the field always highlights something")
            .to_string();
        app.modifiers = winit::keyboard::ModifiersState::empty();
        app.on_super_changed(false, opened + SUPER_TAP * 4);
        assert_eq!(
            app.model.session_id.as_deref(),
            Some(target.as_str()),
            "{letter}: releasing super did not attach to the highlight"
        );
    }
}

/// At least one of the four directions has to leave the session you started
/// in, or "super hjkl moves me" is false however green the wiring tests are.
#[test]
fn super_hjkl_reaches_other_sessions() {
    let mut reached = std::collections::BTreeSet::new();
    for letter in ["h", "l", "k", "j"] {
        let mut app = app();
        app.modifiers = winit::keyboard::ModifiersState::SUPER;
        let opened = hold_super(&mut app);
        settle(&mut app, opened);
        app.key_pressed(&Key::Character(SmolStr::new(letter)), Some(letter));
        app.modifiers = winit::keyboard::ModifiersState::empty();
        app.on_super_changed(false, opened + SUPER_TAP * 4);
        if app.model.session_id.as_deref() != Some("session_mushroom_2_b") {
            reached.insert(app.model.session_id.clone().unwrap_or_default());
        }
    }
    assert!(
        reached.len() >= 2,
        "super+hjkl only reached {reached:?} from the starting session"
    );
}

/// Everything in one project is the common case in this repo, and it used to
/// make j/k dead: `neighbor` refuses to leave the row. A motion key that does
/// nothing reads as a broken binding, so every direction must move.
#[test]
fn no_direction_is_dead_in_a_single_row_field() {
    for dir in [
        crate::overview::Dir::Up,
        crate::overview::Dir::Down,
        crate::overview::Dir::Left,
        crate::overview::Dir::Right,
    ] {
        let entries = (0..4)
            .map(|i| Entry {
                session_id: format!("session_{i}"),
                working_dir: Some("/home/j/jcode".into()),
                busy: false,
                weight: 1_000.0 * f64::from(i + 1),
            })
            .collect::<Vec<_>>();
        let mut app = App::default();
        app.model.session_id = Some("session_0".into());
        app.model.strip = Strip::build(entries, Some("session_0"));
        let opened = hold_super(&mut app);
        settle(&mut app, opened);
        app.move_overview(dir);
        assert_ne!(
            app.model.overview.focus(),
            Some("session_0"),
            "{dir:?} went nowhere in a single-row field"
        );
    }
}
