//! Dedup key for `AvailableModelsUpdated` catalog broadcasts.
//!
//! Split out of `client_lifecycle.rs` to keep that file off the code-size
//! ratchet's growth list.

use crate::protocol::ServerEvent;

/// Dedup key for `AvailableModelsUpdated` that ignores cosmetic relative-age
/// text in route details.
///
/// Route details embed human-readable cache ages ("12m ago", "3h ago"). Those
/// strings tick forward on their own, so a byte-comparison of encoded events
/// reports a "change" every time an age bucket rolls over even though no model
/// or route actually changed. That made otherwise-identical catalogs fan out to
/// every connected client, and each client then invalidated its picker cache,
/// rewrote its on-disk catalog cache, and repainted a full frame, which starved
/// the input line.
///
/// Normalizing those substrings out means a catalog whose only difference is
/// elapsed time is treated as unchanged and never broadcast. Real changes
/// (models, providers, api methods, availability, non-age detail text) still
/// differ and still propagate.
pub(super) fn available_models_dedup_key(event: &ServerEvent) -> String {
    let encoded = crate::protocol::encode_event(event);
    strip_relative_age_text(&encoded)
}

/// Replace `<digits><unit> ago` runs (e.g. `12m ago`, `3h ago`, `5d ago`) with a
/// fixed placeholder so relative-age drift does not register as a change.
pub(super) fn strip_relative_age_text(text: &str) -> String {
    const AGE_SUFFIX: &str = " ago";
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < text.len() {
        // Look for the start of a "<digits><unit> ago" run at this position.
        let digits_end = bytes[i..]
            .iter()
            .position(|b| !b.is_ascii_digit())
            .map(|offset| i + offset)
            .unwrap_or(text.len());
        let has_digits = digits_end > i;
        let unit_is_age = matches!(bytes.get(digits_end), Some(b's' | b'm' | b'h' | b'd'));
        // Only slice past the unit byte once we know a unit byte exists, so a
        // trailing digit run at end-of-string cannot index out of range.
        if has_digits && unit_is_age && text[digits_end + 1..].starts_with(AGE_SUFFIX) {
            out.push_str("<age>");
            i = digits_end + 1 + AGE_SUFFIX.len();
            continue;
        }
        // Not an age run: copy one char and continue from the next boundary.
        // `i` is always a char boundary here, so `chars().next()` yields a char
        // for any non-empty remainder; fall through to the end otherwise.
        let Some(ch) = text[i..].chars().next() else {
            break;
        };
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
#[path = "client_lifecycle_catalog_dedup_tests.rs"]
mod tests;
