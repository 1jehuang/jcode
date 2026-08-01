//! The settings gear and its panel: dispatch, persistence, and the pixel
//! rules that make the mark findable without making it loud.
//!
//! Split from `actions` so neither file grows unbounded: this is one surface
//! with its own hit testing, and the visual half needs a GPU while the
//! behavioural half does not.

use super::visual::Rendered;
use crate::keymap::Action;
use crate::settings::{ROWS, Row, Settings};
use crate::theme::ThemeMode;
use crate::{App, states};

fn app() -> App {
    let mut app = App::default();
    app.model.session_id = Some("session_test".into());
    // Pinned rather than loaded: a test must not read (or later write) the
    // developer's own saved settings.
    app.model.settings = Settings {
        theme: ThemeMode::Light,
        reasoning: crate::reasoning::ReasoningMode::Current,
        motion: true,
    };
    // The window is pinned to match, so a developer whose saved theme is dark
    // does not see a different starting state than one whose is light.
    app.model.theme_preference = ThemeMode::Light;
    app.model.theme = crate::theme::Theme::print_light();
    app.model
        .transcript
        .set_reasoning_mode(crate::reasoning::ReasoningMode::Current);
    app.model.donut = Some(crate::donut::Donut::new(crate::DONUT_GRID));
    app
}

/// Click at a logical point, the way the window's press handler does.
fn click(app: &mut App, x: f64, y: f64) -> bool {
    app.pointer = (x, y);
    app.settings_press(x, y)
}

fn gear_centre(app: &App) -> (f64, f64) {
    let gear = app.frame.gear();
    (gear.x0 + gear.width() / 2.0, gear.y0 + gear.height() / 2.0)
}

#[test]
fn clicking_the_gear_opens_and_shuts_the_panel() {
    let mut app = app();
    let (x, y) = gear_centre(&app);
    assert!(click(&mut app, x, y), "the gear did not take the click");
    assert!(app.model.panel.is_open(), "the gear did not open the panel");
    click(&mut app, x, y);
    assert!(!app.model.panel.is_open(), "the gear did not shut again");
}

#[test]
fn a_click_off_the_panel_only_dismisses_it() {
    // A dismiss that also acted on whatever was underneath would mean the
    // safest way to close a menu is not safe: aiming at the paper to shut it
    // could drop the caret into the composer.
    let mut app = app();
    app.model.panel.open();
    let (x, y) = (app.frame.left + 4.0, app.frame.composer_top + 4.0);
    assert!(click(&mut app, x, y), "the dismiss click was not consumed");
    assert!(
        !app.model.panel.is_open(),
        "the panel survived a click off it"
    );
}

#[test]
fn the_gear_is_inert_while_the_panel_is_shut() {
    // Everything except the gear's own box must fall through, or the page
    // would have an invisible dead zone in it.
    let mut app = app();
    let composer = (app.frame.left + 20.0, app.frame.composer_top + 10.0);
    assert!(
        !app.settings_press(composer.0, composer.1),
        "a shut panel swallowed a click on the composer"
    );
}

#[test]
fn clicking_a_row_cycles_that_setting_and_leaves_the_panel_open() {
    // A menu that closes on every click makes a three-state setting cost three
    // round trips to the gear.
    let mut app = app();
    app.model.panel.open();
    let band = app.frame.panel_row(ROWS.len(), 0);
    let before = app.model.settings.theme;
    click(
        &mut app,
        band.x0 + band.width() / 2.0,
        band.y0 + band.height() / 2.0,
    );
    assert_ne!(app.model.settings.theme, before, "the row did not cycle");
    assert!(app.model.panel.is_open(), "a row click shut the panel");
}

#[test]
fn every_row_reaches_the_running_window() {
    // The panel must not be a display of values it does not apply: each row is
    // checked against the model field it is supposed to drive.
    let mut app = app();
    let theme_before = app.model.theme;
    app.cycle_setting(0);
    assert_eq!(app.model.theme_preference, app.model.settings.theme);
    assert!(
        app.model.theme != theme_before || app.model.settings.theme == ThemeMode::System,
        "the theme row changed nothing on screen"
    );

    app.cycle_setting(1);
    assert_eq!(
        app.model.transcript.reasoning_mode(),
        app.model.settings.reasoning,
        "the thinking row did not reach the transcript"
    );

    let had_donut = app.model.donut.is_some();
    app.cycle_setting(2);
    assert_eq!(
        app.model.donut.is_some(),
        !had_donut,
        "the motion row did not reach the donut"
    );
    assert_eq!(app.model.donut.is_some(), app.model.settings.motion);
}

#[test]
fn an_out_of_range_row_is_ignored() {
    let mut app = app();
    let before = app.model.settings;
    app.cycle_setting(ROWS.len() + 3);
    assert_eq!(app.model.settings, before);
}

#[test]
fn the_keyboard_chord_toggles_the_panel() {
    let mut app = app();
    app.apply(Action::ToggleSettings, None);
    assert!(app.model.panel.is_open());
    app.apply(Action::ToggleSettings, None);
    assert!(!app.model.panel.is_open());
}

