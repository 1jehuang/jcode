//! Transcript text selection, driven through the real pointer handlers.
//!
//! Its own file rather than a module inside `actions`: this is a whole
//! surface's worth of behaviour (hit-test, drag, clipboard precedence, and how
//! it coexists with the composer's own selection), and `actions` is already at
//! the code-size budget.

use crate::App;
use crate::keymap::Action;
use crate::select::Position;
use crate::transcript::Message;
/// An app with a conversation and a frame wide enough to lay it out.
fn app_with_transcript() -> App {
    let mut app = App::default();
    app.model.session_id = Some("session_test".into());
    app.model
        .transcript
        .push(Message::user("what is the answer"));
    app.model
        .transcript
        .push(Message::assistant("the answer is forty two"));
    app.frame = App::frame_for_model((1100, 720), 1.0, &app.model);
    app
}

/// Logical y at the vertical middle of the transcript region.
fn transcript_y(app: &App) -> f64 {
    (app.frame.body_top + app.frame.body_bottom) / 2.0
}

/// Press, move, release over the transcript, through the real handlers.
fn drag_transcript(app: &mut App, from: (f64, f64), to: (f64, f64)) {
    app.pointer = from;
    app.on_pointer_pressed();
    app.pointer = to;
    app.on_pointer_moved();
    app.selecting = false;
}

/// The point where a given transcript position is drawn, so tests aim at
/// real glyphs rather than hardcoded font metrics.
fn point_for(app: &mut App, position: Position) -> (f64, f64) {
    let frame = app.frame;
    let style = crate::scene::transcript_body_style(&app.model);
    let width = (frame.column() - crate::transcript::USER_PAD_X * 2.0).max(1.0);
    let region = (frame.body_bottom - frame.body_top).max(1.0);
    let crate::App {
        painter,
        model: state,
        ..
    } = app;
    let crate::paint::Painter {
        text,
        transcript: cache,
    } = painter;
    let laid = cache.lay_out(
        text,
        &state.transcript,
        width,
        &state.theme,
        style,
        frame.scale,
    );
    let view = crate::viewport::Viewport::new(laid, region, state.scroll);
    let placed = view
        .visible
        .iter()
        .find(|placed| placed.index == position.message)
        .expect("message is not on screen");
    let block = &placed.message.blocks[position.block];
    let caret = parley::Cursor::from_byte_index(
        &block.layout,
        position.offset,
        parley::Affinity::Downstream,
    )
    .geometry(&block.layout, 1.0);
    (
        frame.left + crate::transcript::USER_PAD_X + block.inset + caret.x0 / frame.scale + 0.5,
        frame.body_top
            + placed.top
            + placed.message.top_padding()
            + block.top
            + (caret.y0 + caret.y1) / 2.0 / frame.scale,
    )
}

fn at(message: usize, block: usize, offset: usize) -> Position {
    Position {
        message,
        block,
        offset,
    }
}

/// The bug this feature fixes: a drag over a reply used to do nothing.
#[test]
fn dragging_over_a_reply_selects_its_text() {
    let mut app = app_with_transcript();
    let from = point_for(&mut app, at(1, 0, 0));
    let to = point_for(&mut app, at(1, 0, 10));
    drag_transcript(&mut app, from, to);
    let selection = app.model.selection.expect("no transcript selection");
    assert!(!selection.is_empty(), "the drag selected nothing");
    assert_eq!(
        app.selected_transcript_text().as_deref(),
        Some("the answer"),
        "the wrong characters were selected"
    );
}

/// A drag across the boundary picks up both turns, in reading order.
#[test]
fn dragging_across_messages_selects_both() {
    let mut app = app_with_transcript();
    let from = point_for(&mut app, at(0, 0, 0));
    let to = point_for(&mut app, at(1, 0, 3));
    drag_transcript(&mut app, from, to);
    let copied = app
        .selected_transcript_text()
        .expect("nothing selected across messages");
    assert!(
        copied.starts_with("what is the answer") && copied.ends_with("the"),
        "cross-message copy was {copied:?}"
    );
}

