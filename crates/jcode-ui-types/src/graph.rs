//! Graph-based memory storage with tags, clusters, and semantic links
//!
//! This module provides a graph structure for organizing memories with:
//! - Tag nodes for explicit organization
//! - Cluster nodes for automatic grouping (future)
//! - Various edge types (HasTag, RelatesTo, Supersedes, etc.)
//! - BFS cascade retrieval through the graph

use crate::{MemoryEntry, MemoryStore};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};

/// Current graph format version for migration detection
pub const GRAPH_VERSION: u32 = 2;

#[derive(Debug)]
struct TopKItem<T> {
    score: f32,
    ordinal: usize,
    value: T,
}

impl<T> PartialEq for TopKItem<T> {
    fn eq(&self, other: &Self) -> bool {
        self.score.to_bits() == other.score.to_bits() && self.ordinal == other.ordinal
    }
}

impl<T> Eq for TopKItem<T> {}

impl<T> PartialOrd for TopKItem<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for TopKItem<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.score
            .total_cmp(&other.score)
            .then_with(|| self.ordinal.cmp(&other.ordinal))
    }
}

fn top_k_scored<T, I>(items: I, limit: usize) -> Vec<(T, f32)>
where
    I: IntoIterator<Item = (T, f32)>,
{
    if limit == 0 {
        return Vec::new();
    }

    let mut heap: BinaryHeap<Reverse<TopKItem<T>>> = BinaryHeap::new();
    for (ordinal, (value, score)) in items.into_iter().enumerate() {
        let candidate = Reverse(TopKItem {
            score,
            ordinal,
            value,
        });

        if heap.len() < limit {
            heap.push(candidate);
            continue;
        }

        let replace = heap
            .peek()
            .map(|smallest| score > smallest.0.score)
            .unwrap_or(false);
        if replace {
            heap.pop();
            heap.push(candidate);
        }
    }

    let mut results: Vec<_> = heap
        .into_iter()
        .map(|Reverse(item)| (item.value, item.score, item.ordinal))
        .collect();
    results.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.2.cmp(&b.2)));
    results
        .into_iter()
        .map(|(value, score, _)| (value, score))
        .collect()
}

/// Edge relationship types between nodes
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EdgeKind {
    /// Memory has this explicit tag
    HasTag,
    /// Memory belongs to auto-discovered cluster
    InCluster,
    /// Semantic relationship with weight (0.0-1.0)
    RelatesTo {
        #[serde(default = "default_weight")]
        weight: f32,
    },
    /// Newer memory replaces older one
    Supersedes,
    /// Conflicting information (both kept, flagged)
    Contradicts,
    /// Procedural knowledge derived from facts
    DerivedFrom,
}

fn default_weight() -> f32 {
    1.0
}

impl EdgeKind {
    /// Get the traversal weight for BFS scoring
    pub fn traversal_weight(&self) -> f32 {
        match self {
            EdgeKind::HasTag => 0.8,
            EdgeKind::InCluster => 0.6,
            EdgeKind::RelatesTo { weight } => *weight,
            EdgeKind::Supersedes => 0.9,
            EdgeKind::Contradicts => 0.3,
            EdgeKind::DerivedFrom => 0.7,
        }
    }
}

/// An edge in the memory graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    /// Target node ID
    pub target: String,
    /// Type of relationship
    #[serde(flatten)]
    pub kind: EdgeKind,
}

impl Edge {
    pub fn new(target: impl Into<String>, kind: EdgeKind) -> Self {
        Self {
            target: target.into(),
            kind,
        }
    }
}

/// A tag node in the graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagEntry {
    /// Unique ID (format: "tag:{name}")
    pub id: String,
    /// Display name
    pub name: String,
    /// Optional description
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Number of memories with this tag
    pub count: u32,
    /// When the tag was first created
    pub created_at: DateTime<Utc>,
}

impl TagEntry {
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            id: format!("tag:{}", name),
            name,
            description: None,
            count: 0,
            created_at: Utc::now(),
        }
    }

    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }
}

/// A cluster node (auto-discovered grouping via embeddings)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterEntry {
    /// Unique ID (format: "cluster:{id}")
    pub id: String,
    /// Optional human-readable name
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Centroid embedding (average of member embeddings)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub centroid: Vec<f32>,
    /// Number of memories in this cluster
    pub member_count: u32,
    /// When the cluster was discovered
    pub created_at: DateTime<Utc>,
    /// When the cluster was last updated
    pub updated_at: DateTime<Utc>,
}

impl ClusterEntry {
    pub fn new(id: impl Into<String>) -> Self {
        let id = id.into();
        let now = Utc::now();
        Self {
            id: format!("cluster:{}", id),
            name: None,
            centroid: Vec::new(),
            member_count: 0,
            created_at: now,
            updated_at: now,
        }
    }
}

