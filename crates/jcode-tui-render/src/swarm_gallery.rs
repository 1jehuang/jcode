//! Shared presentation logic for the inline swarm gallery.
//!
//! This is the single source of truth for how swarm-agent viewports look:
//! status accent colors, role glyphs, age formatting, the header line, member
//! sorting, and the gallery [`SwarmGalleryConfig`]. Both the live TUI adapter
//! (`jcode-tui`) and the `swarm_gallery_live` demo map their own data into
//! [`GalleryMember`] and call [`render_gallery`], so the demo renders identical
//! output to production and the two cannot drift.

use ratatui::prelude::*;

use jcode_tui_style::color::rgb;

use crate::swarm_tiles::{SwarmGalleryConfig, SwarmTile, render_swarm_gallery};

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

/// Cadence for active-agent spinner frames.
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
fn card_status_glyph(status: &str) -> &'static str {
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

fn card_status_label(status: &str) -> &'static str {
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

fn format_elapsed(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

fn format_model(model: &str) -> String {
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

/// Combine provider display name and credential route into one metadata piece,
/// e.g. "OpenAI oauth". Returns `None` when both are blank.
fn format_route(provider: Option<&str>, auth_method: Option<&str>) -> Option<String> {
    let provider = provider.map(str::trim).filter(|p| !p.is_empty());
    let auth_method = auth_method.map(str::trim).filter(|m| !m.is_empty());
    match (provider, auth_method) {
        (Some(provider), Some(method)) => Some(format!("{provider} {method}")),
        (Some(provider), None) => Some(provider.to_string()),
        (None, Some(method)) => Some(method.to_string()),
        (None, None) => None,
    }
}

/// Sort rank for stable placement: coordinator first, then everything else.
fn role_rank(role: Option<&str>) -> u8 {
    match role {
        Some("coordinator") => 0,
        _ => 2,
    }
}

/// Sort rank for lifecycle status within a role bucket: still-working agents
/// first, then agents needing attention, then idle ones, then finished ones.
/// This keeps active agents visible on the strip instead of letting them
/// collapse into the "+N" overflow behind completed agents.
fn status_rank(status: &str) -> u8 {
    match status {
        s if is_active_status(s) => 0,
        "blocked" | "waiting_network" | "failed" | "crashed" => 1,
        "completed" | "done" | "stopped" => 3,
        // ready/spawned/unknown: idle but not finished.
        _ => 2,
    }
}

/// The header line shown above the gallery grid.
pub fn gallery_header(total: usize, active: usize) -> Line<'static> {
    Line::from(vec![
        Span::styled("🐝 ", Style::default().fg(rgb(255, 200, 100))),
        Span::styled(
            format!(
                "{} agent{}{}",
                total,
                if total == 1 { "" } else { "s" },
                if active > 0 {
                    format!(" · {active} active")
                } else {
                    String::new()
                }
            ),
            Style::default().fg(rgb(160, 160, 170)),
        ),
    ])
}

/// A renderer-agnostic view of one swarm member, ready for layout.
///
/// Callers are responsible for building the `body` lines (e.g. choosing live
/// output tail vs. status detail); everything else about how the tile looks is
/// handled here.
#[derive(Clone, Debug)]
pub struct GalleryMember {
    /// Display title (friendly name or short id).
    pub label: String,
    /// Optional session icon (emoji) shown in place of the name on the
    /// vertical strip, e.g. "🦊" for a session named "fox".
    pub icon: Option<String>,
    /// Lifecycle status string (drives the badge text and accent color).
    pub status: String,
    /// Short label of the task this member was spawned/assigned for. Shown
    /// dimmed next to the name on the strip so the line answers "who is doing
    /// what", not just "who exists".
    pub task: Option<String>,
    /// Swarm role, if any (drives the title glyph and sort order).
    pub role: Option<String>,
    /// Pre-rendered body lines shown inside the tile.
    pub body: Vec<String>,
    /// Stable tiebreaker for sorting members with equal role rank (e.g. id).
    pub sort_key: String,
    /// Optional todo progress as (completed, total) for the agent's plan/todos.
    /// Rendered as "C/T" next to the agent on the strip when present.
    pub todo: Option<(u32, u32)>,
    /// Compact todo entries (content, status) for the focused detail view.
    /// Status is one of "pending", "in_progress", "completed".
    pub todo_items: Vec<GalleryTodo>,
    /// Provider model currently running this member, when known.
    pub model: Option<String>,
    /// Provider display name and credential route used by this member.
    pub provider: Option<String>,
    pub auth_method: Option<String>,
    /// Reasoning effort selected for this member.
    pub effort: Option<String>,
    /// Seconds since this member was spawned.
    pub elapsed_secs: Option<u64>,
}

/// One compact todo entry shown in the focused swarm detail view.
#[derive(Clone, Debug)]
pub struct GalleryTodo {
    pub content: String,
    /// "pending", "in_progress", or "completed".
    pub status: String,
    /// Up to three recent tool calls made while this item was active.
    pub tool_intents: Vec<GalleryToolIntent>,
}

#[derive(Clone, Debug)]
pub struct GalleryToolIntent {
    pub tool_name: String,
    pub intent: String,
    /// "running", "completed", or "error".
    pub status: String,
    /// Optional live progress as (current, total, unit).
    pub progress: Option<(u64, u64, Option<String>)>,
}

/// Convert members into gallery tiles, sorted for stable placement
/// (coordinator first, worktree manager next, then by `sort_key`).
pub fn members_to_tiles(members: &[GalleryMember]) -> Vec<SwarmTile> {
    sort_members_for_display(members)
        .into_iter()
        .map(|m| {
            let mut tile =
                SwarmTile::new(m.label.clone(), m.status.clone(), status_accent(&m.status))
                    .with_body(m.body.clone());
            if let Some(glyph) = role_glyph(m.role.as_deref()) {
                tile = tile.with_role_glyph(glyph);
            }
            tile
        })
        .collect()
}

/// Render the inline swarm gallery for `members` into `width`-bounded lines.
///
/// `max_height` is the total height budget for the band (including the header);
/// the gallery grid gets `max_height - 1` rows. Returns an empty vec when there
/// are no members.
pub fn render_gallery(
    members: &[GalleryMember],
    width: usize,
    max_height: usize,
) -> Vec<Line<'static>> {
    if members.is_empty() {
        return Vec::new();
    }
    let tiles = members_to_tiles(members);
    let active = members
        .iter()
        .filter(|m| is_active_status(&m.status))
        .count();
    let header = gallery_header(members.len(), active);
    let cfg = SwarmGalleryConfig {
        max_height: max_height.saturating_sub(1).max(4),
        ..Default::default()
    };
    let mut out = render_swarm_gallery(&tiles, width, &cfg, Some(header));
    // The grid cells are width-bounded already, but the header (and any
    // degenerate-width artifacts) are not. Enforce the bound uniformly.
    for line in &mut out {
        clamp_line_to_width(line, width);
    }
    out
}

/// Render stable, one-line swarm summaries beneath `swarm spawn` tool calls.
///
/// Transcript content deliberately excludes elapsed time, output tails, todos,
/// tool progress, and animated glyphs. Those fields update frequently and make
/// old chat rows move while the user is reading them. The dedicated live swarm
/// page owns the detailed, animated representation instead. Spawn-time-stable
/// metadata (model, provider/auth route) is shown since it answers "what is
/// this agent running on" without churning.
pub fn render_swarm_chat_cards(members: &[GalleryMember], width: usize) -> Vec<Line<'static>> {
    if members.is_empty() || width < 8 {
        return Vec::new();
    }

    let mut out = Vec::new();
    for member in sort_members_for_display(members) {
        let accent = status_accent(&member.status);
        let lead = format!(
            "    {} {} ",
            member.icon.as_deref().unwrap_or("🐝"),
            card_status_glyph(&member.status)
        );
        let label = member.label.clone();

        // Stable runtime metadata (model and provider/auth route) is fixed at
        // spawn time, so it can live on the transcript card without making old
        // chat rows churn. Drop trailing pieces first when width is tight.
        let mut metadata = vec![card_status_label(&member.status).to_string()];
        if let Some(model) = member
            .model
            .as_deref()
            .filter(|model| !model.trim().is_empty())
        {
            metadata.push(format_model(model));
        }
        if let Some(route) = format_route(member.provider.as_deref(), member.auth_method.as_deref())
        {
            metadata.push(route);
        }
        let mut tail = format!(" · {}", metadata.join(" · "));
        while metadata.len() > 1 && disp_w(&lead) + disp_w(&label) + disp_w(&tail) > width {
            metadata.pop();
            tail = format!(" · {}", metadata.join(" · "));
        }

        let mut header = vec![
            Span::styled(lead.clone(), Style::default().fg(rgb(255, 200, 100))),
            Span::styled(
                label.clone(),
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
        ];
        let consumed = disp_w(&lead) + disp_w(&label);
        if consumed + disp_w(&tail) <= width {
            header.push(Span::styled(tail, Style::default().fg(rgb(150, 150, 160))));
        }
        out.push(Line::from(header));
    }

    for line in &mut out {
        clamp_line_to_width(line, width);
    }
    out
}

/// Render the selected member's detailed live card for the dedicated swarm
/// page. Active states use the animated spinner and the body includes current
/// todos/tool activity, runtime metadata, and the streamed output tail.
pub fn render_swarm_live_card(
    member: &GalleryMember,
    spinner_frame: usize,
    width: usize,
    max_height: usize,
) -> Vec<Line<'static>> {
    if width < 8 || max_height == 0 {
        return Vec::new();
    }

    let accent = status_accent(&member.status);
    let mut metadata = Vec::new();
    if let Some(elapsed) = member.elapsed_secs {
        metadata.push(format_elapsed(elapsed));
    }
    if let Some(model) = member
        .model
        .as_deref()
        .filter(|model| !model.trim().is_empty())
    {
        metadata.push(format_model(model));
    }
    if let Some(route) = format_route(member.provider.as_deref(), member.auth_method.as_deref()) {
        metadata.push(route);
    }
    if let Some(effort) = member
        .effort
        .as_deref()
        .filter(|effort| !effort.trim().is_empty())
    {
        metadata.push(effort.to_string());
    }

    let lead = format!(
        "  {} {} ",
        member.icon.as_deref().unwrap_or("🐝"),
        status_glyph(&member.status, spinner_frame)
    );
    let label = member.label.clone();
    let mut tail = if metadata.is_empty() {
        String::new()
    } else {
        format!(" · {}", metadata.join(" · "))
    };
    while metadata.len() > 1 && disp_w(&lead) + disp_w(&label) + disp_w(&tail) > width {
        metadata.pop();
        tail = format!(" · {}", metadata.join(" · "));
    }

    let mut header = vec![
        Span::styled(lead.clone(), Style::default().fg(rgb(255, 200, 100))),
        Span::styled(
            label.clone(),
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ),
    ];
    let consumed = disp_w(&lead) + disp_w(&label);
    if consumed + disp_w(&tail) <= width {
        header.push(Span::styled(tail, Style::default().fg(rgb(150, 150, 160))));
    }

    let mut out = vec![Line::from(header)];
    let body_budget = max_height.saturating_sub(1);
    if body_budget > 0 {
        out.extend(hovered_detail_body(member, None, width, body_budget, false));
    }
    out.truncate(max_height);
    for line in &mut out {
        clamp_line_to_width(line, width);
    }
    out
}