/// Ctrl+C must copy the highlight the user can see, not the composer's
/// line, which is what it used to do.
#[test]
fn copy_takes_the_transcript_selection_over_the_composer() {
    let mut app = app_with_transcript();
    app.apply(Action::Insert, Some("a draft"));
    let from = point_for(&mut app, at(1, 0, 0));
    let to = point_for(&mut app, at(1, 0, 10));
    drag_transcript(&mut app, from, to);
    app.apply(Action::Copy, None);
    assert_eq!(
        app.clipboard.get().as_deref(),
        Some("the answer"),
        "copy took the composer instead of the visible highlight"
    );
}

/// With no transcript highlight, copy still falls back to the composer, so
/// this feature cannot break the behaviour it sits next to.
#[test]
fn copy_still_falls_back_to_the_composer() {
    let mut app = app_with_transcript();
    app.apply(Action::Insert, Some("a draft"));
    app.apply(Action::Copy, None);
    assert_eq!(app.clipboard.get().as_deref(), Some("a draft"));
}

/// A plain click clears the highlight rather than leaving a stale band.
#[test]
fn a_plain_click_clears_the_selection() {
    let mut app = app_with_transcript();
    let from = point_for(&mut app, at(1, 0, 0));
    let to = point_for(&mut app, at(1, 0, 10));
    drag_transcript(&mut app, from, to);
    assert!(app.model.selection.is_some());

    app.pointer = from;
    app.on_pointer_pressed();
    app.selecting = false;
    assert!(
        app.model
            .selection
            .is_none_or(|selection| selection.is_empty()),
        "a click left a selection behind"
    );
}

/// Clicking into the composer drops the transcript highlight: two live
/// selections would make Ctrl+C ambiguous.
#[test]
fn clicking_the_composer_drops_the_transcript_selection() {
    let mut app = app_with_transcript();
    let from = point_for(&mut app, at(1, 0, 0));
    let to = point_for(&mut app, at(1, 0, 10));
    drag_transcript(&mut app, from, to);
    app.pointer = (
        app.frame.composer_text_left() + 1.0,
        (app.frame.composer_top + app.frame.composer_bottom) / 2.0,
    );
    app.on_pointer_pressed();
    app.dragging = false;
    assert!(
        app.model.selection.is_none(),
        "the transcript stayed highlighted after clicking the composer"
    );
}

/// Escape dismisses the highlight before touching typed work.
#[test]
fn escape_clears_the_selection_before_the_composer() {
    let mut app = app_with_transcript();
    app.apply(Action::Insert, Some("a draft"));
    let from = point_for(&mut app, at(1, 0, 0));
    let to = point_for(&mut app, at(1, 0, 10));
    drag_transcript(&mut app, from, to);
    app.apply(Action::Cancel, None);
    assert!(app.model.selection.is_none(), "Escape kept the highlight");
    assert_eq!(
        app.model.editor.text(),
        "a draft",
        "Escape reached past the highlight and cleared the draft"
    );
}

/// A drag in the transcript must not move the composer caret, and a drag
/// in the composer must not select the transcript. One gesture, one
/// surface.
#[test]
fn the_two_surfaces_do_not_drive_each_other() {
    let mut app = app_with_transcript();
    app.apply(Action::Insert, Some("a draft"));
    let cursor = app.model.editor.cursor();
    let from = point_for(&mut app, at(1, 0, 0));
    let to = point_for(&mut app, at(1, 0, 10));
    drag_transcript(&mut app, from, to);
    assert_eq!(
        app.model.editor.cursor(),
        cursor,
        "a transcript drag moved the composer caret"
    );
    assert!(
        app.model.editor.selection().is_none(),
        "a transcript drag selected composer text"
    );
}

/// Dragging out of the region keeps extending rather than dropping the
/// gesture, which is what makes selecting to the edge usable.
#[test]
fn dragging_past_the_edge_keeps_extending() {
    let mut app = app_with_transcript();
    let from = point_for(&mut app, at(0, 0, 0));
    app.pointer = from;
    app.on_pointer_pressed();
    app.pointer = (from.0, app.frame.body_bottom + 500.0);
    app.on_pointer_moved();
    app.selecting = false;
    assert!(
        app.selected_transcript_text().is_some(),
        "dragging past the bottom edge selected nothing"
    );
}