/// Graph metadata for tracking statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GraphMetadata {
    /// When clusters were last updated
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_cluster_update: Option<DateTime<Utc>>,
    /// Total retrieval operations
    #[serde(default)]
    pub retrieval_count: u64,
    /// Total links discovered via co-relevance
    #[serde(default)]
    pub link_discovery_count: u64,
}

/// The memory graph - HashMap-based for clean JSON serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryGraph {
    /// Format version for migration detection
    pub graph_version: u32,

    /// Memory nodes by ID
    pub memories: HashMap<String, MemoryEntry>,

    /// Tag nodes by ID (format: "tag:{name}")
    pub tags: HashMap<String, TagEntry>,

    /// Cluster nodes by ID (format: "cluster:{id}")
    #[serde(default)]
    pub clusters: HashMap<String, ClusterEntry>,

    /// Forward edges: source_id -> Vec<Edge>
    #[serde(default)]
    pub edges: HashMap<String, Vec<Edge>>,

    /// Reverse edges for efficient BFS: target_id -> Vec<source_id>
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub reverse_edges: HashMap<String, Vec<String>>,

    /// Graph statistics and metadata
    #[serde(default)]
    pub metadata: GraphMetadata,
}

impl Default for MemoryGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryGraph {
    /// Create a new empty memory graph
    pub fn new() -> Self {
        Self {
            graph_version: GRAPH_VERSION,
            memories: HashMap::new(),
            tags: HashMap::new(),
            clusters: HashMap::new(),
            edges: HashMap::new(),
            reverse_edges: HashMap::new(),
            metadata: GraphMetadata::default(),
        }
    }

    /// Get the number of memories in the graph
    pub fn memory_count(&self) -> usize {
        self.memories.len()
    }

    // ==================== Memory Operations ====================

    /// Add a memory entry to the graph
    /// Also creates tag nodes and HasTag edges for any tags on the entry
    pub fn add_memory(&mut self, mut entry: MemoryEntry) -> String {
        entry.refresh_search_text();
        let id = entry.id.clone();

        // Create tag nodes and edges for existing tags
        for tag_name in &entry.tags {
            self.ensure_tag(tag_name);
            let tag_id = format!("tag:{}", tag_name);
            self.add_edge_internal(&id, &tag_id, EdgeKind::HasTag);

            // Increment tag count
            if let Some(tag) = self.tags.get_mut(&tag_id) {
                tag.count += 1;
            }
        }

        // Handle superseded_by as a Supersedes edge (reverse direction)
        if let Some(ref superseded_by) = entry.superseded_by {
            // The newer memory supersedes this one
            self.add_edge_internal(superseded_by, &id, EdgeKind::Supersedes);
        }

        self.memories.insert(id.clone(), entry);
        id
    }

    /// Get a memory by ID
    pub fn get_memory(&self, id: &str) -> Option<&MemoryEntry> {
        self.memories.get(id)
    }

    /// Get a mutable memory by ID
    pub fn get_memory_mut(&mut self, id: &str) -> Option<&mut MemoryEntry> {
        self.memories.get_mut(id)
    }

    /// Remove a memory from the graph (also removes associated edges)
    pub fn remove_memory(&mut self, id: &str) -> Option<MemoryEntry> {
        // Remove all edges from this memory
        if let Some(edges) = self.edges.remove(id) {
            for edge in edges {
                // Update reverse edges
                if let Some(reverse) = self.reverse_edges.get_mut(&edge.target) {
                    reverse.retain(|src| src != id);
                }
                // Decrement tag count if HasTag
                if matches!(edge.kind, EdgeKind::HasTag)
                    && let Some(tag) = self.tags.get_mut(&edge.target)
                {
                    tag.count = tag.count.saturating_sub(1);
                }
            }
        }

        // Remove all edges pointing to this memory
        if let Some(sources) = self.reverse_edges.remove(id) {
            for source in sources {
                if let Some(edges) = self.edges.get_mut(&source) {
                    edges.retain(|e| e.target != id);
                }
            }
        }

        self.memories.remove(id)
    }

    /// Get all memories (for iteration)
    pub fn all_memories(&self) -> impl Iterator<Item = &MemoryEntry> {
        self.memories.values()
    }

    /// Get all active memories
    pub fn active_memories(&self) -> impl Iterator<Item = &MemoryEntry> {
        self.memories.values().filter(|m| m.active)
    }

    // ==================== Tag Operations ====================

