//! Adapter from swarm member status into the inline gallery layout.
//!
//! All presentation logic (status colors, role glyphs, age formatting, header,
//! sorting, layout config) lives in the shared
//! [`jcode_tui_render::swarm_gallery`] module so the live TUI and the
//! `swarm_gallery_live` demo render identically. This adapter only handles
//! turning a [`SwarmMemberStatus`] into a renderer-agnostic
//! [`GalleryMember`] (label + body lines).

use crate::plan::PlanItem;
use crate::protocol::SwarmMemberStatus;
use jcode_tui_core::keybind::alt_chord_lower;
use jcode_tui_render::swarm_gallery::{
    GalleryMember, SwarmStripHint, display_order, humanize_age, is_active_status, render_gallery,
    render_swarm_compact, render_swarm_dock, render_swarm_live_card, render_swarm_panel,
    render_swarm_strip, render_swarm_strip_vertical, status_accent, status_glyph,
};
use ratatui::prelude::*;
use std::collections::{HashMap, HashSet};

fn member_label(member: &SwarmMemberStatus) -> String {
    member
        .friendly_name
        .clone()
        .unwrap_or_else(|| member.session_id.chars().take(8).collect())
}

/// Session icon (emoji) for a member, derived from its friendly name (session
/// names come from the shared `SESSION_NAMES` word list, e.g. "fox" -> 🦊).
/// Falls back to `None` when the name is unknown so the strip shows the name.
fn member_icon(member: &SwarmMemberStatus) -> Option<String> {
    let name = member.friendly_name.as_deref()?;
    let icon = crate::id::session_icon(name);
    if icon == "💫" {
        // Unknown word: don't show the generic fallback, keep the name.
        None
    } else {
        Some(icon.to_string())
    }
}

/// Age marker appended to member bodies, e.g. "· 7s ago" or "· now".
/// `humanize_age` already yields "now" for fresh updates, which reads wrong
/// with an "ago" suffix.
fn age_marker(age: u64) -> String {
    let human = humanize_age(age);
    if human == "now" {
        "· now".to_string()
    } else {
        format!("· {human} ago")
    }
}

/// Build the body lines shown inside a member's viewport. Prefers live streamed
/// output (the tail) when present; otherwise surfaces the latest detail plus a
/// status-age hint.
fn member_body(member: &SwarmMemberStatus) -> Vec<String> {
    // Live streamed output wins: show the worker's in-progress assistant text.
    if let Some(tail) = member.output_tail.as_ref().filter(|t| !t.trim().is_empty()) {
        let mut body: Vec<String> = tail.lines().map(|l| l.to_string()).collect();
        if let Some(age) = member.status_age_secs {
            body.push(age_marker(age));
        }
        return body;
    }
    let mut body: Vec<String> = Vec::new();
    if let Some(detail) = member.detail.as_ref().filter(|d| !d.trim().is_empty()) {
        body.push(detail.clone());
    }
    if let Some(age) = member.status_age_secs {
        body.push(age_marker(age));
    }
    body
}

