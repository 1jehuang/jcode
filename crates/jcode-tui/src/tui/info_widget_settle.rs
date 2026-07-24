//! Settlement tracking for "resident" info widgets.
//!
//! Info widgets should behave like residents of the transcript: once placed into
//! a pocket of negative space, they belong to that part of the conversation and
//! scroll with it. To make that safe, a *new* widget may only be placed into
//! space that is **settled** - the free-width profile under it has stopped
//! changing (finalized content, not the streaming tail or a re-rendering
//! region).
//!
//! [`SettlementTracker`] observes the per-row free-width margin profile every
//! frame, keyed by absolute transcript line, and reports which rows have held
//! the exact same free widths for [`SETTLE_AFTER_FRAMES`] consecutive
//! observations. The layout engine docks fresh widgets only into settled rows
//! (via the strict `reliable` channel in [`super::info_widget_layout::Margins`]),
//! while already-placed widgets keep riding their transcript line untouched.
//!
//! Invalidation:
//! * A width change on a line resets its settle counter (streaming tail,
//!   markdown re-render, tool output folding).
//! * A viewport width change flushes the whole tracker (every line re-wraps, so
//!   all keys are meaningless). The caller is expected to also flush widget
//!   anchors in that case.

use super::info_widget_layout::Margins;
use std::collections::HashMap;

/// How many consecutive identical observations a transcript line needs before
/// its negative space is considered settled and eligible to host a new widget.
/// Absorbs streaming, markdown re-render, and tool-result collapse churn while
/// still placing widgets within a few frames of content finalizing.
pub(crate) const SETTLE_AFTER_FRAMES: u8 = 3;

/// Free-width observation for one absolute transcript line.
#[derive(Debug, Clone, Copy, Default)]
struct LineSettle {
    right: u16,
    left: u16,
    /// Consecutive frames the widths above have been observed unchanged.
    stable_frames: u8,
}

/// Tracks which transcript lines have settled negative space.
#[derive(Debug, Clone, Default)]
pub(crate) struct SettlementTracker {
    /// Viewport content width the observations were made at. Any change means a
    /// global re-wrap, which invalidates every keyed observation.
    area_width: u16,
    /// Per absolute-transcript-line settlement state.
    lines: HashMap<usize, LineSettle>,
}

/// Settled per-row dock profiles produced by [`SettlementTracker::observe`].
/// Rows whose underlying transcript line has not settled report width 0, so
/// the layout engine cannot dock a new widget there.
pub(crate) struct SettledMargins {
    pub right: Vec<u16>,
    pub left: Vec<u16>,
}

impl SettlementTracker {
    /// Flush all observations (e.g. on re-wrap or an explicit reset).
    pub fn reset(&mut self) {
        self.lines.clear();
    }

    /// Ingest this frame's margin profile and return the settled dock profile.
    ///
    /// Row `r` of the margins corresponds to absolute transcript line
    /// `margins.scroll_top + r`. Returns per-row widths where unsettled rows are
    /// zeroed. Also prunes observations far outside the viewport so memory stays
    /// bounded on long transcripts.
    pub fn observe(&mut self, margins: &Margins, area_width: u16) -> SettledMargins {
        if area_width != self.area_width {
            // Global re-wrap: every line index and width changed meaning.
            self.lines.clear();
            self.area_width = area_width;
        }

        let rows = margins.right_widths.len().max(margins.left_widths.len());
        let mut right = Vec::with_capacity(rows);
        let mut left = Vec::with_capacity(rows);
        for row in 0..rows {
            let line = margins.scroll_top + row;
            let w_right = margins.right_widths.get(row).copied().unwrap_or(0);
            let w_left = margins.left_widths.get(row).copied().unwrap_or(0);
            let entry = self.lines.entry(line).or_default();
            if entry.right == w_right && entry.left == w_left {
                entry.stable_frames = entry.stable_frames.saturating_add(1);
            } else {
                entry.right = w_right;
                entry.left = w_left;
                entry.stable_frames = 0;
            }
            let settled = entry.stable_frames >= SETTLE_AFTER_FRAMES;
            right.push(if settled { w_right } else { 0 });
            left.push(if settled { w_left } else { 0 });
        }

        // Prune entries far from the viewport: they'll be re-learned on the way
        // back, and this keeps the map proportional to the viewport, not the
        // transcript.
        let keep_radius = rows.saturating_mul(8).max(512);
        let lo = margins.scroll_top.saturating_sub(keep_radius);
        let hi = margins.scroll_top.saturating_add(rows + keep_radius);
        if self.lines.len() > keep_radius * 4 {
            self.lines.retain(|&line, _| line >= lo && line < hi);
        }

        SettledMargins { right, left }
    }
}

#[cfg(test)]
#[path = "info_widget_settle_tests.rs"]
mod tests;
