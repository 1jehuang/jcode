//! Stub embedding module when the `embeddings` feature is disabled.
//!
//! Provides the same public API as the real embedding module but all
//! operations return errors or no-ops.

use anyhow::Result;
use serde::Serialize;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::time::Duration;

pub type EmbeddingVec = Vec<f32>;

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

#[derive(Debug, Clone, Serialize)]
pub struct EmbedderStats {
    pub loaded: bool,
    pub model_artifact_bytes: u64,
    pub tokenizer_artifact_bytes: u64,
    pub total_artifact_bytes: u64,
    pub load_count: u64,
    pub unload_count: u64,
    pub embed_calls: u64,
    pub embed_failures: u64,
    pub total_embed_ms: u64,
    pub avg_embed_ms: Option<f64>,
    pub idle_secs: Option<u64>,
    pub loaded_secs: Option<u64>,
    pub cache_hits: u64,
    pub cache_size: usize,
    pub cache_bytes_estimate: u64,
    pub embedding_dim: usize,
}

pub struct Embedder;

impl Embedder {
    pub fn load() -> Result<Self> {
        anyhow::bail!("Embeddings feature not compiled in this build")
    }
}

pub fn get_embedder() -> Result<std::sync::Arc<Embedder>> {
    anyhow::bail!("Embeddings feature not compiled in this build")
}

pub fn embed(_text: &str) -> Result<EmbeddingVec> {
    anyhow::bail!("Embeddings feature not compiled in this build")
}

pub fn maybe_unload_if_idle(_idle_for: Duration) -> bool {
    false
}

pub fn unload_now() -> bool {
    false
}

pub fn stats() -> EmbedderStats {
    EmbedderStats {
        loaded: false,
        model_artifact_bytes: 0,
        tokenizer_artifact_bytes: 0,
        total_artifact_bytes: 0,
        load_count: 0,
        unload_count: 0,
        embed_calls: 0,
        embed_failures: 0,
        total_embed_ms: 0,
        avg_embed_ms: None,
        idle_secs: None,
        loaded_secs: None,
        cache_hits: 0,
        cache_size: 0,
        cache_bytes_estimate: 0,
        embedding_dim: embedding_dim(),
    }
}

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

pub fn batch_cosine_similarity(query: &[f32], candidates: &[&[f32]]) -> Vec<f32> {
    let dim = query.len();
    if dim == 0 || candidates.is_empty() {
        return vec![0.0; candidates.len()];
    }
    candidates
        .iter()
        .map(|c| {
            if c.len() != dim {
                0.0
            } else {
                c.iter().zip(query.iter()).map(|(a, b)| a * b).sum()
            }
        })
        .collect()
}

pub fn find_similar(
    query: &[f32],
    candidates: &[EmbeddingVec],
    threshold: f32,
    top_k: usize,
) -> Vec<(usize, f32)> {
    let refs: Vec<&[f32]> = candidates.iter().map(|v| v.as_slice()).collect();
    let scores = batch_cosine_similarity(query, &refs);
    top_k_scored(
        scores
            .into_iter()
            .enumerate()
            .filter(|(_, score)| *score >= threshold),
        top_k,
    )
}

pub fn is_model_available() -> bool {
    false
}

pub const fn embedding_dim() -> usize {
    384
}
