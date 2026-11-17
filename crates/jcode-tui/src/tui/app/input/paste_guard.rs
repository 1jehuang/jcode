//! Guard against the stray Enter key event some terminals (Windows Terminal /
//! conhost) deliver immediately after a bracketed paste that ends with a
//! newline. Without this, pasting multi-line text submitted the chat (#544).
//!
//! Paste events and key events are both handled on the TUI event-loop thread,
//! so a thread-local timestamp is sufficient and keeps `App` untouched.

use std::cell::Cell;
use std::time::{Duration, Instant};

const PASTE_ENTER_SUPPRESS_WINDOW: Duration = Duration::from_millis(150);

/// How soon after a character a `Ctrl+Enter` must arrive to be that character's
/// pasted line break.
///
/// Measured on a real ConPTY: a terminal paste reaches this TUI as one Press/Release
/// pair per character, and **every newline inside the paste arrives as
/// `KeyCode::Enter` with `KeyModifiers::CONTROL`**, not as a plain Enter and not as a
/// text character. A human's deliberate `Ctrl+Enter` never follows a character
/// keystroke within this window, so the two are distinguishable without a timer.
const PASTED_NEWLINE_MAX_GAP: Duration = Duration::from_millis(60);

thread_local! {
    static LAST_CHAR_PRESS: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Remember that a character arrived from a key press.
pub(super) fn note_char_press() {
    LAST_CHAR_PRESS.with(|cell| cell.set(Some(Instant::now())));
}

/// Whether a `Ctrl+Enter` right now is the line break of a paste, rather than a
/// deliberate chord.
pub(super) fn ctrl_enter_is_pasted_newline() -> bool {
    LAST_CHAR_PRESS.with(|cell| {
        cell.get()
            .is_some_and(|at| at.elapsed() <= PASTED_NEWLINE_MAX_GAP)
    })
}

/// The paste is over once a key other than a character or its newline arrives.
pub(super) fn clear_char_press() {
    LAST_CHAR_PRESS.with(|cell| cell.set(None));
}

/// Test hook: pretend the last character arrived `age` ago, so window edges can be
/// checked without sleeping and without a load-sensitive race.
#[cfg(test)]
pub(super) fn age_char_press_for_test(age: Duration) {
    LAST_CHAR_PRESS.with(|cell| cell.set(Instant::now().checked_sub(age)));
}

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

#[cfg(test)]
mod tests {
    use super::{
        Duration, PASTED_NEWLINE_MAX_GAP, age_char_press_for_test, clear_char_press,
        ctrl_enter_is_pasted_newline,
    };

    #[test]
    fn a_plain_ctrl_enter_with_no_character_before_it_is_the_deliberate_chord() {
        clear_char_press();
        assert!(!ctrl_enter_is_pasted_newline());
    }

    #[test]
    fn a_ctrl_enter_right_after_a_character_is_a_pasted_line_break() {
        age_char_press_for_test(Duration::from_millis(5));
        assert!(ctrl_enter_is_pasted_newline());
    }

    #[test]
    fn a_ctrl_enter_after_the_window_is_the_deliberate_chord() {
        age_char_press_for_test(PASTED_NEWLINE_MAX_GAP + Duration::from_millis(60));
        assert!(!ctrl_enter_is_pasted_newline());
    }

    #[test]
    fn clearing_the_character_press_ends_the_paste() {
        age_char_press_for_test(Duration::from_millis(5));
        clear_char_press();
        assert!(!ctrl_enter_is_pasted_newline());
    }
}
