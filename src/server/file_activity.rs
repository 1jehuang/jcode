use super::FileAccess;

pub(crate) fn parse_file_activity_line_range(summary: Option<&str>) -> Option<(u64, u64)> {
    let summary = summary?;
    let marker_start = summary
        .find("lines ")
        .map(|idx| idx + "lines ".len())
        .or_else(|| summary.find("line ").map(|idx| idx + "line ".len()))?;
    let rest = &summary[marker_start..];
    let mut digits = String::new();
    let mut chars = rest.chars().peekable();
    while let Some(ch) = chars.peek().copied() {
        if ch.is_ascii_digit() {
            digits.push(ch);
            chars.next();
        } else {
            break;
        }
    }
    let start = digits.parse::<u64>().ok()?;
    if chars.peek() == Some(&'-') {
        chars.next();
        let mut end_digits = String::new();
        while let Some(ch) = chars.peek().copied() {
            if ch.is_ascii_digit() {
                end_digits.push(ch);
                chars.next();
            } else {
                break;
            }
        }
        let end = end_digits.parse::<u64>().ok().unwrap_or(start);
        Some((start.min(end), start.max(end)))
    } else {
        Some((start, start))
    }
}

pub(crate) fn file_activity_scope_label(
    previous: &FileAccess,
    current: &crate::bus::FileTouch,
) -> &'static str {
    match (
        parse_file_activity_line_range(previous.summary.as_deref()),
        parse_file_activity_line_range(current.summary.as_deref()),
    ) {
        (Some((prev_start, prev_end)), Some((current_start, current_end)))
            if prev_start <= current_end && current_start <= prev_end =>
        {
            "overlapping lines"
        }
        (Some(_), Some(_)) => "same file, non-overlapping lines",
        _ => "same file",
    }
}