/// Convert swarm members into renderer-agnostic gallery members.
pub(crate) fn members_to_gallery(members: &[SwarmMemberStatus]) -> Vec<GalleryMember> {
    members
        .iter()
        .map(|member| GalleryMember {
            label: member_label(member),
            icon: member_icon(member),
            status: member.status.clone(),
            task: member.task_label.clone(),
            role: member.role.clone(),
            body: member_body(member),
            sort_key: member.session_id.clone(),
            todo: member.todo_progress,
            model: member.runtime.model.clone(),
            provider: member.runtime.provider.clone(),
            auth_method: member.runtime.auth_method.clone(),
            effort: member.runtime.effort.clone(),
            elapsed_secs: member.runtime.elapsed_secs,
            input_tokens: member.runtime.input_tokens,
            output_tokens: member.runtime.output_tokens,
            queue_depth: member.runtime.queue_depth,
            cost_cents: member.runtime.cost_cents,
            todo_items: member
                .todo_items
                .iter()
                .map(|t| jcode_tui_render::swarm_gallery::GalleryTodo {
                    content: t.content.clone(),
                    status: t.status.clone(),
                    tool_intents: t
                        .tool_intents
                        .iter()
                        .map(|tool| jcode_tui_render::swarm_gallery::GalleryToolIntent {
                            tool_name: tool.tool_name.clone(),
                            intent: tool.intent.clone(),
                            status: tool.status.clone(),
                            progress: tool.progress.as_ref().map(|progress| {
                                (progress.current, progress.total, progress.unit.clone())
                            }),
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect()
}

/// Render expanded member cards for insertion directly beneath a swarm tool
/// call in the transcript.
pub(crate) fn render_swarm_chat_card_lines(
    members: &[SwarmMemberStatus],
    width: usize,
) -> Vec<Line<'static>> {
    let mut gallery_members = members_to_gallery(members);
    for (gallery, member) in gallery_members.iter_mut().zip(members) {
        if let Some(label) = member
            .task_label
            .as_deref()
            .map(str::trim)
            .filter(|label| !label.is_empty())
        {
            // Transcript cards belong to the spawn call, so the user-provided
            // spawn label is the useful primary identity. The generated animal
            // name remains represented by the session icon and is still used by
            // the persistent swarm gallery/panel.
            gallery.label = label.to_string();
        }
    }
    jcode_tui_render::swarm_gallery::render_swarm_chat_cards(&gallery_members, width)
}

#[derive(Clone)]
struct SwarmTreeRow<'a> {
    member: &'a SwarmMemberStatus,
    depth: usize,
    is_last: bool,
    ancestor_is_last: Vec<bool>,
}

#[derive(Clone)]
struct DagNode {
    _item_id: String,
    label: String,
    completed: bool,
    active: bool,
    failed: bool,
    children: Vec<DagNode>,
}

/// Render a compact task dependency DAG for the given plan items.
///
/// Items are displayed as an indented tree with status indicators:
/// - ✅ completed (dim)
/// - 🔄 active/running (accent, bold)
/// - ⏳ ready/pending (yellow)
/// - 🔒 blocked (dim red)
/// - ❌ failed/cycle (red)
///
/// Returns at most `max_height` lines (0 when the plan is empty or width is
/// too narrow).
pub(crate) fn render_swarm_plan_dag(
    items: &[PlanItem],
    width: usize,
    max_height: usize,
) -> Vec<Line<'static>> {
    if items.is_empty() || width < 12 || max_height == 0 {
        return Vec::new();
    }

    // Build lookup maps.
    let item_map: HashMap<&str, &PlanItem> = items.iter().map(|i| (i.id.as_str(), i)).collect();

    // Compute blocked status from dependency edges.
    let mut blocked_set: HashSet<&str> = HashSet::new();
    for item in items {
        if item
            .blocked_by
            .iter()
            .any(|dep| !matches!(item_map.get(dep.as_str()), Some(i) if i.status == "completed"))
        {
            blocked_set.insert(item.id.as_str());
        }
    }

    // Classify item status.
    let classify = |item: &PlanItem| -> (&str, bool, bool, bool) {
        let id = item.id.as_str();
        if item.status == "completed" {
            ("completed", true, false, false)
        } else if item.status == "active" || item.status == "running" {
            ("active", false, true, false)
        } else if blocked_set.contains(id) {
            ("blocked", false, false, false)
        } else if item.status == "failed" || item.status == "stopped" {
            ("failed", false, false, true)
        } else {
            ("ready", false, false, false)
        }
    };

    // Build dependency graph: item_id -> children (items that depend on it).
    let mut children_map: HashMap<&str, Vec<&str>> = HashMap::new();
    for item in items {
        for dep in &item.blocked_by {
            if item_map.contains_key(dep.as_str()) {
                children_map.entry(dep.as_str()).or_default().push(&item.id);
            }
        }
    }

    // Topological sort (roots first: items with no dependencies or all deps external).
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut dep_edges: HashMap<&str, Vec<&str>> = HashMap::new();
    for item in items {
        let id = item.id.as_str();
        in_degree.entry(id).or_insert(0);
        for dep in &item.blocked_by {
            if item_map.contains_key(dep.as_str()) {
                dep_edges.entry(dep.as_str()).or_default().push(id);
                *in_degree.entry(id).or_insert(0) += 1;
            }
        }
    }
    let mut topo_order: Vec<&str> = Vec::new();
    let mut queue: Vec<&str> = in_degree
        .iter()
        .filter(|&(_, &d)| d == 0)
        .map(|(&id, _)| id)
        .collect();
    queue.sort();
    while let Some(id) = queue.pop() {
        topo_order.push(id);
        if let Some(deps) = dep_edges.get(id) {
            for &dep_id in deps {
                let e = in_degree.entry(dep_id).or_insert(0);
                *e -= 1;
                if *e == 0 {
                    // Insert sorted to maintain stable order.
                    match queue.binary_search(&dep_id) {
                        Ok(pos) | Err(pos) => queue.insert(pos, dep_id),
                    }
                }
            }
        }
    }
    // Append items not reachable from roots (cycles, disconnected) in stable order.
    let topo_set: HashSet<&str> = topo_order.iter().copied().collect();
    let mut remaining: Vec<&str> = items
        .iter()
        .map(|i| i.id.as_str())
        .filter(|id| !topo_set.contains(id))
        .collect();
    remaining.sort();
    topo_order.extend(remaining);

    // Build tree from topological order.
    let mut node_map: HashMap<&str, DagNode> = HashMap::new();

    for id in &topo_order {
        let item = match item_map.get(id) {
            Some(i) => *i,
            None => continue,
        };
        let (status, completed, active, failed) = classify(item);
        let prefix = match status {
            "completed" => "✅",
            "active" => "🔄",
            "blocked" => "🔒",
            "failed" => "❌",
            _ => "⏳",
        };
        let label = if item.content.len() > 40 {
            format!("{prefix} {}…", &item.content[..39])
        } else {
            format!("{prefix} {}", item.content)
        };
        let mut node = DagNode {
            _item_id: id.to_string(),
            label,
            completed,
            active,
            failed,
            children: Vec::new(),
        };
        if let Some(children) = children_map.get(id) {
            for child_id in children {
                if let Some(child) = node_map.remove(child_id) {
                    node.children.push(child);
                }
            }
        }
        node_map.insert(id, node);
    }

    // Separate roots (items not owned as children by any other item).
    let all_children: HashSet<&str> = children_map.values().flatten().copied().collect();
    let mut roots: Vec<DagNode> = Vec::new();
    for id in &topo_order {
        if !all_children.contains(id)
            && let Some(node) = node_map.remove(id)
        {
            roots.push(node);
        }
    }

    // Render the tree.
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut line_count: usize = 0;
    let item_text_width = width.saturating_sub(4); // Reserve 4 chars for tree prefix.

    fn render_dag_node(
        node: &DagNode,
        prefix: &str,
        is_last: bool,
        item_text_width: usize,
        out: &mut Vec<Line<'static>>,
        line_count: &mut usize,
        max_height: usize,
    ) {
        if *line_count >= max_height {
            return;
        }
        let connector = if prefix.is_empty() {
            String::new()
        } else if is_last {
            format!("{prefix}└─ ")
        } else {
            format!("{prefix}├─ ")
        };

        let (color, bold) = if node.completed {
            (Color::Rgb(100, 100, 110), false)
        } else if node.active {
            (Color::Rgb(120, 180, 255), true)
        } else if node.failed {
            (Color::Rgb(255, 100, 100), false)
        } else {
            (Color::Rgb(200, 180, 60), false)
        };

        let mut style = Style::default().fg(color);
        if bold {
            style = style.add_modifier(Modifier::BOLD);
        }

        let spans = vec![
            Span::styled(connector, Style::default().fg(Color::Rgb(75, 75, 88))),
            Span::styled(
                if node.completed {
                    node.label.clone()
                } else {
                    truncate_str(&node.label, item_text_width)
                },
                style,
            ),
        ];

        out.push(Line::from(spans));
        *line_count += 1;

        let child_prefix = if prefix.is_empty() {
            String::new()
        } else if is_last {
            format!("{prefix}   ")
        } else {
            format!("{prefix}│  ")
        };
        for (i, child) in node.children.iter().enumerate() {
            render_dag_node(
                child,
                &child_prefix,
                i + 1 == node.children.len(),
                item_text_width,
                out,
                line_count,
                max_height,
            );
        }
    }

    for (i, root) in roots.iter().enumerate() {
        render_dag_node(
            root,
            "",
            i + 1 == roots.len(),
            item_text_width,
            &mut out,
            &mut line_count,
            max_height,
        );
    }

    out.truncate(max_height);
    out
}

fn truncate_str(s: &str, max_width: usize) -> String {
    let mut width = 0;
    let mut result = String::new();
    for ch in s.chars() {
        let char_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + char_width > max_width.saturating_sub(1) {
            result.push('…');
            break;
        }
        width += char_width;
        result.push(ch);
    }
    result
}

/// Render the dedicated live swarm page. The upper section is a nested,
/// ownership-aware tree of every managed agent with animated status glyphs.
/// When a swarm plan is active, a compact task DAG is shown below the agent
/// list. The lower section is the selected agent's detailed live card.
pub(crate) fn render_swarm_page_lines(
    members: &[SwarmMemberStatus],
    selected: usize,
    spinner_frame: usize,
    width: usize,
    max_height: usize,
    plan_items: &[PlanItem],
) -> Vec<Line<'static>> {
    if members.is_empty() || width < 8 || max_height == 0 {
        return Vec::new();
    }

    let gallery = members_to_gallery(members);
    let display = display_order(&gallery);
    let selected = selected.min(display.len().saturating_sub(1));
    let selected_member_index = display[selected];
    let selected_id = members[selected_member_index].session_id.as_str();
    let tree = swarm_tree_rows(members, &display);
    let selected_tree_index = tree
        .iter()
        .position(|row| row.member.session_id == selected_id)
        .unwrap_or(0);
    let active = members
        .iter()
        .filter(|member| is_active_status(&member.status))
        .count();

    let mut out = vec![Line::from(vec![
        Span::styled("🐝 ", Style::default().fg(Color::Rgb(255, 200, 100))),
        Span::styled(
            "swarm",
            Style::default().fg(Color::Rgb(230, 230, 240)).bold(),
        ),
        Span::styled(
            format!(
                " · {} agent{} · {active} active",
                members.len(),
                if members.len() == 1 { "" } else { "s" }
            ),
            Style::default().fg(Color::Rgb(150, 150, 160)),
        ),
    ])];
    if max_height > 1 {
        out.push(Line::from(Span::styled(
            format!(
                "{} chat  ·  {} select  ·  {} open  ·  {} prompt  ·  esc chat",
                alt_chord_lower("n"),
                alt_chord_lower("↑/↓"),
                alt_chord_lower("o"),
                alt_chord_lower("shift+p"),
            ),
            Style::default().fg(Color::Rgb(105, 105, 120)),
        )));
    }

    // Render the plan DAG early to reserve budget for it.
    let dag_lines = render_swarm_plan_dag(plan_items, width, max_height.saturating_sub(2));
    let dag_height = dag_lines.len();

    let detail_reserve = if max_height >= 12 { 6 } else { 0 };
    let list_budget = max_height
        .saturating_sub(out.len())
        .saturating_sub(dag_height)
        .saturating_sub(detail_reserve)
        .saturating_sub(1)
        .max(1);
    let first = if tree.len() <= list_budget {
        0
    } else {
        selected_tree_index
            .saturating_sub(list_budget / 2)
            .min(tree.len().saturating_sub(list_budget))
    };
    for row in tree.iter().skip(first).take(list_budget) {
        out.push(render_swarm_tree_row(
            row,
            row.member.session_id == selected_id,
            spinner_frame,
            width,
        ));
    }

    // Append the plan DAG below the agent list when a plan is active.
    if dag_height > 0 {
        out.extend(dag_lines);
    }

    let remaining = max_height.saturating_sub(out.len());
    if remaining >= 3 {
        out.push(Line::from(Span::styled(
            "─".repeat(width),
            Style::default().fg(Color::Rgb(60, 60, 72)),
        )));
        let mut selected_gallery = gallery[selected_member_index].clone();
        if let Some(task) = selected_gallery
            .task
            .as_deref()
            .map(str::trim)
            .filter(|task| !task.is_empty())
            && task != selected_gallery.label
        {
            selected_gallery.label = format!("{} · {task}", selected_gallery.label);
        }
        out.extend(render_swarm_live_card(
            &selected_gallery,
            spinner_frame,
            width,
            max_height.saturating_sub(out.len()),
        ));
    }

    out.truncate(max_height);
    for line in &mut out {
        clamp_line_to_width(line, width);
    }
    out
}

fn swarm_tree_rows<'a>(
    members: &'a [SwarmMemberStatus],
    display: &[usize],
) -> Vec<SwarmTreeRow<'a>> {
    let rank: HashMap<&str, usize> = display
        .iter()
        .enumerate()
        .map(|(rank, &index)| (members[index].session_id.as_str(), rank))
        .collect();
    let ids: HashSet<&str> = members
        .iter()
        .map(|member| member.session_id.as_str())
        .collect();
    let mut children: HashMap<&str, Vec<&SwarmMemberStatus>> = HashMap::new();
    for member in members {
        if let Some(parent) = member.report_back_to_session_id.as_deref()
            && ids.contains(parent)
        {
            children.entry(parent).or_default().push(member);
        }
    }
    for siblings in children.values_mut() {
        siblings.sort_by_key(|member| {
            rank.get(member.session_id.as_str())
                .copied()
                .unwrap_or(usize::MAX)
        });
    }

    let mut roots: Vec<_> = members
        .iter()
        .filter(|member| {
            member
                .report_back_to_session_id
                .as_deref()
                .is_none_or(|parent| !ids.contains(parent))
        })
        .collect();
    roots.sort_by_key(|member| {
        rank.get(member.session_id.as_str())
            .copied()
            .unwrap_or(usize::MAX)
    });

    let mut rows = Vec::new();
    let mut visited: HashSet<&str> = HashSet::new();
    for root in roots {
        append_swarm_tree_rows(root, 0, true, &children, &mut visited, &mut rows, &[]);
    }
    // Corrupt/cyclic parent edges should not make agents disappear. Append any
    // unvisited component as another root in stable display order.
    for &index in display {
        let member = &members[index];
        if !visited.contains(member.session_id.as_str()) {
            append_swarm_tree_rows(member, 0, true, &children, &mut visited, &mut rows, &[]);
        }
    }
    rows
}

