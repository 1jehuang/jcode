//! Mermaid flowchart source generation for a swarm plan's task DAG.
//!
//! Lives in `jcode-plan` (rather than the TUI) so every consumer renders the
//! same graph from the same logic: the TUI's inline plan-graph message, and
//! the renderer stress probe in `jcode-tui-mermaid`
//! (`examples/swarm_plan_stress.rs`) which feeds this exact output through the
//! real mermaid pipeline. Status classification reuses
//! [`summarize_plan_graph`], so node colors always agree with the scheduler's
//! view of ready/blocked/active/done/failed.
//!
//! This module only builds mermaid source; callers decide when to render it.

use crate::{PlanItem, summarize_plan_graph};
use std::collections::{HashMap, HashSet};

/// Max tasks drawn before the graph is truncated with a summary node.
/// Beyond this the diagram stops being readable at terminal cell sizes.
const MAX_GRAPH_NODES: usize = 30;
/// Max characters of task content shown per node label.
const MAX_LABEL_CHARS: usize = 42;
/// Max characters of the assignee suffix shown per node label.
const MAX_ASSIGNEE_CHARS: usize = 12;
/// Switch from top-down to left-right layout above this many drawn nodes.
/// TD squeezes wide graphs into horizontal slivers at terminal widths, while
/// LR stacks labels vertically where the transcript can scroll.
const LR_NODE_THRESHOLD: usize = 10;
/// Switch to left-right layout when any node has more incoming dependency
/// edges than this: wide fan-ins (gate nodes) force huge TD layouts.
const LR_FAN_IN_THRESHOLD: usize = 4;
/// Never use left-right layout when the longest dependency path exceeds
/// this: a deep chain in LR becomes one long horizontal row that fits the
/// terminal width as an unreadable few-pixel strip. TD lets chains flow
/// downward instead.
const LR_MAX_DEPTH: usize = 8;

