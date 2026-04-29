use ratatui::prelude::{Line, Span, Style};

pub fn render_rounded_box(
    title: &str,
    content: Vec<Line<'static>>,
    max_width: usize,
    border_style: Style,
) -> Vec<Line<'static>> {
    if content.is_empty() || max_width < 6 {
        return Vec::new();
    }

    let max_content_width = content
        .iter()
        .map(|line| line.width())
        .max()
        .unwrap_or(0)
        .min(max_width.saturating_sub(4));

    let truncated_title = truncate_line_with_ellipsis_to_width(
        &Line::from(Span::raw(format!(" {} ", title))),
        max_width.saturating_sub(2).max(1),
    );
    let title_text = line_plain_text(&truncated_title);
    let title_len = truncated_title.width();
    let box_content_width = max_content_width.max(title_len.saturating_sub(2));

    if box_content_width < 6 {
        return Vec::new();
    }

    let box_width = box_content_width + 4;
    let border_chars = box_width.saturating_sub(title_len + 2);
    let left_border = "─".repeat(border_chars / 2);
    let right_border = "─".repeat(border_chars - border_chars / 2);

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(Span::styled(
        format!("╭{}{}{}╮", left_border, title_text, right_border),
        border_style,
    )));

    for line in content {
        let truncated = truncate_line_to_width(&line, box_content_width);
        let padding = box_content_width.saturating_sub(truncated.width());
        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(Span::styled("│ ", border_style));
        spans.extend(truncated.spans);
        if padding > 0 {
            spans.push(Span::raw(" ".repeat(padding)));
        }
        spans.push(Span::styled(" │", border_style));
        lines.push(Line::from(spans));
    }

    let bottom_border = "─".repeat(box_width.saturating_sub(2));
    lines.push(Line::from(Span::styled(
        format!("╰{}╯", bottom_border),
        border_style,
    )));

    lines
}

pub fn truncate_line_to_width(line: &Line<'static>, width: usize) -> Line<'static> {
    if width == 0 {
        return Line::from("");
    }

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut remaining = width;
    for span in &line.spans {
        if remaining == 0 {
            break;
        }
        let text = span.content.as_ref();
        let span_width = unicode_width::UnicodeWidthStr::width(text);
        if span_width <= remaining {
            spans.push(span.clone());
            remaining -= span_width;
        } else {
            let mut clipped = String::new();
            let mut used = 0;
            for ch in text.chars() {
                let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if used + cw > remaining {
                    break;
                }
                clipped.push(ch);
                used += cw;
            }
            if !clipped.is_empty() {
                spans.push(Span::styled(clipped, span.style));
            }
            remaining = 0;
        }
    }

    if spans.is_empty() {
        Line::from("")
    } else {
        Line::from(spans)
    }
}

pub fn truncate_line_with_ellipsis_to_width(line: &Line<'static>, width: usize) -> Line<'static> {
    if width == 0 {
        return Line::from("");
    }
    if line.width() <= width {
        return line.clone();
    }
    if width == 1 {
        return Line::from(Span::raw("…"));
    }

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut remaining = width.saturating_sub(1);
    let mut ellipsis_style = Style::default();

    for span in &line.spans {
        if remaining == 0 {
            break;
        }
        let text = span.content.as_ref();
        let span_width = unicode_width::UnicodeWidthStr::width(text);
        if span_width <= remaining {
            spans.push(span.clone());
            remaining -= span_width;
            ellipsis_style = span.style;
        } else {
            let mut clipped = String::new();
            let mut used = 0;
            for ch in text.chars() {
                let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if used + cw > remaining {
                    break;
                }
                clipped.push(ch);
                used += cw;
            }
            if !clipped.is_empty() {
                spans.push(Span::styled(clipped, span.style));
                ellipsis_style = span.style;
            }
            break;
        }
    }

    spans.push(Span::styled("…", ellipsis_style));
    let mut truncated = Line::from(spans);
    truncated.alignment = line.alignment;
    truncated
}

pub fn truncate_line_preserving_suffix_to_width(
    prefix: &Line<'static>,
    suffix: &Line<'static>,
    width: usize,
) -> Line<'static> {
    if width == 0 {
        return Line::from("");
    }

    if suffix.width() == 0 {
        return truncate_line_with_ellipsis_to_width(prefix, width);
    }

    let mut combined_spans = prefix.spans.clone();
    combined_spans.extend(suffix.spans.clone());
    let mut combined = Line::from(combined_spans);
    combined.alignment = prefix.alignment;
    if combined.width() <= width {
        return combined;
    }

    let suffix_width = suffix.width();
    if suffix_width >= width {
        let mut truncated = truncate_line_with_ellipsis_to_width(suffix, width);
        truncated.alignment = prefix.alignment;
        return truncated;
    }

    let prefix_budget = width.saturating_sub(suffix_width);
    let mut prefix_part = truncate_line_with_ellipsis_to_width(prefix, prefix_budget);
    prefix_part.spans.extend(suffix.spans.clone());
    prefix_part.alignment = prefix.alignment;
    prefix_part
}

pub fn line_plain_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}