/// The pointer says the transcript is selectable before it is dragged.
#[test]
fn the_pointer_is_a_text_caret_over_the_transcript() {
    let mut app = app_with_transcript();
    app.pointer = (app.frame.left + 20.0, transcript_y(&app));
    app.update_cursor_icon();
    assert_eq!(app.cursor_icon, winit::window::CursorIcon::Text);
}

/// Pointer input over an empty session must not select or panic: there is
/// nothing laid out, and the donut lives there instead.
#[test]
fn an_empty_transcript_has_nothing_to_select() {
    let mut app = App::default();
    app.frame = App::frame_for_model((1100, 720), 1.0, &app.model);
    app.pointer = (app.frame.left + 20.0, transcript_y(&app));
    app.on_pointer_pressed();
    app.on_pointer_moved();
    assert!(app.model.selection.is_none());
}

// --- auto-copy on select ---
//
// Selecting text publishes it to the primary selection, so middle click pastes
// what was just highlighted. The ordinary clipboard is deliberately untouched:
// this fires on every drag, and a drag that destroyed an explicit Ctrl+C would
// be a far worse bug than no auto-copy at all.

use crate::clipboard::Target;

/// The real release handler, so these tests cannot pass while the window's own
/// path has stopped auto-copying.
fn release(app: &mut App) {
    app.on_pointer_released();
}

fn drag_and_release(app: &mut App, from: (f64, f64), to: (f64, f64)) {
    app.pointer = from;
    app.on_pointer_pressed();
    app.pointer = to;
    app.on_pointer_moved();
    release(app);
}

/// The feature: finishing a transcript drag copies it, with no keystroke.
#[test]
fn finishing_a_transcript_drag_auto_copies_it() {
    let mut app = app_with_transcript();
    let from = point_for(&mut app, at(1, 0, 0));
    let to = point_for(&mut app, at(1, 0, 10));
    drag_and_release(&mut app, from, to);
    assert_eq!(
        app.clipboard.get_from(Target::Primary).as_deref(),
        Some("the answer"),
        "the finished selection was not auto-copied"
    );
}

/// The invariant that makes it safe. Auto-copy fires on every drag, so if it
/// wrote to the ordinary clipboard, highlighting a word would silently destroy
/// whatever the user had deliberately copied.
#[test]
fn auto_copy_never_touches_the_ordinary_clipboard() {
    let mut app = app_with_transcript();
    let _ = app.clipboard.set("deliberately copied earlier");
    let from = point_for(&mut app, at(1, 0, 0));
    let to = point_for(&mut app, at(1, 0, 10));
    drag_and_release(&mut app, from, to);
    assert_eq!(
        app.clipboard.get().as_deref(),
        Some("deliberately copied earlier"),
        "auto-copy clobbered the user's clipboard"
    );
}

/// A composer drag auto-copies too: the two surfaces must not disagree about
/// whether selecting text copies it.
#[test]
fn a_composer_drag_also_auto_copies() {
    let mut app = App::default();
    app.apply(Action::Insert, Some("alpha bravo"));
    app.frame = App::frame_for_model((1100, 720), 1.0, &app.model);
    let y = (app.frame.composer_top + app.frame.composer_bottom) / 2.0;
    let x_at = |app: &mut App, offset: usize| {
        let frame = app.frame;
        let source = app.model.editor.text().to_string();
        let layout = crate::input::InputLayout::new(
            &mut app.painter.text,
            &source,
            frame.composer_text_width(),
            crate::scene::composer_text_style(&app.model),
            frame.scale,
        );
        frame.composer_text_left() + layout.caret_rect(offset, 1.0).x0
    };
    let from = x_at(&mut app, 0);
    let to = x_at(&mut app, 5);
    drag_and_release(&mut app, (from, y), (to, y));
    assert_eq!(
        app.clipboard.get_from(Target::Primary).as_deref(),
        Some("alpha")
    );
}

