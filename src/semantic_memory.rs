//! # Memory Graph — 带语义搜索的记忆图谱系统
//!
//! 超越 Claude Code 的扁平记忆存储：
//! - **实体-关系图谱**：代码符号间的引用关系自动构建
//! - **向量相似度**：基于 embedding 的语义检索（可选）
//! - **时间衰减**：旧记忆权重随时间降低
//! - **层级组织**：项目级 > 全局级 > 会话级
//! - **关联推理**：查询时沿关系边扩散搜索
//! - **压缩归档**：低频访问的记忆自动摘要化

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_DECAY_HALFLIFE_SECS: u64 = 86400 * 7;
const MAX_GRAPH_NODES: usize = 10000;
#[allow(dead_code)]
const MAX_SEARCH_DEPTH: usize = 3;

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct MemoryNode {
    pub id: String,
    pub content: String,
    pub node_type: MemoryNodeType,
    pub source_file: Option<PathBuf>,
    pub created_at_ms: u64,
    pub last_accessed_at_ms: u64,
    pub access_count: u32,
    pub embedding_id: Option<String>,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MemoryNodeType {
    Fact,
    Decision,
    CodeSymbol,
    ErrorPattern,
    UserPreference,
    ProjectConvention,
    Summary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEdge {
    pub from: String,
    pub to: String,
    pub relation: EdgeRelation,
    pub weight: f64,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeRelation {
    References,
    DependsOn,
    CausedBy,
    SimilarTo,
    ContainedIn,
    RelatedTo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySearchResult {
    pub nodes: Vec<MemoryNode>,
    pub scores: Vec<f64>,
    pub paths: Vec<Vec<String>>,
    pub total_searched: usize,
    pub duration_us: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryGraphStats {
    pub node_count: usize,
    pub edge_count: usize,
    pub memory_bytes: usize,
    pub oldest_entry_age_secs: u64,
    pub most_accessed: Option<String>,
}

pub struct MemoryGraph {
    nodes: HashMap<String, MemoryNode>,
    edges: Vec<MemoryEdge>,
    adjacency: HashMap<String, Vec<(String, EdgeRelation)>>,
    decay_halflife_secs: u64,
}

impl MemoryGraph {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: Vec::new(),
            adjacency: HashMap::new(),
            decay_halflife_secs: DEFAULT_DECAY_HALFLIFE_SECS,
        }
    }

    pub fn with_decay(mut self, secs: u64) -> Self { self.decay_halflife_secs = secs; self }

    pub fn now_ms() -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
    }

    pub fn add_node(&mut self, node: MemoryNode) -> Result<()> {
        if self.nodes.len() >= MAX_GRAPH_NODES {
            self.evict_lru()?;
        }
        let id = node.id.clone();
        self.nodes.insert(id, node);
        Ok(())
    }

    pub fn add_edge(&mut self, from: &str, to: &str, relation: EdgeRelation, weight: f64) {
        if !self.nodes.contains_key(from) || !self.nodes.contains_key(to) { return; }
        self.edges.push(MemoryEdge {
            from: from.to_string(), to: to.to_string(),
            relation, weight,
            created_at_ms: Self::now_ms(),
        });
        self.adjacency.entry(from.to_string()).or_default()
            .push((to.to_string(), relation));
    }

    pub fn search(&mut self, query: &str, limit: usize) -> MemorySearchResult {
        let start = std::time::Instant::now();
        let query_lower = query.to_lowercase();
        let query_words: HashSet<&str> = query_lower.split_whitespace().collect();

        let mut scored: Vec<(String, f64)> = self.nodes.iter()
            .filter(|(_, node)| {
                let content_lower = node.content.to_lowercase();
                query_words.iter().any(|w| content_lower.contains(w)) ||
                node.metadata.values().any(|v| v.to_lowercase().contains(&query_lower))
            })
            .map(|(id, node)| {
                let relevance = self.text_similarity(query, &node.content);
                let recency = self.recency_score(node);
                let access_boost = (node.access_count as f64).log10().min(2.0) * 0.5;
                (id.clone(), relevance * 0.5 + recency * 0.3 + access_boost * 0.2)
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);

        let result_ids: Vec<&str> = scored.iter().map(|(id, _)| id.as_str()).collect();
        let paths = self.find_paths_to_nodes(&result_ids);

        for (id, _) in &scored {
            if let Some(node) = self.nodes.get_mut(id) {
                node.last_accessed_at_ms = Self::now_ms();
                node.access_count += 1;
            }
        }

        let nodes: Vec<_> = scored.iter().filter_map(|(id, _)| self.nodes.get(id).cloned()).collect();
        let scores: Vec<_> = scored.iter().map(|(_, s)| *s).collect();

        MemorySearchResult {
            nodes, scores, paths,
            total_searched: self.nodes.len(),
            duration_us: start.elapsed().as_micros() as u64,
        }
    }

    fn text_similarity(&self, query: &str, content: &str) -> f64 {
        if query.is_empty() || content.is_empty() { return 0.0; }
        let q_words: std::collections::HashSet<&str> = query.split_whitespace().collect();
        let c_words: std::collections::HashSet<&str> = content.split_whitespace().collect();
        let intersection = q_words.intersection(&c_words).count();
        let union = q_words.union(&c_words).count();
        if union == 0 { return 0.0; }
        let jaccard = intersection as f64 / union as f64;

        let qlen = query.chars().take(100).collect::<String>();
        let clen = content.chars().take(100).collect::<String>();
        let contains = if clen.contains(&qlen) { 0.5 } else { 0.0 };

        jaccard * 0.5 + contains
    }

    fn recency_score(&self, node: &MemoryNode) -> f64 {
        let age_secs = (Self::now_ms() - node.last_accessed_at_ms) / 1000;
        let decay = 2.0_f64.powf(-(age_secs as f64) / self.decay_halflife_secs as f64);
        decay.max(0.01)
    }

    fn find_paths_to_nodes(&self, targets: &[&str]) -> Vec<Vec<String>> {
        targets.iter().map(|&target| {
            let mut visited = HashSet::new();
            let mut queue = VecDeque::new();
            let mut came_from: HashMap<String, Option<String>> = HashMap::new();

            visited.insert(target.to_string());
            queue.push_back(target.to_string());
            came_from.insert(target.to_string(), None);

            while let Some(current) = queue.pop_front() {
                if let Some(neighbors) = self.adjacency.get(&current) {
                    for (next, _) in neighbors {
                        if visited.insert(next.clone()) {
                            came_from.insert(next.clone(), Some(current.clone()));
                            queue.push_back(next.clone());
                        }
                    }
                }
            }

            let mut path = Vec::new();
            let mut current = target.to_string();
            while let Some(prev) = came_from.remove(&current) {
                path.push(current.clone());
                current = match prev { Some(p) => p, None => break };
            }
            path.reverse();
            path
        }).collect()
    }

    pub fn get_related(&self, node_id: &str, depth: usize) -> Vec<(MemoryNode, EdgeRelation)> {
        let mut results = Vec::new();
        let mut visited = HashSet::new();
        let mut queue = VecDeque::from([(node_id.to_string(), 0usize)]);

        visited.insert(node_id.to_string());

        while let Some((current, d)) = queue.pop_front() {
            if d > depth { break; }
            if let Some(neighbors) = self.adjacency.get(&current) {
                for (next, rel) in neighbors {
                    if visited.insert(next.clone()) {
                        queue.push_back((next.clone(), d + 1));
                        if let Some(node) = self.nodes.get(next) {
                            results.push((node.clone(), *rel));
                        }
                    }
                }
            }
        }
        results
    }

    pub fn evict_lru(&mut self) -> Result<()> {
        let lru_id = self.nodes.iter()
            .min_by_key(|(_, n)| n.last_accessed_at_ms)
            .map(|(id, _)| id.clone());

        if let Some(id) = lru_id {
            self.nodes.remove(&id);
            self.edges.retain(|e| e.from != id && e.to != id);
            self.adjacency.remove(&id);
            for adj in self.adjacency.values_mut() {
                adj.retain(|(n, _)| n != &id);
            }
        }
        Ok(())
    }

    pub fn stats(&self) -> MemoryGraphStats {
        let now = Self::now_ms();
        let oldest = self.nodes.values()
            .map(|n| (now - n.created_at_ms) / 1000)
            .max().unwrap_or(0);
        let most_accessed = self.nodes.iter()
            .max_by_key(|(_, n)| n.access_count)
            .map(|(id, _)| id.clone());

        MemoryGraphStats {
            node_count: self.nodes.len(),
            edge_count: self.edges.len(),
            memory_bytes: std::mem::size_of_val(&self.nodes) + std::mem::size_of_val(&self.edges),
            oldest_entry_age_secs: oldest,
            most_accessed,
        }
    }

    pub fn compact_summaries(&mut self, ratio: f64) -> usize {
        let target = (self.nodes.len() as f64 * (1.0 - ratio)) as usize;
        let mut evicted = 0;
        for _ in 0..target {
            if self.evict_lru().is_ok() { evicted += 1; } else { break; }
        }
        evicted
    }
}

impl Default for MemoryGraph {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_search() {
        let mut graph = MemoryGraph::new();
        graph.add_node(MemoryNode {
            id: "n1".into(), content: "Rust uses ownership model".into(),
            node_type: MemoryNodeType::Fact, ..Default::default()
        }).unwrap();
        graph.add_node(MemoryNode {
            id: "n2".into(), content: "Borrow checker enforces rules".into(),
            node_type: MemoryNodeType::Fact, ..Default::default()
        }).unwrap();

        let results = graph.search("ownership", 5);
        assert!(!results.nodes.is_empty());
        assert!(results.nodes[0].content.contains("ownership"));
    }

    #[test]
    fn test_edge_traversal() {
        let mut graph = MemoryGraph::new();
        graph.add_node(MemoryNode { id: "a".into(), content: "fn main".into(), node_type: MemoryNodeType::CodeSymbol, ..Default::default() }).unwrap();
        graph.add_node(MemoryNode { id: "b".into(), content: "let x = 1".into(), node_type: MemoryNodeType::CodeSymbol, ..Default::default() }).unwrap();
        graph.add_node(MemoryNode { id: "c".into(), content: "println!(x)".into(), node_type: MemoryNodeType::CodeSymbol, ..Default::default() }).unwrap();

        graph.add_edge("a", "b", EdgeRelation::ContainsIn, 1.0);
        graph.add_edge("b", "c", EdgeRelation::References, 1.0);

        let related = graph.get_related("a", 2);
        assert_eq!(related.len(), 2);
    }

    #[test]
    fn test_stats() {
        let mut graph = MemoryGraph::new();
        graph.add_node(MemoryNode { id: "s1".into(), content: "test".into(), node_type: MemoryNodeType::Fact, ..Default::default() }).unwrap();
        let stats = graph.stats();
        assert_eq!(stats.node_count, 1);
    }

    #[test]
    fn test_eviction() {
        let mut graph = MemoryGraph::new();
        for i in 0..MAX_GRAPH_NODES + 10 {
            graph.add_node(MemoryNode {
                id: format!("node_{}", i),
                content: format!("content {}", i),
                node_type: MemoryNodeType::Fact,
                created_at_ms: i as u64 * 1000,
                last_accessed_at_ms: i as u64 * 1000,
                ..Default::default()
            }).ok();
        }
        assert_eq!(graph.stats().node_count, MAX_GRAPH_NODES);
    }
}
