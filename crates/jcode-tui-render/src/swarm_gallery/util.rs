//! Helper functions for the swarm gallery: colors, formatting, sorting, and
//! display-width utilities shared across all gallery renderers.

use ratatui::prelude::*;

use jcode_tui_style::color::rgb;

use super::types::GalleryMember;

/// Accent color for a member lifecycle status.
pub fn status_accent(status: &str) -> Color {
    match status {
        "spawned" => rgb(140, 140, 150),
        "ready" => rgb(120, 180, 120),
        "running" | "streaming" => rgb(255, 200, 100),
        "thinking" => rgb(140, 180, 255),
        "blocked" | "waiting_network" => rgb(255, 170, 80),
        "failed" | "crashed" => rgb(255, 100, 100),
        "completed" | "done" => rgb(100, 200, 100),
        "stopped" => rgb(140, 140, 150),
        _ => rgb(140, 140, 150),
    }
}

/// Optional glyph prefixed to a member's title based on its swarm role.
pub fn role_glyph(role: Option<&str>) -> Option<&'static str> {
    match role {
        Some("coordinator") => Some("★"),
        _ => None,
    }
}

/// Falls back to `None` for unknown roles so callers can use `status_accent()`.
pub fn role_color(role: Option<&str>) -> Option<Color> {
    match role {
        Some("coordinator") => Some(rgb(255, 200, 100)), // gold/amber
        Some("worker") | Some("implementer") => Some(rgb(100, 160, 255)), // blue
        Some("reviewer") => Some(rgb(130, 210, 130)),    // green
        Some("researcher") => Some(rgb(180, 140, 255)),  // purple
        _ => None,
    }
}

/// Compact age formatting for member viewports (now/Ns/Nm/Nh).
pub fn humanize_age(age: u64) -> String {
    if age < 2 {
        "now".to_string()
    } else if age < 60 {
        format!("{age}s")
    } else if age < 3600 {
        format!("{}m", age / 60)
    } else {
        format!("{}h", age / 3600)
    }
}

/// Whether a status counts as "active" for the header's active-agent tally.
pub fn is_active_status(status: &str) -> bool {
    matches!(status, "running" | "streaming" | "thinking")
}

///
/// Keep this aligned with the TUI redraw interval. 80 ms matches the primary
/// status spinner and avoids the visibly stepped motion of the old 125 ms
/// cadence without redrawing faster than the glyph can change.
pub const STRIP_SPINNER_FRAME_MS: u64 = 80;
pub const STRIP_SPINNER_FPS: f32 = 1000.0 / STRIP_SPINNER_FRAME_MS as f32;

/// Frames for the inline status spinner used by active agents on the strip.
pub const STRIP_SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// A glyph summarizing a member's lifecycle status. Active members (running,
/// thinking, streaming) animate via the spinner frame; terminal states get a
/// fixed glyph. `spinner_frame` selects the spinner cell for active members.
pub fn status_glyph(status: &str, spinner_frame: usize) -> &'static str {
    match status {
        "running" | "streaming" | "thinking" => {
            STRIP_SPINNER_FRAMES[spinner_frame % STRIP_SPINNER_FRAMES.len()]
        }
        "completed" | "done" => "✓",
        "ready" => "•",
        "blocked" | "waiting_network" => "⏸",
        "failed" | "crashed" => "✗",
        "stopped" => "◼",
        "spawned" => "·",
        _ => "•",
    }
}

/// Calm lifecycle marker for transcript cards.
///
/// Transcript content should remain stable while users read or scroll it. The
/// dedicated swarm strip retains animated status glyphs for live motion.
pub(crate) fn card_status_glyph(status: &str) -> &'static str {
    match status {
        "running" | "streaming" | "thinking" => "●",
        "completed" | "done" => "✓",
        "ready" => "•",
        "blocked" | "waiting_network" => "⏸",
        "failed" | "crashed" => "✗",
        "stopped" => "◼",
        "spawned" => "·",
        _ => "•",
    }
}

pub(crate) fn card_status_label(status: &str) -> &'static str {
    match status {
        "running" | "streaming" | "thinking" => "Working",
        "completed" | "done" => "Completed",
        "ready" | "spawned" => "Ready",
        "blocked" | "waiting_network" => "Blocked",
        "failed" | "crashed" => "Failed",
        "stopped" => "Stopped",
        _ => "Working",
    }
}

pub(crate) fn format_elapsed(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

/// Short elapsed-time formatting: "Xm Ys" or "Ys".
pub fn format_elapsed_short(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {}s", secs / 60, secs % 60)
    }
}

