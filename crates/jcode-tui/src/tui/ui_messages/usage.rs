//! Usage report formatting and width-bounded rendering.
use super::*;

pub(crate) fn render_usage_message(
    msg: &DisplayMessage,
    width: u16,
    _diff_mode: crate::config::DiffDisplayMode,
) -> Vec<Line<'static>> {
    let width = usize::from(width);
    if width == 0 {
        return Vec::new();
    }
    let border_style = Style::default().fg(rgb(120, 140, 190));
    let title = msg.title.as_deref().unwrap_or("Usage");
    // Keep the usual right margin when there is room. The shared box helper
    // needs at least six content cells, so tiny terminals use unboxed rows.
    let box_width = if width >= 14 { width - 4 } else { width }.min(100);
    let boxed = box_width >= 10;
    let content_width = if boxed { box_width - 4 } else { width };
    let raw_lines: Vec<_> = msg.content.lines().collect();
    let mut content: Vec<_> = raw_lines
        .iter()
        .enumerate()
        .flat_map(|(index, raw)| {
            if raw.is_empty()
                && index > 0
                && raw_lines[index - 1].starts_with("  ")
                && raw_lines.get(index + 1).is_some_and(|next| {
                    next.starts_with("+ ") || next.starts_with("~ ") || next.starts_with("! ")
                })
            {
                vec![Line::from(Span::styled(
                    "  ──────",
                    Style::default().fg(dim_color()),
                ))]
            } else {
                usage_report_lines(raw, content_width)
            }
        })
        .collect();
    if content.is_empty() {
        content.push(usage_styled_line("No usage data available."));
    }
    let mut content = usage_wrap_lines(content, content_width);
    if !boxed {
        return content;
    }
    // render_rounded_box already fits its contents, but returns nothing when
    // both the title and every content row are shorter than six cells.
    if content.iter().all(|line| line.width() < 6) {
        let padding = 6 - content[0].width();
        content[0].spans.push(Span::raw(" ".repeat(padding)));
    }
    render_rounded_box(title, content, box_width, border_style)
}

pub(crate) fn usage_report_lines(raw: &str, width: usize) -> Vec<Line<'static>> {
    if raw.starts_with("# ") {
        return vec![super::truncate_line_with_ellipsis_to_width(
            &usage_styled_line(raw),
            width,
        )];
    }
    // Keep each gauge together when possible. On narrow cards put the label
    // above it rather than wrapping a long label through the middle of a bar.
    if width >= 6
        && raw.width() > width
        && let Some(start) = raw.find(['█', '░'])
        && let Some(end) = raw[start..].find('%').map(|end| start + end + 1)
        && let Some(percent) = raw[start..end].split_whitespace().last()
        && let Ok(value) = percent.trim_end_matches('%').parse::<f32>()
    {
        // The report pads labels to align all providers. Drop that padding
        // when reflowing a narrow card so short names do not wrap needlessly.
        let raw_label = &raw[..start];
        let compact_label = raw_label
            .trim_end()
            .strip_suffix(':')
            .map(|label| format!("{}: ", label.trim_end()));
        let label = compact_label.as_deref().unwrap_or(raw_label);
        let original_bar_width = raw[start..]
            .chars()
            .take_while(|ch| matches!(ch, '█' | '░'))
            .count();
        let keep_label = label.width() + percent.width() + 5 <= width;
        let prefix = if keep_label { label } else { "" };
        let bar_width = width
            .saturating_sub(prefix.width() + percent.width() + 1)
            .min(original_bar_width);
        let filled =
            ((value.clamp(0.0, 100.0) / 100.0 * bar_width as f32).round() as usize).min(bar_width);
        let gauge = format!(
            "{prefix}{}{} {percent}",
            "█".repeat(filled),
            "░".repeat(bar_width - filled)
        );
        let mut lines = Vec::new();
        if !keep_label {
            lines.push(usage_styled_line(label.trim_end()));
        }
        lines.push(usage_styled_line(&gauge));
        let countdown = raw[end..].trim();
        if !countdown.is_empty() {
            lines.push(usage_styled_line(countdown));
        }
        return lines;
    }
    vec![usage_styled_line(raw)]
}

pub(crate) fn usage_utilization_style(percent: f32) -> Style {
    Style::default().fg(if percent >= 90.0 {
        Color::Red
    } else if percent >= 70.0 {
        rgb(255, 200, 100)
    } else {
        rgb(100, 220, 170)
    })
}

