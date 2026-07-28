//! Keyboard and pointer dispatch: drives the real `App::apply` and the pointer
//! handlers, so the wiring between keymap, editor, scrolling, selection, and
//! interrupt semantics is covered rather than just the pure modules.

use crate::keymap::Action;
use crate::{App, DOUBLE_CLICK, Model, keymap};
use winit::keyboard::{Key, ModifiersState, NamedKey, SmolStr};

fn app_with(text: &str) -> App {
    let mut app = App::default();
    app.model.session_id = Some("session_test".into());
    app.apply(Action::Insert, Some(text));
    app
}

/// Press a chord the way the window event handler does: resolve it, then
/// apply it. Returns false when the app would exit.
fn press(app: &mut App, key: Key, mods: ModifiersState, typed: Option<&str>) -> bool {
    let action = keymap::resolve(&key, mods).unwrap_or(Action::Insert);
    app.apply(action, typed)
}

fn ch(c: char) -> Key {
    Key::Character(SmolStr::new(c.to_string()))
}

#[test]
fn escape_clears_the_input_instead_of_quitting() {
    // The starter quit the app on Escape, silently losing typed work.
    let mut app = app_with("a draft message");
    assert!(
        press(
            &mut app,
            Key::Named(NamedKey::Escape),
            ModifiersState::empty(),
            None
        ),
        "Escape asked the app to exit"
    );
    assert!(
        app.model.editor.is_empty(),
        "Escape did not clear the input"
    );
}

#[test]
fn escape_on_an_empty_composer_still_does_not_quit() {
    let mut app = App::default();
    assert!(press(
        &mut app,
        Key::Named(NamedKey::Escape),
        ModifiersState::empty(),
        None
    ));
}

#[test]
fn escape_interrupts_a_running_turn_before_clearing_input() {
    let mut app = app_with("keep me");
    app.model.busy = true;
    press(
        &mut app,
        Key::Named(NamedKey::Escape),
        ModifiersState::empty(),
        None,
    );
    assert!(!app.model.busy, "Escape did not interrupt the turn");
    assert_eq!(
        app.model.editor.text(),
        "keep me",
        "Escape cleared the input while interrupting"
    );
}

#[test]
fn ctrl_c_quits_only_when_idle_and_empty() {
    // While busy: interrupt.
    let mut app = App::default();
    app.model.busy = true;
    assert!(press(&mut app, ch('c'), ModifiersState::CONTROL, None));
    assert!(!app.model.busy);

    // With typed text: clear rather than discard the session.
    let mut app = app_with("unsent");
    assert!(press(&mut app, ch('c'), ModifiersState::CONTROL, None));
    assert!(app.model.editor.is_empty());

    // Idle and empty: quit.
    let mut app = App::default();
    assert!(
        !press(&mut app, ch('c'), ModifiersState::CONTROL, None),
        "Ctrl+C on an idle empty composer should quit"
    );
}

#[test]
fn editing_chords_reach_the_editor() {
    let mut app = app_with("alpha beta");
    // Web binding: Ctrl+A selects all rather than jumping the caret home.
    press(&mut app, ch('a'), ModifiersState::CONTROL, None);
    assert_eq!(
        app.model.editor.selected_text(),
        Some("alpha beta"),
        "Ctrl+A did not select all"
    );
    press(
        &mut app,
        Key::Named(NamedKey::Home),
        ModifiersState::empty(),
        None,
    );
    assert_eq!(
        app.model.editor.cursor(),
        0,
        "Home did not go to line start"
    );
    press(&mut app, ch('e'), ModifiersState::CONTROL, None);
    assert_eq!(
        app.model.editor.cursor(),
        10,
        "Ctrl+E did not go to the end"
    );
    press(&mut app, ch('w'), ModifiersState::CONTROL, None);
    assert_eq!(
        app.model.editor.text(),
        "alpha ",
        "Ctrl+W did not cut a word"
    );
    press(&mut app, ch('u'), ModifiersState::CONTROL, None);
    assert!(app.model.editor.is_empty(), "Ctrl+U did not kill to start");
    press(&mut app, ch('z'), ModifiersState::CONTROL, None);
    assert_eq!(app.model.editor.text(), "alpha ", "Ctrl+Z did not undo");
}

#[test]
fn cut_then_paste_round_trips_through_the_clipboard() {
    let mut app = app_with("cut me");
    press(&mut app, ch('x'), ModifiersState::CONTROL, None);
    assert!(app.model.editor.is_empty());
    press(&mut app, ch('v'), ModifiersState::CONTROL, None);
    assert_eq!(
        app.model.editor.text(),
        "cut me",
        "paste did not restore the cut"
    );
}

/// Web Ctrl+C: with something selected it copies, and it must not interrupt a
/// running turn behind the user's back. Only an unselected Ctrl+C interrupts.
#[test]
fn ctrl_c_copies_when_there_is_a_selection_instead_of_interrupting() {
    let mut app = app_with("copy me");
    app.model.busy = true;
    press(&mut app, ch('a'), ModifiersState::CONTROL, None);
    assert!(press(&mut app, ch('c'), ModifiersState::CONTROL, None));
    assert!(
        app.model.busy,
        "Ctrl+C interrupted while copying a selection"
    );
    assert_eq!(
        app.model.editor.text(),
        "copy me",
        "Ctrl+C on a selection must not clear the composer"
    );
    // Now with nothing selected it falls back to interrupting.
    press(
        &mut app,
        Key::Named(NamedKey::ArrowRight),
        ModifiersState::empty(),
        None,
    );
    assert!(press(&mut app, ch('c'), ModifiersState::CONTROL, None));
    assert!(!app.model.busy, "unselected Ctrl+C did not interrupt");
}

