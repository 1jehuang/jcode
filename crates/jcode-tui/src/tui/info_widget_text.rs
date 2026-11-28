use unicode_width::UnicodeWidthChar;

/// Longest prefix of `s` that fits `max_cells` display cells.
///
/// Info widgets live in a narrow margin, so a cut row ends at the widget edge
/// with no ellipsis: every cell carries content instead of `...`.
pub(super) fn clip(s: &str, max_cells: usize) -> String {
    let mut used = 0;
    for (idx, ch) in s.char_indices() {
        used += ch.width().unwrap_or(0);
        if used > max_cells {
            return s[..idx].trim_end().to_string();
        }
    }
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::clip;

    #[test]
    fn clip_fills_the_width_without_an_ellipsis() {
        assert_eq!(clip("eagle running - keep going", 15), "eagle running -");
        assert_eq!(clip("short", 15), "short");
        assert_eq!(clip("abc", 0), "");
        // Trailing space at the cut is dropped, not rendered.
        assert_eq!(clip("ab cd", 3), "ab");
    }

    #[test]
    fn clip_counts_display_cells_not_chars() {
        // Each CJK glyph is two cells: only two fit in five.
        assert_eq!(clip("日本語テキスト", 5), "日本");
    }
}

/// Truncate to a display-cell budget with an ellipsis, so wide (CJK) names
/// cannot overflow a compact UI row. Char-based truncation passes text that
/// still renders past the right edge: 10 CJK chars fit "width 10" by char
/// count but occupy 20 cells. The repo counts cells for UI chrome, not chars.
pub(super) fn truncate_cells(s: &str, max_cells: usize) -> String {
    if unicode_width::UnicodeWidthStr::width(s) <= max_cells {
        return s.to_string();
    }
    if max_cells == 0 {
        return String::new();
    }
    if max_cells == 1 {
        return "…".to_string();
    }

    let mut out = String::new();
    let mut used = 0usize;
    for ch in s.chars() {
        let ch_cells = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_cells + 1 > max_cells {
            break;
        }
        out.push(ch);
        used += ch_cells;
    }
    out.push('…');
    out
}
