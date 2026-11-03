//! Cell-width-safe gallery labels.

pub(crate) fn truncate_label(s: &str, max: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    if disp_w(s) <= max {
        return s.to_string();
    }
    if max <= 1 {
        return "…".to_string();
    }
    let target = max - 1;
    let mut out = String::new();
    let mut used = 0usize;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if used + cw > target {
            break;
        }
        used += cw;
        out.push(ch);
    }
    out.push('…');
    out
}

/// Terminal display width of a string (wide glyphs like 🐝 count as 2).
pub(crate) fn disp_w(s: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    s.width()
}

/// Number of decimal digits in `n` (for budgeting "+N" markers).
pub(crate) fn count_digits(n: usize) -> usize {
    let mut n = n.max(1);
    let mut d = 0;
    while n > 0 {
        d += 1;
        n /= 10;
    }
    d
}
