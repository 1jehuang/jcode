//! Bridge between the validated [`crate::dag`] engine and the live
//! [`VersionedPlan`] storage used by the swarm runtime.
//!
//! The `dag` engine is the brain: it owns validation (acyclicity, ownership,
//! gate insertion, artifact checks) and the reference simulator. `VersionedPlan`
//! is the live, persisted, broadcast storage. Rather than run two parallel
//! runtimes, server handlers lift the current plan into a `TaskGraph`, apply an
//! engine op, then lower the result back. This keeps a single source of truth and
//! reuses the existing persistence/broadcast/scheduler machinery.

use crate::dag::{HandoffArtifact, Mode, NodeKind, NodeOrigin, NodeStatus, TaskGraph, TaskNode};
use crate::{NodeMeta, PlanItem, VersionedPlan};

/// Parse a mode string ("deep"/"light"); unknown values fall back to light.
pub fn parse_mode(mode: &str) -> Mode {
    match mode.trim().to_ascii_lowercase().as_str() {
        "deep" => Mode::Deep,
        _ => Mode::Light,
    }
}

pub fn mode_str(mode: Mode) -> &'static str {
    match mode {
        Mode::Deep => "deep",
        Mode::Light => "light",
    }
}

/// Parse a node-kind string; unknown/absent values default to `Explore`.
pub fn parse_kind(kind: Option<&str>) -> NodeKind {
    match kind.map(|k| k.trim().to_ascii_lowercase()).as_deref() {
        Some("implement") => NodeKind::Implement,
        Some("verify") => NodeKind::Verify,
        Some("fix") => NodeKind::Fix,
        Some("synthesize") => NodeKind::Synthesize,
        Some("critique") => NodeKind::Critique,
        _ => NodeKind::Explore,
    }
}

pub fn kind_str(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Explore => "explore",
        NodeKind::Implement => "implement",
        NodeKind::Verify => "verify",
        NodeKind::Fix => "fix",
        NodeKind::Synthesize => "synthesize",
        NodeKind::Critique => "critique",
    }
}

/// Parse a node-origin string; unknown/absent values yield `None` (legacy
/// nodes, treated as seeded by the growth accounting).
pub fn parse_origin(origin: Option<&str>) -> Option<NodeOrigin> {
    match origin.map(|o| o.trim().to_ascii_lowercase()).as_deref() {
        Some("seed") => Some(NodeOrigin::Seed),
        Some("expand") => Some(NodeOrigin::Expand),
        Some("gap") => Some(NodeOrigin::Gap),
        Some("gate") => Some(NodeOrigin::Gate),
        _ => None,
    }
}

pub fn origin_str(origin: NodeOrigin) -> &'static str {
    match origin {
        NodeOrigin::Seed => "seed",
        NodeOrigin::Expand => "expand",
        NodeOrigin::Gap => "gap",
        NodeOrigin::Gate => "gate",
    }
}

/// Map a plan status string to an engine [`NodeStatus`].
fn status_from_plan(status: &str) -> NodeStatus {
    match status {
        "running" | "running_stale" => NodeStatus::Running,
        "completed" | "done" => NodeStatus::Done,
        "failed" | "stopped" | "crashed" => NodeStatus::Failed,
        _ => NodeStatus::Queued,
    }
}

/// Map an engine [`NodeStatus`] back to the canonical plan status string.
fn status_to_plan(status: NodeStatus) -> &'static str {
    match status {
        NodeStatus::Queued => "queued",
        NodeStatus::Running => "running",
        NodeStatus::Done => "completed",
        NodeStatus::Failed => "failed",
    }
}

/// Lift a [`VersionedPlan`] into a validated [`TaskGraph`] for engine ops.
pub fn to_task_graph(plan: &VersionedPlan) -> TaskGraph {
    let mut graph = TaskGraph::new(parse_mode(&plan.mode));
    for item in &plan.items {
        let meta = plan.node_meta.get(&item.id).cloned().unwrap_or_default();
        let artifact = meta
            .artifact_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<HandoffArtifact>(json).ok());
        graph.push_node(TaskNode {
            id: item.id.clone(),
            content: item.content.clone(),
            kind: parse_kind(meta.kind.as_deref()),
            status: status_from_plan(&item.status),
            owner: item.assigned_to.clone(),
            parent: meta.parent.clone(),
            depends_on: item.blocked_by.clone(),
            expanded: meta.expanded,
            is_gate: meta.is_gate,
            planner: meta.planner.clone(),
            priority: crate::priority_rank(&item.priority),
            output: artifact,
            origin: parse_origin(meta.origin.as_deref()),
        });
    }
    graph
}

