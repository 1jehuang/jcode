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