/// A keyboard selection auto-copies as well, so how you highlighted the text
/// does not change whether it was copied.
#[test]
fn a_keyboard_selection_auto_copies() {
    let mut app = App::default();
    app.apply(Action::Insert, Some("alpha bravo"));
    app.apply(Action::ExtendWordLeft, None);
    assert_eq!(
        app.clipboard.get_from(Target::Primary).as_deref(),
        Some("bravo"),
        "shift+word-left did not auto-copy"
    );
}

/// A double click completes on press rather than release, so it needs its own
/// publish; without it, the most common way to grab one word would not copy.
#[test]
fn a_double_click_auto_copies_the_word() {
    let mut app = App::default();
    app.apply(Action::Insert, Some("alpha bravo"));
    app.frame = App::frame_for_model((1100, 720), 1.0, &app.model);
    let frame = app.frame;
    let source = app.model.editor.text().to_string();
    let x = {
        let layout = crate::input::InputLayout::new(
            &mut app.painter.text,
            &source,
            frame.composer_text_width(),
            crate::scene::composer_text_style(&app.model),
            frame.scale,
        );
        frame.composer_text_left() + layout.caret_rect(8, 1.0).x0
    };
    let y = (frame.composer_top + frame.composer_bottom) / 2.0;
    app.pointer = (x, y);
    app.on_pointer_pressed();
    app.dragging = false;
    // Second click at the same spot, inside the double-click window.
    app.on_pointer_pressed();
    assert_eq!(
        app.clipboard.get_from(Target::Primary).as_deref(),
        Some("bravo"),
        "a double click did not auto-copy the word"
    );
}

/// A click that selects nothing must leave the previous auto-copy intact,
/// rather than blanking it: clicking to move the caret is not a copy gesture.
#[test]
fn a_click_with_no_selection_does_not_clear_the_auto_copy() {
    let mut app = app_with_transcript();
    let from = point_for(&mut app, at(1, 0, 0));
    let to = point_for(&mut app, at(1, 0, 10));
    drag_and_release(&mut app, from, to);
    app.pointer = from;
    app.on_pointer_pressed();
    release(&mut app);
    assert_eq!(
        app.clipboard.get_from(Target::Primary).as_deref(),
        Some("the answer"),
        "a plain click wiped the auto-copied selection"
    );
}

/// Middle click pastes the primary selection: the other half of the
/// convention, and the thing that makes auto-copy useful at all.
#[test]
fn middle_click_pastes_the_auto_copied_selection() {
    let mut app = app_with_transcript();
    let from = point_for(&mut app, at(1, 0, 0));
    let to = point_for(&mut app, at(1, 0, 10));
    drag_and_release(&mut app, from, to);
    app.paste_primary_selection();
    assert_eq!(
        app.model.editor.text(),
        "the answer",
        "middle click did not paste the auto-copied selection"
    );
}

/// Middle click with nothing selected must not insert anything.
#[test]
fn middle_click_with_an_empty_primary_selection_is_a_no_op() {
    let mut app = app_with_transcript();
    app.paste_primary_selection();
    assert!(app.model.editor.is_empty());
}

// --- click granularity ---
//
// Click places a caret, double click takes the word, triple click takes the
// line. The transcript had only the first of those, which meant quoting a
// single word required a precise drag across it.

/// Press `count` times in a row at one spot, the way a real double or triple
/// click arrives: separate presses, close together, at the same place.
fn multi_click(app: &mut App, point: (f64, f64), count: usize) {
    for _ in 0..count {
        app.pointer = point;
        app.on_pointer_pressed();
        release(app);
    }
}

#[test]
fn double_clicking_the_transcript_selects_a_word() {
    let mut app = app_with_transcript();
    // Land inside "answer" in "the answer is forty two".
    let point = point_for(&mut app, at(1, 0, 6));
    multi_click(&mut app, point, 2);
    assert_eq!(
        app.selected_transcript_text().as_deref(),
        Some("answer"),
        "a double click did not take the word under the pointer"
    );
}