#[test]
fn escape_shuts_the_panel_before_touching_anything_else() {
    let mut app = app();
    app.apply(Action::Insert, Some("a draft"));
    app.model.panel.open();
    app.apply(Action::Cancel, None);
    assert!(!app.model.panel.is_open(), "Escape left the panel up");
    assert!(
        !app.model.editor.is_empty(),
        "Escape reached past the panel and cleared typed work"
    );
}

#[test]
fn the_thinking_chord_and_the_panel_agree() {
    // Two ways to change one setting must not disagree, or the panel would
    // show a value the transcript is not using.
    let mut app = app();
    app.apply(Action::CycleReasoningDisplay, None);
    assert_eq!(
        app.model.settings.reasoning,
        app.model.transcript.reasoning_mode(),
        "the chord changed the transcript behind the panel's back"
    );
}

#[test]
fn hovering_walks_the_rows_and_stops_at_the_edge() {
    let mut app = app();
    app.model.panel.open();
    for index in 0..ROWS.len() {
        let band = app.frame.panel_row(ROWS.len(), index);
        assert!(app.settings_hover(band.x0 + 4.0, band.y0 + band.height() / 2.0));
        assert_eq!(app.model.panel.hover(), Some(index));
    }
    // Off the panel: the highlight goes away rather than sticking to the last
    // row the pointer happened to cross.
    app.settings_hover(app.frame.left + 4.0, app.frame.composer_top + 4.0);
    assert_eq!(app.model.panel.hover(), None);
}

#[test]
fn the_panel_stays_on_paper_at_every_window_size() {
    for size in [(320u32, 240u32), (640, 480), (1100, 720), (3840, 2160)] {
        for scale in [1.0, 1.75, 3.0] {
            let frame = crate::layout::Frame::new(size, scale);
            let panel = frame.panel(ROWS.len());
            assert!(
                panel.x0 >= -0.001 && panel.y0 >= -0.001,
                "{size:?}@{scale}: the panel started off-paper at {panel:?}"
            );
            assert!(
                panel.x1 <= frame.width + 0.001 && panel.y1 <= frame.height + 0.001,
                "{size:?}@{scale}: the panel ran off-paper at {panel:?}"
            );
            let gear = frame.gear();
            assert!(
                gear.y1 <= frame.body_top + 0.001,
                "{size:?}@{scale}: the gear dipped into the transcript"
            );
            assert!(
                gear.x1 <= frame.width + 0.001 && gear.x0 >= 0.0,
                "{size:?}@{scale}: the gear ran off-paper"
            );
            // Every row must be reachable: a row whose own box the hit test
            // does not accept is a setting that cannot be changed.
            for index in 0..ROWS.len() {
                let band = frame.panel_row(ROWS.len(), index);
                let hit = frame.panel_row_at(
                    ROWS.len(),
                    band.x0 + band.width() / 2.0,
                    band.y0 + band.height() / 2.0,
                );
                assert_eq!(
                    hit,
                    Some(index),
                    "{size:?}@{scale}: row {index} was not clickable"
                );
            }
        }
    }
}

#[test]
fn a_row_says_what_it_is_and_what_it_says() {
    // Both halves must be present, or a row is either an unlabelled value or a
    // label with no answer.
    let settings = Settings::default();
    for row in ROWS {
        assert!(!row.label().is_empty());
        assert!(!settings.value(*row).is_empty());
    }
    assert_eq!(ROWS.len(), 3, "the panel grew: is every row worth a click?");
    assert!(ROWS.contains(&Row::Theme));
}

/// The gear has to be findable without being loud: present in the margin, but
/// fainter than the body text it must never compete with.
#[test]
#[ignore = "requires a GPU"]
fn the_gear_is_visible_but_quieter_than_the_text() {
    let model = states::by_name("markdown").expect("markdown node");
    let Some(r) = Rendered::new(&model) else {
        return;
    };
    let gear = r.frame.gear();
    let ink = r.darkest_in(gear.x0, gear.y0, gear.x1, gear.y1);
    assert!(ink < 0.85, "the gear did not paint (darkest luma {ink:.3})");
    let body = r.darkest_in(
        r.frame.left,
        r.frame.body_top,
        r.frame.right,
        r.frame.body_bottom,
    );
    assert!(
        ink > body,
        "the gear ({ink:.3}) is as heavy as the transcript ({body:.3})"
    );
}

/// The panel is a menu over the page, so it must be opaque: text showing
/// through a settings row would make both unreadable.
#[test]
#[ignore = "requires a GPU"]
fn the_open_panel_covers_what_is_under_it() {
    let model = states::by_name("settings_panel").expect("settings_panel node");
    let Some(r) = Rendered::new(&model) else {
        return;
    };
    let panel = r.frame.panel(ROWS.len());
    // Every row must carry ink: a panel drawn with no labels is a blank box.
    for index in 0..ROWS.len() {
        let band = r.frame.panel_row(ROWS.len(), index);
        let ink = r.darkest_in(band.x0 + 4.0, band.y0 + 2.0, band.x1 - 4.0, band.y1 - 2.0);
        assert!(ink < 0.8, "row {index} of the panel is blank ({ink:.3})");
    }
    // And the panel must have an edge, so it reads as a surface rather than as
    // captions floating over the page.
    let border = r.darkest_in(panel.x0, panel.y0, panel.x1, panel.y0 + 1.5);
    assert!(border < 0.95, "the panel has no visible edge ({border:.3})");
}
