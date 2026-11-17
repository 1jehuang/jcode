//! Geometry the renderer publishes after each frame.
//!
//! Handlers and tests outside `draw` need to address regions of the frame they
//! did not build: which rows are the transcript, the diagram, the diff pane, the
//! composer, and the pinned session status line. This module owns that state so
//! `ui.rs` stays about drawing.
#[cfg(not(test))]
use std::sync::{Mutex, OnceLock};

use ratatui::layout::Rect;

#[derive(Clone, Copy, Debug)]
pub struct LayoutSnapshot {
    pub messages_area: Rect,
    pub diagram_area: Option<Rect>,
    pub diff_pane_area: Option<Rect>,
    pub input_area: Option<Rect>,
    /// The pinned session status line below the input, when the frame had room
    /// for it. It is not always the last row of the frame: the compact status
    /// bar is drawn underneath it once the chat area is wide enough.
    pub overscroll_area: Option<Rect>,
}

#[cfg(not(test))]
static LAST_LAYOUT: OnceLock<Mutex<Option<LayoutSnapshot>>> = OnceLock::new();

#[cfg(not(test))]
fn last_layout_state() -> &'static Mutex<Option<LayoutSnapshot>> {
    LAST_LAYOUT.get_or_init(|| Mutex::new(None))
}

#[cfg(test)]
thread_local! {
    static TEST_LAST_LAYOUT: std::cell::RefCell<Option<LayoutSnapshot>> =
        const { std::cell::RefCell::new(None) };
}

fn store(snapshot: LayoutSnapshot) {
    #[cfg(test)]
    {
        TEST_LAST_LAYOUT.with(|slot| *slot.borrow_mut() = Some(snapshot));
    }
    #[cfg(not(test))]
    {
        if let Ok(mut slot) = last_layout_state().lock() {
            *slot = Some(snapshot);
        }
    }
}

pub fn record_layout_snapshot(
    messages_area: Rect,
    diagram_area: Option<Rect>,
    diff_pane_area: Option<Rect>,
    input_area: Option<Rect>,
) {
    if super::secondary_render_active() {
        return;
    }
    store(LayoutSnapshot {
        messages_area,
        diagram_area,
        diff_pane_area,
        input_area,
        overscroll_area: None,
    });
}

/// Record the pinned status line's row for the frame being drawn.
///
/// Separate from [`record_layout_snapshot`] because the status line is laid out
/// and drawn after that snapshot is published. A later
/// `record_layout_snapshot` clears the field, so order per frame is: snapshot,
/// then this.
pub fn record_overscroll_status_area(area: Rect) {
    if super::secondary_render_active() {
        return;
    }
    fn apply(slot: &mut Option<LayoutSnapshot>, area: Rect) {
        if let Some(snapshot) = slot.as_mut() {
            snapshot.overscroll_area = Some(area);
        }
    }
    #[cfg(test)]
    {
        TEST_LAST_LAYOUT.with(|slot| apply(&mut slot.borrow_mut(), area));
    }
    #[cfg(not(test))]
    {
        if let Ok(mut slot) = last_layout_state().lock() {
            apply(&mut slot, area);
        }
    }
}

/// Row geometry of the pinned status line in the last drawn frame.
// Only layout tests address that row today; production consumers use the
// snapshot's other areas. Kept public so the next consumer does not have to
// rediscover the row by guessing.
#[cfg_attr(not(test), allow(dead_code))]
pub fn last_overscroll_status_area() -> Option<Rect> {
    last_layout_snapshot().and_then(|snapshot| snapshot.overscroll_area)
}

pub fn last_layout_snapshot() -> Option<LayoutSnapshot> {
    #[cfg(test)]
    {
        return TEST_LAST_LAYOUT.with(|slot| *slot.borrow());
    }
    #[cfg(not(test))]
    {
        last_layout_state().lock().ok().and_then(|slot| *slot)
    }
}

#[cfg(test)]
pub(crate) fn clear_layout_snapshot_for_tests() {
    TEST_LAST_LAYOUT.with(|slot| *slot.borrow_mut() = None);
}
