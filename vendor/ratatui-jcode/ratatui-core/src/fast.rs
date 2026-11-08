//! ratatui-jcode: fast paths for the per-frame hot loops.
//!
//! Terminal transcripts are overwhelmingly printable ASCII. For such text every
//! byte is its own grapheme cluster with display width 1, so the full Unicode
//! segmentation and width tables can be skipped. Every helper falls back to the
//! exact upstream computation for anything outside printable ASCII.

use unicode_width::UnicodeWidthStr;

/// True when every byte is printable ASCII (0x20..=0x7E).
#[inline]
pub(crate) fn is_printable_ascii(s: &str) -> bool {
    s.bytes().all(|b| (0x20..0x7f).contains(&b))
}

/// Display width of `s`, identical to [`UnicodeWidthStr::width`].
#[inline]
pub fn str_width(s: &str) -> usize {
    if is_printable_ascii(s) {
        s.len()
    } else {
        s.width()
    }
}

/// Display width of a single cell symbol. Single printable ASCII bytes (the
/// common case) are width 1 without a table lookup.
#[inline]
pub(crate) fn symbol_width(s: &str) -> usize {
    match s.as_bytes() {
        [b] if (0x20..0x7f).contains(b) => 1,
        _ => s.width(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_unicode_width() {
        for s in [
            "", " ", "abc", "a\tb", "\u{1b}[0m", "héllo", "日本", "👍🏽", "\u{200b}", "~!@#",
            "\n", "a\r\nb",
        ] {
            assert_eq!(str_width(s), s.width(), "{s:?}");
            assert_eq!(symbol_width(s), s.width(), "{s:?}");
        }
    }
}

#[cfg(test)]
mod grapheme_parity {
    use alloc::vec::Vec;
    use unicode_segmentation::UnicodeSegmentation;

    use crate::style::Style;
    use crate::text::Span;

    #[test]
    fn ascii_fast_path_matches_upstream_segmentation() {
        for s in ["", "hello world", "a\tb", "x\ry", "fn main() {}", "héllo", "e\u{301}", "👍🏽 ok"] {
            let span = Span::raw(s);
            let fast: Vec<&str> = span
                .styled_graphemes(Style::default())
                .map(|g| g.symbol)
                .collect();
            let upstream: Vec<&str> = s
                .graphemes(true)
                .filter(|g| !g.contains(char::is_control))
                .collect();
            assert_eq!(fast, upstream, "{s:?}");
        }
    }
}
