//! HNSW (Hierarchical Navigable Small World) Index for Efficient Vector Similarity Search
//!
//! Provides approximate nearest neighbor search with O(log N) complexity.
//! Used for offline RAG retrieval when pgvector/Milvus is unavailable.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// HNSW graph node
#[derive(Debug, Clone)]
struct HNSWNode {
    /// Point ID
    id: usize,
    /// Vector embedding
    vector: Vec<f32>,
    /// Neighbors at each layer (layer 0 = bottom, highest = top)
    neighbors: Vec<Vec<usize>>,
}

/// HNSW index configuration
#[derive(Debug, Clone)]
pub struct HNSWConfig {
    /// Maximum number of neighbors per node (M parameter)
    pub m: usize,
    /// Maximum neighbors during construction (efConstruction)
    pub ef_construction: usize,
    /// Number of neighbors during search (efSearch)
    pub ef_search: usize,
    /// Maximum layers (0 = auto-calculate based on dataset size)
    pub max_layers: Option<usize>,
    /// Distance metric
    pub distance_metric: DistanceMetric,
}

impl Default for HNSWConfig {
    fn default() -> Self {
        Self {
            m: 16,
            ef_construction: 200,
            ef_search: 50,
            max_layers: None,
            distance_metric: DistanceMetric::Cosine,
        }
    }
}

/// Distance metric for similarity calculation
#[derive(Debug, Clone, Copy)]
pub enum DistanceMetric {
    Cosine,
    Euclidean,
    DotProduct,
}

/// Search result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub id: usize,
    pub distance: f32,
    pub metadata: Option<String>,
}

/// HNSW index for approximate nearest neighbor search
pub struct HNSWIndex {
    config: HNSWConfig,
    nodes: Arc<RwLock<HashMap<usize, HNSWNode>>>,
    entry_point: Arc<RwLock<Option<usize>>>,
    max_layer: Arc<RwLock<usize>>,
    dimension: usize,
    /// Metadata storage (optional)
    metadata: Arc<RwLock<HashMap<usize, String>>>,
}