/// Format token counts with human-readable suffixes.
pub fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M tokens", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K tokens", tokens as f64 / 1_000.0)
    } else {
        format!("{tokens} tokens")
    }
}

/// Format cost in USD cents with a dollar sign.
pub fn format_cost(cents: u64) -> String {
    if cents >= 100 {
        format!("${:.2}", cents as f64 / 100.0)
    } else {
        format!("${:.3}", cents as f64 / 100.0)
    }
}

pub(crate) fn format_model(model: &str) -> String {
    let routed = model.rsplit([':', '/']).next().unwrap_or(model);
    let model = routed
        .strip_suffix("-sol")
        .or_else(|| routed.strip_suffix("-luna"))
        .unwrap_or(routed);
    if let Some(rest) = model.strip_prefix("gpt-") {
        format!("GPT-{rest}")
    } else {
        model.to_string()
    }
}

pub(crate) fn format_route(provider: Option<&str>, auth_method: Option<&str>) -> Option<String> {
    let provider = provider.map(str::trim).filter(|p| !p.is_empty());
    let auth_method = auth_method.map(str::trim).filter(|m| !m.is_empty());
    match (provider, auth_method) {
        (Some(provider), Some(method)) => Some(format!("{provider} {method}")),
        (Some(provider), None) => Some(provider.to_string()),
        (None, Some(method)) => Some(method.to_string()),
        (None, None) => None,
    }
}

pub(crate) fn role_rank(role: Option<&str>) -> u8 {
    match role {
        Some("coordinator") => 0,
        _ => 2,
    }
}

pub(crate) fn status_rank(status: &str) -> u8 {
    match status {
        s if is_active_status(s) => 0,
        "blocked" | "waiting_network" | "failed" | "crashed" => 1,
        "completed" | "done" | "stopped" => 3,
        // ready/spawned/unknown: idle but not finished.
        _ => 2,
    }
}

/// Aggregate stats line shown below agent chips when the swarm strip is focused.
///
/// Formats as: "X/Y agents · Z active · A/B tasks · Xm Ys elapsed"
/// Right-aligned to `width` using dim styling.
pub fn summary_line(
    total: usize,
    active: usize,
    todos_done: u32,
    todos_total: u32,
    max_elapsed: u64,
    width: usize,
) -> Line<'static> {
    let elapsed_text = if max_elapsed < 60 {
        format!("{}s", max_elapsed)
    } else {
        format!("{}m {}s", max_elapsed / 60, max_elapsed % 60)
    };
    let tasks_text = format!("{}/{} tasks", todos_done, todos_total);
    let mut body = format!(
        "{total} agent{} · {active} active · {tasks_text} · {elapsed_text} elapsed",
        if total == 1 { "" } else { "s" },
    );
    let body_w = disp_w(&body);
    if body_w > width {
        // Truncate display-width to fit within the budget.
        let mut w = 0usize;
        let mut end = body.len();
        for (i, ch) in body.char_indices() {
            w += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if w > width {
                end = i;
                break;
            }
        }
        body.truncate(end);
    }
    let body_w = disp_w(&body);
    let mut spans: Vec<Span<'static>> = Vec::new();
    if body_w < width {
        spans.push(Span::raw(" ".repeat(width - body_w)));
    }
    spans.push(Span::styled(body, Style::default().fg(rgb(105, 105, 120))));
    Line::from(spans)
}

