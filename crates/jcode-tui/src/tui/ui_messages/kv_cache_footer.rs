use super::*;

pub(super) fn kv_cache_footer_body(msg: &DisplayMessage) -> Option<&str> {
    if msg.effective_role() != "system" {
        return None;
    }
    let content = msg.content.trim();
    let body = content
        .trim_start_matches("⚠️")
        .trim_start_matches("ℹ️")
        .trim_start();
    if !(body.starts_with("KV cache miss") || body.starts_with("KV cache refresh")) {
        return None;
    }
    Some(body)
}

/// KV cache notices hang directly off the preceding response. Only the miss
/// amount and label carry severity color; supporting evidence stays muted.
pub(super) fn render_kv_cache_footer(
    msg: &DisplayMessage,
    width: u16,
) -> Option<Vec<Line<'static>>> {
    let body = kv_cache_footer_body(msg)?;
    let is_miss = body.starts_with("KV cache miss");
    let color = if is_miss {
        match kv_cache_missed_token_text(body)
            .and_then(parse_kv_token_count)
            .filter(|tokens| tokens.is_finite() && *tokens >= 0.0)
        {
            Some(tokens) if tokens >= 50_000.0 => error_color(),
            Some(tokens) if tokens >= 10_000.0 => warning_color(),
            Some(_) => dim_color(),
            // Missing or malformed amounts must not imply a small loss.
            _ => warning_color(),
        }
    } else if msg.content.trim().starts_with("⚠️") {
        warning_color()
    } else {
        dim_color()
    };
    let summary = kv_cache_footer_summary(body);
    let body = summary.as_deref().unwrap_or(body);
    let prefix = if width >= 2 { "╰ " } else { "" };
    let label_end = if summary.is_some() {
        body.find(" · ").unwrap_or(body.len())
    } else {
        // Unrecognized diagnostic shapes retain their text, but only the
        // recognizable label is emphasized.
        body.find(" [").unwrap_or(body.len())
    };
    let (label, details) = body.split_at(label_end);
    let line = Line::from(vec![
        Span::styled(prefix, Style::default().fg(dim_color())),
        Span::styled(label.to_string(), Style::default().fg(color)),
        Span::styled(details.to_string(), Style::default().fg(dim_color())),
    ]);
    Some(vec![super::super::truncate_line_with_ellipsis_to_width(
        &line,
        usize::from(width),
    )])
}

fn kv_cache_missed_token_text(body: &str) -> Option<&str> {
    body.split_once('~')?
        .1
        .split_once(" tokens")
        .map(|(tokens, _)| tokens.trim())
}

fn parse_kv_token_count(s: &str) -> Option<f64> {
    let s = s.trim().trim_start_matches('~');
    let (num, mult) = match s.chars().last()? {
        'K' | 'k' => (&s[..s.len() - 1], 1e3),
        'M' | 'm' => (&s[..s.len() - 1], 1e6),
        _ => (s, 1.0),
    };
    match num.replace(',', "").parse::<f64>() {
        Ok(tokens) => Some(tokens * mult),
        // Malformed optional diagnostics retain the original notice and warning color.
        Err(_) => None,
    }
}

/// `19K KV cache miss · provider switch · 60% hit` from the full
/// notice text. Returns None when the notice is not in a recognized shape.
pub(super) fn kv_cache_footer_summary(body: &str) -> Option<String> {
    let kind = if body.starts_with("KV cache refresh") {
        "KV cache refresh"
    } else {
        "KV cache miss"
    };
    let reason = body
        .split_once('[')
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(r, _)| r.trim().to_string())?;
    let missed = kv_cache_missed_token_text(body);
    let hit = body.split_once(" cached / ").and_then(|(before, after)| {
        let cached = parse_kv_token_count(before.rsplit(' ').next()?)?;
        let expected = parse_kv_token_count(after.split(' ').next()?)?;
        (expected > 0.0).then(|| ((cached / expected) * 100.0).round().clamp(0.0, 100.0))
    });
    let lead = match missed {
        Some(missed) => format!("{missed} {kind}"),
        None => kind.to_string(),
    };
    let mut parts = vec![lead, reason];
    if body
        .split_once(']')
        .is_some_and(|(_, tail)| tail.trim_start().starts_with("[prefix unchanged]"))
    {
        parts.push("prefix unchanged".to_string());
    }
    if let Some(hit) = hit {
        parts.push(format!("{hit:.0}% hit"));
    }
    Some(parts.join(" · "))
}
