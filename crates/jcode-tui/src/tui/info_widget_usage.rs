use super::{InfoWidgetData, UsageInfo, UsageProvider};
use crate::tui::color_support::rgb;
use ratatui::prelude::*;
use unicode_width::UnicodeWidthStr;

pub(super) fn render_usage_widget(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let Some(info) = &data.usage_info else {
        return Vec::new();
    };
    if !info.available {
        return Vec::new();
    }

    match info.provider {
        UsageProvider::Copilot => {
            vec![Line::from(vec![Span::styled(
                format!(
                    "{} in + {} out",
                    format_tokens(info.input_tokens),
                    format_tokens(info.output_tokens)
                ),
                Style::default().fg(rgb(140, 140, 150)),
            )])]
        }
        UsageProvider::CostBased => {
            vec![
                Line::from(vec![
                    Span::styled("💰 ", Style::default().fg(rgb(140, 180, 255))),
                    Span::styled(
                        format!("${:.4}", info.total_cost),
                        Style::default().fg(rgb(180, 180, 190)).bold(),
                    ),
                ]),
                Line::from(vec![Span::styled(
                    format!(
                        "{} in + {} out",
                        format_tokens(info.input_tokens),
                        format_tokens(info.output_tokens)
                    ),
                    Style::default().fg(rgb(140, 140, 150)),
                )]),
            ]
        }
        _ => {
            let five_hr_used = (info.five_hour * 100.0).round().clamp(0.0, 100.0) as u8;
            let seven_day_used = (info.seven_day * 100.0).round().clamp(0.0, 100.0) as u8;
            let five_hr_left = 100u8.saturating_sub(five_hr_used);
            let seven_day_left = 100u8.saturating_sub(seven_day_used);

            let five_hr_reset = info
                .five_hour_resets_at
                .as_deref()
                .map(crate::usage::format_reset_time);
            let seven_day_reset = info
                .seven_day_resets_at
                .as_deref()
                .map(crate::usage::format_reset_time);

            let mut lines = Vec::new();
            let label = info.provider.label();
            if !label.is_empty() {
                lines.push(Line::from(vec![Span::styled(
                    format!("{} limits", label),
                    Style::default()
                        .fg(rgb(140, 140, 150))
                        .add_modifier(ratatui::style::Modifier::DIM),
                )]));
            }
            if let Some(primary_label) = info.primary_limit_label.as_deref() {
                lines.push(render_labeled_bar(
                    primary_label,
                    five_hr_used,
                    five_hr_left,
                    five_hr_reset.as_deref(),
                    inner.width,
                    data.usage_display_used,
                ));
            }
            if let Some(secondary_label) = info.secondary_limit_label.as_deref() {
                lines.push(render_labeled_bar(
                    secondary_label,
                    seven_day_used,
                    seven_day_left,
                    seven_day_reset.as_deref(),
                    inner.width,
                    data.usage_display_used,
                ));
            }
            if let Some(spark_usage) = info.spark {
                let spark_used = (spark_usage * 100.0).round().clamp(0.0, 100.0) as u8;
                let spark_left = 100u8.saturating_sub(spark_used);
                let spark_reset = info
                    .spark_resets_at
                    .as_deref()
                    .map(crate::usage::format_reset_time);
                lines.push(render_labeled_bar(
                    "Spark",
                    spark_used,
                    spark_left,
                    spark_reset.as_deref(),
                    inner.width,
                    data.usage_display_used,
                ));
            }
            lines
        }
    }
}

