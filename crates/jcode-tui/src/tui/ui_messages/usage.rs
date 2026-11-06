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
    if let Some(card) = ProviderCard::parse(&msg.content) {
        return card.render(msg.title.as_deref().unwrap_or("Usage"), width);
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

/// Widest a provider card grows. Cards share one width so a stack of them
/// lines up instead of ending in a ragged right edge.
const CARD_MAX_WIDTH: usize = 64;
const LABEL_STYLE: (u8, u8, u8) = (140, 140, 150);
const VALUE_STYLE: (u8, u8, u8) = (215, 215, 225);
const ACCENT_STYLE: (u8, u8, u8) = (140, 180, 255);

/// A per-provider `/usage` card. `App::format_usage_provider_card` writes the
/// row protocol: a `*level\tstatus` header, then `= label\tpct\treset` gauges,
/// `: key\tvalue` details (`+` for a highlighted value), `! alert`, `- note`,
/// and blank spacer rows.
struct ProviderCard<'a> {
    status: CardStatus<'a>,
    rows: Vec<CardRow<'a>>,
}

enum CardStatus<'a> {
    /// Peak utilization across the provider's windows, in percent used.
    Peak(u8),
    Level(&'a str, &'a str),
}

enum CardRow<'a> {
    Gauge {
        label: &'a str,
        used: u8,
        reset: Option<&'a str>,
    },
    Detail {
        key: &'a str,
        value: &'a str,
        positive: bool,
    },
    Alert(&'a str),
    Note(&'a str),
    Spacer,
}

fn parse_card_percent(text: &str) -> Option<f32> {
    match text.parse::<f32>() {
        Ok(value) => Some(value),
        Err(error) => {
            crate::logging::info(&format!(
                "Invalid usage card percentage, rendering as text: {error}"
            ));
            None
        }
    }
}

#[cfg(test)]
#[path = "usage_activity_pair_tests.rs"]
mod activity_pair_tests;

#[cfg(test)]
mod card_parse_error_tests {
    use super::*;

    #[test]
    fn activity_table_aligns_metrics_and_preserves_caveats() {
        let lifetime = "10.923 Bil in · 30.130 Mil out · 10.562 Bil cached · $5296.96 est. + unknown cost (3,817 unpriced) · partial tokens · since 2026-09-06";
        let lines = activity_summary_lines("No recorded usage", lifetime, 58);
        let text = lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Jcode usage"));
        assert!(text.contains("Today"));
        assert!(text.contains("Lifetime"));
        for expected in [
            "10.92B",
            "10.56B",
            "30.13M",
            "$5,296.96*",
            "3,817 unpriced",
            "partial tokens",
            "since 2026-09-06",
        ] {
            assert!(text.contains(expected), "missing {expected}: {text}");
        }
        assert_eq!(lines[2].spans[1].style.fg, usage_utilization_style(0.0).fg);
        for width in 1..65 {
            let card = ProviderCard::parse(&format!("*ok\tready\n: Today\tNo recorded usage\n: Lifetime\t{lifetime}\n: Tracking\told caveat")).unwrap().render("OpenAI", width);
            assert!(
                card.iter().all(|line| line.width() <= width),
                "width {width}"
            );
        }
    }

    #[test]
    fn narrow_activity_stacks_periods_and_keeps_cache_creation() {
        let lines = activity_summary_lines(
            "No recorded usage",
            "100 in · 20 out · 40 cache creation · cost unknown",
            24,
        );
        let text = lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("No recorded usage"));
        assert!(text.contains("Cache creation"));
        assert!(text.contains("Unknown*"));
        assert!(lines.iter().all(|line| line.width() <= 24));
    }

    #[test]
    fn invalid_header_falls_back_and_invalid_gauge_remains_visible() {
        assert!(ProviderCard::parse("*%\tnot-a-number").is_none());
        let card = ProviderCard::parse("*ok\tReady\n= Window\tbad\tsoon").unwrap();
        assert!(matches!(card.rows[0], CardRow::Note("Window\tbad\tsoon")));
        let card = ProviderCard::parse("*%\t42.4\n= Window\t101%\tsoon").unwrap();
        assert!(matches!(card.status, CardStatus::Peak(42)));
        assert!(matches!(card.rows[0], CardRow::Gauge { used: 100, .. }));
    }
}

