pub(super) fn truncate_smart(s: &str, max_len: usize) -> String {
    let char_len = s.chars().count();
    if char_len <= max_len {
        return s.to_string();
    }
    if max_len <= 3 {
        return "...".to_string();
    }

    let target = max_len - 3;
    let prefix = truncate_chars(s, target);

    if let Some(pos) = prefix.rfind(' ') {
        let before = &prefix[..pos];
        let pos_chars = before.chars().count();
        if pos_chars > target / 2 {
            return format!("{}...", before);
        }
    }
    format!("{}...", prefix)
}

pub(super) fn truncate_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

pub(super) fn truncate_with_ellipsis(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    if max_chars == 1 {
        return "…".to_string();
    }
    let truncated = truncate_chars(s, max_chars.saturating_sub(1));
    format!("{}…", truncated)
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