    /// Ensure a tag exists, creating it if necessary
    pub fn ensure_tag(&mut self, name: &str) -> &TagEntry {
        let tag_id = format!("tag:{}", name);
        self.tags
            .entry(tag_id.clone())
            .or_insert_with(|| TagEntry::new(name))
    }

    /// Add a tag to a memory
    pub fn tag_memory(&mut self, memory_id: &str, tag_name: &str) {
        // Ensure tag exists
        self.ensure_tag(tag_name);
        let tag_id = format!("tag:{}", tag_name);

        // Check if edge already exists
        if let Some(edges) = self.edges.get(memory_id)
            && edges
                .iter()
                .any(|e| e.target == tag_id && matches!(e.kind, EdgeKind::HasTag))
        {
            return;
        }

        // Add edge
        self.add_edge_internal(memory_id, &tag_id, EdgeKind::HasTag);

        // Update tag count
        if let Some(tag) = self.tags.get_mut(&tag_id) {
            tag.count += 1;
        }

        // Update memory's tags list
        if let Some(memory) = self.memories.get_mut(memory_id)
            && !memory.tags.contains(&tag_name.to_string())
        {
            memory.tags.push(tag_name.to_string());
            memory.refresh_search_text();
        }
    }

    /// Remove a tag from a memory
    pub fn untag_memory(&mut self, memory_id: &str, tag_name: &str) {
        let tag_id = format!("tag:{}", tag_name);

        // Remove edge
        if let Some(edges) = self.edges.get_mut(memory_id) {
            edges.retain(|e| !(e.target == tag_id && matches!(e.kind, EdgeKind::HasTag)));
        }

        // Update reverse edges
        if let Some(sources) = self.reverse_edges.get_mut(&tag_id) {
            sources.retain(|s| s != memory_id);
        }

        // Update tag count
        if let Some(tag) = self.tags.get_mut(&tag_id) {
            tag.count = tag.count.saturating_sub(1);
        }

        // Update memory's tags list
        if let Some(memory) = self.memories.get_mut(memory_id) {
            memory.tags.retain(|t| t != tag_name);
            memory.refresh_search_text();
        }
    }

