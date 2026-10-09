pub(super) fn line_display_width(text: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(text)
}

pub(super) fn display_col_to_byte_offset(text: &str, display_col: usize) -> usize {
    let mut width = 0usize;
    for (idx, ch) in text.char_indices() {
        if width >= display_col {
            return idx;
        }
        let next_width =
            width.saturating_add(unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0));
        if next_width > display_col {
            return idx;
        }
        width = next_width;
    }
    text.len()
}

/// Like [`display_col_to_byte_offset`], but when `display_col` lands inside a
/// wide character the whole character is included. Use for the end of a copy
/// selection, where the mouse can only reach the right half of a wide char.
pub(super) fn display_col_to_byte_offset_inclusive(text: &str, display_col: usize) -> usize {
    let mut width = 0usize;
    for (idx, ch) in text.char_indices() {
        if width >= display_col {
            return idx;
        }
        let next_width =
            width.saturating_add(unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0));
        if next_width > display_col {
            return idx + ch.len_utf8();
        }
        width = next_width;
    }
    text.len()
}

pub(super) fn clamp_display_col(text: &str, display_col: usize) -> usize {
    display_col.min(line_display_width(text))
}

pub(super) fn display_col_slice(text: &str, start_col: usize, end_col: usize) -> &str {
    let start_byte = display_col_to_byte_offset(text, start_col);
    let end_byte = display_col_to_byte_offset(text, end_col);
    &text[start_byte..end_byte]
}

/// Copy-selection slice: the end column includes a wide character it lands
/// inside, so selecting up to the last cell of a CJK char copies that char.
pub(super) fn display_col_slice_copy(text: &str, start_col: usize, end_col: usize) -> &str {
    if end_col <= start_col {
        return "";
    }
    let start_byte = display_col_to_byte_offset(text, start_col);
    let end_byte = display_col_to_byte_offset_inclusive(text, end_col).max(start_byte);
    &text[start_byte..end_byte]
}

#[cfg(test)]
mod tests {
    use super::{
        clamp_display_col, display_col_slice, display_col_slice_copy, display_col_to_byte_offset,
        display_col_to_byte_offset_inclusive, line_display_width,
    };

    #[test]
    fn inclusive_offset_includes_partially_covered_wide_char() {
        let text = "你好世界";
        assert_eq!(display_col_to_byte_offset_inclusive(text, 0), 0);
        assert_eq!(display_col_to_byte_offset_inclusive(text, 1), "你".len());
        assert_eq!(display_col_to_byte_offset_inclusive(text, 2), "你".len());
        assert_eq!(display_col_to_byte_offset_inclusive(text, 7), text.len());
        assert_eq!(display_col_to_byte_offset_inclusive("abc", 2), 2);
    }

    #[test]
    fn copy_slice_keeps_last_cjk_char_when_end_is_inside_it() {
        // Issue #1766: the mouse can only reach col 7 (right half of 界).
        assert_eq!(display_col_slice_copy("你好世界", 0, 7), "你好世界");
        assert_eq!(display_col_slice_copy("你好世界", 0, 8), "你好世界");
        assert_eq!(display_col_slice_copy("你好世界", 2, 5), "好世");
        assert_eq!(display_col_slice_copy("a🙂bc", 0, 2), "a🙂");
        assert_eq!(display_col_slice_copy("abc", 0, 2), "ab");
        assert_eq!(display_col_slice_copy("你好", 1, 1), "");
    }

    #[test]
    fn line_display_width_counts_wide_chars() {
        assert_eq!(line_display_width("abc"), 3);
        assert_eq!(line_display_width("a🙂b"), 4);
    }

    #[test]
    fn display_col_to_byte_offset_stops_before_partial_wide_char() {
        let text = "a🙂b";

        assert_eq!(display_col_to_byte_offset(text, 0), 0);
        assert_eq!(display_col_to_byte_offset(text, 1), 1);
        assert_eq!(display_col_to_byte_offset(text, 2), 1);
        assert_eq!(display_col_to_byte_offset(text, 3), "a🙂".len());
        assert_eq!(display_col_to_byte_offset(text, 99), text.len());
    }

    #[test]
    fn clamp_display_col_caps_at_line_display_width() {
        assert_eq!(clamp_display_col("a🙂b", 99), 4);
        assert_eq!(clamp_display_col("a🙂b", 2), 2);
    }

    #[test]
    fn display_col_slice_respects_wide_char_boundaries() {
        let text = "a🙂bc";

        assert_eq!(display_col_slice(text, 0, 1), "a");
        assert_eq!(display_col_slice(text, 1, 3), "🙂");
        assert_eq!(display_col_slice(text, 2, 4), "🙂b");
        assert_eq!(display_col_slice(text, 3, 99), "bc");
    }
}