/// Ctrl+Z then Ctrl+Shift+Z round-trips an edit, as in any browser field.
#[test]
fn undo_and_redo_round_trip_through_the_app() {
    let mut app = app_with("alpha");
    press(&mut app, ch('x'), ModifiersState::CONTROL, None);
    assert!(app.model.editor.is_empty());
    press(&mut app, ch('z'), ModifiersState::CONTROL, None);
    assert_eq!(
        app.model.editor.text(),
        "alpha",
        "Ctrl+Z did not undo the cut"
    );
    press(
        &mut app,
        ch('z'),
        ModifiersState::CONTROL | ModifiersState::SHIFT,
        None,
    );
    assert!(
        app.model.editor.is_empty(),
        "Ctrl+Shift+Z did not redo the cut"
    );
}

#[test]
fn typing_inserts_at_the_caret_after_moving() {
    let mut app = app_with("ac");
    press(
        &mut app,
        Key::Named(NamedKey::ArrowLeft),
        ModifiersState::empty(),
        None,
    );
    press(&mut app, ch('b'), ModifiersState::empty(), Some("b"));
    assert_eq!(app.model.editor.text(), "abc");
}

#[test]
fn typing_keeps_the_caret_solid() {
    let mut app = App::default();
    press(&mut app, ch('x'), ModifiersState::empty(), Some("x"));
    assert!(
        app.model.caret.visible(),
        "caret was not solid while typing"
    );
}

#[test]
fn history_recall_walks_submitted_messages() {
    let mut app = app_with("first message");
    app.submit_input();
    app.apply(Action::Insert, Some("draft"));
    press(
        &mut app,
        Key::Named(NamedKey::ArrowUp),
        ModifiersState::empty(),
        None,
    );
    assert_eq!(app.model.editor.text(), "first message");
    press(
        &mut app,
        Key::Named(NamedKey::ArrowDown),
        ModifiersState::empty(),
        None,
    );
    assert_eq!(app.model.editor.text(), "draft", "live draft was lost");
}

#[test]
fn submitting_without_a_session_keeps_the_text_and_says_why() {
    let mut app = App::default();
    app.apply(Action::Insert, Some("hello"));
    app.apply(Action::Submit, None);
    assert_eq!(
        app.model.editor.text(),
        "hello",
        "text was discarded while detached"
    );
    assert!(app.model.notice.is_some(), "no notice explained the no-op");
}

/// A conversation long enough to overflow any test region.
fn long_transcript(turns: usize) -> crate::transcript::Transcript {
    use crate::transcript::{Message, Transcript};
    let mut transcript = Transcript::default();
    for n in 1..=turns {
        transcript.push(Message::user(format!("question {n}")));
        transcript.push(Message::assistant(format!(
            "answer {n}. {}",
            "prose that wraps at any sensible width ".repeat(3)
        )));
    }
    transcript
}

#[test]
fn scrolling_clamps_and_returns_to_the_tail() {
    let mut app = App::default();
    app.model.transcript = long_transcript(60);
    app.apply(Action::ScrollTop, None);
    let top = app.model.scroll;
    assert!(top > 0.0, "scrolling up did nothing");
    app.apply(Action::ScrollUp, None);
    assert_eq!(app.model.scroll, top, "scroll ran past the top of history");
    app.apply(Action::ScrollBottom, None);
    assert_eq!(app.model.scroll, 0.0, "did not return to the live tail");
    app.apply(Action::ScrollDown, None);
    assert_eq!(app.model.scroll, 0.0, "scrolled below the tail");
}

#[test]
fn submitting_jumps_back_to_the_live_tail() {
    let mut app = app_with("question");
    app.model.transcript = long_transcript(60);
    app.apply(Action::PageUp, None);
    assert!(app.model.scroll > 0.0);
    app.submit_input();
    assert_eq!(app.model.scroll, 0.0, "reply would stream in off-screen");
}

#[test]
fn a_notice_is_cleared_by_the_next_keypress() {
    let mut app = App::default();
    app.apply(Action::Undo, None);
    assert!(
        app.model.notice.is_some(),
        "undo with empty stack said nothing"
    );
    press(&mut app, ch('a'), ModifiersState::empty(), Some("a"));
    assert!(app.model.notice.is_none(), "stale notice persisted");
}

// --- mouse ---

/// A composer-relative x for the given byte offset, so mouse tests do not
/// hardcode font metrics.
fn x_for_offset(app: &mut App, offset: usize) -> f64 {
    let frame = app.frame;
    x_for_offset_in(app, offset, frame)
}

fn x_for_offset_in(app: &mut App, offset: usize, frame: crate::layout::Frame) -> f64 {
    // Ask the renderer's own layout where that caret is drawn, so mouse tests
    // never hardcode font metrics.
    let source = app.model.editor.text().to_string();
    let caret = composer_layout(app, &source, frame).caret_rect(offset, 1.0);
    frame.composer_text_left() + caret.x0
}

/// The composer's Parley layout for a frame: the same one the renderer and the
/// hit-tester build.
fn composer_layout(
    app: &mut App,
    source: &str,
    frame: crate::layout::Frame,
) -> crate::input::InputLayout {
    crate::input::InputLayout::new(
        &mut app.painter.text,
        source,
        frame.composer_text_width(),
        crate::scene::composer_text_style(&app.model),
        frame.scale,
    )
}