pub(crate) fn usage_styled_line(raw: &str) -> Line<'static> {
    let dim = Style::default().fg(dim_color());
    if let Some(heading) = raw.strip_prefix("# ") {
        return Line::from(Span::styled(
            heading.to_owned(),
            Style::default().fg(Color::White).bold(),
        ));
    }
    for (prefix, marker, percent) in [("+ ", "+", 0.0), ("~ ", "~", 70.0), ("! ", "!", 90.0)] {
        if let Some(account) = raw.strip_prefix(prefix) {
            let style = usage_utilization_style(percent);
            let (name, status) = account.rsplit_once(" - ").unwrap_or((account, ""));
            let mut spans = vec![
                Span::styled(format!("{marker} "), style.bold()),
                Span::styled(name.to_owned(), Style::default().fg(Color::White).bold()),
            ];
            if !status.is_empty() {
                spans.push(Span::styled(format!(" · {status}"), style.dim()));
            }
            return Line::from(spans);
        }
    }
    // Reset credits are actionable counts, unlike passive reset countdowns.
    for label in ["  Banked resets: ", "  Session resets: "] {
        if let Some(rest) = raw.strip_prefix(label)
            && let Some((count, suffix)) = rest.split_once(' ')
            && let Ok(count_value) = count.parse::<u64>()
        {
            return Line::from(vec![
                Span::styled(label.to_owned(), dim),
                Span::styled(
                    count.to_owned(),
                    if count_value > 0 {
                        usage_utilization_style(0.0).bold()
                    } else {
                        dim.bold()
                    },
                ),
                Span::styled(format!(" {suffix}"), dim),
            ]);
        }
    }
    if let Some(start) = raw.find(['█', '░']) {
        let rest = &raw[start..];
        let end = rest.find(|ch| ch != '█' && ch != '░').unwrap_or(rest.len());
        let (bar, tail) = rest.split_at(end);
        let percent = tail
            .trim_start()
            .split('%')
            .next()
            .and_then(|p| match p.parse::<f32>() {
                Ok(percent) => Some(percent),
                Err(error) => {
                    crate::logging::debug(&format!("Invalid usage report percentage: {error}"));
                    None
                }
            });
        if let Some(percent) = percent {
            let style = usage_utilization_style(percent);
            let filled = bar.trim_end_matches('░');
            let empty = &bar[filled.len()..];
            let (value, countdown) = tail.split_once(" · ").unwrap_or((tail, ""));
            let mut spans = vec![
                Span::styled(raw[..start].to_owned(), dim),
                Span::styled(filled.to_owned(), style),
                Span::styled(empty.to_owned(), dim),
                Span::styled(value.to_owned(), style.bold()),
            ];
            if !countdown.is_empty() {
                spans.push(Span::styled(format!(" · {countdown}"), dim));
            }
            return Line::from(spans);
        }
    }
    let style = if raw.trim_start().starts_with("error:") || raw.trim() == "hard limit reached" {
        usage_utilization_style(90.0)
    } else {
        dim
    };
    Line::from(Span::styled(raw.to_owned(), style))
}

// Hard-wrap styled report rows before boxing rather than letting the box
// silently truncate them. A glyph wider than a tiny viewport becomes an
// ellipsis, so even a one-cell terminal cannot overflow.
pub(crate) fn usage_wrap_lines(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    use unicode_width::UnicodeWidthChar;
    let mut result = Vec::new();
    for line in lines.into_iter().flat_map(|line| {
        if line.width() <= width {
            vec![line]
        } else {
            markdown::wrap_lines(vec![line], width)
        }
    }) {
        let mut row = Vec::new();
        let mut used = 0;
        for span in line.spans {
            let mut text = String::new();
            for ch in span.content.chars() {
                let cells = ch.width().unwrap_or(0);
                if used + cells > width && used > 0 {
                    row.push(Span::styled(std::mem::take(&mut text), span.style));
                    result.push(Line::from(std::mem::take(&mut row)));
                    used = 0;
                }
                if cells > width {
                    text.push('…');
                    used += 1;
                } else {
                    text.push(ch);
                    used += cells;
                }
            }
            if !text.is_empty() {
                row.push(Span::styled(text, span.style));
            }
        }
        result.push(Line::from(row));
    }
    result
}