/// Render the swarm panel as a compact list of managed agents plus a detail
/// viewport for the selected agent.
///
/// Layout (top to bottom):
/// ```text
/// 🐝 swarm · N agents · M active
///   ▸ ★ coordinator        [running]   now
///     implementer          [thinking]  3s
///     reviewer             [done]      1m
/// ╭─ implementer ──────────────── [thinking]─╮
/// │ <selected agent's live output tail>      │
/// ╰──────────────────────────────────────────╯
/// ```
///
/// `selected` is clamped into range. `width` bounds every line. `max_height` is
/// the total budget; the list gets one row per agent (capped) and the detail
/// viewport gets the remainder. Returns empty when there are no members.
pub fn render_swarm_panel(
    members: &[GalleryMember],
    selected: usize,
    focused: bool,
    width: usize,
    max_height: usize,
) -> Vec<Line<'static>> {
    if members.is_empty() || width < 8 || max_height < 3 {
        return Vec::new();
    }
    let tiles = members_to_tiles(members);
    // members_to_tiles re-sorts; mirror that ordering for the list so the
    // selected index lines up with what is shown.
    let ordered = sort_members_for_display(members);
    let selected = selected.min(ordered.len().saturating_sub(1));

    let active = members
        .iter()
        .filter(|m| is_active_status(&m.status))
        .count();
    let mut out: Vec<Line<'static>> = Vec::new();
    out.push(panel_header(members.len(), active, focused));

    // Reserve at least 3 lines for the detail viewport when there is room.
    let detail_budget = if max_height >= 7 {
        (max_height / 2).max(3)
    } else {
        0
    };
    let list_budget = max_height.saturating_sub(1).saturating_sub(detail_budget);

    // ---- Agent list ----
    let list_rows = list_budget.min(ordered.len());
    // Scroll the list so the selection stays visible.
    let first = if selected >= list_rows {
        selected + 1 - list_rows
    } else {
        0
    };
    for (idx, member) in ordered
        .iter()
        .enumerate()
        .skip(first)
        .take(list_rows.max(1))
    {
        out.push(list_row(member, idx == selected, focused, width));
    }

    // ---- Detail viewport for the selected agent ----
    if detail_budget >= 3
        && let Some(tile) = tiles.get(display_index_to_tile_index(&ordered, members, selected))
    {
        let detail = crate::swarm_tiles::render_single_tile(tile, width, detail_budget);
        out.extend(detail);
    }

    // Hard bound: the header carries a fixed hint and list rows budget by
    // display width, but never let any line exceed the panel width.
    for line in &mut out {
        clamp_line_to_width(line, width);
    }

    out
}

/// Bounds for per-chip task labels on the strip: never wider than MAX (keeps
/// one agent from dominating the line) and dropped entirely below MIN (a two-
/// column "·…" label is noise, not information).
const CHIP_TASK_MAX_W: usize = 24;
const CHIP_TASK_MIN_W: usize = 6;

/// A key/label pair for the swarm strip hint line.
pub struct SwarmStripHint {
    /// The key chord to show, e.g. "alt+n" or "j/k".
    pub key: String,
    /// What it does, e.g. "select".
    pub label: String,
}

/// Render the compact swarm strip shown directly above the status line.
///
/// - Unfocused: a single line of agent "chips" (status glyph + name + optional
///   `done/total` todo count), colored by status, plus a right-aligned
///   `M/N active` readout. A trailing hint shows how to enter the controls.
/// - Focused: the chips line (selected agent highlighted) + an expanded detail
///   viewport for the hovered agent (transcript tail + todo list, bounded by
///   `max_height`) + a keybinding hint line.
///
/// `max_height` is the total line budget for the strip when focused (chips +
/// detail + hints). Small budgets degrade to a single inline detail line.
/// `spinner_frame` animates the glyph for active agents. Returns empty when
/// there are no members or no width.
///
/// ```text
/// 🐝 swarm  · ⠙ researcher 8/16  ✓ reviewer         2/3 active · ctrl+t controls
/// ```
#[allow(clippy::too_many_arguments)]
pub fn render_swarm_strip(
    members: &[GalleryMember],
    selected: usize,
    focused: bool,
    hints: &[SwarmStripHint],
    enter_hint: Option<&str>,
    spinner_frame: usize,
    width: usize,
    max_height: usize,
) -> Vec<Line<'static>> {
    if members.is_empty() || width < 8 {
        return Vec::new();
    }
    let ordered = sort_members_for_display(members);
    let selected = selected.min(ordered.len().saturating_sub(1));
    let active = members
        .iter()
        .filter(|m| is_active_status(&m.status))
        .count();

    // ---- Leading "🐝 swarm" label ----
    let lead: Vec<Span<'static>> = vec![
        Span::styled("🐝 ", Style::default().fg(rgb(255, 200, 100))),
        Span::styled("swarm", Style::default().fg(rgb(160, 160, 170))),
        Span::styled("  · ", Style::default().fg(rgb(80, 80, 90))),
    ];
    let lead_w: usize = lead.iter().map(|s| disp_w(&s.content)).sum();

    // ---- Right tail: "M/N active" plus, when unfocused, the controls hint.
    // Degrade gracefully on narrow widths: drop the hint first, then the tally,
    // so the chips never get pushed past the line width.
    let tally = format!("{active}/{} active", members.len());
    let tally_w = disp_w(&tally);
    let hint_text = if focused { None } else { enter_hint };
    let hint_sep = " · ";
    let gap = 2usize; // minimum gap between chips and the right tail
    let hint_w = hint_text.map(|h| disp_w(h) + disp_w(hint_sep)).unwrap_or(0);

    // ---- Chips: "<glyph> <name>[·task][ done/total]" ----
    // Task labels are additive: chips are fitted by their base width (glyph +
    // name + todo) so a long task can never hide other agents; leftover line
    // width is then shared out to task labels (see below).
    struct Chip {
        glyph: String,
        name: String,
        task: Option<String>,
        todo: Option<String>,
        color: Color,
        is_sel: bool,
    }
    let chips: Vec<Chip> = ordered
        .iter()
        .enumerate()
        .map(|(idx, m)| Chip {
            glyph: status_glyph(&m.status, spinner_frame).to_string(),
            name: m.label.clone(),
            task: m
                .task
                .as_deref()
                .filter(|t| !t.trim().is_empty())
                .map(|t| truncate_label(t, CHIP_TASK_MAX_W)),
            todo: m.todo.map(|(done, total)| format!("{done}/{total}")),
            color: status_accent(&m.status),
            is_sel: idx == selected,
        })
        .collect();
    let chip_w = |c: &Chip| -> usize {
        disp_w(&c.glyph) + 1 + disp_w(&c.name) + c.todo.as_ref().map(|t| disp_w(t) + 1).unwrap_or(0)
    };

    // Fit as many chips as possible into `budget`, collapsing overflow into a
    // "+N" marker that is itself budgeted so the line can never exceed `width`.
    // Returns (chips shown, columns used including any "+N" marker).
    const CHIP_SEP: &str = "  ";
    let sep_w = disp_w(CHIP_SEP);
    let fit_chips = |budget: usize| -> (usize, usize) {
        let mut shown = 0usize;
        let mut acc = 0usize;
        for (i, chip) in chips.iter().enumerate() {
            let s = if i == 0 { 0 } else { sep_w };
            let w = chip_w(chip);
            // Reserve room for a "+N" marker when chips would remain hidden.
            let remaining_after = chips.len() - i - 1;
            let reserve = if remaining_after > 0 {
                1 + 1 + count_digits(remaining_after)
            } else {
                0
            };
            if acc + s + w + reserve > budget {
                break;
            }
            acc += s + w;
            shown += 1;
        }
        let hidden = chips.len() - shown;
        if hidden > 0 {
            acc += 1 + 1 + count_digits(hidden);
        }
        (shown, acc)
    };

    // Pick the richest right tail that still leaves room for the chips:
    // tally + hint, then tally only, then no tail at all.
    let mut tail_configs: Vec<usize> = Vec::new();
    if hint_w > 0 {
        tail_configs.push(tally_w + hint_w);
    }
    tail_configs.push(tally_w);
    tail_configs.push(0);
    let mut shown = 0usize;
    let mut chips_used = 0usize;
    let mut tail_w = 0usize;
    for tw in tail_configs {
        let reserved = if tw > 0 { tw + gap } else { 0 };
        let budget = match width.checked_sub(lead_w + reserved) {
            Some(b) => b,
            None => continue,
        };
        let (s, u) = fit_chips(budget);
        if s > 0 || tw == 0 {
            shown = s;
            chips_used = u;
            tail_w = tw;
            break;
        }
    }
    let show_hint = tail_w > tally_w;
    let show_tally = tail_w > 0;

    // ---- Task label allocation: share leftover width across shown chips ----
    // Only when every chip already fits does the strip spend columns on task
    // labels, splitting the slack evenly (each capped at CHIP_TASK_MAX_W,
    // dropped entirely below CHIP_TASK_MIN_W so we never show "·…").
    let per_task_w: usize = {
        let budget = width.saturating_sub(lead_w + if tail_w > 0 { tail_w + gap } else { 0 });
        let leftover = budget.saturating_sub(chips_used);
        let task_count = chips
            .iter()
            .take(shown)
            .filter(|c| c.task.is_some())
            .count();
        if shown == chips.len() && task_count > 0 {
            // +1 per label for the '·' separator.
            let per = leftover / task_count;
            if per > CHIP_TASK_MIN_W {
                (per - 1).min(CHIP_TASK_MAX_W)
            } else {
                0
            }
        } else {
            0
        }
    };

    let mut spans: Vec<Span<'static>> = lead;
    let mut task_used = 0usize;
    let used: usize;
    if shown == 0 && !chips.is_empty() {
        // Degenerate width: show the first chip truncated.
        let budget = width.saturating_sub(lead_w + if show_tally { tail_w + gap } else { 0 });
        let c = &chips[0];
        let avail = budget.saturating_sub(disp_w(&c.glyph) + 1);
        let name = truncate_label(&c.name, avail.max(1));
        let style = Style::default().fg(c.color);
        spans.push(Span::styled(format!("{} ", c.glyph), style));
        spans.push(Span::styled(name.clone(), style));
        used = disp_w(&c.glyph) + 1 + disp_w(&name);
    } else {
        for (i, chip) in chips.iter().take(shown).enumerate() {
            if i > 0 {
                spans.push(Span::raw(CHIP_SEP));
            }
            let mut style = Style::default().fg(chip.color);
            if chip.is_sel && focused {
                style = style.add_modifier(Modifier::BOLD | Modifier::REVERSED);
            } else if chip.is_sel {
                style = style.add_modifier(Modifier::BOLD);
            }
            spans.push(Span::styled(format!("{} {}", chip.glyph, chip.name), style));
            if per_task_w > 0
                && let Some(task) = &chip.task
            {
                let label = truncate_label(task, per_task_w);
                task_used += 1 + disp_w(&label);
                spans.push(Span::styled(
                    format!("·{label}"),
                    Style::default().fg(rgb(150, 150, 160)),
                ));
            }
            if let Some(todo) = &chip.todo {
                spans.push(Span::styled(
                    format!(" {todo}"),
                    Style::default().fg(rgb(130, 130, 140)),
                ));
            }
        }
        let hidden = chips.len().saturating_sub(shown);
        if hidden > 0 {
            spans.push(Span::styled(
                format!(" +{hidden}"),
                Style::default().fg(rgb(140, 140, 150)),
            ));
        }
        used = chips_used + task_used;
    }

    // ---- Right-align the tail (tally [+ hint]) ----
    if show_tally {
        let consumed = lead_w + used;
        if consumed + gap + tail_w <= width {
            let pad = width - consumed - tail_w;
            spans.push(Span::raw(" ".repeat(pad)));
            spans.push(Span::styled(
                tally,
                Style::default().fg(if active > 0 {
                    rgb(255, 200, 100)
                } else {
                    rgb(120, 120, 130)
                }),
            ));
            if show_hint && let Some(hint) = hint_text {
                spans.push(Span::styled(
                    hint_sep.to_string(),
                    Style::default().fg(rgb(80, 80, 90)),
                ));
                spans.push(Span::styled(
                    hint.to_string(),
                    Style::default().fg(rgb(110, 130, 170)),
                ));
            }
        }
    }

    let mut out = vec![Line::from(spans)];

    // ---- Focused extras: expanded detail viewport + hint line ----
    if focused {
        // Budget: chips line (already emitted) + hint line are fixed; the
        // detail viewport gets the rest. Degrade to one inline line when the
        // budget is too small for the expanded view.
        let hint_rows = usize::from(!hints.is_empty());
        let detail_budget = max_height.saturating_sub(1 + hint_rows);
        if let Some(m) = ordered.get(selected) {
            if detail_budget >= 4 {
                out.extend(render_hovered_detail(
                    m,
                    spinner_frame,
                    width,
                    detail_budget,
                ));
            } else {
                // Compact fallback: a single inline line of the latest output.
                let detail = m
                    .body
                    .iter()
                    .rev()
                    .find(|l| !l.trim().is_empty() && !l.trim_start().starts_with('·'))
                    .cloned()
                    .unwrap_or_else(|| format!("[{}]", m.status));
                let prefix = format!("   {} ", status_glyph(&m.status, spinner_frame));
                let prefix_w = prefix.chars().count();
                let body = truncate_label(&detail, width.saturating_sub(prefix_w));
                out.push(Line::from(vec![
                    Span::styled(prefix, Style::default().fg(status_accent(&m.status))),
                    Span::styled(body, Style::default().fg(rgb(180, 180, 190))),
                ]));
            }
        }

        if !hints.is_empty() {
            let mut hint_spans: Vec<Span<'static>> = vec![Span::raw("   ")];
            for (i, h) in hints.iter().enumerate() {
                if i > 0 {
                    hint_spans.push(Span::styled(" · ", Style::default().fg(rgb(80, 80, 90))));
                }
                hint_spans.push(Span::styled(
                    h.key.clone(),
                    Style::default().fg(rgb(150, 170, 210)),
                ));
                hint_spans.push(Span::raw(" "));
                hint_spans.push(Span::styled(
                    h.label.clone(),
                    Style::default().fg(rgb(120, 120, 130)),
                ));
            }
            // Trim to width.
            let mut total = 0usize;
            let mut trimmed: Vec<Span<'static>> = Vec::new();
            for s in hint_spans {
                let w = s.content.chars().count();
                if total + w > width {
                    break;
                }
                total += w;
                trimmed.push(s);
            }
            out.push(Line::from(trimmed));
        }
    }

    // Hard bound: no matter how the budgeting above worked out, never emit a
    // line wider than the strip (degenerate widths, wide glyphs, etc.).
    for line in &mut out {
        clamp_line_to_width(line, width);
    }

    out
}

