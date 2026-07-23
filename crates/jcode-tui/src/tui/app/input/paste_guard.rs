//! Guard against the stray Enter key event some terminals (Windows Terminal /
//! conhost) deliver immediately after a bracketed paste that ends with a
//! newline. Without this, pasting multi-line text submitted the chat (#544).
//!
//! Paste events and key events are both handled on the TUI event-loop thread,
//! so a thread-local timestamp is sufficient and keeps `App` untouched.

use std::cell::Cell;
use std::time::{Duration, Instant};

const PASTE_ENTER_SUPPRESS_WINDOW: Duration = Duration::from_millis(150);

thread_local! {
    static LAST_PASTE: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Record that a bracketed-paste event was just handled.
pub(super) fn note_paste() {
    LAST_PASTE.with(|cell| cell.set(Some(Instant::now())));
}

/// Returns true (and consumes the marker) when a bare Enter arrives within the
/// suppression window after a paste, meaning it belongs to the paste rather
/// than being a user submit.
pub(super) fn consume_paste_trailing_enter() -> bool {
    LAST_PASTE.with(|cell| {
        cell.take()
            .is_some_and(|at| at.elapsed() < PASTE_ENTER_SUPPRESS_WINDOW)
    })
}

/// Test hook: age the recorded paste so a subsequent Enter submits normally.
#[cfg(test)]
pub(in crate::tui::app) fn expire_for_test() {
    LAST_PASTE.with(|cell| cell.set(None));
}

/// Media type for image file extensions accepted by drag-and-drop paste.
pub(super) fn image_media_type(path: &std::path::Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "tif" | "tiff" => Some("image/tiff"),
        _ => None,
    }
}
