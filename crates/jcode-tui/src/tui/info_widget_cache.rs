//! Prompt-cache widget rendering and token labels.
use super::*;

pub(super) fn render_kv_cache_widget(data: &InfoWidgetData, inner: Rect) -> Framed {
    let Some(cache) = data.cache_hit_info.as_ref() else {
        return Framed::default();
    };
    // Top border: the headline yield. Body: the per-rate breakdown, then the
    // misses themselves. Bottom border: the miss total and overflow.
    let mut framed =
        Framed::body(vec![render_kv_cache_rates_line(cache)]).title(render_kv_cache_title(cache));

    if cache.miss_attributions.is_empty() {
        return framed.footer_right(Span::styled(
            "no misses",
            Style::default().fg(rgb(110, 210, 140)),
        ));
    }

    let total_missed: u64 = cache
        .miss_attributions
        .iter()
        .map(|sample| sample.missed_tokens)
        .sum();
    framed = framed.footer_right(Line::from(vec![
        Span::styled(
            compact_token_count(total_missed),
            Style::default().fg(rgb(255, 200, 100)),
        ),
        frame::dim(" missed"),
    ]));

    let max_w = inner.width as usize;
    for sample in cache.miss_attributions.iter().take(5) {
        let head = format!(
            "{} {} miss ",
            format_cache_turn_label(sample.turn_number, sample.call_index),
            compact_token_count(sample.missed_tokens)
        );
        let reason_w = max_w.saturating_sub(UnicodeWidthStr::width(head.as_str()) + 2);
        framed.lines.push(Line::from(vec![
            Span::styled(
                format_cache_turn_label(sample.turn_number, sample.call_index),
                Style::default().fg(rgb(140, 180, 255)).bold(),
            ),
            Span::styled(
                format!(" {} miss ", compact_token_count(sample.missed_tokens)),
                Style::default().fg(rgb(255, 200, 100)),
            ),
            Span::styled(
                format!("({})", truncate_smart(&sample.reason, reason_w.max(4))),
                Style::default().fg(rgb(140, 140, 150)),
            ),
        ]));
    }

    if cache.miss_attributions.len() > 5 {
        framed = framed.footer(frame::more(cache.miss_attributions.len() - 5));
    }

    framed
}

pub(super) fn kv_cache_pcts(cache: &CacheHitInfo) -> Option<(u8, Option<u8>, Option<u8>, Color)> {
    let lifetime_ratio = cache.hit_ratio()?;
    let lifetime_pct = ratio_pct(lifetime_ratio);
    let warm_pct = cache.optimal_ratio().map(ratio_pct);
    let last_pct = cache.last_ratio().map(ratio_pct);
    let last_optimal_pct = cache.last_optimal_ratio().map(ratio_pct);
    let health_pct = last_optimal_pct
        .or(last_pct)
        .or(warm_pct)
        .unwrap_or(lifetime_pct);
    Some((
        lifetime_pct,
        warm_pct,
        last_pct,
        kv_cache_optimal_color(health_pct),
    ))
}

/// Border headline: `KV cache 90% yield`, or observed hit rate without a baseline.
pub(super) fn render_kv_cache_title(cache: &CacheHitInfo) -> Line<'static> {
    let mut spans = vec![frame::label("KV cache ")];
    match kv_cache_pcts(cache) {
        Some((_, Some(warm), _, color)) => {
            spans.push(Span::styled(
                format!("{warm}%"),
                Style::default().fg(color).bold(),
            ));
            spans.push(frame::dim(" yield"));
        }
        Some((lifetime_pct, None, _, color)) => {
            // Restored sessions lack the historical reusable-prefix denominator.
            // That is missing telemetry, not evidence the cache is still warming.
            spans.push(Span::styled(
                format!("{lifetime_pct}%"),
                Style::default().fg(color).bold(),
            ));
            spans.push(frame::dim(" hit"));
        }
        None => {}
    }
    Line::from(spans)
}