impl<'a> ProviderCard<'a> {
    fn parse(content: &'a str) -> Option<Self> {
        let mut lines = content.lines();
        let (level, text) = lines.next()?.strip_prefix('*')?.split_once('\t')?;
        let status = if level == "%" {
            CardStatus::Peak(parse_card_percent(text)?.round().clamp(0.0, 100.0) as u8)
        } else {
            CardStatus::Level(level, text)
        };
        let rows = lines
            .map(|raw| {
                let detail = |rest: &'a str, positive| match rest.split_once('\t') {
                    Some((key, value)) => CardRow::Detail {
                        key,
                        value,
                        positive,
                    },
                    None => CardRow::Note(rest),
                };
                if raw.trim().is_empty() {
                    CardRow::Spacer
                } else if let Some(rest) = raw.strip_prefix("= ") {
                    let mut parts = rest.splitn(3, '\t');
                    let Some(label) = parts.next() else {
                        return CardRow::Note(rest);
                    };
                    match parts
                        .next()
                        .and_then(|pct| parse_card_percent(pct.trim_end_matches('%')))
                    {
                        Some(pct) => CardRow::Gauge {
                            label,
                            used: pct.round().clamp(0.0, 100.0) as u8,
                            reset: parts.next().filter(|reset| !reset.is_empty()),
                        },
                        None => CardRow::Note(rest),
                    }
                } else if let Some(rest) = raw.strip_prefix(": ") {
                    detail(rest, false)
                } else if let Some(rest) = raw.strip_prefix("+ ") {
                    detail(rest, true)
                } else if let Some(rest) = raw.strip_prefix("! ") {
                    CardRow::Alert(rest)
                } else {
                    CardRow::Note(raw.strip_prefix("- ").unwrap_or(raw))
                }
            })
            .collect();
        Some(Self { status, rows })
    }

    fn status_spans(&self, display_used: bool) -> Vec<Span<'static>> {
        use crate::tui::info_widget::usage_limit_color;
        let (color, text) = match self.status {
            CardStatus::Peak(used) => (
                usage_limit_color(100 - used),
                if display_used {
                    format!("{used}% used")
                } else {
                    format!("{}% left", 100 - used)
                },
            ),
            CardStatus::Level(level, text) => (
                match level {
                    "ok" => usage_limit_color(100),
                    "warn" => usage_limit_color(50),
                    "crit" => usage_limit_color(0),
                    "busy" => rgb(ACCENT_STYLE.0, ACCENT_STYLE.1, ACCENT_STYLE.2),
                    _ => dim_color(),
                },
                text.to_owned(),
            ),
        };
        let dot = if matches!(self.status, CardStatus::Level("busy", _)) {
            "◌ "
        } else {
            "● "
        };
        vec![
            Span::styled(dot, Style::default().fg(color)),
            Span::styled(text, Style::default().fg(color).bold()),
        ]
    }

    fn render(&self, title: &str, width: usize) -> Vec<Line<'static>> {
        let display_used = crate::config::config().display.usage_display_used();
        let box_width = if width >= 14 { width - 4 } else { width }.min(CARD_MAX_WIDTH);
        let status = self.status_spans(display_used);
        if box_width < 18 {
            // Too narrow for a frame: plain rows, each clipped to the width.
            let mut lines = vec![Line::from(card_title_spans(title)), Line::from(status)];
            lines.extend(self.body_lines(width, display_used));
            return lines
                .iter()
                .map(|line| super::truncate_line_with_ellipsis_to_width(line, width))
                .collect();
        }
        let border = Style::default().fg(rgb(120, 140, 190));
        let inner = box_width - 6;
        let (top, status_in_border) = card_top_border(title, status.clone(), box_width, border);
        let mut body = Vec::new();
        if !status_in_border {
            // No room beside the title: keep the status as the first row
            // rather than dropping it.
            body.push(Line::from(status));
        }
        body.extend(self.body_lines(inner, display_used));
        let mut lines = vec![top];
        for line in body {
            let line = super::truncate_line_with_ellipsis_to_width(&line, inner);
            let pad = inner.saturating_sub(line.width());
            let mut spans = vec![Span::styled("│  ", border)];
            spans.extend(line.spans);
            spans.push(Span::raw(" ".repeat(pad)));
            spans.push(Span::styled("  │", border));
            lines.push(Line::from(spans));
        }
        lines.push(Line::from(Span::styled(
            format!("╰{}╯", "─".repeat(box_width - 2)),
            border,
        )));
        lines
    }

    fn body_lines(&self, inner: usize, display_used: bool) -> Vec<Line<'static>> {
        use crate::tui::info_widget::LimitBar;
        const MIN_BAR: usize = 6;
        const PERCENT: usize = 10; // " 100% left"
        let label_style = Style::default().fg(rgb(LABEL_STYLE.0, LABEL_STYLE.1, LABEL_STYLE.2));
        let value_style = Style::default().fg(rgb(VALUE_STYLE.0, VALUE_STYLE.1, VALUE_STYLE.2));
        let countdown_style = Style::default().fg(rgb(110, 110, 125));
        let countdown = |reset: &str| format!("↻ {reset}");
        let widest = |f: &dyn Fn(&CardRow<'a>) -> Option<usize>| {
            self.rows.iter().filter_map(f).max().unwrap_or(0)
        };
        let gauge_label = widest(&|row| match row {
            CardRow::Gauge { label, .. } => Some(label.width()),
            _ => None,
        });
        let detail_key = widest(&|row| match row {
            CardRow::Detail { key, .. } => Some(key.width()),
            _ => None,
        });
        let countdown_width = widest(&|row| match row {
            CardRow::Gauge {
                reset: Some(reset), ..
            } => Some(2 + countdown(reset).width()),
            _ => None,
        });
        // Gauges sit on one row (label, bar, percent, countdown) when they all
        // fit. Otherwise each label moves above its bar with the countdown
        // right-aligned beside it, so nothing is truncated or dropped.
        let inline = gauge_label + 2 + MIN_BAR + PERCENT + countdown_width <= inner;
        // Details align with the gauge labels when gauges are inline.
        let column = if inline {
            gauge_label.max(detail_key)
        } else {
            detail_key
        };
        // Details stay beside their keys unless any value would have to split
        // a word, then every detail stacks under its key so the card reads
        // consistently.
        let value_width = inner.saturating_sub(column + 2);
        let details_inline = self.rows.iter().all(|row| match row {
            CardRow::Detail { value, .. } => {
                value_width >= 1
                    && value.split_whitespace().map(str::width).max().unwrap_or(0) <= value_width
            }
            _ => true,
        });
        let mut lines: Vec<Line<'static>> = Vec::new();
        let activity = |name: &str| {
            self.rows.iter().find_map(|row| match row {
                CardRow::Detail { key, value, .. } if *key == name => Some(*value),
                _ => None,
            })
        };
        let activity_pair = activity("Today").zip(activity("Lifetime"));
        let mut activity_rendered = false;
        for row in &self.rows {
            if let Some((today, lifetime)) = activity_pair
                && matches!(
                    row,
                    CardRow::Detail {
                        key: "Today" | "Lifetime" | "Tracking",
                        ..
                    }
                )
            {
                if !activity_rendered {
                    lines.extend(activity_summary_lines(today, lifetime, inner));
                    activity_rendered = true;
                }
                continue;
            }
            match *row {
                CardRow::Gauge { label, used, reset } => {
                    let bar = |label: &str, label_column, countdown_column| {
                        LimitBar {
                            label,
                            label_column,
                            used_pct: used,
                            left_pct: 100 - used,
                            reset_time: reset,
                            width: u16::try_from(inner).unwrap_or(u16::MAX),
                            display_used,
                            columns: Some(countdown_column),
                        }
                        .render()
                    };
                    if inline {
                        lines.push(bar(label, column + 2, countdown_width));
                    } else {
                        // Label above the bar, wrapped rather than clipped, with
                        // the countdown right-aligned on the label's last line
                        // (or its own line when the card is very narrow).
                        let tail = match reset {
                            Some(reset) => countdown(reset),
                            None => String::new(),
                        };
                        let room = inner.saturating_sub(tail.width() + 2);
                        let (parts, tail_inline) = if tail.is_empty() {
                            (wrap_words(label, inner), true)
                        } else if room >= 6 {
                            (wrap_words(label, room), true)
                        } else {
                            (wrap_words(label, inner), false)
                        };
                        let last = parts.len().saturating_sub(1);
                        for (index, part) in parts.into_iter().enumerate() {
                            let gap = inner.saturating_sub(part.width() + tail.width());
                            let mut spans = vec![Span::styled(part, label_style)];
                            if index == last && tail_inline && !tail.is_empty() {
                                spans.push(Span::raw(" ".repeat(gap)));
                                spans.push(Span::styled(tail.clone(), countdown_style));
                            }
                            lines.push(Line::from(spans));
                        }
                        if !tail_inline {
                            let pad = inner.saturating_sub(tail.width());
                            lines.push(Line::from(vec![
                                Span::raw(" ".repeat(pad)),
                                Span::styled(tail, countdown_style),
                            ]));
                        }
                        lines.push(bar("", 0, 0));
                    }
                }
                CardRow::Detail {
                    key,
                    value,
                    positive,
                } => {
                    let style = if positive {
                        usage_utilization_style(0.0).bold()
                    } else {
                        value_style
                    };
                    if details_inline {
                        let key = clip_to_width(key, column);
                        let key_cell = format!("{key}{}", " ".repeat(column + 2 - key.width()));
                        for (index, part) in wrap_words(value, value_width).into_iter().enumerate()
                        {
                            let lead = if index == 0 {
                                key_cell.clone()
                            } else {
                                " ".repeat(column + 2)
                            };
                            lines.push(Line::from(vec![
                                Span::styled(lead, label_style),
                                Span::styled(part, style),
                            ]));
                        }
                    } else {
                        for part in wrap_words(key, inner) {
                            lines.push(Line::from(Span::styled(part, label_style)));
                        }
                        for part in wrap_words(value, inner.saturating_sub(2).max(1)) {
                            lines
                                .push(Line::from(vec![Span::raw("  "), Span::styled(part, style)]));
                        }
                    }
                }
                CardRow::Alert(text) => {
                    let style = Style::default().fg(crate::tui::info_widget::usage_limit_color(0));
                    for (index, part) in wrap_words(text, inner.saturating_sub(2).max(1))
                        .into_iter()
                        .enumerate()
                    {
                        let lead = if index == 0 { "▲ " } else { "  " };
                        lines.push(Line::from(vec![
                            Span::styled(lead, style),
                            Span::styled(part, style),
                        ]));
                    }
                }
                CardRow::Note(text) => {
                    for part in wrap_words(text, inner.max(1)) {
                        lines.push(Line::from(note_spans(&part)));
                    }
                }
                CardRow::Spacer => {
                    if lines.last().is_some_and(|line| line.width() > 0) {
                        lines.push(Line::default());
                    }
                }
            }
        }
        while lines.last().is_some_and(|line| line.width() == 0) {
            lines.pop();
        }
        lines
    }
}