fn append_swarm_tree_rows<'a>(
    member: &'a SwarmMemberStatus,
    depth: usize,
    is_last: bool,
    children: &HashMap<&'a str, Vec<&'a SwarmMemberStatus>>,
    visited: &mut HashSet<&'a str>,
    rows: &mut Vec<SwarmTreeRow<'a>>,
    ancestor_is_last: &[bool],
) {
    if !visited.insert(member.session_id.as_str()) {
        return;
    }
    rows.push(SwarmTreeRow {
        member,
        depth,
        is_last,
        ancestor_is_last: ancestor_is_last.to_vec(),
    });

    let Some(member_children) = children.get(member.session_id.as_str()) else {
        return;
    };
    let mut next_ancestors = ancestor_is_last.to_vec();
    next_ancestors.push(is_last);
    for (index, child) in member_children.iter().enumerate() {
        append_swarm_tree_rows(
            child,
            depth + 1,
            index + 1 == member_children.len(),
            children,
            visited,
            rows,
            &next_ancestors,
        );
    }
}

fn render_swarm_tree_row(
    row: &SwarmTreeRow<'_>,
    selected: bool,
    spinner_frame: usize,
    width: usize,
) -> Line<'static> {
    let member = row.member;
    let mut prefix = String::new();
    if row.depth > 0 {
        for &ancestor_last in row
            .ancestor_is_last
            .iter()
            .take(row.depth.saturating_sub(1))
        {
            prefix.push_str(if ancestor_last { "   " } else { "│  " });
        }
        prefix.push_str(if row.is_last { "└─ " } else { "├─ " });
    }
    let label = member_label(member);
    let icon = member_icon(member).unwrap_or_else(|| "🐝".to_string());
    let mut spans = vec![
        Span::styled(
            if selected { "▸ " } else { "  " },
            Style::default().fg(Color::Rgb(255, 200, 100)),
        ),
        Span::styled(prefix, Style::default().fg(Color::Rgb(75, 75, 88))),
        Span::styled(
            format!("{icon} "),
            Style::default().fg(Color::Rgb(255, 200, 100)),
        ),
        Span::styled(
            format!("{} ", status_glyph(&member.status, spinner_frame)),
            Style::default().fg(status_accent(&member.status)),
        ),
        Span::styled(
            label,
            Style::default()
                .fg(status_accent(&member.status))
                .add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ),
    ];
    if let Some(task) = member
        .task_label
        .as_deref()
        .map(str::trim)
        .filter(|task| !task.is_empty())
    {
        spans.push(Span::styled(
            format!(" · {task}"),
            Style::default().fg(Color::Rgb(145, 145, 158)),
        ));
    }
    if let Some((done, total)) = member.todo_progress {
        spans.push(Span::styled(
            format!("  {done}/{total}"),
            Style::default().fg(Color::Rgb(105, 105, 120)),
        ));
    }
    let mut line = Line::from(spans);
    clamp_line_to_width(&mut line, width);
    line
}

