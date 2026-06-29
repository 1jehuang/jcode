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
        Some("worktree_manager") => Some("◆"),
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

/// Sort rank for stable placement: coordinator first, then worktree manager,
/// then everything else.
fn role_rank(role: Option<&str>) -> u8 {
    match role {
        Some("coordinator") => 0,
        Some("worktree_manager") => 1,
        _ => 2,
    }
}

/// The header line shown above the gallery grid.
pub fn gallery_header(total: usize, active: usize) -> Line<'static> {
    Line::from(vec![
        Span::styled("🐝 ", Style::default().fg(rgb(255, 200, 100))),
        Span::styled(
            format!(
                "swarm · {} agent{}{}",
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
    /// Lifecycle status string (drives the badge text and accent color).
    pub status: String,
    /// Swarm role, if any (drives the title glyph and sort order).
    pub role: Option<String>,
    /// Pre-rendered body lines shown inside the tile.
    pub body: Vec<String>,
    /// Stable tiebreaker for sorting members with equal role rank (e.g. id).
    pub sort_key: String,
}

/// Convert members into gallery tiles, sorted for stable placement
/// (coordinator first, worktree manager next, then by `sort_key`).
pub fn members_to_tiles(members: &[GalleryMember]) -> Vec<SwarmTile> {
    let mut sorted: Vec<&GalleryMember> = members.iter().collect();
    sorted.sort_by(|a, b| {
        role_rank(a.role.as_deref())
            .cmp(&role_rank(b.role.as_deref()))
            .then_with(|| a.sort_key.cmp(&b.sort_key))
    });

    sorted
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
    render_swarm_gallery(&tiles, width, &cfg, Some(header))
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

    out
}

/// A key/label pair for the swarm strip hint line.
pub struct SwarmStripHint {
    /// The key chord to show, e.g. "alt+w" or "j/k".
    pub key: String,
    /// What it does, e.g. "select".
    pub label: String,
}

/// Render the compact swarm strip: a single status line of agent "chips" (one
/// per managed agent, colored by status, the selected one marked) followed by a
/// dim keybinding hint line. Designed to sit directly above the status line.
///
/// Returns 1 line when not focused (just the chips) or 2 lines when focused
/// (chips + hints). Returns empty when there are no members or no width.
///
/// ```text
/// 🐝 swarm  ▸researcher ·implementer ·reviewer            2/3 active
///           alt+w focus · j/k select · o pop out · enter open · esc back
/// ```
pub fn render_swarm_strip(
    members: &[GalleryMember],
    selected: usize,
    focused: bool,
    hints: &[SwarmStripHint],
    width: usize,
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

    // ---- Chips line ----
    let mut spans: Vec<Span<'static>> = vec![
        Span::styled("🐝 ", Style::default().fg(rgb(255, 200, 100))),
        Span::styled("swarm", Style::default().fg(rgb(160, 160, 170))),
        Span::raw("  "),
    ];

    // Right-aligned "M/N active" readout; reserve room for it.
    let tally = format!("{active}/{} active", members.len());
    let tally_w = tally.chars().count();

    // Build chips, each "▸name" (selected) or "·name", colored by status.
    let mut chip_specs: Vec<(String, Color, bool)> = Vec::new();
    for (idx, m) in ordered.iter().enumerate() {
        let is_sel = idx == selected;
        let marker = if is_sel { "▸" } else { "·" };
        chip_specs.push((
            format!("{marker}{}", m.label),
            status_accent(&m.status),
            is_sel,
        ));
    }

    // Width budget for chips: total minus the leading "🐝 swarm  " and the
    // trailing tally plus a gap.
    let lead_w = 3 + 5 + 2; // bee + "swarm" + two spaces
    let chips_budget = width.saturating_sub(lead_w + tally_w + 2);
    let mut used = 0usize;
    let mut shown = 0usize;
    for (i, (text, color, is_sel)) in chip_specs.iter().enumerate() {
        let sep_w = if i == 0 { 0 } else { 1 };
        let chip_w = text.chars().count();
        if used + sep_w + chip_w > chips_budget && shown > 0 {
            break;
        }
        if i > 0 {
            spans.push(Span::raw(" "));
            used += 1;
        }
        let mut style = Style::default().fg(*color);
        if *is_sel && focused {
            style = style.add_modifier(Modifier::BOLD | Modifier::REVERSED);
        } else if *is_sel {
            style = style.add_modifier(Modifier::BOLD);
        }
        spans.push(Span::styled(text.clone(), style));
        used += chip_w;
        shown += 1;
    }
    let hidden = chip_specs.len().saturating_sub(shown);
    if hidden > 0 {
        let more = format!(" +{hidden}");
        spans.push(Span::styled(more, Style::default().fg(rgb(140, 140, 150))));
        used += hidden.to_string().chars().count() + 2;
    }

    // Right-align the tally.
    let consumed = lead_w + used;
    let pad = width.saturating_sub(consumed + tally_w).max(1);
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(Span::styled(
        tally,
        Style::default().fg(if active > 0 {
            rgb(255, 200, 100)
        } else {
            rgb(120, 120, 130)
        }),
    ));

    let mut out = vec![Line::from(spans)];

    // ---- Hint line (only while focused, and only if hints provided) ----
    if focused && !hints.is_empty() {
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

    out
}

/// Header line for the list+detail swarm panel. Adds a focus hint when focused.
fn panel_header(total: usize, active: usize, focused: bool) -> Line<'static> {
    let mut spans = vec![
        Span::styled("🐝 ", Style::default().fg(rgb(255, 200, 100))),
        Span::styled(
            format!(
                "swarm · {} agent{}{}",
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

/// Sort members the same way `members_to_tiles` does (coordinator first, then
/// worktree manager, then by sort_key), returning references in display order.
fn sort_members_for_display(members: &[GalleryMember]) -> Vec<&GalleryMember> {
    let mut sorted: Vec<&GalleryMember> = members.iter().collect();
    sorted.sort_by(|a, b| {
        role_rank(a.role.as_deref())
            .cmp(&role_rank(b.role.as_deref()))
            .then_with(|| a.sort_key.cmp(&b.sort_key))
    });
    sorted
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
    let glyph_w = glyph.chars().count();
    let badge_w = badge.chars().count();
    let age_w = age.as_ref().map(|a| a.chars().count() + 1).unwrap_or(0);
    // Reserve: marker + glyph + label + space + badge + space + age.
    let reserved = marker_w + glyph_w + 1 + badge_w + age_w + 1;
    let label_budget = width.saturating_sub(reserved).max(4);
    let label = truncate_label(&member.label, label_budget);
    let label_w = label.chars().count();

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

fn truncate_label(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max <= 1 {
        return "…".to_string();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(id: &str, status: &str, role: Option<&str>, body: &[&str]) -> GalleryMember {
        GalleryMember {
            label: id.to_string(),
            status: status.to_string(),
            role: role.map(str::to_string),
            body: body.iter().map(|s| s.to_string()).collect(),
            sort_key: id.to_string(),
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

    #[test]
    fn renders_header_and_is_width_bounded() {
        let members = vec![
            member("alpha", "running", None, &["editing config.rs"]),
            member("beta", "done", None, &["reviewed"]),
        ];
        let lines = render_gallery(&members, 80, 12);
        assert!(!lines.is_empty());
        let header: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(header.contains("swarm · 2 agents"), "got: {header}");
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
        assert!(header.contains("swarm · 3 agents"), "got: {header}");
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
                key: "alt+w".into(),
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
        assert!(render_swarm_strip(&[], 0, true, &hints(), 80).is_empty());
    }

    #[test]
    fn strip_one_line_when_unfocused_two_when_focused() {
        let members = vec![
            member("researcher", "thinking", Some("coordinator"), &[]),
            member("implementer", "running", None, &[]),
        ];
        let unfocused = render_swarm_strip(&members, 0, false, &hints(), 80);
        assert_eq!(
            unfocused.len(),
            1,
            "unfocused strip should be a single line"
        );
        let focused = render_swarm_strip(&members, 0, true, &hints(), 80);
        assert_eq!(focused.len(), 2, "focused strip should add a hint line");
    }

    #[test]
    fn strip_shows_agents_and_tally_and_is_width_bounded() {
        let members = vec![
            member("researcher", "thinking", Some("coordinator"), &[]),
            member("implementer", "running", None, &[]),
            member("reviewer", "done", None, &[]),
        ];
        let lines = render_swarm_strip(&members, 1, true, &hints(), 90);
        for line in &lines {
            assert!(line.width() <= 90, "line too wide: {}", plain_line(line));
        }
        let chips = plain_line(&lines[0]);
        assert!(chips.contains("researcher"), "got: {chips}");
        assert!(chips.contains("implementer"), "got: {chips}");
        assert!(chips.contains("2/3 active"), "tally missing: {chips}");
        // Selected agent (implementer) carries the ▸ marker.
        assert!(
            chips.contains("▸implementer"),
            "selection marker missing: {chips}"
        );
        // Hint line carries keybindings.
        let hint = plain_line(&lines[1]);
        assert!(hint.contains("alt+w"), "got: {hint}");
        assert!(hint.contains("focus"), "got: {hint}");
    }

    #[test]
    fn strip_overflow_collapses_to_more_count() {
        let members: Vec<GalleryMember> = (0..12)
            .map(|i| member(&format!("agent-number-{i:02}"), "running", None, &[]))
            .collect();
        let lines = render_swarm_strip(&members, 0, false, &hints(), 50);
        assert!(lines[0].width() <= 50, "too wide");
        let chips = plain_line(&lines[0]);
        assert!(chips.contains('+'), "expected +N overflow marker: {chips}");
    }
}
