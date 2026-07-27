//! Selection and clipboard behaviour on the `App`.
//!
//! Split from `main.rs` so the event loop stays a router. These methods form
//! one concern (where the pointer is in the conversation, what that selects,
//! and where the result is published) and they read as a unit; interleaved
//! with window and harness plumbing they read as neither.

use crate::{App, DOUBLE_CLICK, clipboard, select};

impl App {
    /// Scroll the transcript when a selection drag reaches its edge.
    ///
    /// Without this a selection is capped at one screenful: the pointer stops
    /// at the edge of the window, so text above the region can be highlighted
    /// only by releasing, scrolling, and shift-clicking. Every other text
    /// surface scrolls under the drag instead, which is what makes selecting a
    /// long reply possible at all.
    ///
    /// Returns whether the view moved, so the caller can re-hit-test against
    /// the new scroll position rather than the stale one.
    pub(crate) fn autoscroll_for_drag(&mut self, y: f64) -> bool {
        /// How close to the edge the pointer must come, in logical units.
        const MARGIN: f64 = 24.0;
        /// How far one frame of autoscroll moves, in logical units. A fixed
        /// step rather than a rate: the drag only scrolls while the pointer is
        /// moving, so this is the granularity of the gesture, not a speed.
        const STEP: f64 = 18.0;

        let top = self.frame.body_top;
        let bottom = self.frame.body_bottom;
        let max = self.max_scroll();
        // A drag is already a continuous gesture: smoothing it would put the
        // drawn view behind the pointer, so the text the user is selecting is
        // not the text under their finger. Land each step immediately and just
        // keep the scrollbar lit.
        self.model.smooth.settle();
        self.model.smooth.show(std::time::Instant::now());
        if y < top + MARGIN {
            // Scrolling up reveals older text, which is what dragging toward
            // the top of the window is asking for.
            let before = self.model.scroll;
            self.model.scroll_up(STEP, max);
            self.model.scroll != before
        } else if y > bottom - MARGIN {
            let before = self.model.scroll;
            self.model.scroll_down(STEP);
            self.model.scroll != before
        } else {
            false
        }
    }

    /// Whether a logical point is inside the transcript region.
    pub(crate) fn in_transcript(&self, x: f64, y: f64) -> bool {
        !self.model.transcript.is_empty()
            && y >= self.frame.body_top
            && y <= self.frame.body_bottom
            && x >= self.frame.left
            && x <= self.frame.right
    }

    /// The transcript position under a logical point.
    pub(crate) fn transcript_position_at(&mut self, x: f64, y: f64) -> Option<select::Position> {
        select::position_at_in_frame(&mut self.painter, &self.model, self.frame, x, y)
    }

    /// Handle a press inside the transcript. Returns whether it was consumed,
    /// so the caller can fall through to the donut when there is no text here.
    ///
    /// Click places a caret, double click takes the word, triple click takes
    /// the line: the same ladder every other text surface has, and the reason
    /// quoting one word does not require a precise drag.
    pub(crate) fn press_in_transcript(&mut self, x: f64, y: f64) -> bool {
        let granularity = self.click_streak.press(x, y, DOUBLE_CLICK);
        let Some(hit) = select::selection_at_in_frame(
            &mut self.painter,
            &self.model,
            self.frame,
            x,
            y,
            granularity,
        ) else {
            return false;
        };
        self.model.selection = Some(match self.model.selection {
            // Shift+click extends the existing selection, as anywhere else.
            Some(existing)
                if self.modifiers.shift_key() && granularity == select::Granularity::Character =>
            {
                select::Selection::new(existing.anchor, hit.focus)
            }
            _ => hit,
        });
        // A caret drag continues on move; a word or line grab is already
        // complete, so it publishes now rather than waiting for a release.
        if granularity == select::Granularity::Character {
            self.selecting = true;
        } else {
            self.publish_primary_selection();
        }
        self.request_redraw();
        true
    }

    /// The selected transcript text, if any.
    pub(crate) fn selected_transcript_text(&mut self) -> Option<String> {
        select::selection_text_in_frame(&mut self.painter, &self.model, self.frame)
    }

    /// The text currently highlighted on either surface, transcript first
    /// because it is the one drawn as a band over the conversation.
    pub(crate) fn any_selected_text(&mut self) -> Option<String> {
        self.selected_transcript_text()
            .or_else(|| self.model.editor.selected_text().map(str::to_string))
    }

    /// Copy to a system buffer, reporting a failure rather than losing it
    /// silently. A copy that quietly did nothing is the bug users describe as
    /// "the app ate my clipboard"; the in-process fallback still holds the
    /// text, so paste within the app keeps working either way.
    pub(crate) fn copy_to(&mut self, target: clipboard::Target, text: &str) {
        if let Err(error) = self.clipboard.set_to(target, text) {
            self.model
                .set_notice(format!("clipboard unavailable: {error}"));
        }
    }

    /// Publish the live selection to the primary selection, so middle click
    /// pastes what was just highlighted.
    ///
    /// Deliberately *not* the ordinary clipboard: this fires on every drag, so
    /// writing there would mean highlighting a word silently destroys whatever
    /// the user had copied. On platforms without a primary selection this is a
    /// no-op, which is the honest behaviour rather than a surprising one.
    pub(crate) fn publish_primary_selection(&mut self) {
        let Some(text) = self.any_selected_text() else {
            return;
        };
        self.copy_to(clipboard::Target::Primary, &text);
    }

    /// Paste the primary selection at the composer's cursor. Middle click is
    /// the paste half of the select-to-copy convention.
    pub(crate) fn paste_primary_selection(&mut self) {
        let Some(text) = self.clipboard.get_from(clipboard::Target::Primary) else {
            return;
        };
        self.model.editor.insert_str(&text);
        self.model.caret.touch();
        self.request_redraw();
    }
}