/// Mouse tests need a laid-out window, which needs a GPU surface. Instead
/// of a real window, exercise the offset math and editor transitions
/// directly through the same helpers the events call.
#[test]
fn hit_testing_maps_x_to_the_nearest_character_gap() {
    let mut app = app_with("alpha beta");
    let frame = app.frame;
    let text = app.model.editor.text().to_string();
    let input = composer_layout(&mut app, &text, frame);
    // Clicking left of the text lands at 0; far right lands at the end.
    assert_eq!(input.offset_at_point(-50.0, 1.0), 0);
    assert_eq!(input.offset_at_point(10_000.0, 1.0), text.len());
    // Clicking where a caret is drawn lands on that caret's offset.
    for offset in [0, 1, 5, 6, text.len()] {
        let caret = input.caret_rect(offset, 1.0);
        let y = (caret.y0 + caret.y1) / 2.0;
        assert_eq!(
            input.offset_at_point(caret.x0, y),
            offset,
            "click at the caret for offset {offset} missed"
        );
    }
}

#[test]
fn hit_testing_never_splits_a_character() {
    let mut app = app_with("héllo wörld 🌼");
    let frame = app.frame;
    let text = app.model.editor.text().to_string();
    let input = composer_layout(&mut app, &text, frame);
    for step in 0..200 {
        let offset = input.offset_at_point(step as f64 * 3.0, 1.0);
        assert!(
            text.is_char_boundary(offset),
            "hit test returned a mid-character offset at x {}",
            step as f64 * 3.0
        );
    }
}

/// Logical y in the middle of the composer well.
fn composer_y(app: &App) -> f64 {
    (app.frame.composer_top + app.frame.composer_bottom) / 2.0
}

/// A full press/release at a logical x, through the real handlers.
fn click(app: &mut App, x: f64) {
    let y = composer_y(app);
    app.pointer = (x, y);
    app.on_pointer_pressed();
    app.dragging = false;
}

/// Press at `from`, drag to `to`, release: the real selection gesture.
fn drag(app: &mut App, from: f64, to: f64) {
    let y = composer_y(app);
    app.pointer = (from, y);
    app.on_pointer_pressed();
    app.pointer = (to, y);
    app.on_pointer_moved();
    app.dragging = false;
}

/// Clicking at a character's x must put the caret there, going through the
/// same measurement the renderer uses.
#[test]
fn clicking_places_the_caret_at_the_clicked_character() {
    let mut app = app_with("alpha beta");
    app.model.editor.select_all();
    for offset in [0usize, 3, 6, 10] {
        let x = x_for_offset(&mut app, offset);
        click(&mut app, x);
        assert_eq!(
            app.model.editor.cursor(),
            offset,
            "clicking at offset {offset} placed the caret at {}",
            app.model.editor.cursor()
        );
        assert_eq!(
            app.model.editor.selection(),
            None,
            "a plain click left a selection behind"
        );
    }
}

#[test]
fn clicking_outside_the_composer_is_ignored() {
    let mut app = app_with("alpha beta");
    app.model.editor.place_cursor(2);
    let x = x_for_offset(&mut app, 8);
    // Well above the composer: in the transcript area.
    app.pointer = (x, app.frame.body_top + 4.0);
    app.on_pointer_pressed();
    assert_eq!(
        app.model.editor.cursor(),
        2,
        "a click in the transcript moved the composer caret"
    );
    assert!(!app.dragging, "a click outside the well started a drag");
}

#[test]
fn dragging_selects_the_text_between_press_and_release() {
    let mut app = app_with("alpha beta");
    let from = x_for_offset(&mut app, 0);
    let to = x_for_offset(&mut app, 5);
    drag(&mut app, from, to);
    assert_eq!(app.model.editor.selected_text(), Some("alpha"));
}

#[test]
fn dragging_right_to_left_selects_the_same_range() {
    let mut app = app_with("alpha beta");
    let a = x_for_offset(&mut app, 6);
    let b = x_for_offset(&mut app, 10);
    drag(&mut app, b, a);
    assert_eq!(app.model.editor.selected_text(), Some("beta"));
}

#[test]
fn dragging_above_or_below_the_well_keeps_extending() {
    // Dragging out of the box must not drop the selection.
    let mut app = app_with("alpha beta");
    let from = x_for_offset(&mut app, 0);
    let to = x_for_offset(&mut app, 5);
    app.pointer = (from, composer_y(&app));
    app.on_pointer_pressed();
    let below = app.frame.composer_bottom + 200.0;
    app.pointer = (to, below);
    app.on_pointer_moved();
    assert_eq!(app.model.editor.selected_text(), Some("alpha"));
}

#[test]
fn double_clicking_selects_the_word_under_the_pointer() {
    let mut app = app_with("alpha beta gamma");
    let x = x_for_offset(&mut app, 8);
    click(&mut app, x);
    // Second click at the same spot, within the double-click window.
    app.pointer = (x, composer_y(&app));
    app.on_pointer_pressed();
    app.dragging = false;
    assert_eq!(app.model.editor.selected_text(), Some("beta"));
}

#[test]
fn two_slow_clicks_are_not_a_double_click() {
    let mut app = app_with("alpha beta gamma");
    let x = x_for_offset(&mut app, 8);
    click(&mut app, x);
    // Simulate the gap expiring.
    app.last_click = Some((std::time::Instant::now() - DOUBLE_CLICK * 2, 8));
    app.pointer = (x, composer_y(&app));
    app.on_pointer_pressed();
    assert_eq!(
        app.model.editor.selection(),
        None,
        "two slow clicks selected a word"
    );
}