fn clamp_line_to_width(line: &mut Line<'static>, width: usize) {
    let mut remaining = width;
    let mut spans = Vec::with_capacity(line.spans.len());
    for span in line.spans.drain(..) {
        if remaining == 0 {
            break;
        }
        let span_width = unicode_width::UnicodeWidthStr::width(span.content.as_ref());
        if span_width <= remaining {
            remaining -= span_width;
            spans.push(span);
            continue;
        }
        let mut text = String::new();
        for ch in span.content.chars() {
            let char_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if char_width > remaining {
                break;
            }
            remaining -= char_width;
            text.push(ch);
        }
        if !text.is_empty() {
            spans.push(Span::styled(text, span.style));
        }
        break;
    }
    line.spans = spans;
}

/// Render the inline swarm gallery for the given members into `area`-width lines.
///
/// When `selected` is `Some(idx)`, the tile at that index gets a brighter border
/// and dimmed inactive tiles for visual differentiation (gallery-grid focus).
#[allow(dead_code)]
pub(crate) fn render_swarm_gallery_lines(
    members: &[SwarmMemberStatus],
    selected: Option<usize>,
    width: usize,
    max_height: usize,
) -> Vec<Line<'static>> {
    if members.is_empty() {
        return Vec::new();
    }
    render_gallery(&members_to_gallery(members), width, max_height, selected)
}

