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

    let used: usize = if shown == 0 && !chips.is_empty() {
        // Degenerate width: show the first chip truncated.
        let budget = width.saturating_sub(lead_w + if show_tally { tail_w + gap } else { 0 });
        let c = &chips[0];
        let avail = budget.saturating_sub(disp_w(&c.glyph) + 1);
        let name = truncate_label(&c.name, avail.max(1));
        let style = Style::default().fg(c.color);
        spans.push(Span::styled(format!("{} ", c.glyph), style));
        spans.push(Span::styled(name.clone(), style));
        disp_w(&c.glyph) + 1 + disp_w(&name)
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
        chips_used + task_used
    };

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
/// Each row is `<icon> <status glyph> · <task>` with a right-aligned todo
/// counter when present. The worker icon occupies the left-edge slot;
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
/// 🦊 ⠙ · wire the auth flow 3/9               2/3 active · alt+n controls
/// 🐝 ▸ ⠹ · audit the webhook path
///    │ checking the signing secret path
///    │ ▸ verify replay protection
/// 🐅 ✓ · support/contact page
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

    let mut out: Vec<Line<'static>> = Vec::new();
    // Where the selected agent's row landed in `out` (focused accordion).
    let mut selected_row_at: Option<usize> = None;
    for (row, m) in ordered.iter().enumerate().skip(start).take(shown) {
        let first = out.is_empty();
        let is_sel = row == selected;
        let color = status_accent(&m.status);
        let glyph = status_glyph(&m.status, spinner_frame);
        let todo = m.todo.map(|(done, total)| format!("{done}/{total}"));

        let icon = m.icon.as_deref().filter(|icon| !icon.is_empty());
        let row_lead = icon.map(|icon| format!("{icon} ")).unwrap_or_else(|| {
            if first {
                LEAD.to_string()
            } else {
                INDENT.to_string()
            }
        });
        let lead_w = disp_w(&row_lead);
        let mut spans: Vec<Span<'static>> = vec![Span::styled(
            row_lead,
            Style::default().fg(rgb(255, 200, 100)),
        )];

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
        // The worker icon occupies the left-edge slot before the spinner.
        // Only workers without an icon need a textual identity in the body.
        let ident = if icon.is_some() {
            String::new()
        } else {
            m.label.clone()
        };
        let marker = if focused {
            if is_sel { "▸ " } else { "  " }
        } else {
            ""
        };
        let head = if ident.is_empty() {
            format!("{marker}{glyph}")
        } else {
            format!("{marker}{glyph} {ident}")
        };
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
#[path = "swarm_gallery_tests.rs"]
mod tests;