/// Adapt the recorded-usage summary without changing its plain-text consumers.
fn activity_summary_lines(today: &str, lifetime: &str, width: usize) -> Vec<Line<'static>> {
    let label = Style::default().fg(rgb(LABEL_STYLE.0, LABEL_STYLE.1, LABEL_STYLE.2));
    let value = Style::default().fg(rgb(VALUE_STYLE.0, VALUE_STYLE.1, VALUE_STYLE.2));
    let cached = usage_utilization_style(0.0);
    let parse = |text: &str| {
        let mut cells = vec!["—".to_string(); 5];
        let mut notes = Vec::new();
        for part in text.split(" · ") {
            let field = [
                (" in", 0),
                (" cached", 1),
                (" out", 2),
                (" cache creation", 4),
            ]
            .into_iter()
            .find_map(|(suffix, index)| part.strip_suffix(suffix).map(|s| (index, s)));
            if let Some((index, number)) = field {
                let compact = [
                    ("Quint", "E"),
                    ("Quad", "P"),
                    ("Tril", "T"),
                    ("Bil", "B"),
                    ("Mil", "M"),
                ]
                .into_iter()
                .find_map(|(suffix, unit)| {
                    number.strip_suffix(suffix).map(|n| {
                        n.trim()
                            .parse::<f64>()
                            .map(|n| format!("{n:.2}{unit}"))
                            .unwrap_or_else(|_| number.to_string())
                    })
                })
                .unwrap_or_else(|| number.to_string());
                cells[index] = compact;
            } else if part.contains("cost") || part.contains(" est.") {
                let (estimate, rest) = part.split_once(" est.").unwrap_or(("Unknown", part));
                let estimate = estimate
                    .strip_prefix('$')
                    .and_then(|s| s.split_once('.'))
                    .filter(|(whole, _)| whole.bytes().all(|b| b.is_ascii_digit()))
                    .map(|(whole, cents)| {
                        let mut grouped = String::from("$");
                        for (index, ch) in whole.chars().enumerate() {
                            if index > 0 && (whole.len() - index) % 3 == 0 {
                                grouped.push(',');
                            }
                            grouped.push(ch);
                        }
                        format!("{grouped}.{cents}")
                    })
                    .unwrap_or_else(|| estimate.to_string());
                cells[3] = if rest.is_empty() {
                    estimate
                } else {
                    format!("{estimate}*")
                };
                if !rest.is_empty() {
                    notes.push(rest.trim_start_matches(" + ").to_string());
                }
            } else if part != "No recorded usage" {
                notes.push(part.to_string());
            }
        }
        (cells, notes)
    };
    let (today_cells, today_notes) = parse(today);
    let (lifetime_cells, lifetime_notes) = parse(lifetime);
    let labels = [
        "Input",
        "Cached",
        "Output",
        "Est. API value",
        "Cache creation",
    ];
    let key_width = 14;
    let today_width = today_cells
        .iter()
        .map(|s| s.width())
        .max()
        .unwrap_or(0)
        .max(5);
    let lifetime_width = lifetime_cells
        .iter()
        .map(|s| s.width())
        .max()
        .unwrap_or(0)
        .max(8);
    let inline = key_width + 4 + today_width + lifetime_width <= width;
    let mut lines = vec![Line::from(Span::styled("Jcode usage", value.bold()))];
    if inline {
        lines[0] = Line::from(vec![
            Span::styled(format!("{:<key_width$}  ", "Jcode usage"), value.bold()),
            Span::styled(
                format!("{:>today_width$}  {:>lifetime_width$}", "Today", "Lifetime"),
                label,
            ),
        ]);
        for (index, name) in labels.iter().enumerate() {
            if index == 4 && today_cells[index] == "—" && lifetime_cells[index] == "—" {
                continue;
            }
            lines.push(Line::from(vec![
                Span::styled(format!("{name:<key_width$}  "), label),
                Span::styled(
                    format!(
                        "{:>today_width$}  {:>lifetime_width$}",
                        today_cells[index], lifetime_cells[index]
                    ),
                    if index == 1 { cached } else { value },
                ),
            ]));
        }
    } else {
        for (period, cells) in [("Today", &today_cells), ("Lifetime", &lifetime_cells)] {
            lines.push(Line::from(Span::styled(period, label.bold())));
            if cells.iter().all(|s| s == "—") {
                lines.extend(
                    wrap_words("No recorded usage", width)
                        .into_iter()
                        .map(|s| Line::from(Span::styled(s, label))),
                );
                continue;
            }
            for (index, name) in labels.iter().enumerate() {
                if index == 4 && cells[index] == "—" {
                    continue;
                }
                if name.width() + cells[index].width() + 2 <= width {
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!(
                                "{name}{}",
                                " ".repeat(width - name.width() - cells[index].width())
                            ),
                            label,
                        ),
                        Span::styled(
                            cells[index].clone(),
                            if index == 1 { cached } else { value },
                        ),
                    ]));
                } else {
                    lines.push(Line::from(Span::styled(clip_to_width(name, width), label)));
                    lines.push(Line::from(Span::styled(
                        clip_to_width(&cells[index], width),
                        value,
                    )));
                }
            }
        }
    }
    let mut notes = Vec::new();
    for (period, entries) in [("Today", today_notes), ("Lifetime", lifetime_notes)] {
        for note in entries {
            notes.push(if note.starts_with("since ") {
                note
            } else {
                format!("{period}: {note}")
            });
        }
    }
    notes.push("Jcode only · API-equivalent value, not a bill · today from local midnight".into());
    for note in notes {
        lines.extend(
            wrap_words(&note, width)
                .into_iter()
                .map(|s| Line::from(Span::styled(s, Style::default().fg(dim_color())))),
        );
    }
    lines
}