/// Render the vertical swarm strip shown directly above the status line: one
/// agent per row, capped to `max_rows` agent lines with a `+N more` overflow
/// marker on the last row.
///
/// Each row is `<status glyph> <icon> · <task>` with a right-aligned todo
/// counter when present. The first row carries the leading "🐝" swarm marker;
/// the header tally (`M/N active`) and the enter hint live on the first row's
/// right side, mirroring the horizontal strip.
///
/// - Unfocused: up to `max_rows` agent rows.
/// - Focused: an accordion. The selected agent's row gains a `▸` marker and
///   its full icon+name, and its live transcript tail + todos expand in place
///   directly beneath its row; a keybinding hint line closes the strip. All
///   bounded by `max_height` total lines.
///
/// ```text
/// 🐝 ⠙ 🦊 · wire the auth flow 3/9            2/3 active · alt+n controls
///  ▸ ⠹ 🐝 bee · audit the webhook path
///    │ checking the signing secret path
///    │ ▸ verify replay protection
///    ✓ 🐅 · support/contact page
///    alt+↑/↓ select · alt+o open · alt+shift+p prompt · esc exit
/// ```
#[allow(clippy::too_many_arguments)]
pub fn render_swarm_strip_vertical(
    members: &[GalleryMember],
    selected: usize,
    focused: bool,
    hints: &[SwarmStripHint],
    enter_hint: Option<&str>,
    spinner_frame: usize,
    width: usize,
    max_rows: usize,
    max_height: usize,
) -> Vec<Line<'static>> {
    if members.is_empty() || width < 8 || max_rows == 0 {
        return Vec::new();
    }
    let ordered = sort_members_for_display(members);
    let selected = selected.min(ordered.len().saturating_sub(1));
    let active = members
        .iter()
        .filter(|m| is_active_status(&m.status))
        .count();

    // Row budget: reserve one row for "+N more" when not everything fits.
    let shown = if ordered.len() <= max_rows {
        ordered.len()
    } else {
        max_rows.saturating_sub(1).max(1)
    };
    let hidden = ordered.len() - shown;

    // Keep the selected agent visible: window the list around the selection.
    let start = if selected >= shown {
        selected + 1 - shown
    } else {
        0
    };

    // ---- First-row right tail: "M/N active" plus the controls hint. ----
    let tally = format!("{active}/{} active", members.len());
    let tally_w = disp_w(&tally);
    let hint_text = if focused { None } else { enter_hint };
    let hint_sep = " · ";
    let gap = 2usize;
    let hint_w = hint_text.map(|h| disp_w(h) + disp_w(hint_sep)).unwrap_or(0);

    const LEAD: &str = "🐝 ";
    const INDENT: &str = "   ";
    let lead_w = disp_w(LEAD);

    let mut out: Vec<Line<'static>> = Vec::new();
    // Where the selected agent's row landed in `out` (focused accordion).
    let mut selected_row_at: Option<usize> = None;
    for (row, m) in ordered.iter().enumerate().skip(start).take(shown) {
        let first = out.is_empty();
        let is_sel = row == selected;
        let color = status_accent(&m.status);
        let glyph = status_glyph(&m.status, spinner_frame);
        let todo = m.todo.map(|(done, total)| format!("{done}/{total}"));

        let mut spans: Vec<Span<'static>> = Vec::new();
        if first {
            spans.push(Span::styled(
                LEAD.to_string(),
                Style::default().fg(rgb(255, 200, 100)),
            ));
        } else {
            spans.push(Span::raw(INDENT));
        }

        // Right tail only on the first row; degrade by dropping hint first.
        let (row_tail_w, show_hint) = if first {
            if lead_w + gap + tally_w + hint_w + 16 <= width && hint_w > 0 {
                (tally_w + hint_w, true)
            } else if lead_w + gap + tally_w + 12 <= width {
                (tally_w, false)
            } else {
                (0, false)
            }
        } else {
            (0, false)
        };

        let body_budget = width
            .saturating_sub(lead_w)
            .saturating_sub(if row_tail_w > 0 { row_tail_w + gap } else { 0 });

        // Expanded workers use the full-width information card header approved
        // for the inline swarm view. Compact rows retain the older chip layout.
        if focused && is_sel && !m.todo_items.is_empty() {
            let left = format!("{} ", m.label);
            spans.push(Span::styled(
                left.clone(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ));

            let mut metadata = vec![format!(
                "{} {}",
                status_glyph(&m.status, spinner_frame),
                card_status_label(&m.status)
            )];
            if let Some((done, total)) = m.todo {
                metadata.push(format!("Todo {done}/{total}"));
            }
            if let Some(elapsed) = m.elapsed_secs {
                metadata.push(format_elapsed(elapsed));
            }
            if let Some(model) = m.model.as_deref().filter(|model| !model.trim().is_empty()) {
                metadata.push(format_model(model));
            }

            let mut tail = metadata.join(" · ");
            while metadata.len() > 1 && lead_w + disp_w(&left) + gap + disp_w(&tail) > width {
                metadata.pop();
                tail = metadata.join(" · ");
            }
            let consumed = lead_w + disp_w(&left);
            if consumed + gap + disp_w(&tail) <= width {
                spans.push(Span::raw(" ".repeat(width - consumed - disp_w(&tail))));
                spans.push(Span::styled(tail, Style::default().fg(rgb(150, 150, 160))));
            }
            if is_sel {
                selected_row_at = Some(out.len());
            }
            out.push(Line::from(spans));
            continue;
        }

        // <glyph> [icon ]<name>[ · task][ done/total]
        let mut style = Style::default().fg(color);
        if is_sel && focused {
            style = style.add_modifier(Modifier::BOLD);
        }
        // The assigned animal icon already identifies the worker. Keep compact
        // strip rows emoji-only so names such as "sauropod" do not consume the
        // task-label space; the expanded chat card carries the full identity.
        let ident = match m.icon.as_deref().filter(|i| !i.is_empty()) {
            Some(icon) => icon.to_string(),
            None => m.label.clone(),
        };
        let marker = if focused {
            if is_sel { "▸ " } else { "  " }
        } else {
            ""
        };
        let head = format!("{marker}{glyph} {ident}");
        let head_w = disp_w(&head);
        let todo_w = todo.as_ref().map(|t| disp_w(t) + 1).unwrap_or(0);
        let mut used = head_w;
        spans.push(Span::styled(head, style));

        if let Some(task) = m.task.as_deref().filter(|t| !t.trim().is_empty()) {
            let avail = body_budget.saturating_sub(used + todo_w + disp_w(" · "));
            if avail >= CHIP_TASK_MIN_W {
                let label = truncate_label(task, avail);
                used += disp_w(" · ") + disp_w(&label);
                spans.push(Span::styled(
                    format!(" · {label}"),
                    Style::default().fg(rgb(150, 150, 160)),
                ));
            }
        }
        if let Some(todo) = &todo
            && used + todo_w <= body_budget
        {
            used += todo_w;
            spans.push(Span::styled(
                format!(" {todo}"),
                Style::default().fg(rgb(130, 130, 140)),
            ));
        }

        // ---- Right-align the first-row tail ----
        if row_tail_w > 0 {
            let consumed = lead_w + used;
            if consumed + gap + row_tail_w <= width {
                let pad = width - consumed - row_tail_w;
                spans.push(Span::raw(" ".repeat(pad)));
                spans.push(Span::styled(
                    tally.clone(),
                    Style::default().fg(if active > 0 {
                        rgb(255, 200, 100)
                    } else {
                        rgb(120, 120, 130)
                    }),
                ));
                if show_hint && let Some(hint) = hint_text {
                    spans.push(Span::styled(
                        hint_sep.to_string(),
                        Style::default().fg(rgb(80, 80, 90)),
                    ));
                    spans.push(Span::styled(
                        hint.to_string(),
                        Style::default().fg(rgb(110, 130, 170)),
                    ));
                }
            }
        }
        if is_sel {
            selected_row_at = Some(out.len());
        }
        out.push(Line::from(spans));
    }

    if hidden > 0 {
        out.push(Line::from(vec![
            Span::raw(INDENT),
            Span::styled(
                format!("+{hidden} more"),
                Style::default().fg(rgb(140, 140, 150)),
            ),
        ]));
    }

    // ---- Accordion detail under the selected row ----
    // Expanded details belong to the explicitly focused control surface. The
    // always-visible live card is rendered in chat beneath the spawning tool
    // call, so the persistent status strip stays compact.
    if focused {
        let hint_rows = usize::from(focused && !hints.is_empty());
        let detail_budget = max_height.saturating_sub(out.len() + hint_rows);
        if let (Some(m), Some(at)) = (ordered.get(selected), selected_row_at)
            && detail_budget >= 1
        {
            // Insert directly beneath the selected row so the list expands in
            // place (accordion) instead of jumping to a detached pane below.
            let detail = hovered_detail_body(m, Some(spinner_frame), width, detail_budget, true);
            for (i, line) in detail.into_iter().enumerate() {
                out.insert(at + 1 + i, line);
            }
        }
        if focused && !hints.is_empty() {
            let mut hint_spans: Vec<Span<'static>> = vec![Span::raw(INDENT)];
            for (i, h) in hints.iter().enumerate() {
                if i > 0 {
                    hint_spans.push(Span::styled(" · ", Style::default().fg(rgb(80, 80, 90))));
                }
                hint_spans.push(Span::styled(
                    h.key.clone(),
                    Style::default().fg(rgb(150, 170, 210)),
                ));
                hint_spans.push(Span::raw(" "));
                hint_spans.push(Span::styled(
                    h.label.clone(),
                    Style::default().fg(rgb(120, 120, 130)),
                ));
            }
            out.push(Line::from(hint_spans));
        }
    }

    for line in &mut out {
        clamp_line_to_width(line, width);
    }
    out
}

