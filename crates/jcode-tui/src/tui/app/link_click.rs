//! Link opening on mouse release.
//!
//! Terminals reserve plain clicks for text selection, so a bare click over a
//! URL must never launch a browser by accident (that is exactly what happened
//! while selecting text). Links therefore open only on Ctrl+Click, and never
//! when the release ends a text-selection drag.

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::App;

impl App {
    /// True when a click at this cell would hit a link target.
    ///
    /// Cheap, read-only cousin of [`Self::try_open_link_at`]: it never opens
    /// anything, so a plain click can warn the operator without side effects.
    fn link_at(&self, column: u16, row: u16) -> bool {
        super::super::ui::oauth_login_link_message_from_screen(column, row).is_some()
            || super::super::ui::link_target_from_screen_with_state(column, row, self).is_some()
    }

    /// Handle a left-button release over a link.
    ///
    /// `drag_released` must be snapshotted by the caller *before*
    /// `handle_copy_selection_mouse` runs, because that handler clears
    /// `copy_selection_dragging` while consuming the release.
    ///
    /// Returns `true` only when the release actually opened (or attempted to
    /// open) a link, so the caller can treat the event as consumed.
    pub(super) fn handle_link_click_release(
        &mut self,
        mouse: MouseEvent,
        drag_released: bool,
    ) -> bool {
        if drag_released || !matches!(mouse.kind, MouseEventKind::Up(MouseButton::Left)) {
            return false;
        }
        if mouse.modifiers.contains(KeyModifiers::CONTROL) {
            return self.try_open_link_at(mouse.column, mouse.row);
        }
        if self.link_at(mouse.column, mouse.row) {
            // Teach the chord, but stay cheap: only cells that actually hit a
            // link pay for this lookup. Deliberately unconsumed so the click
            // keeps its normal selection/click behaviour.
            self.set_status_notice("Ctrl+Click to open links");
        }
        false
    }
}
