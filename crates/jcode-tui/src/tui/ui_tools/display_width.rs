use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(super) fn display_prefix_by_width(s: &str, max_width: usize) -> &str {
    if max_width == 0 {
        return "";
    }
    let mut used = 0usize;
    let mut end = 0usize;
    for (idx, ch) in s.char_indices() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + cw > max_width {
            break;
        }
        used += cw;
        end = idx + ch.len_utf8();
    }
    &s[..end]
}

pub(super) fn display_suffix_by_width(s: &str, max_width: usize) -> &str {
    if max_width == 0 {
        return "";
    }
    let mut used = 0usize;
    let mut start = s.len();
    for (idx, ch) in s.char_indices().rev() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + cw > max_width {
            break;
        }
        used += cw;
        start = idx;
    }
    &s[start..]
}

pub(super) fn truncate_end_display(s: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(s) <= max_width {
        return s.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "…".to_string();
    }
    format!(
        "{}…",
        display_prefix_by_width(s, max_width.saturating_sub(1))
    )
}

pub(super) fn truncate_middle_display(s: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(s) <= max_width {
        return s.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "…".to_string();
    }
    let remaining = max_width.saturating_sub(1);
    let head = remaining / 2 + remaining % 2;
    let tail = remaining / 2;
    format!(
        "{}…{}",
        display_prefix_by_width(s, head),
        display_suffix_by_width(s, tail)
    )
}