/// Render the swarm dock: a narrow, vertical agent list sized for the
/// info-widget margins (~20-38 inner columns).
///
/// Layout (all lines bounded to `width`, at most `max_height` lines):
/// ```text
/// 🐝 2/4 active · plan 3/7 · ⚠1
/// ▸ ★ coordinator      ⠋ 4/9
///   │ ⚙ bash cargo build 47s
///   │ carving gallery band
///   researcher         ⠙ 2/5
///   reviewer           ✓
///   +2 more
///   j/k · enter · esc
/// ```
///
/// The selected agent (clamped into range) gets a short live tail from its
/// `body` directly beneath its row: up to 2 lines, or 4 when `focused`. When
/// not all agents fit, the list windows around the selection and a `+N more`
/// line reports the rest. The hint line appears only when `focused`.
pub fn render_swarm_dock(
    members: &[GalleryMember],
    selected: usize,
    focused: bool,
    plan: Option<(u32, u32)>,
    spinner_frame: usize,
    width: usize,
    max_height: usize,
) -> Vec<Line<'static>> {
    if members.is_empty() || width < 12 || max_height < 2 {
        return Vec::new();
    }
    let ordered = sort_members_for_display(members);
    let selected = selected.min(ordered.len() - 1);
    let active = members
        .iter()
        .filter(|m| is_active_status(&m.status))
        .count();
    let attention = members
        .iter()
        .filter(|m| {
            matches!(
                m.status.as_str(),
                "blocked" | "failed" | "crashed" | "waiting_network"
            )
        })
        .count();

    let mut out: Vec<Line<'static>> = Vec::new();
    out.push(dock_header(members.len(), active, attention, plan, width));

    // ---- Line budget: header (emitted) + rows + selected tail + hints ----
    let hint_rows = usize::from(focused);
    let budget = max_height.saturating_sub(1 + hint_rows);
    let n = ordered.len();
    let tail_want = if focused { 4 } else { 2 };
    // Prefer listing every agent; leftover lines become the selected tail.
    // When agents alone overflow, window around the selection and spend one
    // line on the "+N more" marker.
    let (list_rows, tail_rows) = if n <= budget {
        (n, budget.saturating_sub(n).min(tail_want))
    } else {
        (budget.saturating_sub(1).max(1), 0)
    };
    let hidden = n.saturating_sub(list_rows);
    let first = if selected >= list_rows {
        selected + 1 - list_rows
    } else {
        0
    };

    let tail_lines = if tail_rows > 0 {
        dock_tail_lines(ordered[selected], width, tail_rows)
    } else {
        Vec::new()
    };

    for (idx, member) in ordered.iter().enumerate().skip(first).take(list_rows) {
        let is_sel = idx == selected;
        out.push(dock_row(member, is_sel, focused, spinner_frame, width));
        if is_sel {
            out.extend(tail_lines.iter().cloned());
        }
    }
    if hidden > 0 {
        out.push(Line::from(Span::styled(
            format!("  +{hidden} more"),
            Style::default().fg(rgb(130, 130, 140)),
        )));
    }

    if focused {
        out.push(Line::from(vec![
            Span::styled("  j/k", Style::default().fg(rgb(150, 170, 210))),
            Span::styled(" · ", Style::default().fg(rgb(80, 80, 90))),
            Span::styled("enter", Style::default().fg(rgb(150, 170, 210))),
            Span::styled(" · ", Style::default().fg(rgb(80, 80, 90))),
            Span::styled("esc", Style::default().fg(rgb(150, 170, 210))),
        ]));
    }

    // Hard bounds: never exceed the height budget (tiny budgets can otherwise
    // overflow via the header + marker + hint fixed rows) or the width.
    out.truncate(max_height);
    for line in &mut out {
        clamp_line_to_width(line, width);
    }
    out
}

/// Render the compact swarm summary: at most two lines for the info-widget
/// margins.
///
/// ```text
/// 🐝 2/4 agents · nodes 5/12 · ⚠1
/// ▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁   (green done · yellow running · dim rest)
/// ```
///
/// Line 1 tallies active/total agents, then (width permitting, dropped
/// right-to-left) done/total task-graph nodes and an attention count. Line 2
/// is the plan progress bar: green cells are done nodes, yellow cells are
/// running nodes, dim cells are the remainder. The bar is omitted when there
/// is no plan or `max_height` < 2.
pub fn render_swarm_compact(
    members: &[GalleryMember],
    plan: Option<(u32, u32, u32)>,
    width: usize,
    max_height: usize,
) -> Vec<Line<'static>> {
    if members.is_empty() || width < 8 || max_height == 0 {
        return Vec::new();
    }
    let active = members
        .iter()
        .filter(|m| is_active_status(&m.status))
        .count();
    let attention = members
        .iter()
        .filter(|m| {
            matches!(
                m.status.as_str(),
                "blocked" | "failed" | "crashed" | "waiting_network"
            )
        })
        .count();

    let sep_style = Style::default().fg(rgb(80, 80, 90));
    let mut spans: Vec<Span<'static>> = vec![
        Span::styled("🐝 ", Style::default().fg(rgb(255, 200, 100))),
        Span::styled(
            format!("{active}/{} agents", members.len()),
            Style::default().fg(if active > 0 {
                rgb(255, 200, 100)
            } else {
                rgb(120, 120, 130)
            }),
        ),
    ];
    let mut used: usize = spans.iter().map(|s| disp_w(&s.content)).sum();
    if let Some((done, _running, total)) = plan {
        let text = format!("nodes {done}/{total}");
        if used + 3 + disp_w(&text) <= width {
            used += 3 + disp_w(&text);
            spans.push(Span::styled(" · ", sep_style));
            spans.push(Span::styled(text, Style::default().fg(rgb(160, 160, 170))));
        }
    }
    if attention > 0 {
        let text = format!("⚠{attention}");
        if used + 3 + disp_w(&text) <= width {
            spans.push(Span::styled(" · ", sep_style));
            spans.push(Span::styled(text, Style::default().fg(rgb(255, 170, 80))));
        }
    }
    let mut out = vec![Line::from(spans)];

    if let Some((done, running, total)) = plan
        && total > 0
        && max_height >= 2
    {
        out.push(plan_progress_bar(done, running, total, width));
    }

    out.truncate(max_height);
    for line in &mut out {
        clamp_line_to_width(line, width);
    }
    out
}

/// The compact widget's plan bar: green = done, yellow = running, dim = the
/// rest. Rendered as a low-profile underline (▁) rather than full-height
/// blocks. Non-empty classes always get at least one cell so tiny progress is
/// visible, and the bar never exceeds `width` cells.
fn plan_progress_bar(done: u32, running: u32, total: u32, width: usize) -> Line<'static> {
    const CELL: &str = "▁";
    let cells = width.max(1);
    let total = total.max(1) as usize;
    let done = (done as usize).min(total);
    let running = (running as usize).min(total - done);

    let mut done_w = done * cells / total;
    if done > 0 {
        done_w = done_w.clamp(1, cells);
    }
    let mut running_w = ((done + running) * cells / total).saturating_sub(done_w);
    if running > 0 {
        running_w = running_w.clamp(1, cells - done_w);
    }
    let empty_w = cells - done_w - running_w;

    let mut spans: Vec<Span<'static>> = Vec::new();
    if done_w > 0 {
        spans.push(Span::styled(
            CELL.repeat(done_w),
            Style::default().fg(rgb(100, 200, 100)),
        ));
    }
    if running_w > 0 {
        spans.push(Span::styled(
            CELL.repeat(running_w),
            Style::default().fg(rgb(255, 200, 100)),
        ));
    }
    if empty_w > 0 {
        spans.push(Span::styled(
            CELL.repeat(empty_w),
            Style::default().fg(rgb(60, 60, 70)),
        ));
    }
    Line::from(spans)
}

/// Dock header: bee + active tally, then plan progress and attention count,
/// dropped right-to-left when the width is too tight.
fn dock_header(
    total: usize,
    active: usize,
    attention: usize,
    plan: Option<(u32, u32)>,
    width: usize,
) -> Line<'static> {
    let sep_style = Style::default().fg(rgb(80, 80, 90));
    let mut spans: Vec<Span<'static>> = vec![
        Span::styled("🐝 ", Style::default().fg(rgb(255, 200, 100))),
        Span::styled(
            format!("{active}/{total} active"),
            Style::default().fg(if active > 0 {
                rgb(255, 200, 100)
            } else {
                rgb(120, 120, 130)
            }),
        ),
    ];
    let mut used: usize = spans.iter().map(|s| disp_w(&s.content)).sum();
    if let Some((done, total)) = plan {
        let text = format!("plan {done}/{total}");
        if used + 3 + disp_w(&text) <= width {
            used += 3 + disp_w(&text);
            spans.push(Span::styled(" · ", sep_style));
            spans.push(Span::styled(text, Style::default().fg(rgb(160, 160, 170))));
        }
    }
    if attention > 0 {
        let text = format!("⚠{attention}");
        if used + 3 + disp_w(&text) <= width {
            spans.push(Span::styled(" · ", sep_style));
            spans.push(Span::styled(text, Style::default().fg(rgb(255, 170, 80))));
        }
    }
    Line::from(spans)
}

/// One dock row: selection marker, optional role glyph, label, then a
/// right-aligned status glyph and optional todo counter.
fn dock_row(
    member: &GalleryMember,
    selected: bool,
    focused: bool,
    spinner_frame: usize,
    width: usize,
) -> Line<'static> {
    let accent = status_accent(&member.status);
    let marker = if selected { "▸ " } else { "  " };
    let glyph = role_glyph(member.role.as_deref())
        .map(|g| format!("{g} "))
        .unwrap_or_default();
    let status = status_glyph(&member.status, spinner_frame);
    let todo = member.todo.map(|(d, t)| format!(" {d}/{t}"));

    let right_w = disp_w(status) + todo.as_ref().map(|t| disp_w(t)).unwrap_or(0);
    let fixed = 2 + disp_w(&glyph) + 1 + right_w;
    let label = truncate_label(&member.label, width.saturating_sub(fixed).max(4));
    let filler = width
        .saturating_sub(2 + disp_w(&glyph) + disp_w(&label) + right_w)
        .max(1);

    let mut label_style = Style::default().fg(if selected {
        rgb(235, 235, 245)
    } else {
        rgb(170, 170, 180)
    });
    if selected && focused {
        label_style = label_style.add_modifier(Modifier::BOLD);
    }
    let mut spans = vec![Span::styled(
        marker.to_string(),
        Style::default().fg(if selected { accent } else { rgb(90, 90, 100) }),
    )];
    if !glyph.is_empty() {
        spans.push(Span::styled(glyph, Style::default().fg(accent)));
    }
    spans.push(Span::styled(label, label_style));
    spans.push(Span::raw(" ".repeat(filler)));
    spans.push(Span::styled(
        status.to_string(),
        Style::default().fg(accent),
    ));
    if let Some(todo) = todo {
        spans.push(Span::styled(todo, Style::default().fg(rgb(130, 130, 140))));
    }
    Line::from(spans)
}

/// The selected agent's live tail: the last `rows` non-meta body lines,
/// dim, behind a `│` gutter.
fn dock_tail_lines(member: &GalleryMember, width: usize, rows: usize) -> Vec<Line<'static>> {
    let text_budget = width.saturating_sub(4);
    member
        .body
        .iter()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('·'))
        .rev()
        .take(rows)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|l| {
            Line::from(vec![
                Span::styled("  │ ", Style::default().fg(rgb(80, 80, 90))),
                Span::styled(
                    truncate_label(l, text_budget),
                    Style::default().fg(rgb(160, 160, 170)),
                ),
            ])
        })
        .collect()
}

/// Truncate a styled line so its display width never exceeds `max_width`.
/// Splits mid-span if needed, dropping a trailing wide glyph that would
/// straddle the boundary.
fn clamp_line_to_width(line: &mut Line<'static>, max_width: usize) {
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

/// Render the expanded detail viewport for the hovered agent in the focused
/// strip: a header, a tail of the agent's live transcript, and its todo list.
///
/// Layout (at most `budget` lines, fewer when there is less content):
/// ```text
///    ⠙ researcher · thinking · 12s
///    │ Editing crates/jcode-tui/src/tui/ui.rs
///    │   carving the gallery band off chat_area
///    ├ todos 3/9
///    │ ✓ wire the bus tap
///    │ ▸ carve the gallery band
///    │ · run the ui tests
/// ```
/// Todos get at most half the budget (they are skimmable); the transcript tail
/// takes the rest. Every line is truncated to `width`.
fn render_hovered_detail(
    m: &GalleryMember,
    spinner_frame: usize,
    width: usize,
    budget: usize,
) -> Vec<Line<'static>> {
    let accent = status_accent(&m.status);
    let dim = rgb(120, 120, 130);
    const GUTTER: &str = "   ";

    // The age hint the adapter appends ('·'-prefixed meta) moves into the header.
    let age: Option<String> = m
        .body
        .iter()
        .rev()
        .find(|l| l.trim_start().starts_with('·'))
        .map(|l| l.trim().trim_start_matches('·').trim().to_string());

    let mut out: Vec<Line<'static>> = Vec::new();

    // ---- Header ----
    let mut header: Vec<Span<'static>> = vec![
        Span::raw(GUTTER),
        Span::styled(
            format!("{} {}", status_glyph(&m.status, spinner_frame), m.label),
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" · {}", m.status), Style::default().fg(dim)),
    ];
    if let Some(age) = age {
        header.push(Span::styled(format!(" · {age}"), Style::default().fg(dim)));
    }
    out.push(Line::from(header));
    out.extend(hovered_detail_body(
        m,
        Some(spinner_frame),
        width,
        budget.saturating_sub(1),
        true,
    ));
    out
}