/// Lower a [`TaskGraph`] back into the plan's items + node_meta, preserving the
/// fields the engine does not own (subsystem, file_scope, original priority
/// string) from the prior plan where ids still match.
pub fn apply_task_graph(plan: &mut VersionedPlan, graph: &TaskGraph) {
    plan.mode = mode_str(graph.mode).to_string();

    // Index prior items to retain non-engine fields.
    let prior: std::collections::HashMap<String, PlanItem> = plan
        .items
        .iter()
        .map(|item| (item.id.clone(), item.clone()))
        .collect();

    let mut items = Vec::with_capacity(graph.nodes().len());
    let mut node_meta = std::collections::HashMap::new();

    for node in graph.nodes() {
        let prev = prior.get(&node.id);
        items.push(PlanItem {
            content: node.content.clone(),
            status: status_to_plan(node.status).to_string(),
            priority: prev
                .map(|p| p.priority.clone())
                .unwrap_or_else(|| priority_string(node.priority)),
            id: node.id.clone(),
            subsystem: prev.and_then(|p| p.subsystem.clone()),
            file_scope: prev.map(|p| p.file_scope.clone()).unwrap_or_default(),
            blocked_by: node.depends_on.clone(),
            assigned_to: node.owner.clone(),
        });
        node_meta.insert(
            node.id.clone(),
            NodeMeta {
                kind: Some(kind_str(node.kind).to_string()),
                parent: node.parent.clone(),
                expanded: node.expanded,
                is_gate: node.is_gate,
                planner: node.planner.clone(),
                artifact_json: node
                    .output
                    .as_ref()
                    .and_then(|a| serde_json::to_string(a).ok()),
                origin: node.origin.map(|o| origin_str(o).to_string()),
            },
        );
    }

    plan.replace_items(items);
    plan.node_meta = node_meta;
}

fn priority_string(rank: u8) -> String {
    match rank {
        0 => "high".to_string(),
        2 => "low".to_string(),
        _ => "medium".to_string(),
    }
}

/// Build the forward-dataflow context for a task: the merged handoff artifacts of
/// all its completed upstream dependencies, formatted for injection into the
/// assigned worker's prompt. Returns `None` when the task has no completed
/// dependencies with artifacts, so callers can skip appending anything.
///
/// This is the live counterpart of `dag::assemble_input`, but it reads artifacts
/// from the plan's `node_meta` side-map instead of a `TaskGraph`, so it can run
/// directly on the assignment path without lifting the whole graph.
pub fn upstream_context(plan: &VersionedPlan, task_id: &str) -> Option<String> {
    let item = plan.items.iter().find(|item| item.id == task_id)?;
    if item.blocked_by.is_empty() {
        return None;
    }

    let mut sections = Vec::new();
    for dep_id in &item.blocked_by {
        let Some(dep) = plan.items.iter().find(|i| &i.id == dep_id) else {
            continue;
        };
        if !crate::is_completed_status(&dep.status) {
            continue;
        }
        let Some(meta) = plan.node_meta.get(dep_id) else {
            continue;
        };
        let Some(json) = meta.artifact_json.as_deref() else {
            continue;
        };
        let Ok(artifact) = serde_json::from_str::<HandoffArtifact>(json) else {
            continue;
        };

        let kind = meta.kind.as_deref().unwrap_or("task");
        sections.push(artifact.render_section(dep_id, kind));
    }

    if sections.is_empty() {
        None
    } else {
        Some(format!(
            "# Inputs from completed dependencies\n\n{}",
            sections.join("\n")
        ))
    }
}

/// Prepend upstream dependency context (if any) to a task's assignment content.
pub fn hydrate_assignment(plan: &VersionedPlan, task_id: &str, content: &str) -> String {
    match upstream_context(plan, task_id) {
        Some(context) => format!("{content}\n\n{context}"),
        None => content.to_string(),
    }
}

/// Growth accounting for a plan: how far the graph outgrew its seed. This is
/// deep mode's visibility signal — a deep plan whose node count equals its
/// seed count never decomposed or gated anything, which almost always means
/// under-exploration rather than a genuinely atomic plan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GrowthStats {
    /// Nodes from the initial seed batch (plus legacy nodes with no origin).
    pub seeded: usize,
    /// Nodes born from `expand_node` decomposition.
    pub from_expansion: usize,
    /// Nodes injected by gates that found gaps/failures.
    pub from_gaps: usize,
    /// Auto-inserted critique/verify gates (including the root gate).
    pub gates: usize,
}