impl HNSWIndex {
    /// Create a new HNSW index
    pub fn new(dimension: usize, config: HNSWConfig) -> Self {
        Self {
            config,
            nodes: Arc::new(RwLock::new(HashMap::new())),
            entry_point: Arc::new(RwLock::new(None)),
            max_layer: Arc::new(RwLock::new(0)),
            dimension,
            metadata: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Insert a vector into the index
    pub async fn insert(&self, id: usize, vector: Vec<f32>, metadata: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
        if vector.len() != self.dimension {
            return Err(format!(
                "Vector dimension mismatch: expected {}, got {}",
                self.dimension, vector.len()
            ).into());
        }

        let mut nodes = self.nodes.write().await;
        let mut entry_point = self.entry_point.write().await;
        let mut max_layer = self.max_layer.write().await;

        // Calculate random layer for this node
        let node_layer = self.random_layer();

        // If this is the first node, set as entry point
        if entry_point.is_none() {
            *entry_point = Some(id);
            *max_layer = node_layer;
        }

        // Create node with empty neighbor lists for each layer
        let mut node = HNSWNode {
            id,
            vector: vector.clone(),
            neighbors: vec![Vec::new(); node_layer + 1],
        };

        // Find neighbors at each layer (from top to bottom)
        if let Some(ep) = *entry_point {
            let mut current_ep = ep;

            for layer in (node_layer + 1..=*max_layer).rev() {
                current_ep = self.search_layer(&nodes, &vector, current_ep, 1, layer).await?;
            }

            // Insert node and find neighbors at each layer
            for layer in (0..=node_layer).rev() {
                let ef = if layer == 0 { self.config.ef_construction } else { 1 };
                let candidates = self.search_layer(&nodes, &vector, current_ep, ef, layer).await?;

                // Select M nearest neighbors
                let selected = self.select_neighbors(&candidates, self.config.m).await;

                node.neighbors[layer] = selected.clone();

                // Update neighbors' links (bidirectional)
                for &neighbor_id in &selected {
                    if let Some(neighbor) = nodes.get_mut(&neighbor_id) {
                        if layer < neighbor.neighbors.len() {
                            neighbor.neighbors[layer].push(id);

                            // Prune if exceeds M
                            if neighbor.neighbors[layer].len() > self.config.m {
                                neighbor.neighbors[layer].truncate(self.config.m);
                            }
                        }
                    }
                }

                current_ep = candidates[0].0;
            }
        }

        nodes.insert(id, node);

        // Store metadata if provided
        if let Some(meta) = metadata {
            self.metadata.write().await.insert(id, meta);
        }

        debug!("Inserted vector {} at layer {}", id, node_layer);
        Ok(())
    }

    /// Search for k nearest neighbors
    pub async fn search(&self, query: &[f32], k: usize) -> Result<Vec<SearchResult>, Box<dyn std::error::Error>> {
        if query.len() != self.dimension {
            return Err(format!(
                "Query dimension mismatch: expected {}, got {}",
                self.dimension, query.len()
            ).into());
        }

        let nodes = self.nodes.read().await;
        let entry_point = *self.entry_point.read().await;

        if entry_point.is_none() || nodes.is_empty() {
            return Ok(Vec::new());
        }

        let mut current_ep = entry_point.unwrap();

        // Greedy search from top layer to bottom
        for layer in (1..=*self.max_layer.read().await).rev() {
            loop {
                let neighbors = &nodes[&current_ep].neighbors[layer];
                let mut best = current_ep;
                let mut best_dist = self.distance(query, &nodes[&current_ep].vector);

                for &neighbor_id in neighbors {
                    let dist = self.distance(query, &nodes[&neighbor_id].vector);
                    if dist < best_dist {
                        best = neighbor_id;
                        best_dist = dist;
                    }
                }

                if best == current_ep {
                    break;
                }
                current_ep = best;
            }
        }

        // Bottom layer search with ef candidates
        let candidates = self.search_layer(&nodes, query, current_ep, self.config.ef_search, 0).await?;

        // Return top-k results
        let results: Vec<SearchResult> = candidates
            .into_iter()
            .take(k)
            .map(|(id, dist)| {
                let metadata = self.metadata.read().await.get(&id).cloned();
                SearchResult {
                    id,
                    distance: dist,
                    metadata,
                }
            })
            .collect();

        Ok(results)
    }

    /// Get number of vectors in index
    pub async fn len(&self) -> usize {
        self.nodes.read().await.len()
    }

    /// Check if index is empty
    pub async fn is_empty(&self) -> bool {
        self.nodes.read().await.is_empty()
    }

    /// Clear all vectors
    pub async fn clear(&self) {
        self.nodes.write().await.clear();
        self.metadata.write().await.clear();
        *self.entry_point.write().await = None;
        *self.max_layer.write().await = 0;
    }

    /// Search a single layer using priority queue
    async fn search_layer(
        &self,
        nodes: &HashMap<usize, HNSWNode>,
        query: &[f32],
        entry_point: usize,
        ef: usize,
        layer: usize,
    ) -> Result<Vec<(usize, f32)>, Box<dyn std::error::Error>> {
        use std::collections::BinaryHeap;

        let mut visited = HashSet::new();
        let mut candidates = BinaryHeap::new(); // Max-heap (negate distance for min-heap behavior)
        let mut results = BinaryHeap::new();

        let initial_dist = self.distance(query, &nodes[&entry_point].vector);
        candidates.push((std::cmp::Reverse(initial_dist), entry_point));
        results.push((std::cmp::Reverse(initial_dist), entry_point));
        visited.insert(entry_point);

        while let Some((std::cmp::Reverse(dist), current)) = candidates.pop() {
            // If current distance is worse than worst in results, stop
            if let Some(&(std::cmp::Reverse(worst_dist), _)) = results.peek() {
                if dist > worst_dist {
                    break;
                }
            }

            // Explore neighbors
            if let Some(node) = nodes.get(&current) {
                if layer < node.neighbors.len() {
                    for &neighbor_id in &node.neighbors[layer] {
                        if !visited.contains(&neighbor_id) {
                            visited.insert(neighbor_id);
                            let neighbor_dist = self.distance(query, &nodes[&neighbor_id].vector);

                            if results.len() < ef || neighbor_dist < results.peek().unwrap().0 .0 {
                                candidates.push((std::cmp::Reverse(neighbor_dist), neighbor_id));
                                results.push((std::cmp::Reverse(neighbor_dist), neighbor_id));

                                if results.len() > ef {
                                    results.pop();
                                }
                            }
                        }
                    }
                }
            }
        }

        // Convert to sorted vector (ascending by distance)
        let mut sorted_results: Vec<_> = results.into_iter().map(|(r, id)| (id, r.0)).collect();
        sorted_results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        Ok(sorted_results)
    }

    /// Select M nearest neighbors from candidates
    async fn select_neighbors(&self, candidates: &[(usize, f32)], m: usize) -> Vec<usize> {
        candidates
            .iter()
            .take(m)
            .map(|(id, _)| *id)
            .collect()
    }

    /// Calculate distance between two vectors
    fn distance(&self, a: &[f32], b: &[f32]) -> f32 {
        match self.config.distance_metric {
            DistanceMetric::Cosine => 1.0 - Self::cosine_similarity(a, b),
            DistanceMetric::Euclidean => Self::euclidean_distance(a, b),
            DistanceMetric::DotProduct => -Self::dot_product(a, b),
        }
    }

    /// Calculate cosine similarity
    fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

        if norm_a == 0.0 || norm_b == 0.0 {
            0.0
        } else {
            dot / (norm_a * norm_b)
        }
    }

    /// Calculate Euclidean distance
    fn euclidean_distance(a: &[f32], b: &[f32]) -> f32 {
        a.iter()
            .zip(b.iter())
            .map(|(x, y)| (x - y).powi(2))
            .sum::<f32>()
            .sqrt()
    }

    /// Calculate dot product
    fn dot_product(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
    }

    /// Randomly assign a layer based on exponential decay
    fn random_layer(&self) -> usize {
        let max_layer = self.config.max_layers.unwrap_or_else(|| {
            // Auto-calculate based on dataset size heuristic
            ((self.nodes.read().await.len() as f64).ln() / (self.config.m as f64).ln()) as usize
        });

        let mut layer = 0;
        while rand::random::<f64>() < 1.0 / (self.config.m as f64) && layer < max_layer {
            layer += 1;
        }
        layer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_hnsw_insert_search() {
        let config = HNSWConfig::default();
        let index = HNSWIndex::new(3, config);

        // Insert vectors
        index.insert(1, vec![1.0, 0.0, 0.0], None).await.unwrap();
        index.insert(2, vec![0.0, 1.0, 0.0], None).await.unwrap();
        index.insert(3, vec![1.0, 0.1, 0.0], None).await.unwrap();

        assert_eq!(index.len().await, 3);

        // Search for similar to [1.0, 0.05, 0.0]
        let query = vec![1.0, 0.05, 0.0];
        let results = index.search(&query, 2).await.unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, 1); // Most similar
        assert_eq!(results[1].id, 3); // Second most similar
    }

    #[tokio::test]
    async fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        let c = vec![0.0, 1.0, 0.0];

        assert!((HNSWIndex::cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);
        assert!((HNSWIndex::cosine_similarity(&a, &c)).abs() < 1e-6);
    }
}