/// Provider name in bold, with a qualifier such as "(ChatGPT)" or "- work"
/// dimmed so the brand reads first.
fn card_title_spans(title: &str) -> Vec<Span<'static>> {
    let strong = Style::default().fg(Color::White).bold();
    let split = title
        .find(" (")
        .or_else(|| title.find(" - "))
        .or_else(|| title.find(" · "));
    match split {
        Some(index) if index > 0 => vec![
            Span::styled(title[..index].to_owned(), strong),
            Span::styled(title[index..].to_owned(), Style::default().fg(dim_color())),
        ],
        _ => vec![Span::styled(title.to_owned(), strong)],
    }
}

/// `╭─ Title ───────── ● status ─╮`. When the card is too narrow it drops the
/// title qualifier, then moves the status out of the border (the returned flag
/// is false) and clips the title, so the border never wraps.
fn card_top_border(
    title: &str,
    status: Vec<Span<'static>>,
    box_width: usize,
    border: Style,
) -> (Line<'static>, bool) {
    let status_width: usize = status.iter().map(|span| span.width()).sum();
    // Frame and padding take 8 cells, plus at least one cell of border rule.
    let fits = |line: &Line<'_>| box_width > 8 + line.width() + status_width;
    // Prefer dropping the dim qualifier ("(Claude)") over the status pill,
    // and the status over clipping the brand.
    let full = Line::from(card_title_spans(title));
    let brand = Line::from(
        card_title_spans(title)
            .into_iter()
            .take(1)
            .collect::<Vec<_>>(),
    );
    let (title_line, with_status) = if fits(&full) {
        (full, true)
    } else if fits(&brand) {
        (brand, true)
    } else {
        (
            super::truncate_line_with_ellipsis_to_width(&full, box_width.saturating_sub(6)),
            false,
        )
    };
    let mut spans = vec![Span::styled("╭─ ", border)];
    spans.extend(title_line.spans.iter().cloned());
    if with_status {
        let fill = box_width - 8 - title_line.width() - status_width;
        spans.push(Span::styled(format!(" {} ", "─".repeat(fill)), border));
        spans.extend(status);
        spans.push(Span::styled(" ─╮", border));
    } else {
        let fill = box_width.saturating_sub(5 + title_line.width()).max(1);
        spans.push(Span::styled(format!(" {}╮", "─".repeat(fill)), border));
    }
    (Line::from(spans), with_status)
}

/// Dim note text with `code` spans picked out in the accent color, so slash
/// commands in hints stand out.
fn note_spans(text: &str) -> Vec<Span<'static>> {
    text.split('`')
        .enumerate()
        .filter(|(_, part)| !part.is_empty())
        .map(|(index, part)| {
            let color = if index % 2 == 1 {
                rgb(ACCENT_STYLE.0, ACCENT_STYLE.1, ACCENT_STYLE.2)
            } else {
                dim_color()
            };
            Span::styled(part.to_owned(), Style::default().fg(color))
        })
        .collect()
}

fn clip_to_width(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let cells = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + cells + 1 > width {
            break;
        }
        out.push(ch);
        used += cells;
    }
    if width > 0 {
        out.push('…');
    }
    out
}

/// Greedy word wrap by display cells. Words wider than the line are split.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let gap = usize::from(!current.is_empty());
        if current.width() + gap + word.width() <= width {
            if gap == 1 {
                current.push(' ');
            }
            current.push_str(word);
            continue;
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        for ch in word.chars() {
            let cells = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if current.width() + cells > width && !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            current.push(ch);
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
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