pub(super) fn render_usage_compact(
    info: &UsageInfo,
    width: u16,
    usage_display_used: bool,
) -> Vec<Line<'static>> {
    if !info.available {
        return Vec::new();
    }

    if matches!(info.provider, UsageProvider::CostBased) {
        return vec![Line::from(vec![Span::styled(
            format!(
                "${:.4} · {} in + {} out",
                info.total_cost,
                format_tokens(info.input_tokens),
                format_tokens(info.output_tokens)
            ),
            Style::default().fg(rgb(140, 140, 150)),
        )])];
    }

    let five_hr_used = (info.five_hour * 100.0).round().clamp(0.0, 100.0) as u8;
    let seven_day_used = (info.seven_day * 100.0).round().clamp(0.0, 100.0) as u8;
    let five_hr_left = 100u8.saturating_sub(five_hr_used);
    let seven_day_left = 100u8.saturating_sub(seven_day_used);
    let five_hr_reset = info
        .five_hour_resets_at
        .as_deref()
        .map(crate::usage::format_reset_time);
    let seven_day_reset = info
        .seven_day_resets_at
        .as_deref()
        .map(crate::usage::format_reset_time);

    let mut lines = Vec::new();
    let label = info.provider.label();
    if !label.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            format!("{} limits", label),
            Style::default()
                .fg(rgb(140, 140, 150))
                .add_modifier(ratatui::style::Modifier::DIM),
        )]));
    }
    if let Some(primary_label) = info.primary_limit_label.as_deref() {
        lines.push(render_labeled_bar(
            primary_label,
            five_hr_used,
            five_hr_left,
            five_hr_reset.as_deref(),
            width,
            usage_display_used,
        ));
    }
    if let Some(secondary_label) = info.secondary_limit_label.as_deref() {
        lines.push(render_labeled_bar(
            secondary_label,
            seven_day_used,
            seven_day_left,
            seven_day_reset.as_deref(),
            width,
            usage_display_used,
        ));
    }
    if let Some(spark_usage) = info.spark {
        let spark_used = (spark_usage * 100.0).round().clamp(0.0, 100.0) as u8;
        let spark_left = 100u8.saturating_sub(spark_used);
        let spark_reset = info
            .spark_resets_at
            .as_deref()
            .map(crate::usage::format_reset_time);
        lines.push(render_labeled_bar(
            "Spark",
            spark_used,
            spark_left,
            spark_reset.as_deref(),
            width,
            usage_display_used,
        ));
    }
    lines
}

fn render_labeled_bar(
    label: &str,
    used_pct: u8,
    left_pct: u8,
    reset_time: Option<&str>,
    width: u16,
    usage_display_used: bool,
) -> Line<'static> {
    let color = if left_pct <= 20 {
        rgb(255, 100, 100)
    } else if left_pct <= 50 {
        rgb(255, 200, 100)
    } else {
        rgb(100, 200, 100)
    };

    const LABEL_WIDTH: usize = 7;
    const MIN_BAR_WIDTH: usize = 4;

    let (display_pct, display_word) = if usage_display_used {
        (used_pct, "used")
    } else {
        (left_pct, "left")
    };
    let percentage_suffix = format!(" {}% {}", display_pct, display_word);
    let full_suffix = match reset_time {
        Some(reset) if left_pct == 0 => format!(" resets {}", reset),
        Some(reset) => format!("{} · {}", percentage_suffix, reset),
        None => percentage_suffix.clone(),
    };
    // On narrow widgets keep the percentage wording unambiguous, dropping the
    // reset countdown before sacrificing the bar. Exhausted wording is unchanged.
    let suffix = match reset_time {
        Some(_) if left_pct > 0 => {
            let budget = usize::from(width).saturating_sub(LABEL_WIDTH + MIN_BAR_WIDTH);
            if UnicodeWidthStr::width(full_suffix.as_str()) <= budget {
                full_suffix
            } else {
                percentage_suffix
            }
        }
        _ => full_suffix,
    };
    let suffix_width = UnicodeWidthStr::width(suffix.as_str());
    let label_width = LABEL_WIDTH.min(usize::from(width).saturating_sub(suffix_width));
    let bar_width = usize::from(width)
        .saturating_sub(label_width + suffix_width)
        .min(12);

    let filled = ((used_pct as f32 / 100.0) * bar_width as f32).round() as usize;
    let empty = bar_width.saturating_sub(filled);

    let bar_filled = "▰".repeat(filled);
    let bar_empty = "▱".repeat(empty);

    let visible_label: String = label.chars().take(label_width).collect();
    let padded_label = format!("{visible_label:<label_width$}");

    Line::from(vec![
        Span::styled(padded_label, Style::default().fg(rgb(140, 140, 150))),
        Span::styled(bar_filled, Style::default().fg(color)),
        Span::styled(bar_empty, Style::default().fg(rgb(50, 50, 60))),
        Span::styled(suffix, Style::default().fg(color)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn usage_bar_shows_reset_countdown_before_exhaustion() {
        let text = line_text(&render_labeled_bar(
            "5-hour",
            38,
            62,
            Some("4h 5m"),
            40,
            false,
        ));

        assert!(text.contains("62% left · 4h 5m"));
        assert!(UnicodeWidthStr::width(text.as_str()) <= 40);
    }

    #[test]
    fn usage_bar_keeps_wording_unambiguous_within_narrow_width() {
        let text = line_text(&render_labeled_bar(
            "Weekly",
            19,
            81,
            Some("1d 4h"),
            23,
            true,
        ));

        assert!(text.contains("19% used"));
        assert!(!text.contains("1d 4h"));
        assert!(UnicodeWidthStr::width(text.as_str()) <= 23);
        assert!(text.contains('▰') || text.contains('▱'));
    }

    #[test]
    fn used_wording_does_not_change_remaining_budget_color_thresholds() {
        let left = render_labeled_bar("5-hour", 85, 15, None, 24, false);
        let used = render_labeled_bar("5-hour", 85, 15, None, 24, true);

        assert!(line_text(&left).contains("15% left"));
        assert!(line_text(&used).contains("85% used"));
        assert_eq!(left.spans[1].style.fg, Some(rgb(255, 100, 100)));
        assert_eq!(used.spans[1].style.fg, left.spans[1].style.fg);
    }

    #[test]
    fn exhausted_usage_bar_preserves_resets_wording_and_width() {
        let text = line_text(&render_labeled_bar("5-hour", 100, 0, Some("12m"), 24, true));

        assert!(text.contains("resets 12m"));
        assert!(!text.contains("0% left"));
        assert!(!text.contains("100% used"));
        assert!(UnicodeWidthStr::width(text.as_str()) <= 24);
    }

    #[test]
    fn openai_monthly_usage_renders_only_the_reported_window() {
        let info = UsageInfo {
            provider: UsageProvider::OpenAI,
            primary_limit_label: Some("Monthly".to_string()),
            five_hour: 1.0,
            available: true,
            ..Default::default()
        };

        let lines = render_usage_compact(&info, 40, false);
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("Monthly"));
        assert!(!text.contains("5-hour"));
        assert!(!text.contains("Weekly"));
        assert_eq!(lines.len(), 2); // Provider label plus one quota bar.
    }
}

pub(super) fn render_usage_pill(
    used_tokens: usize,
    limit_tokens: usize,
    width: u16,
) -> Line<'static> {
    let safe_limit = limit_tokens.max(1);
    let bar_width = (width as usize).min(24);
    if bar_width == 0 {
        return Line::default();
    }

    let mut used_cells = ((used_tokens as f64 / safe_limit as f64) * bar_width as f64)
        .round()
        .max(0.0) as usize;
    if used_cells > bar_width {
        used_cells = bar_width;
    }

    let used_pct = ((used_tokens as f64 / safe_limit as f64) * 100.0)
        .round()
        .clamp(0.0, 100.0) as u8;
    let left_pct = 100u8.saturating_sub(used_pct);
    let used_color = if left_pct <= 20 {
        rgb(255, 100, 100)
    } else if left_pct <= 50 {
        rgb(255, 200, 100)
    } else {
        rgb(100, 200, 100)
    };

    let empty_cells = bar_width.saturating_sub(used_cells);
    let mut spans = Vec::new();
    spans.push(Span::styled(
        "▰".repeat(used_cells),
        Style::default().fg(used_color),
    ));
    if empty_cells > 0 {
        spans.push(Span::styled(
            "▱".repeat(empty_cells),
            Style::default().fg(rgb(50, 50, 60)),
        ));
    }
    Line::from(spans)
}