#[test]
fn shift_clicking_extends_from_the_existing_caret() {
    let mut app = app_with("alpha beta");
    let home = x_for_offset(&mut app, 0);
    click(&mut app, home);
    app.modifiers = ModifiersState::SHIFT;
    let to = x_for_offset(&mut app, 5);
    app.pointer = (to, composer_y(&app));
    app.on_pointer_pressed();
    assert_eq!(app.model.editor.selected_text(), Some("alpha"));
}

#[test]
fn a_pointer_move_without_a_press_changes_nothing() {
    let mut app = app_with("alpha beta");
    app.model.editor.place_cursor(3);
    app.pointer = (x_for_offset(&mut app, 9), composer_y(&app));
    app.on_pointer_moved();
    assert_eq!(app.model.editor.cursor(), 3);
    assert_eq!(app.model.editor.selection(), None);
}

#[test]
fn releasing_ends_the_drag_so_later_moves_do_not_select() {
    let mut app = app_with("alpha beta");
    let from = x_for_offset(&mut app, 0);
    let to = x_for_offset(&mut app, 5);
    drag(&mut app, from, to);
    let selected = app.model.editor.selection();
    app.pointer = (x_for_offset(&mut app, 10), composer_y(&app));
    app.on_pointer_moved();
    assert_eq!(
        app.model.editor.selection(),
        selected,
        "selection changed after the button was released"
    );
}

#[test]
fn clicking_keeps_the_caret_solid() {
    let mut app = app_with("alpha");
    let x = x_for_offset(&mut app, 2);
    click(&mut app, x);
    assert!(app.model.caret.visible(), "caret blinked out on click");
}

/// The frame used for input must be the frame the scene was built with.
/// Rendering records it; this asserts the recorded value matches what
/// `build_scene` would use for the same surface.
#[test]
fn the_recorded_frame_matches_the_rendered_geometry() {
    for (size, scale) in [
        ((1100u32, 720u32), 1.0f64),
        ((2400, 1400), 1.75),
        ((800, 600), 2.0),
    ] {
        let recorded = App::frame_for_model(size, scale, &Model::default());
        let rendered = crate::layout::Frame::new(size, scale);
        assert_eq!(
            recorded, rendered,
            "input geometry diverged from the rendered frame at {size:?} @ {scale}"
        );
    }
}

/// A resize must be reflected in hit-testing, not just in drawing.
#[test]
fn resizing_moves_the_hit_test_with_the_layout() {
    let mut app = app_with("alpha beta");
    let narrow = App::frame_for_model((700, 600), 1.0, &Model::default());
    let wide = App::frame_for_model((2000, 1200), 1.0, &Model::default());
    assert_ne!(
        narrow.left, wide.left,
        "test needs two frames with different columns"
    );
    // The same logical x means different offsets in different layouts, so
    // clicking the same character requires the current frame.
    app.frame = wide;
    let x = x_for_offset_in(&mut app, 5, wide);
    click(&mut app, x);
    assert_eq!(app.model.editor.cursor(), 5);

    app.frame = narrow;
    // Clear the click history: clicking the same offset twice in quick
    // succession is legitimately a double click.
    app.last_click = None;
    let x = x_for_offset_in(&mut app, 5, narrow);
    click(&mut app, x);
    assert_eq!(
        app.model.editor.cursor(),
        5,
        "hit-testing did not follow the resized layout"
    );
}

#[test]
fn the_pointer_becomes_a_text_caret_over_the_composer() {
    let mut app = app_with("alpha");
    // Over the transcript: default arrow.
    app.pointer = (app.frame.left + 10.0, app.frame.body_top + 10.0);
    app.update_cursor_icon();
    assert_eq!(app.cursor_icon, winit::window::CursorIcon::Default);
    // Over the composer: text caret, so the box looks editable.
    app.pointer = (app.frame.left + 10.0, composer_y(&app));
    app.update_cursor_icon();
    assert_eq!(app.cursor_icon, winit::window::CursorIcon::Text);
    // And back.
    app.pointer = (app.frame.left + 10.0, app.frame.body_top + 10.0);
    app.update_cursor_icon();
    assert_eq!(app.cursor_icon, winit::window::CursorIcon::Default);
}

#[test]
fn the_composer_hit_area_matches_the_drawn_well() {
    // The pointer shape and the click target must agree, or the cursor
    // would promise editability where clicking does nothing.
    let mut app = app_with("alpha");
    let f = app.frame;
    let inside = [
        (f.left + 1.0, f.composer_top + 1.0),
        (f.right - 1.0, f.composer_bottom - 1.0),
    ];
    for (x, y) in inside {
        assert!(app.in_composer(x, y));
        assert!(
            app.composer_offset_at(x, y).is_some(),
            "({x}, {y}) shows a text cursor but is not clickable"
        );
    }
    let outside = [
        (f.left + 1.0, f.composer_top - 2.0),
        (f.left + 1.0, f.composer_bottom + 2.0),
        (f.left - 2.0, composer_y(&app)),
        (f.right + 2.0, composer_y(&app)),
    ];
    for (x, y) in outside {
        assert!(!app.in_composer(x, y));
        assert!(
            app.composer_offset_at(x, y).is_none(),
            "({x}, {y}) is clickable but shows no text cursor"
        );
    }
}

