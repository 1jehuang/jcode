use super::{InfoWidgetData, UsageInfo, UsageProvider};
use crate::tui::color_support::rgb;
use ratatui::prelude::*;
use unicode_width::UnicodeWidthStr;

const HOUR_SECONDS: u64 = 60 * 60;
const DAY_SECONDS: u64 = 24 * HOUR_SECONDS;

/// Length of a quota window, inferred from the label the provider reported.
///
/// Anthropic uses fixed `5-hour`/`Weekly` labels; OpenAI derives labels from
/// the window length it reports (`5-hour`, `7-day`, `Monthly`, ...). Anything
/// unrecognised (notably Codex `Spark`) yields `None`, and the caller keeps the
/// reset countdown instead of an elapsed percentage.
fn window_seconds_for_label(label: &str) -> Option<u64> {
    let normalized = label.trim().to_ascii_lowercase();
    let normalized = normalized.trim_end_matches(" window").trim();
    match normalized {
        "weekly" => return Some(7 * DAY_SECONDS),
        "daily" => return Some(DAY_SECONDS),
        "monthly" => return Some(30 * DAY_SECONDS),
        _ => {}
    }
    if let Some(hours) = normalized.strip_suffix("-hour") {
        return hours.parse::<u64>().ok().map(|h| h * HOUR_SECONDS);
    }
    if let Some(days) = normalized.strip_suffix("-day") {
        return days.parse::<u64>().ok().map(|d| d * DAY_SECONDS);
    }
    None
}

/// The trailing detail rendered after the quota percentage.
#[derive(Debug, Clone, PartialEq, Eq)]
enum WindowDetail {
    /// How much of the rolling window has already elapsed, in percent. Pairs
    /// with the consumed percentage so the two can be read against each other.
    Elapsed(u8),
    /// Human-readable countdown to the window reset (the default).
    Countdown(String),
}