#[test]
fn triple_clicking_the_transcript_selects_the_line() {
    let mut app = app_with_transcript();
    let point = point_for(&mut app, at(1, 0, 6));
    multi_click(&mut app, point, 3);
    assert_eq!(
        app.selected_transcript_text().as_deref(),
        Some("the answer is forty two"),
        "a triple click did not take the whole line"
    );
}

/// A word or line grab completes on press, so it must publish immediately
/// rather than waiting for a drag that never comes.
#[test]
fn a_double_click_in_the_transcript_auto_copies() {
    let mut app = app_with_transcript();
    let point = point_for(&mut app, at(1, 0, 6));
    multi_click(&mut app, point, 2);
    assert_eq!(
        app.clipboard.get_from(Target::Primary).as_deref(),
        Some("answer")
    );
}

/// Two clicks far apart in time are two clicks, not a double: otherwise
/// clicking to place a caret, pausing, and clicking again would grab a word.
#[test]
fn two_slow_clicks_are_not_a_double_click() {
    let mut app = app_with_transcript();
    let point = point_for(&mut app, at(1, 0, 6));
    multi_click(&mut app, point, 1);
    // A press outside the double-click window is a fresh click, driven
    // through the streak's own clock so the test does not have to sleep.
    let later = std::time::Instant::now() + crate::DOUBLE_CLICK * 2;
    let granularity = app
        .click_streak
        .press_at(later, point.0, point.1, crate::DOUBLE_CLICK);
    assert_eq!(
        granularity,
        crate::select::Granularity::Character,
        "a slow second click was treated as a double click"
    );
    assert!(
        app.selected_transcript_text().is_none(),
        "two slow clicks selected a word"
    );
}

/// And two clicks in different places are two clicks, however fast: a double
/// click is one spot pressed twice, not any two presses.
#[test]
fn two_clicks_in_different_places_are_not_a_double_click() {
    let mut app = app_with_transcript();
    let first = point_for(&mut app, at(1, 0, 0));
    multi_click(&mut app, first, 1);
    let elsewhere = point_for(&mut app, at(1, 0, 18));
    multi_click(&mut app, elsewhere, 1);
    assert!(
        app.selected_transcript_text().is_none(),
        "clicks at two different spots were treated as a double click"
    );
}

/// A fourth click starts the ladder over rather than sticking on whole lines,
/// which is what someone leaning on the button expects.
#[test]
fn a_fourth_click_returns_to_a_caret() {
    let mut app = app_with_transcript();
    let point = point_for(&mut app, at(1, 0, 6));
    multi_click(&mut app, point, 4);
    assert!(
        app.model
            .selection
            .is_none_or(|selection| selection.is_empty()),
        "a fourth click did not return to placing a caret"
    );
}

/// A drag ends the streak: releasing after a drag and pressing again is a
/// fresh click, not the second half of a double click.
#[test]
fn a_drag_then_a_click_is_not_a_double_click() {
    let mut app = app_with_transcript();
    let from = point_for(&mut app, at(1, 0, 0));
    let to = point_for(&mut app, at(1, 0, 10));
    drag_and_release(&mut app, from, to);
    multi_click(&mut app, from, 1);
    assert!(
        app.selected_transcript_text().is_none(),
        "a click after a drag grabbed a word"
    );
}

// --- selection under scrolling and streaming ---
//
// A selection is (message, block, offset), not screen coordinates. That is
// what should make it survive anything that moves the text around, and it is
// worth asserting rather than assuming: the alternative design, caching
// rectangles, silently highlights the wrong words after a scroll.

/// A conversation long enough to scroll.
fn app_with_long_transcript() -> App {
    let mut app = App::default();
    app.model.session_id = Some("session_test".into());
    for n in 0..12 {
        app.model
            .transcript
            .push(Message::user(format!("question number {n}")));
        app.model.transcript.push(Message::assistant(format!(
            "answer number {n}, with enough prose to take up a line or two of the region"
        )));
    }
    app.frame = App::frame_for_model((1100, 720), 1.0, &app.model);
    app
}

