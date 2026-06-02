//! Issue #41: floating diagram mode state primitive.
//!
//! Storage + state machine for "floating diagram" mode, where a
//! Mermaid (or other) diagram is detached from the chat scroll and
//! pinned to a movable / resizable overlay. This module ships the
//! state primitive only — rendering / hotkey wiring is a follow-up.
//!
//! ## State machine
//!
//! ```text
//!     Hidden  ──pin──>  Visible(rect, source)
//!     Visible ──drag──>  Visible(rect', source)
//!     Visible ──unpin─>  Hidden
//!     Visible ──source_change──> Visible(rect, source')
//! ```
//!
//! ## Out of scope (#40 / #41 follow-ups)
//!
//! - Renderer integration (drawing the floating overlay)
//! - Hotkey routing (drag with arrow keys / mouse, resize with
//!   Shift+arrow)
//! - Variable-width reflow inside the floating window
//! - Save/restore on session reload

use serde::{Deserialize, Serialize};

/// Position + size of the floating diagram window. Coordinates are
/// terminal cells, origin top-left.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FloatRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl FloatRect {
    /// A reasonable starting rectangle: 60×20 cells, offset 4×2 from
    /// the top-left so it doesn't completely cover the title bar.
    pub fn default_initial() -> Self {
        Self {
            x: 4,
            y: 2,
            width: 60,
            height: 20,
        }
    }

    /// Clamp this rectangle to fit inside `(viewport_width, viewport_height)`.
    /// Preserves the upper-left corner; shrinks width/height if needed.
    /// Returns the clamped rect (does not mutate self).
    pub fn clamped_to(&self, viewport_width: u16, viewport_height: u16) -> Self {
        let mut out = *self;
        if out.x >= viewport_width {
            out.x = viewport_width.saturating_sub(1);
        }
        if out.y >= viewport_height {
            out.y = viewport_height.saturating_sub(1);
        }
        let max_w = viewport_width.saturating_sub(out.x);
        let max_h = viewport_height.saturating_sub(out.y);
        out.width = out.width.min(max_w).max(1);
        out.height = out.height.min(max_h).max(1);
        out
    }

    /// Move the rectangle by `(dx, dy)` cells, clamped to the viewport.
    pub fn moved_by(&self, dx: i32, dy: i32, viewport_w: u16, viewport_h: u16) -> Self {
        let new_x =
            (self.x as i32 + dx).clamp(0, viewport_w.saturating_sub(self.width) as i32) as u16;
        let new_y =
            (self.y as i32 + dy).clamp(0, viewport_h.saturating_sub(self.height) as i32) as u16;
        Self {
            x: new_x,
            y: new_y,
            width: self.width,
            height: self.height,
        }
    }
}

/// State of the floating diagram window.
#[derive(Debug, Clone, Default)]
pub enum FloatingDiagramState {
    #[default]
    Hidden,
    /// Pinned + visible. `source` is the raw Mermaid (or other)
    /// source that the renderer will use; `rect` is the position
    /// + size in viewport cells.
    Visible { rect: FloatRect, source: String },
}

impl FloatingDiagramState {
    /// Pin the diagram with the given source. Re-pinning replaces
    /// the source but preserves the existing position if the state
    /// was already `Visible`.
    pub fn pin(&mut self, source: String) {
        match self {
            FloatingDiagramState::Visible { rect, source: s } => {
                *s = source;
                // rect preserved
                let _ = rect;
            }
            FloatingDiagramState::Hidden => {
                *self = FloatingDiagramState::Visible {
                    rect: FloatRect::default_initial(),
                    source,
                };
            }
        }
    }

    /// Unpin (hide) the diagram. Position + source are dropped.
    pub fn unpin(&mut self) {
        *self = FloatingDiagramState::Hidden;
    }

    pub fn is_visible(&self) -> bool {
        matches!(self, FloatingDiagramState::Visible { .. })
    }

    /// Update the position by dx/dy cells. No-op when hidden.
    pub fn drag_by(&mut self, dx: i32, dy: i32, viewport_w: u16, viewport_h: u16) {
        if let FloatingDiagramState::Visible { rect, .. } = self {
            *rect = rect.moved_by(dx, dy, viewport_w, viewport_h);
        }
    }