/// Render the list+detail swarm panel: a compact list of managed agents plus a
/// detail viewport for the `selected` one. `focused` adds an interaction hint.
#[allow(dead_code)]
pub(crate) fn render_swarm_panel_lines(
    members: &[SwarmMemberStatus],
    selected: usize,
    focused: bool,
    width: usize,
    max_height: usize,
    rename_hint: Option<&str>,
) -> Vec<Line<'static>> {
    if members.is_empty() {
        return Vec::new();
    }
    render_swarm_panel(
        &members_to_gallery(members),
        selected,
        focused,
        width,
        max_height,
        rename_hint,
    )
}

/// Render the compact swarm strip (agent chips + status glyphs + todo counts)
/// shown directly above the status line.
///
/// The layout follows `agents.swarm_strip_layout`: `vertical` (default) lists
/// one agent per row (session icon + task, capped to a few rows), while
/// `horizontal` packs all agents as chips on a single row.
///
/// `focus_key` is the configured chord to enter the controls (e.g. "ctrl+t"),
/// used for the unfocused enter-hint.
/// `spinner_frame` animates active agents' glyphs. `max_height` bounds the
/// focused strip (chips + expanded hovered-agent detail + hints).
/// Summary line with optional batch mode info appended.
fn summary_line_with_batch(
    members: &[GalleryMember],
    width: usize,
    batch_info: &str,
) -> Line<'static> {
    let inner = jcode_tui_render::swarm_gallery::summary_line_from_members(members, width);
    if batch_info.is_empty() {
        return inner;
    }
    // Append batch info after the right-aligned summary.
    let mut spans = inner.spans;
    spans.push(Span::styled(
        batch_info.to_string(),
        Style::default().fg(jcode_tui_style::color::rgb(105, 105, 120)),
    ));
    Line::from(spans)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_swarm_strip_lines(
    members: &[SwarmMemberStatus],
    selected: usize,
    focused: bool,
    focus_key: &str,
    spinner_frame: usize,
    width: usize,
    max_height: usize,
    selected_agents: &std::collections::HashSet<usize>,
    batch_mode: bool,
) -> Vec<Line<'static>> {
    if members.is_empty() {
        return Vec::new();
    }
    let enter_hint = format!("{focus_key} controls");
    // Focused hints: only Alt-chords (plus esc) are claimed so plain typing
    // keeps flowing to the chat input while the panel is focused.
    let mut hints = vec![
        SwarmStripHint {
            key: alt_chord_lower("n").into(),
            label: "page".into(),
        },
        SwarmStripHint {
            key: alt_chord_lower("↑/↓").into(),
            label: "select".into(),
        },
        SwarmStripHint {
            key: alt_chord_lower("o").into(),
            label: "open".into(),
        },
        SwarmStripHint {
            key: alt_chord_lower("shift+p").into(),
            label: "prompt".into(),
        },
        SwarmStripHint {
            key: "esc".into(),
            label: "exit".into(),
        },
    ];
    if batch_mode {
        hints.insert(
            0,
            SwarmStripHint {
                key: "space".into(),
                label: "toggle".into(),
            },
        );
        hints.insert(
            2,
            SwarmStripHint {
                key: "s".into(),
                label: "stop".into(),
            },
        );
        hints.insert(
            3,
            SwarmStripHint {
                key: "r".into(),
                label: "restart".into(),
            },
        );
        hints.insert(
            4,
            SwarmStripHint {
                key: "p".into(),
                label: "prompt".into(),
            },
        );
    } else {
        hints.insert(
            0,
            SwarmStripHint {
                key: "space".into(),
                label: "select".into(),
            },
        );
        hints.insert(
            1,
            SwarmStripHint {
                key: "shift+tab".into(),
                label: "batch".into(),
            },
        );
    }
    let mut out = match crate::config::config().agents.swarm_strip_layout {
        crate::config::SwarmStripLayout::Vertical => render_swarm_strip_vertical(
            &members_to_gallery(members),
            selected,
            focused,
            &hints,
            if focused {
                None
            } else {
                Some(enter_hint.as_str())
            },
            spinner_frame,
            width,
            SWARM_STRIP_VERTICAL_MAX_ROWS,
            max_height,
            if batch_mode {
                Some(selected_agents)
            } else {
                None
            },
        ),
        crate::config::SwarmStripLayout::Horizontal => render_swarm_strip(
            &members_to_gallery(members),
            selected,
            focused,
            &hints,
            if focused {
                None
            } else {
                Some(enter_hint.as_str())
            },
            spinner_frame,
            width,
            max_height,
            if batch_mode {
                Some(selected_agents)
            } else {
                None
            },
        ),
    };
    // ---- Aggregate summary bar (focused only) ----
    if focused {
        // Insert summary before the last line (hint) so it sits between the
        // detail viewport and the keybinding hints.
        let pos = out.len().saturating_sub(1);
        let batch_info = if batch_mode && !selected_agents.is_empty() {
            format!(" · batch: {} selected", selected_agents.len())
        } else if batch_mode {
            " · batch mode".to_string()
        } else {
            String::new()
        };
        let gallery = members_to_gallery(members);
        out.insert(pos, summary_line_with_batch(&gallery, width, &batch_info));
    }
    out
}