/// Pick the trailing detail for a quota bar: the window's elapsed percentage
/// when the user asked for it and the window length is known, else a countdown.
fn window_detail(
    label: &str,
    resets_at: Option<&str>,
    prefer_elapsed: bool,
) -> Option<WindowDetail> {
    let resets_at = resets_at?;
    if prefer_elapsed
        && let Some(window_seconds) = window_seconds_for_label(label)
        && let Some(elapsed) = crate::usage::window_elapsed_percent(resets_at, window_seconds)
    {
        return Some(WindowDetail::Elapsed(elapsed));
    }
    Some(WindowDetail::Countdown(crate::usage::format_reset_time(
        resets_at,
    )))
}

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
                let detail = window_detail(
                    primary_label,
                    info.five_hour_resets_at.as_deref(),
                    data.usage_display_elapsed,
                );
                lines.push(render_labeled_bar(
                    primary_label,
                    five_hr_used,
                    five_hr_left,
                    detail.as_ref(),
                    inner.width,
                    data.usage_display_used,
                ));
            }
            if let Some(secondary_label) = info.secondary_limit_label.as_deref() {
                let detail = window_detail(
                    secondary_label,
                    info.seven_day_resets_at.as_deref(),
                    data.usage_display_elapsed,
                );
                lines.push(render_labeled_bar(
                    secondary_label,
                    seven_day_used,
                    seven_day_left,
                    detail.as_ref(),
                    inner.width,
                    data.usage_display_used,
                ));
            }
            if let Some(spark_usage) = info.spark {
                let spark_used = (spark_usage * 100.0).round().clamp(0.0, 100.0) as u8;
                let spark_left = 100u8.saturating_sub(spark_used);
                let spark_reset = window_detail(
                    "Spark",
                    info.spark_resets_at.as_deref(),
                    data.usage_display_elapsed,
                );
                lines.push(render_labeled_bar(
                    "Spark",
                    spark_used,
                    spark_left,
                    spark_reset.as_ref(),
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
    usage_display_elapsed: bool,
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
        let detail = window_detail(
            primary_label,
            info.five_hour_resets_at.as_deref(),
            usage_display_elapsed,
        );
        lines.push(render_labeled_bar(
            primary_label,
            five_hr_used,
            five_hr_left,
            detail.as_ref(),
            width,
            usage_display_used,
        ));
    }
    if let Some(secondary_label) = info.secondary_limit_label.as_deref() {
        let detail = window_detail(
            secondary_label,
            info.seven_day_resets_at.as_deref(),
            usage_display_elapsed,
        );
        lines.push(render_labeled_bar(
            secondary_label,
            seven_day_used,
            seven_day_left,
            detail.as_ref(),
            width,
            usage_display_used,
        ));
    }
    if let Some(spark_usage) = info.spark {
        let spark_used = (spark_usage * 100.0).round().clamp(0.0, 100.0) as u8;
        let spark_left = 100u8.saturating_sub(spark_used);
        let spark_reset = window_detail(
            "Spark",
            info.spark_resets_at.as_deref(),
            usage_display_elapsed,
        );
        lines.push(render_labeled_bar(
            "Spark",
            spark_used,
            spark_left,
            spark_reset.as_ref(),
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
    detail: Option<&WindowDetail>,
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
    // In elapsed mode the two percentages are the point of the widget, so they
    // stay glued together (`43%/91%`) and survive every width; the wording is
    // what gets dropped first. Countdown mode keeps its existing behaviour.
    let (percentage_suffix, full_suffix) = match detail {
        Some(WindowDetail::Elapsed(elapsed_pct)) => {
            let compact = format!(" {}%/{}%", display_pct, elapsed_pct);
            let full = format!("{} {} / elapsed", compact, display_word);
            (compact, full)
        }
        Some(WindowDetail::Countdown(reset)) => {
            let compact = format!(" {}% {}", display_pct, display_word);
            let full = if left_pct == 0 {
                format!(" resets {}", reset)
            } else {
                format!("{} · {}", compact, reset)
            };
            (compact, full)
        }
        None => {
            let compact = format!(" {}% {}", display_pct, display_word);
            (compact.clone(), compact)
        }
    };
    // On narrow widgets keep the percentage wording unambiguous, dropping the
    // trailing detail before sacrificing the bar. Exhausted wording is unchanged.
    let keep_full_suffix = !matches!(detail, Some(WindowDetail::Countdown(_)) if left_pct == 0);
    let suffix = if detail.is_some() && keep_full_suffix {
        let budget = usize::from(width).saturating_sub(LABEL_WIDTH + MIN_BAR_WIDTH);
        if UnicodeWidthStr::width(full_suffix.as_str()) <= budget {
            full_suffix
        } else {
            percentage_suffix
        }
    } else {
        full_suffix
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

    let mut spans = vec![
        Span::styled(padded_label, Style::default().fg(rgb(140, 140, 150))),
        Span::styled(bar_filled, Style::default().fg(color)),
        Span::styled(bar_empty, Style::default().fg(rgb(50, 50, 60))),
    ];
    spans.extend(suffix_spans(&suffix, detail, color));
    Line::from(spans)
}

/// Style the trailing detail. In elapsed mode the pair is split at the slash so
/// the consumed percentage keeps the quota color while the elapsed percentage
/// renders in a neutral blue (the palette's info tone): it measures time, not
/// quota health, so it must not borrow the traffic-light coloring.
fn suffix_spans(
    suffix: &str,
    detail: Option<&WindowDetail>,
    quota_color: Color,
) -> Vec<Span<'static>> {
    if matches!(detail, Some(WindowDetail::Elapsed(_)))
        && let Some(split) = suffix.find('/')
    {
        let (used_part, elapsed_part) = suffix.split_at(split);
        return vec![
            Span::styled(used_part.to_owned(), Style::default().fg(quota_color)),
            Span::styled(
                elapsed_part.to_owned(),
                Style::default().fg(rgb(140, 180, 255)),
            ),
        ];
    }
    vec![Span::styled(
        suffix.to_owned(),
        Style::default().fg(quota_color),
    )]
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
            Some(&WindowDetail::Countdown("4h 5m".into())),
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
            Some(&WindowDetail::Countdown("1d 4h".into())),
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
        let text = line_text(&render_labeled_bar(
            "5-hour",
            100,
            0,
            Some(&WindowDetail::Countdown("12m".into())),
            24,
            true,
        ));

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

        let lines = render_usage_compact(&info, 40, false, false);
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("Monthly"));
        assert!(!text.contains("5-hour"));
        assert!(!text.contains("Weekly"));
        assert_eq!(lines.len(), 2); // Provider label plus one quota bar.
    }

    #[test]
    fn elapsed_mode_pairs_consumed_quota_against_window_progress() {
        let text = line_text(&render_labeled_bar(
            "5-hour",
            43,
            57,
            Some(&WindowDetail::Elapsed(91)),
            40,
            true,
        ));

        // The pair is the point: 43% of quota spent against 91% of the window.
        assert!(text.contains("43%/91%"), "got {text}");
        assert!(text.contains("used / elapsed"), "got {text}");
        assert!(!text.contains("left"));
        assert!(UnicodeWidthStr::width(text.as_str()) <= 40);
    }

    #[test]
    fn elapsed_pair_survives_narrow_widths_by_dropping_only_the_wording() {
        let text = line_text(&render_labeled_bar(
            "Weekly",
            19,
            81,
            Some(&WindowDetail::Elapsed(81)),
            24,
            true,
        ));

        assert!(text.contains("19%/81%"), "got {text}");
        assert!(!text.contains("elapsed"));
        assert!(text.contains('▰') || text.contains('▱'));
        assert!(UnicodeWidthStr::width(text.as_str()) <= 24);
    }

    #[test]
    fn elapsed_mode_keeps_the_bar_color_keyed_to_remaining_quota() {
        let healthy =
            render_labeled_bar("5-hour", 20, 80, Some(&WindowDetail::Elapsed(95)), 40, true);
        let critical =
            render_labeled_bar("5-hour", 90, 10, Some(&WindowDetail::Elapsed(20)), 40, true);

        assert_eq!(healthy.spans[1].style.fg, Some(rgb(100, 200, 100)));
        assert_eq!(critical.spans[1].style.fg, Some(rgb(255, 100, 100)));
    }

    #[test]
    fn elapsed_pair_colors_the_two_percentages_differently() {
        let line = render_labeled_bar("5-hour", 43, 57, Some(&WindowDetail::Elapsed(91)), 40, true);

        let used_span = line
            .spans
            .iter()
            .find(|span| span.content.ends_with("43%"))
            .expect("consumed percentage span");
        let elapsed_span = line
            .spans
            .iter()
            .find(|span| span.content.starts_with("/91%"))
            .expect("elapsed percentage span");

        // Quota health colors the consumed figure; the elapsed figure measures
        // time and reads in its own neutral tone.
        assert_eq!(used_span.style.fg, Some(rgb(100, 200, 100)));
        assert_eq!(elapsed_span.style.fg, Some(rgb(140, 180, 255)));

        let critical =
            render_labeled_bar("5-hour", 90, 10, Some(&WindowDetail::Elapsed(20)), 40, true);
        let critical_used = critical
            .spans
            .iter()
            .find(|span| span.content.ends_with("90%"))
            .expect("consumed percentage span");
        assert_eq!(critical_used.style.fg, Some(rgb(255, 100, 100)));
    }

    #[test]
    fn elapsed_pair_splits_into_two_colored_spans_even_when_narrow() {
        let line = render_labeled_bar("Weekly", 19, 81, Some(&WindowDetail::Elapsed(81)), 24, true);

        let used_span = line
            .spans
            .iter()
            .find(|span| span.content.ends_with("19%"))
            .expect("consumed percentage span");
        let elapsed_span = line
            .spans
            .iter()
            .find(|span| span.content.starts_with("/81%"))
            .expect("elapsed percentage span");
        assert_eq!(used_span.style.fg, Some(rgb(100, 200, 100)));
        assert_eq!(elapsed_span.style.fg, Some(rgb(140, 180, 255)));
    }

    #[test]
    fn countdown_mode_keeps_a_single_suffix_span() {
        let line = render_labeled_bar(
            "5-hour",
            38,
            62,
            Some(&WindowDetail::Countdown("4h 5m".into())),
            40,
            false,
        );

        let suffix_spans = &line.spans[3..];
        assert_eq!(suffix_spans.len(), 1);
        assert!(suffix_spans[0].content.contains("62% left · 4h 5m"));
        assert_eq!(suffix_spans[0].style.fg, Some(rgb(100, 200, 100)));
    }

    #[test]
    fn window_lengths_are_inferred_from_provider_labels() {
        assert_eq!(window_seconds_for_label("5-hour"), Some(5 * HOUR_SECONDS));
        assert_eq!(
            window_seconds_for_label("5-hour window"),
            Some(5 * HOUR_SECONDS)
        );
        assert_eq!(window_seconds_for_label("Weekly"), Some(7 * DAY_SECONDS));
        assert_eq!(window_seconds_for_label("7-day"), Some(7 * DAY_SECONDS));
        assert_eq!(window_seconds_for_label("Monthly"), Some(30 * DAY_SECONDS));
        // Codex Spark has no advertised window length, so it keeps a countdown.
        assert_eq!(window_seconds_for_label("Spark"), None);
    }

    #[test]
    fn unknown_window_length_falls_back_to_the_countdown() {
        let resets_at = (chrono::Utc::now() + chrono::Duration::minutes(30)).to_rfc3339();

        let spark = window_detail("Spark", Some(&resets_at), true);
        assert!(matches!(spark, Some(WindowDetail::Countdown(_))));

        let known = window_detail("5-hour", Some(&resets_at), true);
        assert!(matches!(known, Some(WindowDetail::Elapsed(_))));

        // Opting out keeps the countdown even for a known window.
        let opted_out = window_detail("5-hour", Some(&resets_at), false);
        assert!(matches!(opted_out, Some(WindowDetail::Countdown(_))));

        assert!(window_detail("5-hour", None, true).is_none());
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
