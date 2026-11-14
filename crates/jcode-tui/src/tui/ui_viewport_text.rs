//! Width-safe line trimming for viewport copy badges.
use super::*;

pub(crate) fn truncate_line_in_place_to_width(line: &mut Line<'static>, max_width: usize) {
    let mut remaining = max_width;
    // Copy badges are prepared during rendering. Retain the existing span
    // storage instead of draining into a newly allocated vector each time.
    let mut kept = 0;

    for span in &mut line.spans {
        if remaining == 0 {
            break;
        }

        let span_width = span.content.as_ref().width();
        if span_width <= remaining {
            remaining = remaining.saturating_sub(span_width);
            kept += 1;
            continue;
        }

        let mut text = String::new();
        let mut used = 0usize;
        for ch in span.content.chars() {
            let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if ch_width > 0 && used.saturating_add(ch_width) > remaining {
                break;
            }
            text.push(ch);
            used = used.saturating_add(ch_width);
        }
        if !text.is_empty() {
            span.content = text.into();
            kept += 1;
        }
        break;
    }

    line.spans.truncate(kept);
}

/// Remove trailing plain spaces from a line so an appended badge sits exactly
/// one separator space after the content, regardless of how the source line
/// was rendered (code block headers end with a space, blockquote lines don't).
pub(crate) fn trim_line_trailing_spaces(line: &mut Line<'static>) {
    while let Some(last) = line.spans.last_mut() {
        let trimmed = last.content.trim_end_matches(' ');
        if trimmed.len() == last.content.len() {
            break;
        }
        if trimmed.is_empty() {
            line.spans.pop();
        } else {
            last.content = std::borrow::Cow::Owned(trimmed.to_string());
            break;
        }
    }
}

/// Prepare a line to host an inline badge within `max_content_width` cells.
/// When the content already fits, only trailing spaces are trimmed (the badge
/// separator space is accounted for in the reserved width). When the content
/// must be cut, end it with a dim ellipsis so the truncation is visible
/// instead of silently swallowing words.
pub(crate) fn truncate_line_for_copy_badge(line: &mut Line<'static>, max_content_width: usize) {
    if max_content_width == 0 {
        line.spans.clear();
        return;
    }
    if line.width() <= max_content_width {
        trim_line_trailing_spaces(line);
        return;
    }
    truncate_line_in_place_to_width(line, max_content_width.saturating_sub(1));
    trim_line_trailing_spaces(line);
    line.spans
        .push(Span::styled("…", Style::default().fg(dim_color())));
}

pub(super) fn copy_badge_alt_label_from_config(configured: &str) -> String {
    let configured = configured.trim();
    if configured.is_empty() {
        COPY_BADGE_ALT_LABEL.to_string()
    } else {
        configured.to_string()
    }
}

#[cfg(test)]
mod allocation_tests {
    use super::*;

    // Preserve the pre-optimization implementation as the output oracle.
    fn legacy_truncate(line: &mut Line<'static>, max_width: usize) {
        let mut remaining = max_width;
        let mut kept = Vec::new();
        for span in line.spans.drain(..) {
            if remaining == 0 {
                break;
            }
            let width = span.content.as_ref().width();
            if width <= remaining {
                remaining = remaining.saturating_sub(width);
                kept.push(span);
                continue;
            }
            let mut text = String::new();
            let mut used = 0usize;
            for ch in span.content.chars() {
                let width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if width > 0 && used.saturating_add(width) > remaining {
                    break;
                }
                text.push(ch);
                used = used.saturating_add(width);
            }
            if !text.is_empty() {
                kept.push(Span::styled(text, span.style));
            }
            break;
        }
        line.spans = kept;
    }

    #[test]
    fn viewport_truncation_matches_legacy_for_unicode_and_styles() {
        let samples = [
            "",
            "abc ",
            "日本語",
            "e\u{301}",
            "\u{301}",
            "👩‍💻",
            "\t",
            "x\ny",
        ];
        for first in samples {
            for second in samples {
                for width in 0..=16 {
                    let mut actual = Line::from(vec![
                        Span::styled(first, Style::default().fg(Color::Red)),
                        Span::styled(second.to_owned(), Style::default().bg(Color::Blue)),
                        Span::raw(" tail "),
                    ]);
                    let mut expected = actual.clone();
                    legacy_truncate(&mut expected, width);
                    truncate_line_in_place_to_width(&mut actual, width);
                    assert_eq!(actual, expected, "{first:?}, {second:?}, width={width}");
                }
            }
        }
    }

    #[test]
    fn viewport_truncation_reuses_span_storage() {
        let mut legacy_replacements = 0;
        let mut current_replacements = 0;
        for width in 0..=16 {
            let mut spans = Vec::with_capacity(32);
            spans.extend([Span::raw("abc"), Span::raw("日本語"), Span::raw("tail")]);
            let mut line = Line::from(spans);
            let mut legacy = line.clone();
            let legacy_pointer = legacy.spans.as_ptr();
            legacy_truncate(&mut legacy, width);
            legacy_replacements += usize::from(legacy.spans.as_ptr() != legacy_pointer);
            let pointer = line.spans.as_ptr();
            let capacity = line.spans.capacity();
            truncate_line_in_place_to_width(&mut line, width);
            current_replacements += usize::from(line.spans.as_ptr() != pointer);
            assert_eq!(line.spans.as_ptr(), pointer, "width={width}");
            assert_eq!(line.spans.capacity(), capacity, "width={width}");
        }
        assert_eq!(legacy_replacements, 17);
        assert_eq!(current_replacements, 0);
        eprintln!(
            "span storage replacements across 17 widths: legacy={legacy_replacements}, current={current_replacements}"
        );
    }
}
