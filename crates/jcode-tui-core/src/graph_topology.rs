#[derive(Debug, Clone)]
pub struct GraphNode {
    /// Stable node ID from memory graph (mem:*, tag:*, cluster:*)
    pub id: String,
    /// Human-readable display label
    pub label: String,
    /// Category: "fact", "preference", "correction", "tag"
    pub kind: String,
    /// Whether this node is a memory (vs tag/cluster)
    pub is_memory: bool,
    /// Whether this node is active (superseded memories are inactive)
    pub is_active: bool,
    /// Effective confidence score (0.0-1.0)
    pub confidence: f32,
    /// Number of connections (degree)
    pub degree: usize,
}

#[derive(Debug, Clone)]
pub struct GraphEdge {
    /// Source index into MemoryInfo::graph_nodes
    pub source: usize,
    /// Target index into MemoryInfo::graph_nodes
    pub target: usize,
    /// Edge kind (has_tag, supersedes, contradicts, ...)
    pub kind: String,
}

fn truncate_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

fn truncate_smart(s: &str, max_len: usize) -> String {
    let char_len = s.chars().count();
    if char_len <= max_len {
        return s.to_string();
    }
    if max_len <= 3 {
        return "...".to_string();
    }

    let target = max_len - 3;
    let prefix = truncate_chars(s, target);

    if let Some(pos) = prefix.rfind(' ') {
        let before = &prefix[..pos];
        let pos_chars = before.chars().count();
        if pos_chars > target / 2 {
            return format!("{}...", before);
        }
    }
    format!("{}...", prefix)
}

use jcode_memory_types::{EdgeKind, MemoryGraph};
use std::collections::{HashMap, HashSet};

/// Build graph topology (nodes + edges) from a MemoryGraph for visualization.
/// Combines project and global graphs, sampling nodes if there are too many.
pub fn build_graph_topology(
    project: Option<&MemoryGraph>,
    global: Option<&MemoryGraph>,
) -> (Vec<GraphNode>, Vec<GraphEdge>) {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut id_to_idx: HashMap<String, usize> = HashMap::new();

    // Collect all memory nodes from both graphs.
    // Sort keys for deterministic iteration order (HashMap order is random,
    // which causes the graph layout to jitter on every frame redraw).
    let graphs: Vec<&MemoryGraph> = [project, global].into_iter().flatten().collect();

    for graph in &graphs {
        collect_memory_nodes(graph, &mut nodes, &mut id_to_idx);
        collect_tag_nodes(graph, &mut nodes, &mut id_to_idx);
        collect_cluster_nodes(graph, &mut nodes, &mut id_to_idx);
    }

    collect_edges(&graphs, &id_to_idx, &mut nodes, &mut edges);

    bound_topology_size(nodes, edges)
}

fn collect_memory_nodes(
    graph: &MemoryGraph,
    nodes: &mut Vec<GraphNode>,
    id_to_idx: &mut HashMap<String, usize>,
) {
    let mut memory_ids: Vec<&String> = graph.memories.keys().collect();
    memory_ids.sort();

    for id in memory_ids {
        let entry = &graph.memories[id];
        if id_to_idx.contains_key(id) {
            continue;
        }

        let idx = nodes.len();
        id_to_idx.insert(id.clone(), idx);
        nodes.push(GraphNode {
            id: id.clone(),
            label: truncate_smart(&entry.content, 30),
            kind: entry.category.to_string(),
            is_memory: true,
            is_active: entry.active,
            confidence: entry.effective_confidence(),
            degree: 0,
        });
    }
}

fn collect_tag_nodes(
    graph: &MemoryGraph,
    nodes: &mut Vec<GraphNode>,
    id_to_idx: &mut HashMap<String, usize>,
) {
    let mut tag_ids: Vec<&String> = graph.tags.keys().collect();
    tag_ids.sort();

    for id in tag_ids {
        if id_to_idx.contains_key(id) {
            continue;
        }

        let idx = nodes.len();
        let label = graph
            .tags
            .get(id)
            .map(|tag| truncate_smart(&tag.name, 22))
            .unwrap_or_else(|| id.trim_start_matches("tag:").to_string());
        id_to_idx.insert(id.clone(), idx);
        nodes.push(GraphNode {
            id: id.clone(),
            label,
            kind: "tag".to_string(),
            is_memory: false,
            is_active: true,
            confidence: 1.0,
            degree: 0,
        });
    }
}

fn collect_cluster_nodes(
    graph: &MemoryGraph,
    nodes: &mut Vec<GraphNode>,
    id_to_idx: &mut HashMap<String, usize>,
) {
    let mut cluster_ids: Vec<&String> = graph.clusters.keys().collect();
    cluster_ids.sort();

    for id in cluster_ids {
        if id_to_idx.contains_key(id) {
            continue;
        }

        let idx = nodes.len();
        let label = graph
            .clusters
            .get(id)
            .and_then(|cluster| cluster.name.clone())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| id.trim_start_matches("cluster:").to_string());
        id_to_idx.insert(id.clone(), idx);
        nodes.push(GraphNode {
            id: id.clone(),
            label: truncate_smart(&label, 22),
            kind: "cluster".to_string(),
            is_memory: false,
            is_active: true,
            confidence: 1.0,
            degree: 0,
        });
    }
}