    /// Get all memories with a specific tag
    pub fn get_memories_by_tag(&self, tag_name: &str) -> Vec<&MemoryEntry> {
        let tag_id = format!("tag:{}", tag_name);

        // Find all sources pointing to this tag via HasTag
        self.reverse_edges
            .get(&tag_id)
            .map(|sources| {
                sources
                    .iter()
                    .filter_map(|id| self.memories.get(id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get all tags
    pub fn all_tags(&self) -> impl Iterator<Item = &TagEntry> {
        self.tags.values()
    }

    // ==================== Edge Operations ====================

    /// Add an edge between two nodes (internal, no validation)
    fn add_edge_internal(&mut self, from: &str, to: &str, kind: EdgeKind) {
        // Add forward edge
        self.edges
            .entry(from.to_string())
            .or_default()
            .push(Edge::new(to, kind));

        // Add reverse edge
        self.reverse_edges
            .entry(to.to_string())
            .or_default()
            .push(from.to_string());
    }

    /// Add an edge between two nodes
    pub fn add_edge(&mut self, from: &str, to: &str, kind: EdgeKind) {
        // Check if edge already exists
        if let Some(edges) = self.edges.get(from)
            && edges.iter().any(|e| e.target == to && e.kind == kind)
        {
            return;
        }

        self.add_edge_internal(from, to, kind);
    }

    /// Remove an edge between two nodes
    pub fn remove_edge(&mut self, from: &str, to: &str, kind: &EdgeKind) {
        if let Some(edges) = self.edges.get_mut(from) {
            edges.retain(|e| !(e.target == to && &e.kind == kind));
        }
        if let Some(sources) = self.reverse_edges.get_mut(to) {
            sources.retain(|s| s != from);
        }
    }

    /// Get all edges from a node
    pub fn get_edges(&self, node_id: &str) -> &[Edge] {
        self.edges.get(node_id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Get all nodes pointing to this node
    pub fn get_incoming(&self, node_id: &str) -> Vec<&str> {
        self.reverse_edges
            .get(node_id)
            .map(|v| v.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default()
    }

    /// Link two memories with a RelatesTo edge
    pub fn link_memories(&mut self, from: &str, to: &str, weight: f32) {
        self.add_edge(from, to, EdgeKind::RelatesTo { weight });
        self.metadata.link_discovery_count += 1;
    }

    /// Mark a memory as superseding another
    pub fn supersede(&mut self, newer_id: &str, older_id: &str) {
        self.add_edge(newer_id, older_id, EdgeKind::Supersedes);
        // Mark older as inactive
        if let Some(older) = self.memories.get_mut(older_id) {
            older.active = false;
            older.superseded_by = Some(newer_id.to_string());
        }
    }

    /// Mark two memories as contradicting
    pub fn mark_contradiction(&mut self, id_a: &str, id_b: &str) {
        self.add_edge(id_a, id_b, EdgeKind::Contradicts);
        self.add_edge(id_b, id_a, EdgeKind::Contradicts);
    }

    // ==================== Graph Stats ====================

    /// Get total number of nodes (memories + tags + clusters)
    pub fn node_count(&self) -> usize {
        self.memories.len() + self.tags.len() + self.clusters.len()
    }

    /// Get total number of edges
    pub fn edge_count(&self) -> usize {
        self.edges.values().map(|v| v.len()).sum()
    }

    // ==================== Cascade Retrieval ====================

    /// Perform BFS cascade retrieval starting from seed memories
    ///
    /// Starting from embedding search hits (seeds), traverse through the graph
    /// via tags and other edges to find related memories.
    ///
    /// Returns (memory_id, score) pairs sorted by score descending.
    pub fn cascade_retrieve(
        &mut self,
        seed_ids: &[String],
        seed_scores: &[f32],
        max_depth: usize,
        max_results: usize,
    ) -> Vec<(String, f32)> {
        self.metadata.retrieval_count += 1;

        let mut visited: HashSet<String> = HashSet::new();
        let mut results: HashMap<String, f32> = HashMap::new();
        let mut queue: VecDeque<(String, f32, usize)> = VecDeque::new();

        // Initialize with seeds
        for (id, score) in seed_ids.iter().zip(seed_scores.iter()) {
            if self.memories.contains_key(id) {
                queue.push_back((id.clone(), *score, 0));
                results.insert(id.clone(), *score);
            }
        }

        // BFS traversal
        while let Some((node_id, score, depth)) = queue.pop_front() {
            if visited.contains(&node_id) {
                continue;
            }
            visited.insert(node_id.clone());

            if depth >= max_depth {
                continue;
            }

            // Traverse edges from this node
            for edge in self.get_edges(&node_id).to_vec() {
                let target = &edge.target;

                // Skip if already visited
                if visited.contains(target) {
                    continue;
                }

                // Calculate decayed score
                let edge_weight = edge.kind.traversal_weight();
                let decay = 0.7_f32.powi(depth as i32 + 1);
                let new_score = score * edge_weight * decay;

                // If target is a tag, find all memories with this tag
                if target.starts_with("tag:") {
                    for source_id in self.get_incoming(target).iter() {
                        let source_id = source_id.to_string();
                        if !visited.contains(&source_id) && self.memories.contains_key(&source_id) {
                            let existing = results.get(&source_id).copied().unwrap_or(0.0);
                            if new_score > existing {
                                results.insert(source_id.clone(), new_score);
                                queue.push_back((source_id, new_score, depth + 1));
                            }
                        }
                    }
                }
                // If target is a memory, add it
                else if self.memories.contains_key(target) {
                    let existing = results.get(target).copied().unwrap_or(0.0);
                    if new_score > existing {
                        results.insert(target.clone(), new_score);
                        queue.push_back((target.clone(), new_score, depth + 1));
                    }
                }
            }
        }

        // Keep only the top-scoring results
        top_k_scored(results, max_results)
    }

    // ==================== Migration ====================

    /// Convert a legacy MemoryStore to a MemoryGraph
    ///
    /// This handles migration from the old flat JSON format to the graph format.
    pub fn from_legacy_store(store: MemoryStore) -> Self {
        let mut graph = MemoryGraph::new();

        for entry in store.entries {
            let memory_id = entry.id.clone();
            let tags = entry.tags.clone();
            let superseded_by = entry.superseded_by.clone();

            // Add memory (this will also create tag nodes and HasTag edges)
            graph.memories.insert(memory_id.clone(), entry);

            // Create tag nodes and edges
            for tag_name in &tags {
                graph.ensure_tag(tag_name);
                let tag_id = format!("tag:{}", tag_name);
                graph.add_edge_internal(&memory_id, &tag_id, EdgeKind::HasTag);

                // Update tag count
                if let Some(tag) = graph.tags.get_mut(&tag_id) {
                    tag.count += 1;
                }
            }

            // Create Supersedes edge if applicable
            if let Some(ref newer_id) = superseded_by {
                // newer_id supersedes memory_id
                graph.add_edge_internal(newer_id, &memory_id, EdgeKind::Supersedes);
            }
        }

        graph
    }

    /// Check if this graph was migrated from legacy format
    pub fn is_migrated(&self) -> bool {
        self.graph_version == GRAPH_VERSION
    }
}

#[cfg(test)]
mod graph_tests;