/// Row cap for the vertical strip: agents beyond this collapse into a
/// `+N more` line (the cap includes that overflow row).
const SWARM_STRIP_VERTICAL_MAX_ROWS: usize = 4;

/// Render the compact swarm widget body: at most two lines, an agents/nodes
/// summary plus a green/yellow/empty plan progress bar. `plan` is the
/// coordinator's task-graph progress as (done, running, total).
pub(crate) fn render_swarm_compact_lines(
    members: &[SwarmMemberStatus],
    plan: Option<(u32, u32, u32)>,
    width: usize,
    max_height: usize,
) -> Vec<Line<'static>> {
    if members.is_empty() {
        return Vec::new();
    }
    render_swarm_compact(&members_to_gallery(members), plan, width, max_height)
}

/// Render the swarm dock widget body: a narrow vertical agent list for the
/// info-widget margins. `plan` is the coordinator's swarm plan progress
/// (completed, total), shown in the header when present.
#[allow(dead_code)]
pub(crate) fn render_swarm_dock_lines(
    members: &[SwarmMemberStatus],
    selected: usize,
    focused: bool,
    plan: Option<(u32, u32)>,
    spinner_frame: usize,
    width: usize,
    max_height: usize,
) -> Vec<Line<'static>> {
    if members.is_empty() {
        return Vec::new();
    }
    render_swarm_dock(
        &members_to_gallery(members),
        selected,
        focused,
        plan,
        spinner_frame,
        width,
        max_height,
    )
}