    /// Borrow the current rect, if visible.
    pub fn rect(&self) -> Option<FloatRect> {
        match self {
            FloatingDiagramState::Visible { rect, .. } => Some(*rect),
            FloatingDiagramState::Hidden => None,
        }
    }

    /// Borrow the current source, if visible.
    pub fn source(&self) -> Option<&str> {
        match self {
            FloatingDiagramState::Visible { source, .. } => Some(source),
            FloatingDiagramState::Hidden => None,
        }
    }

    /// Reapply viewport bounds (called on terminal resize). Clamps
    /// the rect to the new viewport. No-op when hidden.
    pub fn on_viewport_resize(&mut self, viewport_w: u16, viewport_h: u16) {
        if let FloatingDiagramState::Visible { rect, .. } = self {
            *rect = rect.clamped_to(viewport_w, viewport_h);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_is_hidden() {
        let s = FloatingDiagramState::default();
        assert!(!s.is_visible());
        assert_eq!(s.rect(), None);
        assert_eq!(s.source(), None);
    }

    #[test]
    fn pin_makes_visible() {
        let mut s = FloatingDiagramState::default();
        s.pin("graph TD\nA-->B".to_string());
        assert!(s.is_visible());
        assert_eq!(s.source(), Some("graph TD\nA-->B"));
        assert_eq!(s.rect(), Some(FloatRect::default_initial()));
    }

    #[test]
    fn re_pin_preserves_position() {
        let mut s = FloatingDiagramState::default();
        s.pin("first".to_string());
        s.drag_by(10, 5, 200, 50);
        let rect_before = s.rect().unwrap();
        s.pin("second".to_string());
        let rect_after = s.rect().unwrap();
        assert_eq!(rect_before, rect_after, "rect must be preserved on re-pin");
        assert_eq!(s.source(), Some("second"));
    }

    #[test]
    fn unpin_drops_state() {
        let mut s = FloatingDiagramState::default();
        s.pin("graph".to_string());
        s.unpin();
        assert!(!s.is_visible());
    }

    #[test]
    fn drag_by_shifts_position() {
        let mut s = FloatingDiagramState::default();
        s.pin("graph".to_string());
        let initial = s.rect().unwrap();
        s.drag_by(5, 3, 200, 50);
        let after = s.rect().unwrap();
        assert_eq!(after.x, initial.x + 5);
        assert_eq!(after.y, initial.y + 3);
    }

    #[test]
    fn drag_clamps_to_viewport() {
        let mut s = FloatingDiagramState::default();
        s.pin("graph".to_string());
        // Try to drag way beyond viewport.
        s.drag_by(1000, 1000, 80, 24);
        let rect = s.rect().unwrap();
        // Clamped: x + width <= 80, y + height <= 24
        assert!(rect.x + rect.width <= 80);
        assert!(rect.y + rect.height <= 24);
    }

    #[test]
    fn drag_clamps_to_origin() {
        let mut s = FloatingDiagramState::default();
        s.pin("graph".to_string());
        s.drag_by(-1000, -1000, 200, 50);
        let rect = s.rect().unwrap();
        assert_eq!(rect.x, 0);
        assert_eq!(rect.y, 0);
    }

    #[test]
    fn drag_when_hidden_is_no_op() {
        let mut s = FloatingDiagramState::default();
        s.drag_by(10, 10, 200, 50);
        assert!(!s.is_visible());
    }

    #[test]
    fn on_resize_shrinks_to_fit_smaller_viewport() {
        let mut s = FloatingDiagramState::default();
        s.pin("graph".to_string());
        // Default initial is 60x20 at (4, 2). Resize viewport to 30x10.
        s.on_viewport_resize(30, 10);
        let rect = s.rect().unwrap();
        assert!(rect.x + rect.width <= 30);
        assert!(rect.y + rect.height <= 10);
    }

    #[test]
    fn float_rect_clamped_to_preserves_upper_left() {
        let r = FloatRect {
            x: 5,
            y: 5,
            width: 100,
            height: 100,
        };
        let clamped = r.clamped_to(40, 20);
        assert_eq!(clamped.x, 5);
        assert_eq!(clamped.y, 5);
        assert_eq!(clamped.width, 35); // 40 - 5
        assert_eq!(clamped.height, 15); // 20 - 5
    }
}