#[test]
fn a_selection_survives_scrolling() {
    let mut app = app_with_long_transcript();
    // Select inside the last reply, which is on screen at the tail.
    let last = app.model.transcript.messages().len() - 1;
    let from = point_for(&mut app, at(last, 0, 0));
    let to = point_for(&mut app, at(last, 0, 6));
    drag_and_release(&mut app, from, to);
    let before = app
        .selected_transcript_text()
        .expect("nothing selected to begin with");

    let max = app.max_scroll();
    assert!(max > 0.0, "the test conversation does not scroll");
    app.model.scroll_up(max, max);

    assert_eq!(
        app.selected_transcript_text().as_deref(),
        Some(before.as_str()),
        "scrolling changed which characters were selected"
    );
}

/// Streaming appends to the tail message, so a selection made earlier in the
/// conversation must not slide onto different words as the reply grows.
#[test]
fn a_selection_survives_a_streaming_reply() {
    let mut app = app_with_transcript();
    let from = point_for(&mut app, at(0, 0, 0));
    let to = point_for(&mut app, at(0, 0, 4));
    drag_and_release(&mut app, from, to);
    let before = app.selected_transcript_text().expect("nothing selected");

    for _ in 0..5 {
        app.model.transcript.append_assistant(" and more text");
    }

    assert_eq!(
        app.selected_transcript_text().as_deref(),
        Some(before.as_str()),
        "a streaming reply moved an earlier selection"
    );
}

// --- autoscroll while dragging ---
//
// Without this a selection is capped at one screenful: the pointer runs out of
// window, so text above the region can only be reached by releasing,
// scrolling, and shift-clicking.

#[test]
fn dragging_to_the_top_edge_scrolls_the_conversation() {
    let mut app = app_with_long_transcript();
    let last = app.model.transcript.messages().len() - 1;
    let start = point_for(&mut app, at(last, 0, 0));
    app.pointer = start;
    app.on_pointer_pressed();

    let before = app.model.scroll;
    for _ in 0..5 {
        app.pointer = (start.0, app.frame.body_top + 1.0);
        app.on_pointer_moved();
    }
    assert!(
        app.model.scroll > before,
        "dragging to the top edge did not scroll ({before} -> {})",
        app.model.scroll
    );
}

/// And the selection grows as it scrolls, rather than the view moving out from
/// under a selection that stays the same size.
#[test]
fn autoscrolling_extends_the_selection_over_the_revealed_text() {
    let mut app = app_with_long_transcript();
    let last = app.model.transcript.messages().len() - 1;
    let start = point_for(&mut app, at(last, 0, 4));
    app.pointer = start;
    app.on_pointer_pressed();
    app.pointer = (start.0, app.frame.body_top + 1.0);
    app.on_pointer_moved();
    let short = app.selected_transcript_text().unwrap_or_default().len();

    for _ in 0..8 {
        app.pointer = (start.0, app.frame.body_top + 1.0);
        app.on_pointer_moved();
    }
    let long = app.selected_transcript_text().unwrap_or_default().len();
    assert!(
        long > short,
        "autoscrolling did not extend the selection ({short} -> {long} bytes)"
    );
}

/// Autoscroll must stop at the ends rather than running forever, and must not
/// fire in the middle of the region where the user is just dragging normally.
#[test]
fn autoscroll_is_bounded_and_does_not_fire_mid_region() {
    let mut app = app_with_long_transcript();
    let middle = (
        app.frame.left + 40.0,
        (app.frame.body_top + app.frame.body_bottom) / 2.0,
    );
    let before = app.model.scroll;
    app.pointer = middle;
    app.on_pointer_pressed();
    app.on_pointer_moved();
    assert_eq!(
        app.model.scroll, before,
        "a drag in the middle of the region scrolled the view"
    );

    // Drive it hard into the top: it must stop at the head, not run away.
    for _ in 0..200 {
        app.pointer = (middle.0, app.frame.body_top + 1.0);
        app.on_pointer_moved();
    }
    let max = app.max_scroll();
    assert!(
        app.model.scroll <= max + 0.5,
        "autoscroll ran past the head: {} > {max}",
        app.model.scroll
    );
}