/// Build mermaid flowchart source for a swarm plan, or `None` when the plan
/// is empty. Node styling encodes scheduler status (done/active/failed/
/// blocked/pending), gate nodes (`*::gate`) render as hexagons, and edges
/// follow `blocked_by` dependencies.
pub fn swarm_plan_mermaid(items: &[PlanItem]) -> Option<String> {
    if items.is_empty() {
        return None;
    }

    // Classify against the FULL plan (not just drawn nodes) with the same
    // logic the scheduler uses, so a queued task with unmet deps shows as
    // blocked rather than pending.
    let summary = summarize_plan_graph(items);
    let done: HashSet<&str> = summary.completed_ids.iter().map(String::as_str).collect();
    let failed: HashSet<&str> = summary.failed_ids.iter().map(String::as_str).collect();
    let active: HashSet<&str> = summary.active_ids.iter().map(String::as_str).collect();
    let blocked: HashSet<&str> = summary.blocked_ids.iter().map(String::as_str).collect();
    let class_of = |id: &str, status: &str| -> &'static str {
        if done.contains(id) {
            "done"
        } else if failed.contains(id) {
            "failed"
        } else if active.contains(id) {
            "active"
        } else if blocked.contains(id) {
            "blocked"
        } else {
            // Statuses outside the scheduler vocabulary (external plans can
            // inject arbitrary strings) keep their legacy visual mapping.
            match status {
                "in_progress" | "active" => "active",
                "cancelled" => "failed",
                _ => "pending",
            }
        }
    };

    // When the plan is over the cap, drop completed tasks first: they are the
    // least interesting nodes, and a long-running plan otherwise fills the
    // whole graph with stale green boxes while live work gets truncated.
    let shown: Vec<&PlanItem> = if items.len() <= MAX_GRAPH_NODES {
        items.iter().collect()
    } else {
        let mut keep: Vec<usize> = (0..items.len())
            .filter(|&i| !done.contains(items[i].id.as_str()))
            .take(MAX_GRAPH_NODES)
            .collect();
        if keep.len() < MAX_GRAPH_NODES {
            for (i, item) in items.iter().enumerate() {
                if done.contains(item.id.as_str()) {
                    keep.push(i);
                    if keep.len() == MAX_GRAPH_NODES {
                        break;
                    }
                }
            }
            keep.sort_unstable();
        }
        keep.into_iter().map(|i| &items[i]).collect()
    };

    // Mermaid-safe node ids. Distinct item ids can sanitize to the same node
    // id (`a-1` and `a_1` both become `t_a_1`), which mermaid silently merges
    // last-wins; suffix collisions so every drawn task stays visible.
    let mut node_ids: HashMap<&str, String> = HashMap::new();
    let mut taken: HashSet<String> = HashSet::new();
    for item in &shown {
        if node_ids.contains_key(item.id.as_str()) {
            // Duplicate raw item id (invalid plan, but defend anyway): the
            // first occurrence wins the id; later duplicates merge.
            continue;
        }
        let mut id = node_id(&item.id);
        if !taken.insert(id.clone()) {
            let mut n = 2usize;
            loop {
                let candidate = format!("{id}_{n}");
                if taken.insert(candidate.clone()) {
                    id = candidate;
                    break;
                }
                n += 1;
            }
        }
        node_ids.insert(item.id.as_str(), id);
    }

    // Dependency edges, deduped, only between drawn nodes, no self-loops.
    let mut edges: Vec<(String, String)> = Vec::new();
    let mut seen_edges: HashSet<(String, String)> = HashSet::new();
    let mut fan_in: HashMap<&str, usize> = HashMap::new();
    for item in &shown {
        let Some(to) = node_ids.get(item.id.as_str()) else {
            continue;
        };
        for dep in &item.blocked_by {
            if dep == &item.id {
                continue;
            }
            let Some(from) = node_ids.get(dep.as_str()) else {
                continue;
            };
            let edge = (from.clone(), to.clone());
            if seen_edges.insert(edge.clone()) {
                *fan_in.entry(item.id.as_str()).or_default() += 1;
                edges.push(edge);
            }
        }
    }

    // Direction: TD reads best for small plans; larger plans or wide fan-ins
    // (deep-mode gates commonly collect 10+ deps) become unreadable slivers
    // in TD at terminal widths, so lay those out LR. Exceptions where LR is
    // strictly worse: deep chains (LR turns them into one long horizontal
    // row) and structureless flat lists (LR packs disconnected nodes into a
    // single row) both stay TD.
    let max_fan_in = fan_in.values().copied().max().unwrap_or(0);
    let depth = longest_path_len(&shown, &node_ids, &edges);
    let wants_lr = shown.len() > LR_NODE_THRESHOLD || max_fan_in > LR_FAN_IN_THRESHOLD;
    let chain_like = depth > LR_MAX_DEPTH;
    let flat_list = edges.is_empty();
    let direction = if wants_lr && !chain_like && !flat_list {
        "LR"
    } else {
        "TD"
    };
    let mut out = format!("flowchart {direction}\n");

    for item in &shown {
        let Some(id) = node_ids.get(item.id.as_str()) else {
            continue;
        };
        let class = class_of(&item.id, &item.status);
        let label = node_label(item, class);
        // Items with `<parent>::gate` ids (from older plans) render as
        // hexagons so they stand out from normal tasks.
        if item.id.ends_with("::gate") {
            out.push_str(&format!("    {id}{{{{\"{label}\"}}}}:::{class}\n"));
        } else {
            out.push_str(&format!("    {id}[\"{label}\"]:::{class}\n"));
        }
    }

    for (from, to) in &edges {
        out.push_str(&format!("    {from} --> {to}\n"));
    }

    let hidden = items.len().saturating_sub(shown.len());
    if hidden > 0 {
        out.push_str(&format!(
            "    more[\"…and {hidden} more tasks\"]:::pending\n"
        ));
        // Tie the summary node to the graph with a dashed edge so it does not
        // float disconnected in a corner of the layout.
        if let Some(last) = shown.last().and_then(|item| node_ids.get(item.id.as_str())) {
            out.push_str(&format!("    {last} -.-> more\n"));
        }
    }

    // Palette mirrors the swarm gallery status accents.
    out.push_str("    classDef done fill:#1d3a1d,stroke:#64c864,color:#a8e0a8\n");
    out.push_str("    classDef active fill:#3a321d,stroke:#ffc864,color:#ffe0a8\n");
    out.push_str("    classDef failed fill:#3a1d1d,stroke:#ff6464,color:#ffa8a8\n");
    out.push_str("    classDef blocked fill:#3a2a1d,stroke:#ffaa50,color:#ffd0a0\n");
    out.push_str("    classDef pending fill:#26262e,stroke:#8c8c96,color:#b4b4be\n");
    Some(out)
}

