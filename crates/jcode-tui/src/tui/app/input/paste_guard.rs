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

/// Test hook: bytes of a real 1x1 PNG, for drop tests that need content
/// `load_dropped_image` accepts.
#[cfg(test)]
pub(crate) fn tiny_png_bytes_for_test() -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(1, 1, image::Rgb([1, 2, 3])))
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("encode png");
    out.into_inner()
}

/// True for file extensions drag-and-drop paste treats as images.
fn has_image_extension(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tif" | "tiff"
            )
        })
}

/// Load a dropped image file as a provider-safe `(media_type, bytes)` pair.
///
/// The extension only decides whether to try. The media type comes from the
/// bytes, and BMP/ICO/TIFF are converted to PNG, because an unsupported image
/// block in history makes every later request fail (#1712). Returns `None`
/// when the file is not an image the model can view, so the caller falls back
/// to inserting the path as text.
pub(super) fn load_dropped_image(path: &std::path::Path) -> Option<(String, Vec<u8>)> {
    if !has_image_extension(path) {
        return None;
    }
    let data = match std::fs::read(path) {
        Ok(data) => data,
        Err(error) => {
            crate::logging::info(&format!(
                "Dropped image {} could not be read, inserting the path instead: {error}",
                path.display()
            ));
            return None;
        }
    };
    match crate::image_normalize::normalize_image_bytes(data) {
        Ok(image) => Some((image.media_type.to_string(), image.data)),
        Err(reason) => {
            crate::logging::info(&format!(
                "dropped file {} not attached as image: {reason}",
                path.display()
            ));
            None
        }
    }
}
