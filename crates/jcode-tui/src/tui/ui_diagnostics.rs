//! Layout diagnostics for ConPTY hosts that move their pane without a resize.

use ratatui::prelude::*;

/// Log the rows the layout chose, so a capture can tell a terminal-size
/// disagreement from an undetected screen shift (issue #1812).
///
/// On Windows the composer row comes from crossterm's `terminal::size()` (the
/// console window rect), never from an observed cursor. A ConPTY host such as
/// herdr can lay the frame out for rows it does not show, which puts the footer
/// on the host's status row.
pub(super) fn log_composer_row(frame: &Frame, chat_area: Rect, chunks: &[Rect]) {
    crate::logging::info(&format!(
        "composer-row: terminal::size()={:?} frame.area={:?} chat_area={:?} \
         composer_y={} composer_height={} status_y={}",
        crossterm::terminal::size(),
        frame.area(),
        chat_area,
        chunks[7].y,
        chunks[7].height,
        chunks[3].y,
    ));
}