/// Session ids of `members` in the same order the panel/gallery displays them
/// (coordinator first, then worktree manager, then by session id). Lets the TUI
/// map a selected panel index back to a concrete session for pop-out.
///
/// Delegates to the renderer's [`display_order`] on the exact same
/// [`GalleryMember`] conversion used for rendering, so the pop-out index can
/// never drift from what is on screen.
pub(crate) fn members_display_order(members: &[SwarmMemberStatus]) -> Vec<String> {
    display_order(&members_to_gallery(members))
        .into_iter()
        .map(|i| members[i].session_id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_tui_render::swarm_gallery::members_to_tiles;

    fn member(
        id: &str,
        status: &str,
        detail: Option<&str>,
        role: Option<&str>,
    ) -> SwarmMemberStatus {
        SwarmMemberStatus {
            session_id: id.to_string(),
            friendly_name: Some(id.to_string()),
            status: status.to_string(),
            detail: detail.map(str::to_string),
            task_label: None,
            role: role.map(str::to_string),
            is_headless: Some(true),
            live_attachments: None,
            status_age_secs: Some(3),
            output_tail: None,
            report_back_to_session_id: None,
            todo_progress: None,
            todo_items: Vec::new(),
            runtime: crate::protocol::SwarmMemberRuntime::default(),
        }
    }

    #[test]
    fn coordinator_sorts_first() {
        let members = vec![
            member("zeta", "running", None, None),
            member("alpha", "running", None, Some("coordinator")),
        ];
        let tiles = members_to_tiles(&members_to_gallery(&members));
        assert_eq!(tiles[0].title, "alpha");
        assert_eq!(tiles[0].role_glyph.as_deref(), Some("★"));
    }

    /// Regression: pop-out selection resolves `swarm_panel_selected` through
    /// `members_display_order`, so its order must match what the renderer
    /// actually draws (tile order) for mixed roles, ties, and unnamed
    /// sessions. If this ever diverges, pop-out opens the wrong agent.
    #[test]
    fn members_display_order_matches_rendered_tile_order() {
        let mut members = vec![
            member("zeta-session", "running", None, None),
            member("wt-session", "done", None, Some("mystery_role_2")),
            member("coord-session", "running", None, Some("coordinator")),
            member("mystery-session", "thinking", None, Some("mystery_role")),
            member("alpha-session", "failed", None, None),
        ];
        // Unnamed session: label falls back to a session-id prefix.
        let mut unnamed = member("beta-session-long-id", "ready", None, None);
        unnamed.friendly_name = None;
        members.push(unnamed);

        let order = members_display_order(&members);
        assert_eq!(order.len(), members.len());

        // Map each ordered session id to the label the renderer would show.
        let ordered_labels: Vec<String> = order
            .iter()
            .map(|id| {
                let m = members.iter().find(|m| &m.session_id == id).unwrap();
                member_label(m)
            })
            .collect();
        let tile_titles: Vec<String> = members_to_tiles(&members_to_gallery(&members))
            .into_iter()
            .map(|t| t.title)
            .collect();
        assert_eq!(
            ordered_labels, tile_titles,
            "pop-out order must match rendered tile order"
        );

        // Sanity: coordinator first, then the rest active-first
        // (thinking/running), then failed, then idle/finished, ties by id.
        assert_eq!(order[0], "coord-session");
        assert_eq!(
            &order[1..],
            &[
                "mystery-session".to_string(),
                "zeta-session".to_string(),
                "alpha-session".to_string(),
                "beta-session-long-id".to_string(),
                "wt-session".to_string(),
            ]
        );
    }

    #[test]
    fn renders_header_and_boxes() {
        let members = vec![
            member("alpha", "running", Some("editing config.rs"), None),
            member("beta", "done", Some("reviewed"), None),
        ];
        let lines = render_swarm_gallery_lines(&members, None, 80, 12);
        assert!(!lines.is_empty());
        let header: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(header.contains("🐝 2 agents · 1 active"), "got: {header}");
        assert!(!header.contains("swarm"), "got: {header}");
        for line in &lines {
            assert!(line.width() <= 80);
        }
    }

    #[test]
    fn empty_members_render_nothing() {
        assert!(render_swarm_gallery_lines(&[], None, 80, 12).is_empty());
    }

    #[test]
    fn output_tail_takes_priority_over_detail() {
        let mut m = member("alpha", "running", Some("the detail line"), None);
        m.output_tail = Some("line one\nline two".to_string());
        let body = member_body(&m);
        assert_eq!(body[0], "line one");
        assert_eq!(body[1], "line two");
        assert!(!body.iter().any(|l| l.contains("the detail line")));
    }

    #[test]
    fn transcript_card_prefers_spawn_label_over_generated_name() {
        let mut m = member("cow", "ready", None, None);
        m.task_label = Some("card demo".to_string());
        let rendered = render_swarm_chat_card_lines(&[m], 80)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert!(rendered.contains("card demo"), "rendered={rendered}");
        assert!(!rendered.contains("cow"), "rendered={rendered}");
    }

    #[test]
    fn full_page_tree_is_width_bounded_and_cycle_safe() {
        let mut root = member("root", "running", Some("coordinating"), None);
        root.task_label = Some("Root reviewer".to_string());
        root.report_back_to_session_id = Some("grandchild".to_string());
        let mut child = member("child", "running", Some("testing"), None);
        child.task_label = Some("Auth tests".to_string());
        child.report_back_to_session_id = Some("root".to_string());
        let mut grandchild = member("grandchild", "running", Some("fuzzing"), None);
        grandchild.task_label = Some("Race check".to_string());
        grandchild.report_back_to_session_id = Some("child".to_string());
        let members = vec![root.clone(), child, grandchild];

        for width in 0..80 {
            let lines = render_swarm_page_lines(&members, 0, 0, width, 12, &[]);
            assert!(lines.len() <= 12, "cycle expanded at width {width}");
            for line in lines {
                let text: String = line
                    .spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect();
                assert!(
                    unicode_width::UnicodeWidthStr::width(text.as_str()) <= width,
                    "line exceeded width {width}: {text:?}"
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Targeted tests for gallery and page rendering with selected indices.
    // -----------------------------------------------------------------------

    #[test]
    fn gallery_lines_selected_first_second_and_none() {
        let members = vec![
            member(
                "alpha",
                "running",
                Some("coordinating"),
                Some("coordinator"),
            ),
            member("beta", "done", Some("reviewed"), None),
            member("gamma", "thinking", Some("editing"), None),
        ];

        // selected=Some(0)
        let lines_0 = render_swarm_gallery_lines(&members, Some(0), 80, 12);
        assert!(
            !lines_0.is_empty(),
            "gallery with selected=0 must produce lines"
        );
        let header_0: String = lines_0[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(
            header_0.contains("🐝 3 agents · 2 active"),
            "got: {header_0}"
        );

        // selected=Some(2)
        let lines_2 = render_swarm_gallery_lines(&members, Some(2), 80, 12);
        assert!(
            !lines_2.is_empty(),
            "gallery with selected=2 must produce lines"
        );

        // selected=None
        let lines_none = render_swarm_gallery_lines(&members, None, 80, 12);
        assert!(
            !lines_none.is_empty(),
            "gallery with selected=None must produce lines"
        );

        // Width bounds for all variants
        for lines in [&lines_0, &lines_2, &lines_none] {
            for line in lines {
                assert!(
                    line.width() <= 80,
                    "gallery line exceeded width: {:?}",
                    line
                );
            }
        }

        // Empty returns empty
        assert!(render_swarm_gallery_lines(&[], None, 80, 12).is_empty());
        assert!(render_swarm_gallery_lines(&[], Some(0), 80, 12).is_empty());
    }

    #[test]
    fn page_lines_selected_indices_no_panic() {
        let members = vec![
            member("a", "running", Some("task-a"), None),
            member("b", "done", Some("task-b"), None),
            member("c", "thinking", Some("task-c"), None),
            member("d", "failed", Some("task-d"), None),
        ];

        // selected=0 (first)
        let lines = render_swarm_page_lines(&members, 0, 0, 80, 16, &[]);
        assert!(!lines.is_empty(), "page with selected=0 must produce lines");
        for line in &lines {
            assert!(line.width() <= 80, "page line exceeded width: {:?}", line);
        }

        // selected=2 (mid)
        let lines = render_swarm_page_lines(&members, 2, 0, 80, 16, &[]);
        assert!(!lines.is_empty(), "page with selected=2 must produce lines");
        for line in &lines {
            assert!(line.width() <= 80, "page line exceeded width: {:?}", line);
        }

        // selected=3 (last)
        let lines = render_swarm_page_lines(&members, 3, 0, 80, 16, &[]);
        assert!(!lines.is_empty(), "page with selected=3 must produce lines");

        // selected far beyond range (clamped)
        let lines = render_swarm_page_lines(&members, 999, 0, 80, 16, &[]);
        assert!(
            !lines.is_empty(),
            "page with out-of-range selected must produce lines"
        );

        // Empty returns empty
        assert!(render_swarm_page_lines(&[], 0, 0, 80, 16, &[]).is_empty());
        // Width < 8 returns empty
        assert!(render_swarm_page_lines(&members, 0, 0, 4, 16, &[]).is_empty());
        // max_height = 0 returns empty
        assert!(render_swarm_page_lines(&members, 0, 0, 80, 0, &[]).is_empty());
    }

    #[test]
    fn page_lines_height_bounded() {
        let members: Vec<_> = (0..20)
            .map(|i| {
                member(
                    &format!("agent-{i}"),
                    if i % 3 == 0 { "running" } else { "done" },
                    Some(&format!("task-{i}")),
                    None,
                )
            })
            .collect();

        for max_height in [1, 4, 8, 12, 20] {
            let lines = render_swarm_page_lines(&members, 0, 0, 80, max_height, &[]);
            assert!(
                lines.len() <= max_height,
                "page with max_height={max_height} produced {} lines",
                lines.len()
            );
        }
    }
}