impl GrowthStats {
    pub fn total(&self) -> usize {
        self.seeded + self.from_expansion + self.from_gaps + self.gates
    }

    /// Machinery-generated nodes (everything that is not seed).
    pub fn grown(&self) -> usize {
        self.from_expansion + self.from_gaps + self.gates
    }

    /// One-line human summary, e.g.
    /// `12 seeded -> 87 nodes (+55 expansion, +14 gap, +6 gates)`.
    pub fn summary_line(&self) -> String {
        format!(
            "{} seeded -> {} nodes (+{} expansion, +{} gap, +{} gates)",
            self.seeded,
            self.total(),
            self.from_expansion,
            self.from_gaps,
            self.gates
        )
    }
}

/// Compute growth stats from the plan's `node_meta` origin records. Nodes with
/// no recorded origin (legacy plans, hand-written items) count as seeded.
pub fn growth_stats(plan: &VersionedPlan) -> GrowthStats {
    let mut stats = GrowthStats::default();
    for item in &plan.items {
        let origin = plan
            .node_meta
            .get(&item.id)
            .and_then(|meta| parse_origin(meta.origin.as_deref()));
        match origin {
            Some(NodeOrigin::Expand) => stats.from_expansion += 1,
            Some(NodeOrigin::Gap) => stats.from_gaps += 1,
            Some(NodeOrigin::Gate) => stats.gates += 1,
            Some(NodeOrigin::Seed) | None => stats.seeded += 1,
        }
    }
    stats
}

