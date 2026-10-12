//! Copy-selection entry points: text and "N chars · M lines" metrics.
//!
//! The transcript and side panel draw `│ ` gutters and `┌─`/`└─` edges around
//! code blocks, display math, and blockquotes. That art is not content, so it
//! is excluded while each row is sliced, never by post-processing the selected
//! text. Each row is classified with the whole pane as context
//! (`copy_selection::for_each_copy_row`), so a selection that starts mid-block,
//! or covers only part of a gutter, still keeps literal `│` characters in code
//! and the padding of table cells. The composer holds the user's own typed text
//! and is copied verbatim.
//!
//! Text and metrics share one set of row walkers, so the status count always
//! matches the copied text, and metrics never build the selection string.

use super::copy_selection::{copy_selection_metrics_any, copy_selection_text_any};
use super::{CopyViewportSnapshot, copy_snapshot_for_pane};
use crate::tui::{CopySelectionPoint, CopySelectionRange};

pub(crate) fn copy_selection_text(range: CopySelectionRange) -> Option<String> {
    let (snapshot, start, end) = ordered_selection(range)?;
    copy_selection_text_any(&snapshot, start, end)
}

/// Compute `(char_count, line_count)` for the current copy selection without
/// allocating the selection text, so it can run on every render frame and drag
/// move. Matches what `copy_selection_text` copies.
pub(crate) fn copy_selection_metrics(range: CopySelectionRange) -> Option<(usize, usize)> {
    let (snapshot, start, end) = ordered_selection(range)?;
    copy_selection_metrics_any(&snapshot, start, end)
}

fn ordered_selection(
    range: CopySelectionRange,
) -> Option<(CopyViewportSnapshot, CopySelectionPoint, CopySelectionPoint)> {
    if range.start.pane != range.end.pane {
        return None;
    }
    let snapshot = copy_snapshot_for_pane(range.start.pane)?;
    let (start, end) =
        if (range.start.abs_line, range.start.column) <= (range.end.abs_line, range.end.column) {
            (range.start, range.end)
        } else {
            (range.end, range.start)
        };
    let count = snapshot.wrapped_plain_line_count();
    if start.abs_line >= count || end.abs_line >= count {
        return None;
    }
    Some((snapshot, start, end))
}