#[test]
fn window_geometry_is_remembered_across_resizes() {
    let mut app = App::default();
    app.geometry.width = 1600.0;
    app.geometry.height = 1000.0;
    app.geometry.position = Some((120.0, 40.0));
    let restored = crate::window_state::Geometry::parse(&app.geometry.serialize());
    assert_eq!(restored, app.geometry);
}

#[test]
fn clicking_a_lower_line_lands_on_that_line() {
    let mut app = app_with("first line\nsecond line\nthird line");
    app.frame = App::frame_for_model((1400, 900), 1.0, &app.model);
    let text_top = app.frame.composer_top + crate::layout::COMPOSER_TEXT_OFFSET;
    for (row, expected_line) in [(0usize, 0usize), (1, 1), (2, 2)] {
        let y = text_top + row as f64 * crate::layout::COMPOSER_LINE_HEIGHT + 4.0;
        app.pointer = (app.frame.composer_text_left() + 1.0, y);
        app.last_click = None;
        app.on_pointer_pressed();
        app.dragging = false;
        assert_eq!(
            app.model.editor.cursor_line_col().0,
            expected_line,
            "clicking row {row} landed on the wrong line"
        );
    }
}

#[test]
fn shift_enter_makes_a_new_line_and_enter_still_submits() {
    let mut app = app_with("first");
    app.apply(Action::InsertNewline, None);
    app.apply(Action::Insert, Some("second"));
    assert_eq!(app.model.editor.text(), "first\nsecond");
    app.apply(Action::Submit, None);
    assert!(
        app.model.editor.is_empty(),
        "Enter did not submit a multi-line message"
    );
    assert!(app.model.transcript.plain_text().contains("first\nsecond"));
}

#[test]
fn up_moves_between_lines_before_recalling_history() {
    let mut app = app_with("one");
    app.apply(Action::Submit, None);
    app.apply(Action::Insert, Some("line a"));
    app.apply(Action::InsertNewline, None);
    app.apply(Action::Insert, Some("line b"));
    // Cursor is on line 1: Up moves to line 0, not into history.
    app.apply(Action::HistoryPrev, None);
    assert_eq!(app.model.editor.cursor_line_col().0, 0);
    assert_eq!(app.model.editor.text(), "line a\nline b");
    // Another Up is at the top edge, so now history recall takes over.
    app.apply(Action::HistoryPrev, None);
    assert_eq!(app.model.editor.text(), "one");
}

#[test]
fn down_moves_between_lines_before_returning_from_history() {
    let mut app = app_with("alpha\nbeta");
    app.apply(Action::MoveHome, None);
    app.apply(Action::HistoryPrev, None); // already at top, no history
    app.apply(Action::HistoryNext, None);
    assert_eq!(
        app.model.editor.cursor_line_col().0,
        1,
        "Down did not move to the next line"
    );
}

/// Wrapped rows must fit the well they were wrapped to. Parley wraps to a
/// pixel width, so this is the invariant that replaced the old character
/// budget: no laid-out line may be wider than the space available for text.
#[test]
fn wrapped_rows_fit_inside_the_composer_well() {
    let long = "word ".repeat(80);
    let mut app = app_with(&long);
    for &(size, scale) in &[
        ((1400u32, 900u32), 1.0f64),
        ((700, 600), 1.75),
        ((2000, 1200), 2.0),
    ] {
        let frame = crate::layout::Frame::new(size, scale);
        let source = app.model.editor.text().to_string();
        let usable = frame.composer_text_width();
        let input = composer_layout(&mut app, &source, frame);
        let lines = input.lines();
        assert!(lines.len() > 1, "a long line did not wrap at {size:?}");
        for line in &lines {
            let right = input.caret_rect(line.end, 1.0).x0;
            assert!(
                right <= usable + 1.0,
                "a wrapped row reached {right:.1}px but only {usable:.1}px fit"
            );
        }
    }
}

#[test]
fn a_long_line_wraps_into_multiple_rows_and_grows_the_well() {
    let long = "word ".repeat(60);
    let app = app_with(&long);
    let mut app = app;
    let single = crate::layout::Frame::new((1400, 900), 1.0);
    let source = app.model.editor.text().to_string();
    let lines = composer_layout(&mut app, &source, single).line_count();
    assert!(lines > 1, "a long line did not wrap");
    let frame = App::frame_for_model((1400, 900), 1.0, &app.model);
    // Height, not an edge: a hero page grows the well downward.
    let grew =
        frame.composer_bottom - frame.composer_top > single.composer_bottom - single.composer_top;
    assert!(grew, "the well did not grow for wrapped rows");
    // Wrapping is a view concern: it must not touch the buffer.
    assert_eq!(app.model.editor.text(), long);
    assert_eq!(app.model.editor.line_count(), 1);
}

#[test]
fn clicking_a_wrapped_row_lands_on_that_row() {
    let long = "alpha bravo charlie delta echo foxtrot golf hotel india juliet ".repeat(3);
    let mut app = app_with(&long);
    app.frame = App::frame_for_model((900, 800), 1.0, &app.model);
    let frame = app.frame;
    let source = app.model.editor.text().to_string();
    let rows = composer_layout(&mut app, &source, frame).lines();
    assert!(rows.len() > 1, "test needs a wrapped input");
    let text_top = frame.composer_top + crate::layout::COMPOSER_TEXT_OFFSET;
    for (index, row) in rows.iter().enumerate().take(3) {
        // Aim at the vertical middle of the row Parley actually laid out.
        let y = text_top + (row.top + row.bottom) / 2.0;
        app.pointer = (frame.composer_text_left() + 1.0, y);
        app.last_click = None;
        app.on_pointer_pressed();
        app.dragging = false;
        assert_eq!(
            app.model.editor.cursor(),
            row.start,
            "clicking the start of wrapped row {index} missed"
        );
    }
}