/// Longest path (in nodes) through the drawn dependency DAG, used by the
/// layout-direction heuristic. Iterative relaxation over the edge list keeps
/// it simple; drawn graphs are capped at [`MAX_GRAPH_NODES`], and cycles
/// terminate via the pass bound.
fn longest_path_len(
    shown: &[&PlanItem],
    node_ids: &HashMap<&str, String>,
    edges: &[(String, String)],
) -> usize {
    if shown.is_empty() {
        return 0;
    }
    let mut depth: HashMap<&str, usize> = node_ids.values().map(|id| (id.as_str(), 1)).collect();
    // At most N-1 relaxation passes are needed for a DAG of N nodes.
    for _ in 0..shown.len() {
        let mut changed = false;
        for (from, to) in edges {
            let from_depth = depth.get(from.as_str()).copied().unwrap_or(1);
            let to_depth = depth.entry(to.as_str()).or_insert(1);
            if from_depth + 1 > *to_depth {
                *to_depth = from_depth + 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    depth.values().copied().max().unwrap_or(1)
}

/// A mermaid-safe node id derived from a plan item id.
fn node_id(raw: &str) -> String {
    let mut id: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if id.is_empty() {
        id.push('x');
    }
    // Mermaid ids must not start with a digit for some directives; prefix
    // uniformly so ids stay predictable.
    format!("t_{id}")
}

/// Node label: status glyph + truncated content + optional short assignee.
fn node_label(item: &PlanItem, class: &str) -> String {
    let glyph = match class {
        "done" => "✓",
        "active" => "▶",
        "failed" => "✗",
        "blocked" => "⏸",
        _ => "·",
    };
    let content = truncate_chars(&sanitize_label(&item.content), MAX_LABEL_CHARS);
    // Keep labels single-line plain text: HTML-ish line breaks (<br/>) are
    // not reliably supported by the Rust mermaid renderer's SVG output.
    match &item.assigned_to {
        Some(who) if !who.is_empty() => {
            format!("{glyph} {content} · @{}", short_assignee(who))
        }
        _ => format!("{glyph} {content}"),
    }
}

/// Compact an assignee for display: session ids like
/// `session_hamster_1783199147688_8fa34a84b95fe291` reduce to the friendly
/// animal name (`hamster`); anything else is truncated. Raw session ids are
/// half the label width and all look identical at a glance.
fn short_assignee(who: &str) -> String {
    let sanitized = sanitize_label(who);
    if let Some(rest) = sanitized.strip_prefix("session_") {
        let name = rest.split('_').next().unwrap_or(rest);
        if !name.is_empty() {
            return truncate_chars(name, MAX_ASSIGNEE_CHARS);
        }
    }
    truncate_chars(&sanitized, MAX_ASSIGNEE_CHARS)
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() > max {
        let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    } else {
        text.to_string()
    }
}

/// Strip characters that would break out of a mermaid quoted label.
///
/// Both quote characters are replaced with a typographic apostrophe: the
/// mermaid tokenizer treats an unbalanced `'` or `"` inside a quoted label as
/// a string delimiter and shatters the line into phantom nodes, which was the
/// primary cause of illegible real-world plan graphs.
fn sanitize_label(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '"' | '\'' => '’',
            '\n' | '\r' | '\t' => ' ',
            '[' | ']' | '{' | '}' => '(',
            _ => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, content: &str, status: &str, blocked_by: &[&str]) -> PlanItem {
        PlanItem {
            content: content.to_string(),
            status: status.to_string(),
            priority: "normal".to_string(),
            id: id.to_string(),
            subsystem: None,
            file_scope: Vec::new(),
            blocked_by: blocked_by.iter().map(|s| s.to_string()).collect(),
            assigned_to: None,
        }
    }

    #[test]
    fn empty_plan_yields_no_graph() {
        assert!(swarm_plan_mermaid(&[]).is_none());
    }

    #[test]
    fn graph_has_nodes_edges_and_status_classes() {
        let mut assigned = item("b-2", "carve the gallery band", "running", &["a-1"]);
        assigned.assigned_to = Some("worker-fox".to_string());
        let items = vec![
            item("a-1", "wire the bus tap", "completed", &[]),
            assigned,
            item("c-3", "run the ui tests", "queued", &["b-2"]),
        ];
        let graph = swarm_plan_mermaid(&items).expect("graph");
        assert!(graph.starts_with("flowchart TD"), "got: {graph}");
        assert!(
            graph.contains("t_a_1[\"✓ wire the bus tap\"]:::done"),
            "got: {graph}"
        );
        assert!(graph.contains(":::active"), "got: {graph}");
        assert!(graph.contains("@worker-fox"), "got: {graph}");
        assert!(
            !graph.contains("<br"),
            "labels must stay single-line: {graph}"
        );
        assert!(graph.contains("t_a_1 --> t_b_2"), "got: {graph}");
        assert!(graph.contains("t_b_2 --> t_c_3"), "got: {graph}");
        assert!(graph.contains("classDef done"), "got: {graph}");
    }

    #[test]
    fn labels_are_sanitized_and_truncated() {
        let items = vec![item(
            "x!y",
            "a \"quoted\" [bracketed]\nmultiline label that is much longer than the cap allows here",
            "weird-status",
            &["missing-dep"],
        )];
        let graph = swarm_plan_mermaid(&items).expect("graph");
        // Quotes/brackets/newlines neutralized; unresolvable dep -> blocked.
        assert!(
            graph.contains("t_x_y[\"⏸ a ’quoted’ (bracketed( multiline"),
            "got: {graph}"
        );
        assert!(graph.contains(":::blocked"), "got: {graph}");
        assert!(graph.contains('…'), "expected truncation: {graph}");
        // Edge to an undrawn/missing dependency is dropped.
        assert!(!graph.contains("-->"), "got: {graph}");
    }

    #[test]
    fn quotes_never_survive_into_labels() {
        // A lone apostrophe inside a quoted mermaid label shatters the graph
        // into phantom nodes (renderer tokenizer bug), so both quote chars
        // must be replaced, not passed through.
        let items = vec![item(
            "q",
            "verify the work of 'fix-swarm-member-task'",
            "queued",
            &[],
        )];
        let graph = swarm_plan_mermaid(&items).expect("graph");
        assert!(!graph.contains('\''), "raw apostrophe leaked: {graph}");
        assert!(
            graph.contains("’fix-swarm-member-task’"),
            "expected typographic replacement: {graph}"
        );
    }

    #[test]
    fn session_id_assignees_shorten_to_friendly_name() {
        let mut assigned = item("t1", "do the thing", "running", &[]);
        assigned.assigned_to = Some("session_hamster_1783199147688_8fa34a84b95fe291".to_string());
        let graph = swarm_plan_mermaid(&[assigned]).expect("graph");
        assert!(graph.contains("· @hamster\""), "got: {graph}");
        assert!(
            !graph.contains("8fa34a84b95fe291"),
            "raw session id leaked into label: {graph}"
        );
    }

    #[test]
    fn gate_nodes_render_as_hexagons() {
        let items = vec![
            item("work", "implement the feature", "completed", &[]),
            item("work::gate", "verify the work", "queued", &["work"]),
        ];
        let graph = swarm_plan_mermaid(&items).expect("graph");
        assert!(
            graph.contains("t_work__gate{{\"· verify the work\"}}:::pending"),
            "gate should be a hexagon: {graph}"
        );
        assert!(
            graph.contains("t_work[\"✓ implement the feature\"]:::done"),
            "normal tasks stay rectangles: {graph}"
        );
    }

    #[test]
    fn queued_items_with_unmet_deps_render_blocked() {
        let items = vec![
            item("dep", "still running", "running", &[]),
            item("waiting", "waits on dep", "queued", &["dep"]),
        ];
        let graph = swarm_plan_mermaid(&items).expect("graph");
        assert!(
            graph.contains("t_waiting[\"⏸ waits on dep\"]:::blocked"),
            "dep-blocked queued item should style as blocked: {graph}"
        );
    }

    #[test]
    fn small_graphs_stay_td_large_or_wide_fan_in_switch_to_lr() {
        let small: Vec<PlanItem> = (0..5)
            .map(|i| item(&format!("s{i}"), &format!("task {i}"), "queued", &[]))
            .collect();
        assert!(
            swarm_plan_mermaid(&small)
                .expect("graph")
                .starts_with("flowchart TD"),
            "small plans keep TD"
        );

        // Large connected plan (shallow fan-out from one root) switches to LR.
        let mut large = vec![item("root", "kick off", "completed", &[])];
        large.extend(
            (0..14).map(|i| item(&format!("l{i}"), &format!("task {i}"), "queued", &["root"])),
        );
        assert!(
            swarm_plan_mermaid(&large)
                .expect("graph")
                .starts_with("flowchart LR"),
            "large plans switch to LR"
        );

        // Large but structureless (no edges at all): LR would pack one long
        // row, so flat lists stay TD.
        let flat: Vec<PlanItem> = (0..15)
            .map(|i| item(&format!("f{i}"), &format!("task {i}"), "queued", &[]))
            .collect();
        assert!(
            swarm_plan_mermaid(&flat)
                .expect("graph")
                .starts_with("flowchart TD"),
            "flat lists stay TD"
        );

        // Large but chain-shaped (depth > LR_MAX_DEPTH): LR would render one
        // endless horizontal row, so deep chains stay TD.
        let chain: Vec<PlanItem> = (0..15usize)
            .map(|i| {
                let dep = format!("c{}", i.saturating_sub(1));
                let deps: Vec<&str> = if i == 0 { vec![] } else { vec![dep.as_str()] };
                item(&format!("c{i}"), &format!("step {i}"), "queued", &deps)
            })
            .collect();
        assert!(
            swarm_plan_mermaid(&chain)
                .expect("graph")
                .starts_with("flowchart TD"),
            "deep chains stay TD"
        );

        // 6 nodes but one gate collecting 5 deps: fan-in forces LR.
        let mut wide: Vec<PlanItem> = (0..5)
            .map(|i| item(&format!("w{i}"), &format!("task {i}"), "completed", &[]))
            .collect();
        let deps: Vec<String> = (0..5).map(|i| format!("w{i}")).collect();
        let dep_refs: Vec<&str> = deps.iter().map(String::as_str).collect();
        wide.push(item("gate", "verify all", "queued", &dep_refs));
        assert!(
            swarm_plan_mermaid(&wide)
                .expect("graph")
                .starts_with("flowchart LR"),
            "wide fan-in switches to LR"
        );
    }

    #[test]
    fn duplicate_sanitized_ids_get_suffixed_and_self_edges_drop() {
        let items = vec![
            item("a-1", "first flavor", "completed", &["a-1"]),
            item("a_1", "second flavor", "running", &["a-1"]),
        ];
        let graph = swarm_plan_mermaid(&items).expect("graph");
        assert!(graph.contains("t_a_1[\"✓ first flavor\"]"), "got: {graph}");
        assert!(
            graph.contains("t_a_1_2[\"▶ second flavor\"]"),
            "colliding sanitized id must be suffixed, not silently merged: {graph}"
        );
        assert!(
            !graph.contains("t_a_1 --> t_a_1\n"),
            "self-dependency edges must be dropped: {graph}"
        );
        assert!(graph.contains("t_a_1 --> t_a_1_2"), "got: {graph}");
    }

    #[test]
    fn oversized_plans_truncate_dropping_done_first_with_linked_summary_node() {
        // 25 completed + 15 queued: the queued (live) tasks must all survive
        // truncation, completed ones fill the remaining slots.
        let mut items: Vec<PlanItem> = (0..25)
            .map(|i| {
                item(
                    &format!("d{i}"),
                    &format!("done task {i}"),
                    "completed",
                    &[],
                )
            })
            .collect();
        items.extend(
            (0..15).map(|i| item(&format!("q{i}"), &format!("live task {i}"), "queued", &[])),
        );
        let graph = swarm_plan_mermaid(&items).expect("graph");
        assert!(graph.contains("…and 10 more tasks"), "got: {graph}");
        for i in 0..15 {
            assert!(
                graph.contains(&format!("live task {i}")),
                "live task {i} must survive truncation: {graph}"
            );
        }
        assert!(
            !graph.contains("done task 20"),
            "oldest surplus done tasks are dropped: {graph}"
        );
        // The summary node is tied into the graph rather than floating.
        assert!(graph.contains("-.-> more"), "got: {graph}");
    }
}