/// Summary line built directly from a `&[GalleryMember]` slice.
///
/// Counts members by status category and formats as:
/// `Agents: {total} · {running} running · {queued} queued · {idle} idle`
/// Categories with zero count are omitted. Right-aligned to `width`.
pub fn summary_line_from_members(members: &[GalleryMember], width: usize) -> Line<'static> {
    let total = members.len();

    let mut running = 0usize;
    let mut queued = 0usize;
    let mut done = 0usize;
    let mut failed = 0usize;
    let mut idle = 0usize;

    for m in members {
        match m.status.as_str() {
            "running" | "streaming" | "thinking" => running += 1,
            "queued" | "waiting" => queued += 1,
            "completed" | "done" => done += 1,
            "failed" | "error" => failed += 1,
            _ => idle += 1,
        }
    }

    // Build the "Agents: N" prefix.
    let mut parts: Vec<String> = Vec::new();
    parts.push(format!("Agents: {total}"));

    let dim = rgb(105, 105, 120);
    let accent = rgb(255, 200, 100);

    // Collect non-zero categories as (label, color) pairs.
    let mut segments: Vec<(String, Color)> = Vec::new();
    if running > 0 {
        segments.push((format!("{running} running"), accent));
    }
    if queued > 0 {
        segments.push((format!("{queued} queued"), dim));
    }
    if done > 0 {
        segments.push((format!("{done} done"), dim));
    }
    if failed > 0 {
        segments.push((format!("{failed} failed"), rgb(255, 100, 100)));
    }
    if idle > 0 {
        segments.push((format!("{idle} idle"), dim));
    }

    // Compose the full body text to measure width.
    let mut body_text = parts.join(" · ");
    for (label, _) in &segments {
        body_text.push_str(" · ");
        body_text.push_str(label);
    }

    let body_w = disp_w(&body_text);
    if body_w > width {
        let mut w = 0usize;
        let mut end = body_text.len();
        for (i, ch) in body_text.char_indices() {
            w += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if w > width {
                end = i;
                break;
            }
        }
        body_text.truncate(end);
    }

    let body_w = disp_w(&body_text);
    let mut spans: Vec<Span<'static>> = Vec::new();
    if body_w < width {
        spans.push(Span::raw(" ".repeat(width - body_w)));
    }

    // Re-segment with styling: prefix in dim, category labels colored.
    // Simple approach: the prefix (before first ·) is dim, then each segment
    // gets its own color.
    let prefix_text = parts.join(" · ");

    // If the full body was truncated, just render it in dim.
    if body_w < disp_w(&prefix_text) + 20 {
        // Truncated or very narrow: render everything dim.
        spans.push(Span::styled(body_text, Style::default().fg(dim)));
    } else {
        // Render prefix dim, then each colored segment.
        spans.push(Span::styled(prefix_text, Style::default().fg(dim)));
        for (label, color) in &segments {
            let sep = " · ";
            spans.push(Span::styled(sep, Style::default().fg(dim)));
            spans.push(Span::styled(label.clone(), Style::default().fg(*color)));
        }
    }

    Line::from(spans)
}

/// Truncate a styled line so its display width never exceeds `max_width`.
/// Splits mid-span if needed, dropping a trailing wide glyph that would
/// straddle the boundary.
pub(crate) fn clamp_line_to_width(line: &mut Line<'static>, max_width: usize) {
    use unicode_width::UnicodeWidthChar;
    let mut used = 0usize;
    let mut clamped: Vec<Span<'static>> = Vec::new();
    for span in line.spans.drain(..) {
        let w = disp_w(&span.content);
        if used + w <= max_width {
            used += w;
            clamped.push(span);
            continue;
        }
        // Partial span: take chars while they fit.
        let mut taken = String::new();
        for ch in span.content.chars() {
            let cw = ch.width().unwrap_or(0);
            if used + cw > max_width {
                break;
            }
            used += cw;
            taken.push(ch);
        }
        if !taken.is_empty() {
            clamped.push(Span::styled(taken, span.style));
        }
        break;
    }
    line.spans = clamped;
}

/// Indices of `members` in display order: coordinator first, then worktree
/// manager, then everything else. Within a role bucket, still-working agents
/// sort before blocked/failed ones, which sort before idle and then finished
/// ones, so active agents never hide behind completed ones in the "+N"
/// overflow. Remaining ties break by `sort_key` (stable for full ties). This
/// is the single source of truth for how the gallery, panel, and strip order
/// members; callers that need to map a displayed row back to an input member
/// (e.g. pop-out selection) must use this rather than re-implementing the
/// sort.
pub fn display_order(members: &[GalleryMember]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..members.len()).collect();
    idx.sort_by(|&a, &b| {
        let (a, b) = (&members[a], &members[b]);
        role_rank(a.role.as_deref())
            .cmp(&role_rank(b.role.as_deref()))
            .then_with(|| status_rank(&a.status).cmp(&status_rank(&b.status)))
            .then_with(|| a.sort_key.cmp(&b.sort_key))
    });
    idx
}

/// References to `members` in [`display_order`], for rendering.
pub(crate) fn sort_members_for_display(members: &[GalleryMember]) -> Vec<&GalleryMember> {
    display_order(members)
        .into_iter()
        .map(|i| &members[i])
        .collect()
}

/// The tile index (in `members_to_tiles(members)` order) for a display row.
/// Since both orderings use the same sort, the display index equals the tile
/// index, but resolve via sort_key to stay correct if that ever diverges.
pub(crate) fn display_index_to_tile_index(
    ordered: &[&GalleryMember],
    _members: &[GalleryMember],
    display_idx: usize,
) -> usize {
    // tiles are produced by the same sort, so display order == tile order.
    let _ = ordered;
    display_idx
}

/// Truncate `s` to at most `max` display columns (wide glyphs count as 2),
/// appending an ellipsis when truncated.
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