#[test]
fn the_composer_frame_follows_the_input_line_count() {
    let mut app = app_with("one");
    let single = App::frame_for_model((1400, 900), 1.0, &app.model);
    app.apply(Action::InsertNewline, None);
    app.apply(Action::Insert, Some("two"));
    let double = App::frame_for_model((1400, 900), 1.0, &app.model);
    // Height, not an edge: a hero page grows the well downward.
    let grew =
        double.composer_bottom - double.composer_top > single.composer_bottom - single.composer_top;
    assert!(grew, "the composer did not grow when a line was added");
}

#[test]
fn hit_testing_uses_the_frame_that_was_actually_drawn() {
    // If input used a different frame than the renderer, clicks would land
    // in the wrong place after a resize.
    let mut app = app_with("alpha beta");
    app.frame = crate::layout::Frame::new((2400, 1400), 2.0);
    let frame = app.frame;
    let x = x_for_offset_in(&mut app, 5, frame);
    click(&mut app, x);
    assert_eq!(app.model.editor.cursor(), 5);
}

#[test]
fn the_wheel_scrolls_and_clamps_like_the_keyboard() {
    let mut app = App::default();
    app.model.transcript = long_transcript(60);
    let max = app.max_scroll();
    app.model.scroll_up(30.0, max);
    assert_eq!(app.model.scroll, 30.0);
    app.model.scroll_down(9_000.0);
    assert_eq!(app.model.scroll, 0.0, "wheel scrolled past the tail");
}

#[test]
fn copy_prefers_the_selection_over_the_whole_line() {
    let mut app = app_with("alpha beta");
    app.model.editor.place_cursor(0);
    app.model.editor.extend_to(5);
    app.apply(Action::Copy, None);
    assert_eq!(app.clipboard.get().as_deref(), Some("alpha"));
}

#[test]
fn cut_removes_only_the_selection_when_there_is_one() {
    let mut app = app_with("alpha beta");
    app.model.editor.place_cursor(0);
    app.model.editor.extend_to(6);
    app.apply(Action::CutLine, None);
    assert_eq!(app.model.editor.text(), "beta");
    assert_eq!(app.clipboard.get().as_deref(), Some("alpha "));
}

#[test]
fn shift_arrow_selection_then_typing_replaces_it() {
    let mut app = app_with("alpha beta");
    app.apply(Action::MoveHome, None);
    for _ in 0..5 {
        app.apply(Action::ExtendRight, None);
    }
    app.apply(Action::Insert, Some("omega"));
    assert_eq!(app.model.editor.text(), "omega beta");
}

#[test]
fn select_all_then_delete_empties_the_composer() {
    let mut app = app_with("throw this away");
    app.apply(Action::SelectAll, None);
    app.apply(Action::DeleteBack, None);
    assert!(app.model.editor.is_empty());
    app.apply(Action::Undo, None);
    assert_eq!(app.model.editor.text(), "throw this away");
}

/// `--script` is the manual verification path; if the chord spellings it
/// accepts drift from the parity table, scripted checks silently stop
/// testing what they claim to.
#[test]
fn every_ported_chord_is_scriptable() {
    for row in keymap::PORTED {
        // The table documents some rows as alternatives (e.g. "ctrl+j / ...")
        // only in NOT_PORTED; ported rows must all parse.
        assert!(
            keymap::parse_chord(row.chord).is_some(),
            "ported chord '{}' cannot be scripted",
            row.chord
        );
    }
}

/// Every action must be dispatchable without panicking, including on an
/// empty model: a crash on an edge key is worse than a no-op.
#[test]
fn every_action_is_safe_on_an_empty_model() {
    let actions = [
        Action::Insert,
        Action::Submit,
        Action::InsertNewline,
        Action::MoveLeft,
        Action::MoveRight,
        Action::MoveWordLeft,
        Action::MoveWordRight,
        Action::MoveHome,
        Action::MoveEnd,
        Action::DeleteBack,
        Action::DeleteForward,
        Action::DeleteWordBack,
        Action::DeleteWordForward,
        Action::KillToStart,
        Action::KillToEnd,
        Action::CutLine,
        Action::Undo,
        Action::Copy,
        Action::Paste,
        Action::HistoryPrev,
        Action::HistoryNext,
        Action::ScrollUp,
        Action::ScrollDown,
        Action::PageUp,
        Action::PageDown,
        Action::ScrollTop,
        Action::ScrollBottom,
        Action::Cancel,
    ];
    for action in actions {
        let mut app = App::default();
        app.apply(action, Some("x"));
    }
}

/// Every chord in the parity table must survive real dispatch.
#[test]
fn every_ported_chord_dispatches_without_panicking() {
    for row in keymap::PORTED {
        let mut app = app_with("alpha beta gamma");
        app.apply(row.action, Some("x"));
    }
}

/// The donut's interaction, driven through the real pointer handlers.
mod donut {
    use super::*;

    /// Logical centre of the donut in the current frame.
    fn donut_centre(app: &App) -> (f64, f64) {
        let box_ = app.frame.hero().expect("a hero in the test frame").donut;
        ((box_.x0 + box_.x1) / 2.0, (box_.y0 + box_.y1) / 2.0)
    }