/// Body row: `last 94% · session 39%`.
pub(super) fn render_kv_cache_rates_line(cache: &CacheHitInfo) -> Line<'static> {
    let Some((lifetime_pct, _, last_pct, color)) = kv_cache_pcts(cache) else {
        return Line::default();
    };
    let mut spans = Vec::new();
    if let Some(last_pct) = last_pct {
        spans.push(Span::styled(
            "last ",
            Style::default().fg(rgb(140, 140, 150)),
        ));
        spans.push(Span::styled(
            format!("{}%", last_pct),
            Style::default().fg(color).bold(),
        ));
        spans.push(Span::styled(" · ", Style::default().fg(rgb(80, 80, 90))));
    }
    spans.push(Span::styled(
        "session ",
        Style::default().fg(rgb(140, 140, 150)),
    ));
    spans.push(Span::styled(
        format!("{}%", lifetime_pct),
        Style::default().fg(color).bold(),
    ));
    Line::from(spans)
}

#[cfg(test)]
pub(super) fn render_kv_cache_summary_line(cache: &CacheHitInfo) -> Line<'static> {
    let Some(lifetime_ratio) = cache.hit_ratio() else {
        return Line::default();
    };

    let lifetime_pct = ratio_pct(lifetime_ratio);
    let warm_pct = cache.optimal_ratio().map(ratio_pct);
    let last_pct = cache.last_ratio().map(ratio_pct);
    let last_optimal_pct = cache.last_optimal_ratio().map(ratio_pct);
    let health_pct = last_optimal_pct
        .or(last_pct)
        .or(warm_pct)
        .unwrap_or(lifetime_pct);
    let color = kv_cache_optimal_color(health_pct);

    let mut spans = vec![Span::styled(
        "KV cache: ",
        Style::default().fg(rgb(180, 180, 190)).bold(),
    )];

    if let Some(warm_pct) = warm_pct {
        spans.push(Span::styled(
            "yield ",
            Style::default().fg(rgb(140, 140, 150)),
        ));
        spans.push(Span::styled(
            format!("{}%", warm_pct),
            Style::default().fg(color).bold(),
        ));
    } else {
        spans.push(Span::styled(
            format!("hit {}%", lifetime_pct),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    }

    if let Some(last_pct) = last_pct {
        spans.push(Span::styled(" · ", Style::default().fg(rgb(80, 80, 90))));
        spans.push(Span::styled(
            "last ",
            Style::default().fg(rgb(140, 140, 150)),
        ));
        spans.push(Span::styled(
            format!("{}%", last_pct),
            Style::default().fg(color).bold(),
        ));
    }

    spans.push(Span::styled(" · ", Style::default().fg(rgb(80, 80, 90))));
    spans.push(Span::styled(
        "session ",
        Style::default().fg(rgb(140, 140, 150)),
    ));
    spans.push(Span::styled(
        format!("{}%", lifetime_pct),
        Style::default().fg(color).bold(),
    ));

    Line::from(spans)
}

pub(super) fn ratio_pct(ratio: f32) -> u8 {
    (ratio * 100.0).round().clamp(0.0, 100.0) as u8
}

pub(super) fn kv_cache_optimal_color(pct: u8) -> Color {
    match pct {
        0..=24 => rgb(255, 110, 110),
        25..=59 => rgb(255, 200, 100),
        60..=84 => rgb(140, 180, 255),
        _ => rgb(110, 210, 140),
    }
}

pub(super) fn format_cache_turn_label(turn_number: usize, call_index: u16) -> String {
    if call_index <= 1 {
        format!("{}>", turn_number)
    } else {
        format!("{}.{}>", turn_number, call_index)
    }
}

pub(super) fn compact_token_count(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f32 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.0}k", tokens as f32 / 1_000.0)
    } else {
        tokens.to_string()
    }
}