/// The body of the hovered-agent detail viewport: a tail of the agent's live
/// transcript plus its todo list, gutter-indented, without the header row.
/// Used directly by the vertical strip (where the selected agent's row already
/// serves as the header) and by [`render_hovered_detail`].
fn hovered_detail_body(
    m: &GalleryMember,
    spinner_frame: Option<usize>,
    width: usize,
    budget: usize,
    show_member_rail: bool,
) -> Vec<Line<'static>> {
    let dim = rgb(120, 120, 130);
    let text_fg = rgb(190, 190, 200);
    let gutter_fg = rgb(80, 80, 90);
    const GUTTER: &str = "   ";
    const BAR: &str = "│   ";

    if budget == 0 {
        return Vec::new();
    }

    let mut out: Vec<Line<'static>> = Vec::new();

    // ---- Todo card ----
    // Show a sliding window of four item names. Tool activity belongs to the
    // active item and is nested immediately below it, newest at the bottom.
    if !m.todo_items.is_empty() {
        const TODO_WINDOW: usize = 4;
        const TOOL_WINDOW: usize = 3;
        let active = m
            .todo_items
            .iter()
            .position(|t| t.status == "in_progress")
            .or_else(|| m.todo_items.iter().position(|t| t.status != "completed"))
            .unwrap_or_else(|| m.todo_items.len().saturating_sub(1));
        let shown = TODO_WINDOW.min(m.todo_items.len());
        let start = active
            .saturating_sub(1)
            .min(m.todo_items.len().saturating_sub(shown));
        let text_budget = width.saturating_sub(GUTTER.len() + BAR.len() + 2);

        for (visible_idx, (idx, todo)) in m
            .todo_items
            .iter()
            .enumerate()
            .skip(start)
            .take(shown)
            .enumerate()
        {
            if out.len() >= budget {
                break;
            }
            let (glyph, glyph_fg, emph) = match todo.status.as_str() {
                "completed" => ("✓".to_string(), rgb(100, 200, 100), false),
                "in_progress" => (
                    spinner_frame
                        .map(|frame| status_glyph("running", frame))
                        .unwrap_or("●")
                        .to_string(),
                    rgb(255, 200, 100),
                    true,
                ),
                _ => ("○".to_string(), dim, false),
            };
            let mut style = Style::default().fg(if emph { text_fg } else { dim });
            if emph {
                style = style.add_modifier(Modifier::BOLD);
            }
            let todo_rail = if !show_member_rail {
                "    "
            } else if visible_idx + 1 == shown {
                "└─  "
            } else {
                BAR
            };
            out.push(Line::from(vec![
                Span::raw(GUTTER),
                Span::styled(todo_rail, Style::default().fg(gutter_fg)),
                Span::styled(format!("{glyph} "), Style::default().fg(glyph_fg)),
                Span::styled(truncate_label(&todo.content, text_budget), style),
            ]));

            if idx == active {
                let tools: Vec<&GalleryToolIntent> = todo
                    .tool_intents
                    .iter()
                    .rev()
                    .take(TOOL_WINDOW)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                let tool_count = tools.len();
                for (tool_idx, tool) in tools.into_iter().enumerate() {
                    if out.len() >= budget {
                        break;
                    }
                    let (tool_glyph, fg) = match tool.status.as_str() {
                        "running" => (
                            spinner_frame
                                .map(|frame| status_glyph("running", frame))
                                .unwrap_or("●"),
                            rgb(255, 200, 100),
                        ),
                        "error" => ("✗", rgb(230, 100, 100)),
                        _ => ("✓", rgb(100, 200, 100)),
                    };
                    let branch = if tool_idx + 1 == tool_count {
                        "└─"
                    } else {
                        "├─"
                    };
                    let progress = tool
                        .progress
                        .as_ref()
                        .map(|(current, total, _unit)| format!(" · {current}/{total}"))
                        .unwrap_or_default();
                    let label = format!("{} · {}{progress}", tool.tool_name, tool.intent);
                    let nested_budget = width.saturating_sub(GUTTER.len() + BAR.len() + 7);
                    out.push(Line::from(vec![
                        Span::raw(GUTTER),
                        Span::styled(
                            if show_member_rail {
                                "│     "
                            } else {
                                "      "
                            },
                            Style::default().fg(gutter_fg),
                        ),
                        Span::styled(format!("{branch} "), Style::default().fg(gutter_fg)),
                        Span::styled(format!("{tool_glyph} "), Style::default().fg(fg)),
                        Span::styled(
                            truncate_label(&label, nested_budget),
                            Style::default().fg(rgb(155, 155, 165)),
                        ),
                    ]));
                }
            }
        }
        return out;
    }

    // No todos yet: retain the live transcript fallback.
    let transcript: Vec<&str> = m
        .body
        .iter()
        .map(|l| l.as_str())
        .filter(|l| !l.trim_start().starts_with('·'))
        .collect();
    let text_budget = width.saturating_sub(GUTTER.len() + BAR.len());
    let shown: Vec<&str> = transcript
        .iter()
        .copied()
        .rev()
        .take(budget)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if shown.iter().all(|l| l.trim().is_empty()) {
        out.push(Line::from(vec![
            Span::raw(GUTTER),
            Span::styled(
                if show_member_rail { BAR } else { "    " },
                Style::default().fg(gutter_fg),
            ),
            Span::styled(format!("[{}]", m.status), Style::default().fg(dim)),
        ]));
    } else {
        for line in shown {
            out.push(Line::from(vec![
                Span::raw(GUTTER),
                Span::styled(
                    if show_member_rail { BAR } else { "    " },
                    Style::default().fg(gutter_fg),
                ),
                Span::styled(
                    truncate_label(line, text_budget),
                    Style::default().fg(text_fg),
                ),
            ]));
        }
    }

    out
}
fn panel_header(total: usize, active: usize, focused: bool) -> Line<'static> {
    let mut spans = vec![
        Span::styled("🐝 ", Style::default().fg(rgb(255, 200, 100))),
        Span::styled(
            format!(
                "{} agent{}{}",
                total,
                if total == 1 { "" } else { "s" },
                if active > 0 {
                    format!(" · {active} active")
                } else {
                    String::new()
                }
            ),
            Style::default().fg(rgb(160, 160, 170)),
        ),
    ];
    if focused {
        spans.push(Span::styled(
            "  (j/k select · o pop out · esc)",
            Style::default().fg(rgb(110, 110, 120)),
        ));
    }
    Line::from(spans)
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
fn sort_members_for_display(members: &[GalleryMember]) -> Vec<&GalleryMember> {
    display_order(members)
        .into_iter()
        .map(|i| &members[i])
        .collect()
}

/// The tile index (in `members_to_tiles(members)` order) for a display row.
/// Since both orderings use the same sort, the display index equals the tile
/// index, but resolve via sort_key to stay correct if that ever diverges.
fn display_index_to_tile_index(
    ordered: &[&GalleryMember],
    _members: &[GalleryMember],
    display_idx: usize,
) -> usize {
    // tiles are produced by the same sort, so display order == tile order.
    let _ = ordered;
    display_idx
}

/// One row in the agent list: a selection marker, optional role glyph, the
/// label, a status badge, and an age hint, all bounded to `width`.
fn list_row(member: &GalleryMember, selected: bool, focused: bool, width: usize) -> Line<'static> {
    let accent = status_accent(&member.status);
    let marker = if selected { "▸ " } else { "  " };
    let glyph = role_glyph(member.role.as_deref())
        .map(|g| format!("{g} "))
        .unwrap_or_default();

    // Badge + age live on the right; build them first to know how much room the
    // label gets.
    let badge = format!("[{}]", member.status);
    let age = member
        .body
        .iter()
        .rev()
        .find_map(|l| l.strip_prefix("· ").map(|s| s.trim_end_matches(" ago")))
        .map(|a| a.to_string());

    let marker_w = 2;
    let glyph_w = disp_w(&glyph);
    let badge_w = disp_w(&badge);
    let age_w = age.as_ref().map(|a| disp_w(a) + 1).unwrap_or(0);
    // Reserve: marker + glyph + label + space + badge + space + age.
    let reserved = marker_w + glyph_w + 1 + badge_w + age_w + 1;
    let label_budget = width.saturating_sub(reserved).max(4);
    let label = truncate_label(&member.label, label_budget);
    let label_w = disp_w(&label);

    let label_style = if selected {
        Style::default().fg(rgb(235, 235, 245))
    } else {
        Style::default().fg(rgb(170, 170, 180))
    };
    let marker_style = if selected && focused {
        Style::default().fg(accent)
    } else if selected {
        Style::default().fg(rgb(150, 150, 160))
    } else {
        Style::default().fg(rgb(90, 90, 100))
    };

    // Compute filler so the badge/age right-align.
    let used = marker_w + glyph_w + label_w;
    let right_w = badge_w + age_w;
    let filler = width.saturating_sub(used + right_w).max(1);

    let mut spans = vec![Span::styled(marker.to_string(), marker_style)];
    if !glyph.is_empty() {
        spans.push(Span::styled(glyph, Style::default().fg(accent)));
    }
    spans.push(Span::styled(label, label_style));
    spans.push(Span::raw(" ".repeat(filler)));
    spans.push(Span::styled(badge, Style::default().fg(accent)));
    if let Some(age) = age {
        spans.push(Span::styled(
            format!(" {age}"),
            Style::default().fg(rgb(110, 110, 120)),
        ));
    }
    Line::from(spans)
}