    fn app_on_empty_session() -> App {
        let mut app = App::default();
        app.model.session_id = Some("session_test".into());
        app
    }

    #[test]
    fn dragging_the_donut_spins_it_and_coasts_to_rest() {
        let mut app = app_on_empty_session();
        let (cx, cy) = donut_centre(&app);
        app.pointer = (cx, cy);
        app.on_pointer_pressed();
        assert!(
            app.model.spin.dragging,
            "press on the donut must start a drag"
        );
        app.pointer = (cx + 60.0, cy);
        app.on_pointer_moved();
        assert!(app.model.spin.velocity > 0.0, "drag must impart spin");
        app.model.spin.release();
        for _ in 0..500 {
            app.model.spin.advance(1.0 / 60.0);
        }
        assert_eq!(app.model.spin.velocity, 0.0, "spin must come to rest");
        assert!(app.model.spin.offset > 0.0);
    }

    #[test]
    fn dragging_the_donut_never_touches_the_composer() {
        let mut app = app_on_empty_session();
        app.apply(Action::Insert, Some("keep me"));
        let cursor = app.model.editor.cursor();
        let (cx, cy) = donut_centre(&app);
        app.pointer = (cx, cy);
        app.on_pointer_pressed();
        app.pointer = (cx + 40.0, cy);
        app.on_pointer_moved();
        assert_eq!(app.model.editor.text(), "keep me");
        assert_eq!(app.model.editor.cursor(), cursor);
        assert!(app.model.editor.selection().is_none());
    }

    #[test]
    fn a_click_in_the_composer_does_not_spin_the_donut() {
        let mut app = app_on_empty_session();
        app.apply(Action::Insert, Some("hello"));
        let x = app.frame.left + 20.0;
        click(&mut app, x);
        assert!(!app.model.spin.dragging);
        assert_eq!(app.model.spin.velocity, 0.0);
    }

    #[test]
    fn the_donut_stands_down_once_there_is_a_transcript() {
        let mut app = app_on_empty_session();
        assert!(app.donut_visible());
        app.model
            .transcript
            .push(crate::transcript::Message::user("hi"));
        app.model.transcript.append_assistant("hello");
        assert!(!app.donut_visible(), "content must take the donut's space");
        // And a press where the donut was is inert, not a hidden hit target.
        let (cx, cy) = donut_centre(&app);
        app.pointer = (cx, cy);
        app.on_pointer_pressed();
        assert!(!app.model.spin.dragging);
    }

    #[test]
    fn an_unfocused_or_hidden_donut_does_not_animate() {
        let mut app = app_on_empty_session();
        assert!(app.donut_animating());
        app.model.focused = false;
        assert!(
            !app.donut_animating(),
            "decorative motion must idle when unfocused"
        );
        app.model.focused = true;
        app.model.donut = None;
        assert!(!app.donut_animating());
        // And animating is then a no-op rather than advancing a clock nobody sees.
        let before = app.model.spin.time;
        app.animate_donut();
        assert_eq!(app.model.spin.time, before);
    }

    #[test]
    fn animation_advances_the_clock_and_the_field() {
        let mut app = app_on_empty_session();
        app.animate_donut();
        let first = app.model.spin.time;
        assert!(first > 0.0, "the clock must advance");
        std::thread::sleep(std::time::Duration::from_millis(5));
        app.animate_donut();
        assert!(app.model.spin.time > first);
    }
}

/// The caption prefers the model id, because that is the fact the user wants,
/// and degrades to the provider rather than showing nothing.
#[test]
fn the_model_caption_prefers_the_model_then_the_provider() {
    use crate::ModelId;
    let both = ModelId {
        provider: Some("anthropic".into()),
        model: Some("claude-sonnet-4-5".into()),
    };
    assert_eq!(both.caption().as_deref(), Some("claude-sonnet-4-5"));
    let provider_only = ModelId {
        provider: Some("anthropic".into()),
        model: None,
    };
    assert_eq!(provider_only.caption().as_deref(), Some("anthropic"));
    assert_eq!(ModelId::default().caption(), None);
}

/// Empty strings from the wire must not render as a blank caption row.
#[test]
fn the_model_caption_ignores_empty_strings() {
    use crate::ModelId;
    let empty = ModelId {
        provider: Some(String::new()),
        model: Some(String::new()),
    };
    assert_eq!(empty.caption(), None);
}

/// The harness reporting a model must land on the model, or the caption never
/// appears in the real app however well the label is unit-tested.
#[test]
fn a_reported_model_reaches_the_model() {
    let mut app = App::default();
    app.model.model = Some(crate::ModelId {
        provider: Some("openai".into()),
        model: Some("gpt-5.6".into()),
    });
    assert_eq!(
        app.model
            .model
            .as_ref()
            .and_then(|id| id.caption())
            .as_deref(),
        Some("gpt-5.6")
    );
}

/// R6: the session strip. Moving must actually switch the conversation, not
/// merely recolour a bar, and it must never leak the previous session's
/// transcript into the new one.
mod session_strip {
    use super::*;
    use crate::strip::{Entry, Strip};

    fn app_with_sessions(focused: &str) -> App {
        let mut app = App::default();
        let entries = vec![
            Entry {
                session_id: "s_a1".into(),
                working_dir: Some("/home/j/jcode".into()),
                busy: false,
                weight: 0.0,
            },
            Entry {
                session_id: "s_a2".into(),
                working_dir: Some("/home/j/jcode".into()),
                busy: false,
                weight: 0.0,
            },
            Entry {
                session_id: "s_b1".into(),
                working_dir: Some("/home/j/site".into()),
                busy: false,
                weight: 0.0,
            },
        ];
        app.model.strip = Strip::build(entries, Some(focused));
        app.model.session_id = Some(focused.into());
        app
    }