fn collect_edges(
    graphs: &[&MemoryGraph],
    id_to_idx: &HashMap<String, usize>,
    nodes: &mut [GraphNode],
    edges: &mut Vec<GraphEdge>,
) {
    let mut edge_seen: HashSet<(usize, usize, String)> = HashSet::new();

    for graph in graphs {
        let mut edge_src_ids: Vec<&String> = graph.edges.keys().collect();
        edge_src_ids.sort();

        for src_id in edge_src_ids {
            let edge_list = &graph.edges[src_id];
            let Some(&src_idx) = id_to_idx.get(src_id) else {
                continue;
            };

            let mut sorted_edges = edge_list.clone();
            sorted_edges.sort_by(|a, b| {
                a.target
                    .cmp(&b.target)
                    .then_with(|| edge_kind_name(&a.kind).cmp(edge_kind_name(&b.kind)))
            });

            for edge in sorted_edges {
                let Some(&tgt_idx) = id_to_idx.get(&edge.target) else {
                    continue;
                };
                if src_idx == tgt_idx {
                    continue;
                }

                let kind = edge_kind_name(&edge.kind).to_string();
                if !edge_seen.insert((src_idx, tgt_idx, kind.clone())) {
                    continue;
                }

                edges.push(GraphEdge {
                    source: src_idx,
                    target: tgt_idx,
                    kind,
                });
                if src_idx < nodes.len() {
                    nodes[src_idx].degree += 1;
                }
                if tgt_idx < nodes.len() {
                    nodes[tgt_idx].degree += 1;
                }
            }
        }
    }
}

fn bound_topology_size(
    mut nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
) -> (Vec<GraphNode>, Vec<GraphEdge>) {
    // Bound topology size for stable redraw cost while preserving enough
    // neighborhood signal for contextual subgraph selection.
    const MAX_NODES: usize = 96;

    if nodes.len() <= MAX_NODES {
        return (nodes, edges);
    }

    let mut indices: Vec<usize> = (0..nodes.len()).collect();
    indices.sort_by(|&a, &b| {
        graph_node_score(&nodes[b])
            .partial_cmp(&graph_node_score(&nodes[a]))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.cmp(&a))
    });

    let keep: HashSet<usize> = indices.into_iter().take(MAX_NODES).collect();

    let mut new_nodes = Vec::new();
    let mut old_to_new: HashMap<usize, usize> = HashMap::new();
    for (old_idx, node) in nodes.drain(..).enumerate() {
        if keep.contains(&old_idx) {
            let new_idx = new_nodes.len();
            old_to_new.insert(old_idx, new_idx);
            new_nodes.push(node);
        }
    }

    let new_edges = edges
        .into_iter()
        .filter_map(|edge| {
            let source = *old_to_new.get(&edge.source)?;
            let target = *old_to_new.get(&edge.target)?;
            Some(GraphEdge {
                source,
                target,
                kind: edge.kind,
            })
        })
        .collect();

    (new_nodes, new_edges)
}

fn edge_kind_name(kind: &EdgeKind) -> &'static str {
    match kind {
        EdgeKind::HasTag => "has_tag",
        EdgeKind::InCluster => "in_cluster",
        EdgeKind::RelatesTo { .. } => "relates_to",
        EdgeKind::Supersedes => "supersedes",
        EdgeKind::Invalidates => "invalidates",
        EdgeKind::Contradicts => "contradicts",
        EdgeKind::DerivedFrom => "derived_from",
    }
}

pub fn graph_node_score(node: &GraphNode) -> f32 {
    let memory_bias = if node.is_memory { 2.0 } else { 0.0 };
    let active_bias = if node.is_active { 1.0 } else { 0.0 };
    node.degree as f32 + memory_bias + active_bias + node.confidence * 2.0
}

#[cfg(test)]
mod tests {
    use super::build_graph_topology;
    use jcode_memory_types::{Edge, EdgeKind, MemoryCategory, MemoryEntry, MemoryGraph};

    #[test]
    fn build_graph_topology_deduplicates_nodes_across_project_and_global_graphs() {
        let mut graph = MemoryGraph::new();
        let mut entry = MemoryEntry::new(MemoryCategory::Fact, "Rust uses cargo workspaces");
        entry.tags.push("rust".to_string());
        let memory_id = graph.add_memory(entry);
        graph
            .edges
            .entry(memory_id.clone())
            .or_default()
            .push(Edge::new("tag:rust", EdgeKind::HasTag));

        let (nodes, edges) = build_graph_topology(Some(&graph), Some(&graph));

        assert_eq!(nodes.len(), 2);
        assert_eq!(edges.len(), 1);
    }

    #[test]
    fn build_graph_topology_caps_large_graphs_for_stable_rendering() {
        let mut graph = MemoryGraph::new();
        for i in 0..120 {
            graph.add_memory(MemoryEntry::new(
                MemoryCategory::Fact,
                format!("Fact {i}: topology remains bounded"),
            ));
        }

        let (nodes, _) = build_graph_topology(Some(&graph), None);

        assert_eq!(nodes.len(), 96);
    }
}
