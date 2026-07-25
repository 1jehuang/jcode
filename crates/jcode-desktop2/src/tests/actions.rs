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
    press(&mut app, ch('a'), ModifiersState::CONTROL, None);
    assert_eq!(app.model.editor.cursor(), 0, "Ctrl+A did not go home");
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

#[test]
fn scrolling_clamps_and_returns_to_the_tail() {
    let mut app = App::default();
    app.model.transcript = (1..=100)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    app.apply(Action::ScrollTop, None);
    let top = app.model.scroll;
    assert!(top > 0, "scrolling up did nothing");
    app.apply(Action::ScrollUp, None);
    assert_eq!(app.model.scroll, top, "scroll ran past the top of history");
    app.apply(Action::ScrollBottom, None);
    assert_eq!(app.model.scroll, 0, "did not return to the live tail");
    app.apply(Action::ScrollDown, None);
    assert_eq!(app.model.scroll, 0, "scrolled below the tail");
}

#[test]
fn submitting_jumps_back_to_the_live_tail() {
    let mut app = app_with("question");
    app.model.transcript = (1..=100)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    app.apply(Action::PageUp, None);
    assert!(app.model.scroll > 0);
    app.submit_input();
    assert_eq!(app.model.scroll, 0, "reply would stream in off-screen");
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
    let style = crate::text::ParagraphStyle {
        font_size: crate::layout::BODY_SIZE,
        ..Default::default()
    };
    let text = app.model.editor.text()[..offset].to_string();
    frame.left + crate::layout::COMPOSER_PAD_X + app.text.measure_width(&text, style, frame.scale)
}

/// Mouse tests need a laid-out window, which needs a GPU surface. Instead
/// of a real window, exercise the offset math and editor transitions
/// directly through the same helpers the events call.
#[test]
fn hit_testing_maps_x_to_the_nearest_character_gap() {
    let mut app = app_with("alpha beta");
    let style = crate::text::ParagraphStyle {
        font_size: crate::layout::BODY_SIZE,
        ..Default::default()
    };
    let text = app.model.editor.text().to_string();
    // Clicking left of the text lands at 0; far right lands at the end.
    assert_eq!(app.text.offset_at_x(&text, -50.0, style, 1.0), 0);
    assert_eq!(
        app.text.offset_at_x(&text, 10_000.0, style, 1.0),
        text.len()
    );
    // Clicking at the measured width of a prefix lands on that boundary.
    for offset in [0, 1, 5, 6, text.len()] {
        let width = app.text.measure_width(&text[..offset], style, 1.0);
        assert_eq!(
            app.text.offset_at_x(&text, width, style, 1.0),
            offset,
            "click at the boundary of offset {offset} missed"
        );
    }
}

#[test]
fn hit_testing_never_splits_a_character() {
    let mut app = app_with("héllo wörld 🌼");
    let style = crate::text::ParagraphStyle {
        font_size: crate::layout::BODY_SIZE,
        ..Default::default()
    };
    let text = app.model.editor.text().to_string();
    for step in 0..200 {
        let offset = app.text.offset_at_x(&text, step as f64 * 3.0, style, 1.0);
        assert!(
            text.is_char_boundary(offset),
            "hit test returned a mid-character offset"
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
        app.pointer = (app.frame.left + crate::layout::COMPOSER_PAD_X + 1.0, y);
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
    assert!(app.model.transcript.contains("first\nsecond"));
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

/// The wrap budget is a character count derived from an assumed monospace
/// advance. If that assumption drifts from the real font, wrapped text
/// silently overflows the well, so check it against measured text.
#[test]
fn the_wrap_budget_matches_the_measured_font_width() {
    let mut app = app_with("");
    let frame = crate::layout::Frame::new((1400, 900), 1.0);
    let budget = frame.composer_char_budget();
    let style = crate::text::ParagraphStyle {
        font_size: crate::layout::BODY_SIZE,
        ..Default::default()
    };
    let usable = frame.column() - crate::layout::COMPOSER_PAD_X * 2.0;
    let full: String = "m".repeat(budget);
    let measured = app.text.measure_width(&full, style, 1.0);
    assert!(
        measured <= usable,
        "a full row of {budget} chars measures {measured:.1}px but only {usable:.1}px fit"
    );
    // The budget must also not be needlessly small.
    let over: String = "m".repeat(budget + 3);
    assert!(
        app.text.measure_width(&over, style, 1.0) > usable,
        "the wrap budget is far smaller than the available width"
    );
}

#[test]
fn a_long_line_wraps_into_multiple_rows_and_grows_the_well() {
    let long = "word ".repeat(60);
    let app = app_with(&long);
    let single = crate::layout::Frame::new((1400, 900), 1.0);
    let rows = app.model.composer_rows(single.composer_char_budget());
    assert!(rows.len() > 1, "a long line did not wrap");
    let frame = App::frame_for_model((1400, 900), 1.0, &app.model);
    assert!(
        frame.composer_top < single.composer_top,
        "the well did not grow for wrapped rows"
    );
    // Wrapping is a view concern: it must not touch the buffer.
    assert_eq!(app.model.editor.text(), long);
    assert_eq!(app.model.editor.line_count(), 1);
}

#[test]
fn clicking_a_wrapped_row_lands_on_that_row() {
    let long = "alpha bravo charlie delta echo foxtrot golf hotel india juliet ".repeat(3);
    let mut app = app_with(&long);
    app.frame = App::frame_for_model((900, 800), 1.0, &app.model);
    let rows = app.model.composer_rows(app.frame.composer_char_budget());
    assert!(rows.len() > 1, "test needs a wrapped input");
    let text_top = app.frame.composer_top + crate::layout::COMPOSER_TEXT_OFFSET;
    for (index, row) in rows.iter().enumerate().take(3) {
        let y = text_top + index as f64 * crate::layout::COMPOSER_LINE_HEIGHT + 4.0;
        app.pointer = (app.frame.left + crate::layout::COMPOSER_PAD_X + 1.0, y);
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
    assert!(
        double.composer_top < single.composer_top,
        "the composer did not grow when a line was added"
    );
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
    app.model.transcript = (1..=100)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let visible = app.visible_lines();
    app.model.scroll_up(3, visible);
    assert_eq!(app.model.scroll, 3);
    app.model.scroll_down(99);
    assert_eq!(app.model.scroll, 0, "wheel scrolled past the tail");
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