    /// Switching sessions must never leave the previous session's directory on
    /// screen: the caption and title would then name the wrong project while
    /// you type into a different one.
    #[test]
    fn switching_sessions_renames_the_place() {
        let mut app = app_with_sessions("s_a1");
        app.model.working_dir = Some("/home/j/jcode".into());
        app.apply(Action::SessionDown, None);
        assert_eq!(app.model.session_id.as_deref(), Some("s_b1"));
        assert_eq!(app.model.working_dir.as_deref(), Some("/home/j/site"));
        assert!(
            crate::place::window_title(app.model.working_dir.as_deref())
                .starts_with("/home/j/site")
        );
    }

    /// R2 revisited: the top row is also earned by having a directory to name,
    /// so a single-session window still says where it is.
    #[test]
    fn a_lone_session_with_a_directory_gets_the_top_row() {
        let mut app = App::default();
        app.model.working_dir = Some("/home/j/jcode".into());
        assert!(
            App::frame_for_model((1400, 900), 1.0, &app.model)
                .strip()
                .is_some(),
            "a lone attached session had nowhere to show its directory"
        );
    }

    #[test]
    fn moving_right_switches_the_attached_session() {
        let mut app = app_with_sessions("s_a1");
        app.model
            .transcript
            .push(crate::transcript::Message::assistant(
                "output from the old session",
            ));
        app.model.busy = true;
        app.model.scroll = 4.0;

        app.apply(Action::SessionRight, None);

        assert_eq!(
            app.model.session_id.as_deref(),
            Some("s_a2"),
            "the highlight moved but the session did not"
        );
        assert!(
            app.model.transcript.is_empty(),
            "the previous session's transcript leaked into the new one"
        );
        assert!(!app.model.busy, "carried the old session's busy state");
        assert_eq!(app.model.scroll, 0.0, "carried the old session's scroll");
    }

    #[test]
    fn moving_down_switches_to_the_other_working_directory() {
        let mut app = app_with_sessions("s_a1");
        app.apply(Action::SessionDown, None);
        assert_eq!(app.model.session_id.as_deref(), Some("s_b1"));
        assert_eq!(app.model.strip.group_index(), 1);
    }

    /// With one session there is nowhere to go, so the keys must leave the
    /// attached session completely alone rather than "switching" to itself
    /// and wiping the transcript.
    #[test]
    fn navigation_is_inert_with_a_single_session() {
        let mut app = App::default();
        app.model.strip = Strip::build(
            vec![Entry {
                session_id: "solo".into(),
                working_dir: Some("/tmp".into()),
                busy: false,
                weight: 0.0,
            }],
            Some("solo"),
        );
        app.model.session_id = Some("solo".into());
        app.model
            .transcript
            .push(crate::transcript::Message::assistant("keep me"));

        for action in [
            Action::SessionLeft,
            Action::SessionRight,
            Action::SessionUp,
            Action::SessionDown,
        ] {
            app.apply(action, None);
        }

        assert_eq!(app.model.session_id.as_deref(), Some("solo"));
        assert_eq!(
            app.model.transcript.plain_text().trim(),
            "keep me",
            "an inert move still wiped the transcript"
        );
    }

    /// R2 at the model level: the strip only claims a row once there is
    /// somewhere to move to.
    #[test]
    fn the_strip_band_appears_only_with_more_than_one_session() {
        let mut app = App::default();
        assert_eq!(
            App::frame_for_model((1400, 900), 1.0, &app.model).strip(),
            None,
            "an empty session list still reserved a strip row"
        );

        app.model.strip = Strip::build(
            vec![Entry {
                session_id: "solo".into(),
                working_dir: Some("/tmp".into()),
                busy: false,
                weight: 0.0,
            }],
            Some("solo"),
        );
        assert_eq!(
            App::frame_for_model((1400, 900), 1.0, &app.model).strip(),
            None,
            "a single session still reserved a strip row"
        );

        app = app_with_sessions("s_a1");
        assert!(
            App::frame_for_model((1400, 900), 1.0, &app.model)
                .strip()
                .is_some(),
            "several sessions did not get a strip"
        );
    }

    /// A poll must not yank the user somewhere else: the refreshed strip has
    /// to stay pointed at the conversation on screen.
    #[test]
    fn a_session_list_refresh_keeps_the_current_session_focused() {
        let mut app = app_with_sessions("s_a1");
        app.apply(Action::SessionDown, None);
        assert_eq!(app.model.session_id.as_deref(), Some("s_b1"));

        // A later poll reports the same sessions in the same order.
        app.model.strip = Strip::build(
            vec![
                Entry {
                    session_id: "s_a1".into(),
                    working_dir: Some("/home/j/jcode".into()),
                    busy: false,
                    weight: 0.0,
                },
                Entry {
                    session_id: "s_a2".into(),
                    working_dir: Some("/home/j/jcode".into()),
                    busy: false,
                    weight: 0.0,
                },
                Entry {
                    session_id: "s_b1".into(),
                    working_dir: Some("/home/j/site".into()),
                    busy: false,
                    weight: 0.0,
                },
            ],
            app.model.session_id.as_deref(),
        );
        assert_eq!(
            app.model.strip.focused_session(),
            Some("s_b1"),
            "a refresh moved the highlight off the visible session"
        );
    }
}