pub(super) fn render_context_usage_line(
    label: &str,
    used_tokens: usize,
    limit_tokens: usize,
    width: u16,
) -> Line<'static> {
    let tokens = format!(
        "{}/{}",
        format_token_k(used_tokens),
        format_token_k(limit_tokens)
    );
    let used_pct = ((used_tokens as f64 / limit_tokens.max(1) as f64) * 100.0)
        .round()
        .clamp(0.0, 100.0) as u8;
    let left_pct = 100u8.saturating_sub(used_pct);
    let token_color = if left_pct <= 20 {
        rgb(255, 100, 100)
    } else if left_pct <= 50 {
        rgb(255, 200, 100)
    } else {
        rgb(100, 200, 100)
    };

    let label_width = UnicodeWidthStr::width(label);
    let tokens_width = UnicodeWidthStr::width(tokens.as_str());
    // label + space + tokens + space + bar
    let bar_width = width.saturating_sub((label_width + 1 + tokens_width + 1) as u16);

    let mut spans = vec![
        Span::styled(format!("{label} "), Style::default().fg(rgb(140, 140, 150))),
        Span::styled(
            format!("{tokens} "),
            Style::default().fg(token_color).bold(),
        ),
    ];

    if bar_width >= 3 {
        spans.extend(render_usage_pill(used_tokens, limit_tokens, bar_width).spans);
    }
    Line::from(spans)
}

fn format_token_k(tokens: usize) -> String {
    if tokens >= 1000 {
        format!("{}k", tokens / 1000)
    } else {
        format!("{}", tokens)
    }
}

fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        format!("{}", tokens)
    }
}
