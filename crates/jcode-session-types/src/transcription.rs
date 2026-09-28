//! Speech-to-text output is marked so the model knows the words were
//! transcribed (and may contain recognition errors), not typed. Same format as
//! Jcode Desktop, so sessions render identically in every client.

use std::borrow::Cow;

pub const TRANSCRIPTION_OPEN: &str = "<transcription>";
pub const TRANSCRIPTION_CLOSE: &str = "</transcription>";

/// Wrap a transcript for the model.
pub fn wrap_transcription(text: &str) -> String {
    format!(
        "{TRANSCRIPTION_OPEN}\n{}\n{TRANSCRIPTION_CLOSE}",
        text.trim()
    )
}

/// Remove transcription tags for display. Returns the visible text and
/// whether any complete tagged segment was found. Unbalanced text is kept
/// verbatim so an unrelated literal `<transcription>` is never hidden.
pub fn strip_transcription(text: &str) -> (Cow<'_, str>, bool) {
    if !text.contains(TRANSCRIPTION_OPEN) || !text.contains(TRANSCRIPTION_CLOSE) {
        return (Cow::Borrowed(text), false);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut found = false;
    while let Some(start) = rest.find(TRANSCRIPTION_OPEN) {
        let after_open = &rest[start + TRANSCRIPTION_OPEN.len()..];
        let Some(end) = after_open.find(TRANSCRIPTION_CLOSE) else {
            break;
        };
        out.push_str(&rest[..start]);
        out.push_str(after_open[..end].trim_matches('\n'));
        rest = &after_open[end + TRANSCRIPTION_CLOSE.len()..];
        found = true;
    }
    if !found {
        return (Cow::Borrowed(text), false);
    }
    out.push_str(rest);
    (Cow::Owned(out.trim().to_string()), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_and_strip_round_trip() {
        let wrapped = wrap_transcription("  fix the flaky test ");
        assert_eq!(
            wrapped,
            "<transcription>\nfix the flaky test\n</transcription>"
        );
        assert_eq!(
            strip_transcription(&wrapped),
            (Cow::Borrowed("fix the flaky test"), true)
        );
    }

    #[test]
    fn strip_keeps_typed_text_and_multiple_segments() {
        let text = format!(
            "Typed first\n{}\nand {}",
            wrap_transcription("one"),
            wrap_transcription("two")
        );
        let (visible, found) = strip_transcription(&text);
        assert!(found);
        assert_eq!(visible, "Typed first\none\nand two");
    }

    #[test]
    fn unbalanced_or_absent_tags_are_untouched() {
        for text in [
            "plain",
            "<transcription> open only",
            "close only </transcription>",
        ] {
            assert_eq!(strip_transcription(text), (Cow::Borrowed(text), false));
        }
    }
}