/// Truncate `s` to at most `max` display columns (wide glyphs count as 2),
/// appending an ellipsis when truncated.
fn truncate_label(s: &str, max: usize) -> String {
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
fn disp_w(s: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    s.width()
}

/// Number of decimal digits in `n` (for budgeting "+N" markers).
fn count_digits(n: usize) -> usize {
    let mut n = n.max(1);
    let mut d = 0;
    while n > 0 {
        d += 1;
        n /= 10;
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(id: &str, status: &str, role: Option<&str>, body: &[&str]) -> GalleryMember {
        GalleryMember {
            label: id.to_string(),
            icon: None,
            status: status.to_string(),
            task: None,
            role: role.map(str::to_string),
            body: body.iter().map(|s| s.to_string()).collect(),
            sort_key: id.to_string(),
            todo: None,
            todo_items: Vec::new(),
            model: None,
            provider: None,
            auth_method: None,
            effort: None,
            elapsed_secs: None,
        }
    }

    #[test]
    fn coordinator_sorts_first() {
        let members = vec![
            member("zeta", "running", None, &[]),
            member("alpha", "running", Some("coordinator"), &[]),
        ];
        let tiles = members_to_tiles(&members);
        assert_eq!(tiles[0].title, "alpha");
        assert_eq!(tiles[0].role_glyph.as_deref(), Some("★"));
    }

    /// `display_order` is the contract callers (e.g. pop-out selection) use to
    /// map a displayed row back to an input member, so it must match tile
    /// order exactly, including for mixed/unknown roles and sort_key ties.
    #[test]
    fn display_order_matches_tile_order_for_mixed_members() {
        let mut members = vec![
            member("zeta", "running", None, &[]),
            member("mid", "done", Some("mystery_role_2"), &[]),
            member("boss", "running", Some("coordinator"), &[]),
            member("alpha", "thinking", Some("mystery_role"), &[]),
            member("beta", "failed", None, &[]),
        ];
        // Full tie with "beta" on both role rank and sort_key.
        let mut dup = member("beta-label-2", "ready", None, &[]);
        dup.sort_key = "beta".to_string();
        members.push(dup);

        let order = display_order(&members);
        assert_eq!(order.len(), members.len());
        let ordered_labels: Vec<String> = order.iter().map(|&i| members[i].label.clone()).collect();
        let tile_titles: Vec<String> = members_to_tiles(&members)
            .into_iter()
            .map(|t| t.title)
            .collect();
        assert_eq!(ordered_labels, tile_titles);
        let ordered_labels: Vec<&str> = ordered_labels.iter().map(String::as_str).collect();
        // Role buckets come first (coordinator only), then active-before-
        // finished status order, then sort_key within the same status rank.
        assert_eq!(ordered_labels[0], "boss");
        assert_eq!(
            &ordered_labels[1..],
            &["alpha", "zeta", "beta", "beta-label-2", "mid"]
        );
    }

    /// Active agents must sort ahead of finished ones within a role bucket, so
    /// they stay visible on the strip instead of hiding in the "+N" overflow.
    #[test]
    fn display_order_puts_active_agents_before_finished_ones() {
        let members = vec![
            member("ant", "completed", None, &[]),
            member("bat", "done", None, &[]),
            member("crab", "running", None, &[]),
            member("dove", "failed", None, &[]),
            member("elk", "ready", None, &[]),
            member("fox", "thinking", None, &[]),
            member("gnu", "stopped", None, &[]),
            member("hen", "blocked", None, &[]),
        ];
        let order = display_order(&members);
        let labels: Vec<&str> = order.iter().map(|&i| members[i].label.as_str()).collect();
        assert_eq!(
            labels,
            // active (crab, fox) < attention (dove, hen) < idle (elk)
            // < finished (ant, bat, gnu), each bucket sorted by sort_key.
            ["crab", "fox", "dove", "hen", "elk", "ant", "bat", "gnu"]
        );
    }

    #[test]
    fn renders_header_and_is_width_bounded() {
        let members = vec![
            member("alpha", "running", None, &["editing config.rs"]),
            member("beta", "done", None, &["reviewed"]),
        ];
        let lines = render_gallery(&members, 80, 12);
        assert!(!lines.is_empty());
        let header: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(header.contains("🐝 2 agents · 1 active"), "got: {header}");
        assert!(!header.contains("swarm"), "got: {header}");
        for line in &lines {
            assert!(line.width() <= 80);
        }
    }

    #[test]
    fn active_count_in_header() {
        let members = vec![
            member("a", "running", None, &[]),
            member("b", "thinking", None, &[]),
            member("c", "done", None, &[]),
        ];
        let lines = render_gallery(&members, 100, 12);
        let header: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(header.contains("2 active"), "got: {header}");
    }

    #[test]
    fn empty_members_render_nothing() {
        assert!(render_gallery(&[], 80, 12).is_empty());
    }

    #[test]
    fn humanize_age_buckets() {
        assert_eq!(humanize_age(0), "now");
        assert_eq!(humanize_age(5), "5s");
        assert_eq!(humanize_age(120), "2m");
        assert_eq!(humanize_age(7200), "2h");
    }

    fn plain_line(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn panel_empty_renders_nothing() {
        assert!(render_swarm_panel(&[], 0, true, 60, 12).is_empty());
    }

    #[test]
    fn panel_lists_all_agents_and_is_width_bounded() {
        let members = vec![
            member("researcher", "thinking", Some("coordinator"), &["· 1s ago"]),
            member("implementer", "running", None, &["building", "· 3s ago"]),
            member("reviewer", "done", None, &["LGTM", "· 1m ago"]),
        ];
        let lines = render_swarm_panel(&members, 0, true, 70, 14);
        assert!(!lines.is_empty());
        for line in &lines {
            assert!(line.width() <= 70, "line too wide: {}", plain_line(line));
        }
        let header = plain_line(&lines[0]);
        assert!(header.contains("🐝 3 agents"), "got: {header}");
        assert!(!header.contains("swarm"), "got: {header}");
        // Every agent label appears as a list row.
        let joined: String = lines.iter().map(plain_line).collect::<Vec<_>>().join("\n");
        for name in ["researcher", "implementer", "reviewer"] {
            assert!(joined.contains(name), "missing {name} in:\n{joined}");
        }
    }

    #[test]
    fn panel_marks_selected_row() {
        let members = vec![
            member("a", "running", Some("coordinator"), &[]),
            member("b", "running", None, &[]),
        ];
        // After sort, coordinator "a" is index 0; selecting 1 marks "b".
        let lines = render_swarm_panel(&members, 1, true, 60, 14);
        let selected_row = lines
            .iter()
            .map(plain_line)
            .find(|l| l.contains('▸'))
            .expect("a row should be marked selected");
        assert!(selected_row.contains('b'), "got: {selected_row}");
    }

    #[test]
    fn panel_detail_shows_selected_agent_body() {
        let members = vec![
            member("a", "running", Some("coordinator"), &["alpha work"]),
            member("b", "running", None, &["beta output here"]),
        ];
        let lines = render_swarm_panel(&members, 1, true, 60, 14);
        let joined: String = lines.iter().map(plain_line).collect::<Vec<_>>().join("\n");
        // The detail viewport (bordered box) shows the selected agent's tail.
        assert!(joined.contains("beta output here"), "got:\n{joined}");
        // And a bordered box was drawn.
        assert!(
            joined.contains('╭') && joined.contains('╰'),
            "got:\n{joined}"
        );
    }

    #[test]
    fn panel_clamps_out_of_range_selection() {
        let members = vec![member("only", "running", None, &["x"])];
        // selected far beyond range must not panic and still render.
        let lines = render_swarm_panel(&members, 99, true, 40, 12);
        assert!(!lines.is_empty());
    }

    #[test]
    fn panel_focus_hint_only_when_focused() {
        let members = vec![member("a", "running", None, &[])];
        let focused = plain_line(&render_swarm_panel(&members, 0, true, 60, 12)[0]);
        let unfocused = plain_line(&render_swarm_panel(&members, 0, false, 60, 12)[0]);
        assert!(focused.contains("pop out"), "got: {focused}");
        assert!(!unfocused.contains("pop out"), "got: {unfocused}");
    }

    fn hints() -> Vec<SwarmStripHint> {
        vec![
            SwarmStripHint {
                key: "alt+n".into(),
                label: "focus".into(),
            },
            SwarmStripHint {
                key: "j/k".into(),
                label: "select".into(),
            },
            SwarmStripHint {
                key: "o".into(),
                label: "pop out".into(),
            },
            SwarmStripHint {
                key: "esc".into(),
                label: "back".into(),
            },
        ]
    }

    #[test]
    fn strip_empty_renders_nothing() {
        assert!(render_swarm_strip(&[], 0, true, &hints(), None, 0, 80, 12).is_empty());
    }

    #[test]
    fn vertical_strip_empty_renders_nothing() {
        assert!(render_swarm_strip_vertical(&[], 0, true, &hints(), None, 0, 80, 4, 12).is_empty());
    }

    #[test]
    fn vertical_strip_lists_one_agent_per_row_with_icon_and_task() {
        let mut a = member("fox", "running", None, &[]);
        a.icon = Some("🦊".to_string());
        a.task = Some("wire the auth flow".to_string());
        a.todo = Some((3, 9));
        let mut b = member("bee", "completed", None, &[]);
        b.icon = Some("🐝".to_string());
        b.task = Some("audit the webhook path".to_string());
        let lines = render_swarm_strip_vertical(
            &[a, b],
            0,
            false,
            &hints(),
            Some("alt+n controls"),
            0,
            90,
            4,
            12,
        );
        assert_eq!(lines.len(), 2, "one row per agent");
        let row0 = plain_line(&lines[0]);
        let row1 = plain_line(&lines[1]);
        assert!(
            row0.contains("🐝"),
            "first row carries the swarm marker: {row0:?}"
        );
        assert!(row0.contains("🦊"), "icon replaces the name: {row0:?}");
        assert!(
            !row0.contains("fox"),
            "name hidden when icon present: {row0:?}"
        );
        assert!(row0.contains("wire the auth flow"), "task shown: {row0:?}");
        assert!(row0.contains("3/9"), "todo counter shown: {row0:?}");
        assert!(row0.contains("1/2 active"), "tally on first row: {row0:?}");
        assert!(
            row0.contains("alt+n controls"),
            "hint on first row: {row0:?}"
        );
        assert!(
            row1.contains("audit the webhook path"),
            "second agent row: {row1:?}"
        );
        assert!(
            !row1.contains("active"),
            "tally only on first row: {row1:?}"
        );
        for line in &lines {
            assert!(line.width() <= 90);
        }
    }

    #[test]
    fn vertical_strip_falls_back_to_name_without_icon() {
        let members = vec![member("zeta", "running", None, &[])];
        let lines = render_swarm_strip_vertical(&members, 0, false, &hints(), None, 0, 80, 4, 12);
        assert!(plain_line(&lines[0]).contains("zeta"));
    }

    #[test]
    fn vertical_strip_caps_rows_and_reports_overflow() {
        let members: Vec<GalleryMember> = (0..7)
            .map(|i| member(&format!("agent{i}"), "running", None, &[]))
            .collect();
        let lines = render_swarm_strip_vertical(&members, 0, false, &hints(), None, 0, 80, 4, 12);
        assert_eq!(lines.len(), 4, "capped to max_rows lines");
        let last = plain_line(&lines[3]);
        assert!(last.contains("+4 more"), "overflow marker: {last:?}");
    }

    #[test]
    fn vertical_strip_windows_to_keep_selection_visible() {
        let members: Vec<GalleryMember> = (0..7)
            .map(|i| member(&format!("agent{i}"), "running", None, &[]))
            .collect();
        let lines = render_swarm_strip_vertical(&members, 6, true, &[], None, 0, 80, 4, 4);
        let text: String = lines.iter().map(plain_line).collect();
        assert!(text.contains("agent6"), "selected agent visible: {text:?}");
    }

    #[test]
    fn vertical_strip_focused_expands_accordion_under_selected_row() {
        let mut fox = member("fox", "running", None, &["compiling the renderer"]);
        fox.icon = Some("🦊".to_string());
        let mut bee = member("bee", "ready", None, &["waiting for work"]);
        bee.icon = Some("🐝".to_string());
        let lines = render_swarm_strip_vertical(&[fox, bee], 1, true, &hints(), None, 0, 80, 4, 14);
        let texts: Vec<String> = lines.iter().map(plain_line).collect();
        let all = texts.join("\n");
        // Selected agent (bee, sorted second: fox is active and sorts first)
        // shows marker + icon without repeating the animal name; its detail
        // slides in directly beneath.
        let bee_row = texts
            .iter()
            .position(|l| l.contains("▸") && l.contains("🐝"))
            .expect("selected row shows marker and icon: {texts:?}");
        assert!(
            !texts[bee_row].contains("bee"),
            "selected compact row should stay emoji-only: {texts:?}"
        );
        assert!(
            texts[bee_row + 1].contains("waiting for work"),
            "detail expands directly under the selected row: {texts:?}"
        );
        // Unselected agent stays icon-only.
        assert!(
            !texts.iter().any(|l| l.contains("fox") && !l.contains("▸")),
            "unselected agents stay icon-only: {texts:?}"
        );
        assert!(
            !all.contains("compiling the renderer"),
            "unselected agent's transcript stays collapsed: {all:?}"
        );
        assert!(all.contains("pop out"), "hint line shown: {all:?}");
    }

    #[test]
    fn chat_card_is_stable_while_live_card_shows_details() {
        let mut worker = member("reviewer", "running", None, &[]);
        worker.icon = Some("🦕".to_string());
        worker.task = Some("review authentication changes".to_string());
        worker.todo = Some((2, 4));
        worker.model = Some("openai:gpt-5.6-sol".into());
        worker.provider = Some("OpenAI".into());
        worker.auth_method = Some("OAuth".into());
        worker.effort = Some("high".into());
        worker.elapsed_secs = Some(18);
        worker.todo_items = vec![
            GalleryTodo {
                content: "inspect middleware".into(),
                status: "completed".into(),
                tool_intents: Vec::new(),
            },
            GalleryTodo {
                content: "test token refresh flow".into(),
                status: "in_progress".into(),
                tool_intents: vec![
                    GalleryToolIntent {
                        tool_name: "read".into(),
                        intent: "Old intent that should roll off".into(),
                        status: "completed".into(),
                        progress: None,
                    },
                    GalleryToolIntent {
                        tool_name: "agentgrep".into(),
                        intent: "Locate refresh implementation".into(),
                        status: "completed".into(),
                        progress: None,
                    },
                    GalleryToolIntent {
                        tool_name: "read".into(),
                        intent: "Inspect refresh-token handler".into(),
                        status: "completed".into(),
                        progress: None,
                    },
                    GalleryToolIntent {
                        tool_name: "bash".into(),
                        intent: "Run targeted authentication tests".into(),
                        status: "running".into(),
                        progress: Some((27, 43, Some("tests".into()))),
                    },
                ],
            },
            GalleryTodo {
                content: "check secure cookie configuration".into(),
                status: "pending".into(),
                tool_intents: Vec::new(),
            },
            GalleryTodo {
                content: "report findings".into(),
                status: "pending".into(),
                tool_intents: Vec::new(),
            },
        ];

        let lines = render_swarm_chat_cards(std::slice::from_ref(&worker), 100);
        let all = lines.iter().map(plain_line).collect::<Vec<_>>().join("\n");
        assert_eq!(lines.len(), 1, "chat summary must remain one line: {all}");
        assert!(
            all.contains("🦕 ● reviewer"),
            "assigned agent emoji missing: {all}"
        );
        assert!(
            all.contains("reviewer · Working · GPT-5.6 · OpenAI OAuth"),
            "stable status/model/route metadata missing: {all}"
        );
        assert!(
            STRIP_SPINNER_FRAMES
                .iter()
                .all(|frame| !all.contains(frame)),
            "transcript cards must not animate while being read: {all}"
        );
        assert!(
            !all.contains("00:18")
                && !all.contains("test token refresh flow")
                && !all.contains("Run targeted authentication tests")
                && !all.contains("27/43"),
            "chat summary must exclude frequently changing live details: {all}"
        );

        let live_lines = render_swarm_live_card(&worker, 0, 100, 12);
        let live = live_lines
            .iter()
            .map(plain_line)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            live.contains("reviewer · 00:18 · GPT-5.6 · OpenAI OAuth · high"),
            "live header metadata missing: {live}"
        );
        assert!(
            live.contains("test token refresh flow")
                && live.contains("bash · Run targeted authentication tests · 27/43"),
            "live page should retain todo and tool progress: {live}"
        );
    }

    /// The transcript card's model/route metadata must degrade gracefully:
    /// narrower widths drop trailing pieces rather than overflowing, and a
    /// member with no known runtime still renders its status label.
    #[test]
    fn chat_card_metadata_degrades_and_is_width_bounded() {
        let mut worker = member("reviewer", "running", None, &[]);
        worker.icon = Some("🦕".to_string());
        worker.model = Some("openai:gpt-5.6-sol".into());
        worker.provider = Some("OpenAI".into());
        worker.auth_method = Some("OAuth".into());

        for width in 0..80usize {
            for line in render_swarm_chat_cards(std::slice::from_ref(&worker), width) {
                assert!(
                    disp_w(&plain_line(&line)) <= width,
                    "chat card exceeded width {width}: {:?}",
                    plain_line(&line)
                );
            }
        }

        // Wide: everything fits. Narrow: route drops before model, model before
        // status, and the label always survives.
        let wide = plain_line(&render_swarm_chat_cards(std::slice::from_ref(&worker), 100)[0]);
        assert!(wide.contains("Working · GPT-5.6 · OpenAI OAuth"), "{wide}");
        let mid = plain_line(&render_swarm_chat_cards(std::slice::from_ref(&worker), 34)[0]);
        assert!(
            mid.contains("Working") && !mid.contains("OpenAI OAuth"),
            "{mid}"
        );

        let mut bare = member("plain", "running", None, &[]);
        bare.icon = Some("🦕".to_string());
        let bare_line = plain_line(&render_swarm_chat_cards(std::slice::from_ref(&bare), 100)[0]);
        assert_eq!(bare_line.trim(), "🦕 ● plain · Working");
    }

    #[test]
    fn vertical_strip_unfocused_keeps_live_card_details_in_chat() {
        let mut worker = member("reviewer", "running", None, &[]);
        worker.icon = Some("🐝".to_string());
        worker.task = Some("review authentication changes".to_string());
        worker.todo = Some((2, 4));
        worker.todo_items = vec![GalleryTodo {
            content: "test token refresh flow".into(),
            status: "in_progress".into(),
            tool_intents: Vec::new(),
        }];

        let lines = render_swarm_strip_vertical(
            &[worker],
            0,
            false,
            &hints(),
            Some("alt+n controls"),
            2,
            100,
            4,
            16,
        );
        let all = lines.iter().map(plain_line).collect::<Vec<_>>().join("\n");
        assert_eq!(lines.len(), 1, "unfocused strip stays compact: {all}");
        assert!(all.contains("🐝"), "assigned icon missing: {all}");
        assert!(
            !all.contains("reviewer"),
            "compact strip should not repeat the animal name: {all}"
        );
        assert!(
            !all.contains("test token refresh flow"),
            "details leaked: {all}"
        );
    }

    /// Focused vertical strip: plain typing must never be part of the deal.
    /// (Key mapping lives in the TUI crate; this just pins that the strip
    /// renders hint labels for the alt-chord scheme it advertises.)
    #[test]
    fn vertical_strip_focused_detail_stays_within_height_budget() {
        let mut m = member(
            "fox",
            "running",
            None,
            &["line 1", "line 2", "line 3", "line 4", "line 5", "line 6"],
        );
        m.todo_items = vec![
            GalleryTodo {
                content: "first".into(),
                status: "completed".into(),
                tool_intents: Vec::new(),
            },
            GalleryTodo {
                content: "second".into(),
                status: "in_progress".into(),
                tool_intents: Vec::new(),
            },
        ];
        let members = vec![m, member("bee", "ready", None, &[])];
        for max_height in 3..=14 {
            let lines = render_swarm_strip_vertical(
                &members,
                0,
                true,
                &hints(),
                None,
                0,
                80,
                4,
                max_height,
            );
            assert!(
                lines.len() <= max_height,
                "focused vertical strip exceeded max_height {max_height}: {} lines",
                lines.len()
            );
        }
    }

    #[test]
    fn vertical_strip_is_display_width_bounded_at_all_widths() {
        let mut wide = member("調整役エージェント", "running", Some("coordinator"), &[]);
        wide.icon = Some("🐯".to_string());
        wide.task = Some("寬字元邊界の検証テスト for the renderer".to_string());
        wide.todo = Some((12, 34));
        let members = vec![
            wide,
            member("bee", "completed", None, &[]),
            member("fox", "failed", None, &[]),
            member("owl", "thinking", None, &[]),
            member("ram", "ready", None, &[]),
        ];
        for width in 0..=120 {
            for focused in [false, true] {
                let lines = render_swarm_strip_vertical(
                    &members,
                    2,
                    focused,
                    &hints(),
                    Some("alt+n controls"),
                    3,
                    width,
                    4,
                    12,
                );
                for line in &lines {
                    assert!(
                        line.width() <= width,
                        "line wider than {width}: {:?}",
                        plain_line(line)
                    );
                }
            }
        }
    }

    #[test]
    fn strip_one_line_when_unfocused_expands_when_focused() {
        let members = vec![
            member("researcher", "thinking", Some("coordinator"), &["working"]),
            member("implementer", "running", None, &["building"]),
        ];
        let unfocused = render_swarm_strip(&members, 0, false, &hints(), None, 0, 80, 12);
        assert_eq!(
            unfocused.len(),
            1,
            "unfocused strip should be a single line"
        );
        // Focused: chips line + expanded detail viewport + hint line, bounded
        // by the max_height budget.
        let focused = render_swarm_strip(&members, 0, true, &hints(), None, 0, 80, 12);
        assert!(
            focused.len() > 3,
            "focused strip should expand into a detail viewport, got {} lines",
            focused.len()
        );
        assert!(focused.len() <= 12, "focused strip must respect max_height");
        // With a tiny budget the focused strip degrades to the compact
        // 3-line form (chips + one detail line + hints).
        let tiny = render_swarm_strip(&members, 0, true, &hints(), None, 0, 80, 3);
        assert_eq!(tiny.len(), 3, "tiny budget should degrade to 3 lines");
    }

    #[test]
    fn strip_focused_detail_prioritizes_todo_names() {
        let mut m = member(
            "researcher",
            "thinking",
            Some("coordinator"),
            &["editing ui.rs", "running tests now"],
        );
        m.todo = Some((1, 3));
        m.todo_items = vec![
            GalleryTodo {
                content: "wire the bus tap".into(),
                status: "completed".into(),
                tool_intents: Vec::new(),
            },
            GalleryTodo {
                content: "carve the gallery band".into(),
                status: "in_progress".into(),
                tool_intents: Vec::new(),
            },
            GalleryTodo {
                content: "run the ui tests".into(),
                status: "pending".into(),
                tool_intents: Vec::new(),
            },
        ];
        let lines = render_swarm_strip(&[m], 0, true, &hints(), None, 0, 80, 14);
        let text: Vec<String> = lines.iter().map(plain_line).collect();
        let all = text.join("\n");
        // Todo names replace the less actionable transcript tail when present.
        assert!(!all.contains("editing ui.rs"), "got: {all}");
        assert!(!all.contains("running tests now"), "got: {all}");
        assert!(all.contains("carve the gallery band"), "got: {all}");
        assert!(all.contains("run the ui tests"), "got: {all}");
        // Hint line still present.
        assert!(all.contains("pop out"), "got: {all}");
        for line in &lines {
            assert!(line.width() <= 80, "line too wide: {}", plain_line(line));
        }
    }

    #[test]
    fn strip_shows_agents_and_tally_and_is_width_bounded() {
        let members = vec![
            member("researcher", "thinking", Some("coordinator"), &["working"]),
            member("implementer", "running", None, &["building"]),
            member("reviewer", "done", None, &["done"]),
        ];
        let lines = render_swarm_strip(&members, 1, true, &hints(), None, 0, 90, 12);
        for line in &lines {
            assert!(line.width() <= 90, "line too wide: {}", plain_line(line));
        }
        let chips = plain_line(&lines[0]);
        assert!(chips.contains("researcher"), "got: {chips}");
        assert!(chips.contains("implementer"), "got: {chips}");
        assert!(chips.contains("2/3 active"), "tally missing: {chips}");
        // Hint line carries the keybindings.
        let hint = plain_line(lines.last().unwrap());
        assert!(hint.contains("pop out"), "got: {hint}");
        assert!(hint.contains("select"), "got: {hint}");
    }

    #[test]
    fn strip_unfocused_shows_enter_controls_hint() {
        let members = vec![member("a", "running", None, &[])];
        let lines = render_swarm_strip(
            &members,
            0,
            false,
            &hints(),
            Some("alt+n controls"),
            0,
            90,
            12,
        );
        let chips = plain_line(&lines[0]);
        assert!(chips.contains("alt+n controls"), "got: {chips}");
    }

    #[test]
    fn strip_shows_todo_counter() {
        let mut m = member("worker", "running", None, &["step"]);
        m.todo = Some((8, 16));
        let lines = render_swarm_strip(&[m], 0, false, &hints(), None, 0, 90, 12);
        let chips = plain_line(&lines[0]);
        assert!(chips.contains("8/16"), "todo counter missing: {chips}");
    }

    #[test]
    fn strip_shows_task_label_when_width_allows() {
        let mut a = member("fox", "running", None, &[]);
        a.task = Some("fix parser".to_string());
        let mut b = member("owl", "running", None, &[]);
        b.task = Some("write docs".to_string());
        let lines = render_swarm_strip(&[a, b], 0, false, &hints(), None, 0, 100, 12);
        let chips = plain_line(&lines[0]);
        assert!(chips.contains("fox·fix parser"), "got: {chips}");
        assert!(chips.contains("owl·write docs"), "got: {chips}");
        assert!(lines[0].width() <= 100);
    }

    #[test]
    fn strip_drops_task_labels_before_hiding_agents() {
        // Same members with and without long task labels: labels are additive
        // only, so they must never reduce how many agents are visible.
        let base: Vec<GalleryMember> = (0..8)
            .map(|i| member(&format!("agent{i}"), "running", None, &[]))
            .collect();
        let with_tasks: Vec<GalleryMember> = base
            .iter()
            .map(|m| {
                let mut m = m.clone();
                m.task = Some("a very long task description that would eat the line".into());
                m
            })
            .collect();
        for width in [40usize, 60, 90, 120] {
            let plain =
                plain_line(&render_swarm_strip(&base, 0, false, &hints(), None, 0, width, 12)[0]);
            let labeled_lines =
                render_swarm_strip(&with_tasks, 0, false, &hints(), None, 0, width, 12);
            let labeled = plain_line(&labeled_lines[0]);
            assert!(labeled_lines[0].width() <= width);
            let count = |s: &str| (0..8).filter(|i| s.contains(&format!("agent{i}"))).count();
            assert_eq!(
                count(&plain),
                count(&labeled),
                "labels hid agents at width {width}: plain={plain} labeled={labeled}"
            );
        }
    }

    #[test]
    fn strip_task_labels_never_break_width_bound_across_widths() {
        let members: Vec<GalleryMember> = (0..5)
            .map(|i| {
                let mut m = member(&format!("worker-{i}"), "running", None, &[]);
                m.task = Some(format!("task {i}: refactor the swarm gallery renderer"));
                m.todo = Some((i as u32, 9));
                m
            })
            .collect();
        for width in 8..200 {
            let lines = render_swarm_strip(&members, 2, false, &hints(), None, 0, width, 12);
            for line in &lines {
                assert!(
                    line.width() <= width,
                    "width {width} exceeded: {}",
                    plain_line(line)
                );
            }
        }
    }

    #[test]
    fn strip_overflow_collapses_to_more_count() {
        let members: Vec<GalleryMember> = (0..12)
            .map(|i| member(&format!("agent-number-{i:02}"), "running", None, &[]))
            .collect();
        let lines = render_swarm_strip(&members, 0, false, &hints(), None, 0, 50, 12);
        assert!(lines[0].width() <= 50, "too wide");
        let chips = plain_line(&lines[0]);
        assert!(chips.contains('+'), "expected +N overflow marker: {chips}");
    }

    #[test]
    fn strip_is_display_width_bounded_at_all_widths() {
        use unicode_width::UnicodeWidthStr;
        let mut members: Vec<GalleryMember> = (0..9)
            .map(|i| member(&format!("agent-{i}"), "running", None, &["working"]))
            .collect();
        members[0].todo = Some((3, 8));
        members[0].role = Some("coordinator".into());
        for width in 8..=140 {
            for focused in [false, true] {
                let lines = render_swarm_strip(
                    &members,
                    2,
                    focused,
                    &hints(),
                    Some("ctrl+shift+tab controls"),
                    0,
                    width,
                    12,
                );
                for line in &lines {
                    let text = plain_line(line);
                    let w = text.as_str().width();
                    assert!(
                        w <= width,
                        "width {width} focused {focused}: line overflows ({w}): {text:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn strip_drops_hint_before_tally_when_narrow() {
        let members: Vec<GalleryMember> = (0..6)
            .map(|i| member(&format!("agent-{i}"), "running", None, &[]))
            .collect();
        // Narrow enough that the long hint cannot fit, but the tally can.
        let lines = render_swarm_strip(
            &members,
            0,
            false,
            &hints(),
            Some("ctrl+shift+tab controls"),
            0,
            60,
            12,
        );
        let chips = plain_line(&lines[0]);
        assert!(chips.contains("6/6 active"), "tally missing: {chips}");
        assert!(!chips.contains("controls"), "hint should drop: {chips}");
    }

    #[test]
    fn panel_is_display_width_bounded_with_wide_labels() {
        use unicode_width::UnicodeWidthStr;
        let members = vec![
            member(
                "日本語のエージェント名前が長い場合の確認",
                "running",
                None,
                &["· 3s ago"],
            ),
            member("ascii", "done", None, &["· 1m ago"]),
        ];
        for width in 8..=80 {
            for focused in [false, true] {
                for lines in [
                    render_swarm_panel(&members, 0, focused, width, 14),
                    render_gallery(&members, width, 12),
                ] {
                    for line in &lines {
                        let text = plain_line(line);
                        let w = text.as_str().width();
                        assert!(w <= width, "width {width}: line overflows ({w}): {text:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn degenerate_sizes_and_large_member_counts_do_not_panic() {
        let statuses = [
            "spawned",
            "ready",
            "running",
            "thinking",
            "blocked",
            "failed",
            "completed",
            "stopped",
            "unknown-status",
        ];
        for count in [0usize, 1, 3, 50, 400] {
            let members: Vec<GalleryMember> = (0..count)
                .map(|i| {
                    let mut m = member(
                        &format!("agent-🐝-{i}"),
                        statuses[i % statuses.len()],
                        if i == 0 { Some("coordinator") } else { None },
                        &["line 一二三四五", "· 2s ago"],
                    );
                    m.todo = Some((i as u32, (i as u32).max(1)));
                    m
                })
                .collect();
            for width in [0usize, 1, 2, 7, 8, 9, 40] {
                for height in [0usize, 1, 2, 3, 7, 20] {
                    let _ = render_gallery(&members, width, height);
                    let _ = render_swarm_panel(&members, count + 5, true, width, height);
                    let _ = render_swarm_strip(
                        &members,
                        count + 5,
                        true,
                        &hints(),
                        Some("ctrl+t controls"),
                        usize::MAX / 2,
                        width,
                        12,
                    );
                }
            }
        }
    }

    #[test]
    fn strip_right_tail_is_right_aligned() {
        use unicode_width::UnicodeWidthStr;
        let members = vec![
            member("alpha", "running", None, &[]),
            member("beta", "done", None, &[]),
        ];
        let width = 80;
        let lines = render_swarm_strip(
            &members,
            0,
            false,
            &hints(),
            Some("alt+n controls"),
            0,
            width,
            12,
        );
        let chips = plain_line(&lines[0]);
        assert!(
            chips.as_str().width() == width,
            "tail should pad to exactly the width: got {} for {chips:?}",
            chips.as_str().width()
        );
        assert!(chips.trim_end().ends_with("controls"), "got: {chips}");
    }

    #[test]
    fn dock_empty_renders_nothing() {
        assert!(render_swarm_dock(&[], 0, false, None, 0, 30, 10).is_empty());
    }

    #[test]
    fn compact_empty_renders_nothing() {
        assert!(render_swarm_compact(&[], Some((1, 1, 3)), 30, 2).is_empty());
    }

    #[test]
    fn compact_shows_agent_tally_node_counts_and_bar() {
        let members = vec![
            member("a", "running", Some("coordinator"), &[]),
            member("b", "thinking", None, &[]),
            member("c", "completed", None, &[]),
            member("d", "blocked", None, &[]),
        ];
        let lines = render_swarm_compact(&members, Some((5, 3, 12)), 32, 2);
        assert_eq!(lines.len(), 2, "expected summary + bar");
        let header = plain_line(&lines[0]);
        assert!(header.contains("2/4 agents"), "got: {header}");
        assert!(header.contains("nodes 5/12"), "got: {header}");
        assert!(header.contains("⚠1"), "got: {header}");

        // Bar: green done, yellow running, dim remainder, exactly `width` cells.
        let bar = &lines[1];
        let bar_text = plain_line(bar);
        assert_eq!(disp_w(&bar_text), 32, "bar fills the width: {bar_text:?}");
        assert_eq!(bar.spans.len(), 3, "done + running + empty segments");
        for span in &bar.spans {
            assert!(span.content.chars().all(|c| c == '▁'), "got: {bar_text:?}");
        }
        assert_eq!(bar.spans[0].style.fg, Some(rgb(100, 200, 100)));
        assert_eq!(bar.spans[1].style.fg, Some(rgb(255, 200, 100)));
    }

    #[test]
    fn compact_without_plan_is_single_line() {
        let members = vec![member("a", "running", None, &[])];
        let lines = render_swarm_compact(&members, None, 30, 2);
        assert_eq!(lines.len(), 1);
        assert!(plain_line(&lines[0]).contains("1/1 agents"));
    }

    #[test]
    fn compact_bar_gives_nonempty_classes_at_least_one_cell() {
        let members = vec![member("a", "running", None, &[])];
        // 1 done + 1 running out of 100: both must still be visible.
        let lines = render_swarm_compact(&members, Some((1, 1, 100)), 20, 2);
        let bar = &lines[1];
        assert!(bar.spans.len() >= 3, "got {} spans", bar.spans.len());
        assert!(!bar.spans[0].content.is_empty());
        assert!(!bar.spans[1].content.is_empty());
    }

    #[test]
    fn compact_is_width_and_height_bounded_at_all_sizes() {
        use unicode_width::UnicodeWidthStr;
        let members: Vec<GalleryMember> = (0..9)
            .map(|i| member(&format!("agent-🐝-{i}"), "running", None, &[]))
            .collect();
        for width in 0..=60 {
            for height in 0..=4 {
                for plan in [
                    None,
                    Some((0, 0, 0)),
                    Some((0, 1, 1)),
                    Some((7, 3, 9)),
                    Some((u32::MAX, u32::MAX, u32::MAX)),
                ] {
                    let lines = render_swarm_compact(&members, plan, width, height);
                    assert!(
                        lines.len() <= height.min(2),
                        "w={width} h={height} plan={plan:?}: {} lines",
                        lines.len()
                    );
                    for line in &lines {
                        let text = plain_line(line);
                        let w = text.as_str().width();
                        assert!(w <= width, "w={width} overflow ({w}): {text:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn dock_lists_agents_with_header_and_selected_tail() {
        let mut m1 = member(
            "researcher",
            "thinking",
            Some("coordinator"),
            &["tracing refresh path", "· 2s ago"],
        );
        m1.todo = Some((2, 5));
        let m2 = member("implementer", "running", None, &["building"]);
        let m3 = member("reviewer", "completed", None, &["LGTM"]);
        let lines = render_swarm_dock(&[m1, m2, m3], 0, false, Some((3, 7)), 0, 34, 12);
        let all: Vec<String> = lines.iter().map(plain_line).collect();
        let joined = all.join("\n");
        assert!(joined.contains("2/3 active"), "got:\n{joined}");
        assert!(joined.contains("plan 3/7"), "got:\n{joined}");
        for name in ["researcher", "implementer", "reviewer"] {
            assert!(joined.contains(name), "missing {name} in:\n{joined}");
        }
        // Selected (coordinator, sorts first) shows its live tail with gutter.
        assert!(joined.contains("│ tracing refresh path"), "got:\n{joined}");
        // Meta age line stays out of the tail.
        assert!(!joined.contains("2s ago"), "got:\n{joined}");
        // Todo counter on the row.
        assert!(joined.contains("2/5"), "got:\n{joined}");
        // Unfocused: no hint line.
        assert!(!joined.contains("j/k"), "got:\n{joined}");
    }

    #[test]
    fn dock_focused_shows_hints_and_attention_count() {
        let members = vec![
            member("a", "running", None, &["working"]),
            member("b", "blocked", None, &["stuck"]),
            member("c", "failed", None, &["boom"]),
        ];
        let lines = render_swarm_dock(&members, 1, true, None, 0, 34, 14);
        let joined: String = lines.iter().map(plain_line).collect::<Vec<_>>().join("\n");
        assert!(joined.contains("⚠2"), "got:\n{joined}");
        assert!(joined.contains("j/k"), "got:\n{joined}");
        // Selected row marked.
        let sel = lines
            .iter()
            .map(plain_line)
            .find(|l| l.contains('▸'))
            .expect("selected row");
        assert!(sel.contains('b'), "got: {sel}");
    }

    #[test]
    fn dock_windows_list_and_reports_overflow() {
        let members: Vec<GalleryMember> = (0..12)
            .map(|i| member(&format!("agent-{i:02}"), "running", None, &["w"]))
            .collect();
        // Budget: header + 5 rows -> 7 hidden.
        let lines = render_swarm_dock(&members, 11, false, None, 0, 30, 7);
        assert!(lines.len() <= 7, "got {} lines", lines.len());
        let joined: String = lines.iter().map(plain_line).collect::<Vec<_>>().join("\n");
        // Selection stays visible even at the end of the list.
        assert!(joined.contains("agent-11"), "got:\n{joined}");
        assert!(joined.contains("+7 more"), "got:\n{joined}");
    }

    #[test]
    fn dock_is_display_width_bounded_at_all_widths() {
        use unicode_width::UnicodeWidthStr;
        let mut members: Vec<GalleryMember> = (0..6)
            .map(|i| {
                member(
                    &format!("agent-🐝-{i}"),
                    "running",
                    None,
                    &["line 一二三四五"],
                )
            })
            .collect();
        members[0].role = Some("coordinator".into());
        members[0].todo = Some((3, 9));
        for width in 8..=60 {
            for focused in [false, true] {
                for height in [0usize, 1, 2, 3, 8, 20] {
                    let lines = render_swarm_dock(
                        &members,
                        3,
                        focused,
                        Some((2, 9)),
                        usize::MAX / 2,
                        width,
                        height,
                    );
                    assert!(
                        lines.len() <= height.max(1),
                        "width {width} height {height}: {} lines",
                        lines.len()
                    );
                    for line in &lines {
                        let text = plain_line(line);
                        let w = text.as_str().width();
                        assert!(
                            w <= width,
                            "width {width} focused {focused}: line overflows ({w}): {text:?}"
                        );
                    }
                }
            }
        }
    }
}