/// Ids of completed plan items whose stored artifact self-reported LOW
/// confidence. Live counterpart of `TaskGraph::low_confidence_done_ids`,
/// reading artifacts from the plan's `node_meta` side-map so status surfaces
/// (plan_status, run_plan reports) can flag shaky coverage without lifting the
/// whole graph. Gate nodes are excluded: their confidence describes the gate's
/// judgement, not the underlying work.
pub fn low_confidence_completed_ids(plan: &VersionedPlan) -> Vec<String> {
    plan.items
        .iter()
        .filter(|item| crate::is_completed_status(&item.status))
        .filter(|item| {
            let Some(meta) = plan.node_meta.get(&item.id) else {
                return false;
            };
            if meta.is_gate {
                return false;
            }
            meta.artifact_json
                .as_deref()
                .and_then(|json| serde_json::from_str::<HandoffArtifact>(json).ok())
                .and_then(|artifact| artifact.confidence_level())
                == Some(crate::dag::ConfidenceLevel::Low)
        })
        .map(|item| item.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag::{NodeSpec, complete_node, dispatch, expand_node, seed};

    fn plan_item(id: &str, status: &str) -> PlanItem {
        PlanItem {
            content: format!("task {id}"),
            status: status.to_string(),
            priority: "medium".to_string(),
            id: id.to_string(),
            subsystem: None,
            file_scope: Vec::new(),
            blocked_by: Vec::new(),
            assigned_to: None,
        }
    }

    #[test]
    fn round_trip_preserves_items_and_edges() {
        let mut plan = VersionedPlan::new();
        plan.mode = "deep".to_string();
        plan.items = vec![
            plan_item("a", "completed"),
            PlanItem {
                blocked_by: vec!["a".to_string()],
                ..plan_item("b", "queued")
            },
        ];

        let graph = to_task_graph(&plan);
        assert_eq!(graph.mode, Mode::Deep);
        assert_eq!(graph.len(), 2);
        assert!(graph.get("a").unwrap().is_done());
        assert_eq!(graph.get("b").unwrap().depends_on, vec!["a".to_string()]);

        let mut plan2 = plan.clone();
        apply_task_graph(&mut plan2, &graph);
        assert_eq!(plan2.items.len(), 2);
        let b = plan2.items.iter().find(|i| i.id == "b").unwrap();
        assert_eq!(b.blocked_by, vec!["a".to_string()]);
        assert_eq!(b.status, "queued");
    }

    #[test]
    fn engine_op_through_bridge_updates_plan() {
        let mut plan = VersionedPlan::new();
        plan.mode = "deep".to_string();

        // Seed via engine, lower back into the plan. Deep mode auto-inserts a
        // plan-wide root gate alongside the seeded node.
        let mut graph = to_task_graph(&plan);
        seed(
            &mut graph,
            vec![NodeSpec::new("root", "explore X", NodeKind::Explore)],
        )
        .unwrap();
        apply_task_graph(&mut plan, &graph);
        assert_eq!(plan.items.len(), 2);
        assert_eq!(plan.node_meta["root"].kind.as_deref(), Some("explore"));
        assert_eq!(plan.node_meta["root"].origin.as_deref(), Some("seed"));
        let root_gate_id = plan
            .items
            .iter()
            .map(|i| i.id.clone())
            .find(|id| {
                plan.node_meta
                    .get(id)
                    .map(|m| m.is_gate && m.parent.is_none())
                    .unwrap_or(false)
            })
            .expect("deep seed must lower a root gate into the plan");
        assert_eq!(
            plan.node_meta[&root_gate_id].origin.as_deref(),
            Some("gate")
        );

        // Dispatch + expand via engine, lower back; the composite's own gate must
        // appear in the plan with the composite parent marked expanded.
        let mut graph = to_task_graph(&plan);
        dispatch(&mut graph, "root", "w0");
        expand_node(
            &mut graph,
            "root",
            "w0",
            vec![NodeSpec::new("root.1", "facet", NodeKind::Explore)],
        )
        .unwrap();
        apply_task_graph(&mut plan, &graph);

        assert!(plan.node_meta["root"].expanded);
        assert_eq!(plan.node_meta["root.1"].origin.as_deref(), Some("expand"));
        let gate = plan
            .items
            .iter()
            .find(|i| {
                plan.node_meta
                    .get(&i.id)
                    .map(|m| m.is_gate && m.parent.as_deref() == Some("root"))
                    .unwrap_or(false)
            })
            .expect("gate should exist in lowered plan");
        assert_eq!(plan.node_meta[&gate.id].kind.as_deref(), Some("critique"));

        // Complete the child + gate + synthesis end to end through the bridge.
        let mut graph = to_task_graph(&plan);
        dispatch(&mut graph, "root.1", "w0");
        complete_node(
            &mut graph,
            "root.1",
            "w0",
            HandoffArtifact {
                findings: "found".into(),
                what_i_did_not_check: vec!["nothing".into()],
                confidence: Some("high".into()),
                ..HandoffArtifact::default()
            },
        )
        .unwrap();
        apply_task_graph(&mut plan, &graph);
        // The child's artifact round-trips through node_meta JSON.
        let stored = &plan.node_meta["root.1"].artifact_json;
        assert!(stored.as_ref().unwrap().contains("found"));
    }

    #[test]
    fn upstream_context_merges_completed_dependency_artifacts() {
        let mut plan = VersionedPlan::new();
        plan.items = vec![
            plan_item("dep", "completed"),
            PlanItem {
                blocked_by: vec!["dep".to_string()],
                ..plan_item("task", "queued")
            },
        ];
        plan.node_meta.insert(
            "dep".to_string(),
            NodeMeta {
                kind: Some("explore".to_string()),
                artifact_json: Some(
                    serde_json::to_string(&HandoffArtifact {
                        findings: "API in foo.rs".into(),
                        evidence: vec!["crates/foo/api.rs:12".into()],
                        ..HandoffArtifact::default()
                    })
                    .unwrap(),
                ),
                ..NodeMeta::default()
            },
        );

        let hydrated = hydrate_assignment(&plan, "task", "do the work");
        assert!(hydrated.contains("do the work"));
        assert!(hydrated.contains("Inputs from completed dependencies"));
        assert!(hydrated.contains("API in foo.rs"));
        assert!(hydrated.contains("crates/foo/api.rs:12"));

        // A task with no deps is returned unchanged.
        assert_eq!(hydrate_assignment(&plan, "dep", "x"), "x");
    }

    #[test]
    fn upstream_context_skips_incomplete_dependencies() {
        let mut plan = VersionedPlan::new();
        plan.items = vec![
            plan_item("dep", "running"),
            PlanItem {
                blocked_by: vec!["dep".to_string()],
                ..plan_item("task", "queued")
            },
        ];
        plan.node_meta.insert(
            "dep".to_string(),
            NodeMeta {
                artifact_json: Some(
                    serde_json::to_string(&HandoffArtifact::brief("partial")).unwrap(),
                ),
                ..NodeMeta::default()
            },
        );
        // dep is not completed, so no context is injected.
        assert_eq!(upstream_context(&plan, "task"), None);
    }
}
