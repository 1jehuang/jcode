//! The settings gear's side of the app: what a click on it does, and how a
//! changed setting reaches the running window.
//!
//! Split out of `main` for the same reason the overview is: this is a small,
//! self-contained mode with its own hit testing, and it is the part worth
//! testing without a GPU.

use crate::App;
use crate::settings::{ROWS, Row};

impl App {
    /// A press somewhere on the page, while the settings UI might want it.
    /// Returns whether it was consumed, so the caller's own hit testing (the
    /// composer, the transcript, the donut) only runs when the gear did not
    /// take the click.
    pub(crate) fn settings_press(&mut self, x: f64, y: f64) -> bool {
        if self.frame.hits_gear(x, y) {
            self.model.panel.toggle();
            self.request_redraw();
            return true;
        }
        if !self.model.panel.is_open() {
            return false;
        }
        // The panel is a menu, so it is modal over the pointer: a click on a
        // row applies it and a click anywhere else dismisses without also
        // doing whatever was under the pointer. Dismiss-and-act would mean a
        // click aimed at closing the menu could land in the composer.
        match self.frame.panel_row_at(ROWS.len(), x, y) {
            Some(index) => self.cycle_setting(index),
            None => self.model.panel.close(),
        }
        self.request_redraw();
        true
    }

    /// Track the highlight under the pointer. Returns whether a repaint is
    /// needed.
    pub(crate) fn settings_hover(&mut self, x: f64, y: f64) -> bool {
        if !self.model.panel.is_open() {
            return false;
        }
        let row = self.frame.panel_row_at(ROWS.len(), x, y);
        self.model.panel.set_hover(row)
    }

    /// Advance one setting and apply it to the live window.
    ///
    /// Applied immediately rather than on a "save" button: every setting here
    /// is visible in the window itself, so the change *is* the feedback, and a
    /// confirmation step would only delay it.
    pub(crate) fn cycle_setting(&mut self, index: usize) {
        let Some(row) = ROWS.get(index).copied() else {
            return;
        };
        // The desktop's own preference is part of the answer for the theme
        // row, so it is read once here and handed down rather than consulted
        // twice with a chance of disagreeing between the step and the resolve.
        let system_dark = crate::theme::system_prefers_dark();
        self.model.settings.cycle(row, system_dark);
        self.apply_settings(row, system_dark);
        // Persisted per change, so a crash cannot lose a choice the user has
        // already seen take effect. The file is three lines.
        self.model.settings.save();
    }

    /// Push one setting into the running model.
    fn apply_settings(&mut self, row: Row, system_dark: bool) {
        match row {
            Row::Theme => {
                let mode = self.model.settings.theme;
                self.model.theme_preference = mode;
                self.model.theme = crate::theme::Theme::for_mode(mode, system_dark);
            }
            Row::Reasoning => {
                let mode = self.model.settings.reasoning;
                self.model.transcript.set_reasoning_mode(mode);
            }
            Row::Motion => {
                // Turning motion off drops the donut's field entirely rather
                // than freezing it: the field is the only thing the animation
                // clock exists for, so an idle window then sleeps instead of
                // repainting a still image sixty times a second.
                self.model.donut = self
                    .model
                    .settings
                    .motion
                    .then(|| crate::donut::Donut::new(crate::DONUT_GRID));
            }
        }
    }

    /// Set the palette outright, for the keyboard chord and any future menu
    /// item that names a mode rather than stepping through them.
    ///
    /// Shares the persist-and-apply path with the panel, so a theme changed by
    /// keyboard is remembered exactly like one changed by clicking, and the
    /// panel's own row updates to match rather than showing a stale value.
    pub(crate) fn set_theme(&mut self, mode: crate::theme::ThemeMode) {
        self.model.settings.theme = mode;
        self.apply_settings(Row::Theme, crate::theme::system_prefers_dark());
        self.model.settings.save();
    }

    /// Flip between light and dark, the way the chord and the row both mean
    /// it: away from whatever is currently *on screen*, so `system` resolves
    /// before it is stepped and the window always visibly changes.
    pub(crate) fn toggle_theme(&mut self) {
        let next = self
            .model
            .settings
            .next_theme(crate::theme::system_prefers_dark());
        self.set_theme(next);
    }

    /// Keep the two ways of changing the thinking display in step: the
    /// Ctrl+Shift+R chord writes through the settings so the panel never shows
    /// a stale value, and the choice survives a restart like any other.
    pub(crate) fn set_reasoning_from_keyboard(&mut self, mode: crate::reasoning::ReasoningMode) {
        self.model.settings.reasoning = mode;
        self.model.transcript.set_reasoning_mode(mode);
        self.model.settings.save();
    }
}
