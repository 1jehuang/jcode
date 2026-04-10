//! Embedding facade for jcode.
//!
//! The heavy ONNX/tokenizer implementation lives in the `jcode-embedding`
//! workspace crate so unchanged embedding code can stay cached across self-dev
//! builds. This module keeps jcode's process-wide cache, stats, and local path /
//! logging integration stable.

use anyhow::Result;
use jcode_embedding as backend;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::storage::jcode_dir;

/// LRU cache capacity for recent embeddings
const EMBEDDING_CACHE_CAPACITY: usize = 128;

/// Global embedder cache and runtime stats.
///
/// This is process-wide: all server sessions share one embedding model.
static EMBEDDER_CACHE: OnceLock<Mutex<EmbedderCache>> = OnceLock::new();

/// Embedding vector type
pub type EmbeddingVec = backend::EmbeddingVec;

/// The embedder handles model loading and inference.
pub struct Embedder {
    inner: backend::Embedder,
}

#[derive(Default)]
struct EmbedderCache {
    embedder: Option<Arc<Embedder>>,
    load_error: Option<String>,
    loaded_at: Option<Instant>,
    last_used_at: Option<Instant>,
    load_count: u64,
    unload_count: u64,
    embed_calls: u64,
    embed_failures: u64,
    total_embed_ms: u64,
    /// LRU embedding cache: maps text hash -> (embedding, insertion order)
    embedding_lru: std::collections::HashMap<u64, (EmbeddingVec, u64)>,
    lru_counter: u64,
    cache_hits: u64,
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

fn embedder_cache() -> &'static Mutex<EmbedderCache> {
    EMBEDDER_CACHE.get_or_init(|| Mutex::new(EmbedderCache::default()))
}

fn saturating_u64_from_u128(value: u128) -> u64 {
    if value > u64::MAX as u128 {
        u64::MAX
    } else {
        value as u64
    }
}

impl Embedder {
    /// Load the model from disk (or download if missing)
    pub fn load() -> Result<Self> {
        let model_dir = models_dir()?;
        if !backend::is_model_available(&model_dir) {
            crate::logging::info("Embedding model missing; downloading (one-time setup)...");
        }
        let inner = backend::Embedder::load_from_dir(&model_dir)?;
        Ok(Self { inner })
    }

    /// Generate embedding for a single text
    pub fn embed(&self, text: &str) -> Result<EmbeddingVec> {
        self.inner.embed(text)
    }

    /// Generate embeddings for multiple texts (batched)
    pub fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbeddingVec>> {
        self.inner.embed_batch(texts)
    }
}

/// Get or create the global embedder instance.
///
/// Returns an `Arc` so callers can keep using the model even if an idle
/// unload happens concurrently in the background.
pub fn get_embedder() -> Result<Arc<Embedder>> {
    let mut cache = embedder_cache()
        .lock()
        .map_err(|_| anyhow::anyhow!("Embedder cache lock poisoned"))?;

    cache.last_used_at = Some(Instant::now());

    if let Some(embedder) = cache.embedder.as_ref() {
        return Ok(Arc::clone(embedder));
    }

    if let Some(err) = cache.load_error.as_ref() {
        return Err(anyhow::anyhow!("{}", err));
    }

    let loaded = match Embedder::load() {
        Ok(embedder) => Arc::new(embedder),
        Err(e) => {
            let msg = e.to_string();
            cache.load_error = Some(msg.clone());
            return Err(anyhow::anyhow!(msg));
        }
    };

    cache.embedder = Some(Arc::clone(&loaded));
    cache.load_error = None;
    cache.load_count = cache.load_count.saturating_add(1);
    let now = Instant::now();
    cache.loaded_at = Some(now);
    cache.last_used_at = Some(now);

    crate::logging::info("Embedding model loaded into memory");
    Ok(loaded)
}

/// Hash text for the LRU embedding cache.
fn hash_text(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// Generate embedding for text using the global embedder.
///
/// Results are cached in an LRU so repeated queries for the same text
/// return instantly.
pub fn embed(text: &str) -> Result<EmbeddingVec> {
    let text_hash = hash_text(text);

    // Check cache first
    if let Ok(mut cache) = embedder_cache().lock() {
        if let Some((emb, _)) = cache.embedding_lru.get(&text_hash) {
            let result = emb.clone();
            cache.cache_hits = cache.cache_hits.saturating_add(1);
            cache.last_used_at = Some(Instant::now());
            let counter = cache.lru_counter;
            cache.lru_counter = counter.wrapping_add(1);
            if let Some(entry) = cache.embedding_lru.get_mut(&text_hash) {
                entry.1 = counter;
            }
            return Ok(result);
        }
    }

    let embedder = get_embedder()?;
    let started = Instant::now();
    let result = embedder.embed(text);
    let elapsed_ms = saturating_u64_from_u128(started.elapsed().as_millis());

    if let Ok(mut cache) = embedder_cache().lock() {
        cache.embed_calls = cache.embed_calls.saturating_add(1);
        cache.total_embed_ms = cache.total_embed_ms.saturating_add(elapsed_ms);
        cache.last_used_at = Some(Instant::now());
        if let Ok(ref emb) = result {
            if cache.embedding_lru.len() >= EMBEDDING_CACHE_CAPACITY {
                let oldest_key = cache
                    .embedding_lru
                    .iter()
                    .min_by_key(|(_, (_, counter))| *counter)
                    .map(|(&k, _)| k);
                if let Some(k) = oldest_key {
                    cache.embedding_lru.remove(&k);
                }
            }
            let counter = cache.lru_counter;
            cache.lru_counter = counter.wrapping_add(1);
            cache
                .embedding_lru
                .insert(text_hash, (emb.clone(), counter));
        } else {
            cache.embed_failures = cache.embed_failures.saturating_add(1);
        }
    }

    result
}

/// Unload the embedding model if it has been idle for at least `idle_for`.
///
/// Returns `true` when an unload occurred.
pub fn maybe_unload_if_idle(idle_for: Duration) -> bool {
    let mut unloaded = false;
    let mut idle_secs = 0u64;

    if let Ok(mut cache) = embedder_cache().lock() {
        if cache.embedder.is_none() {
            return false;
        }

        let Some(last_used) = cache.last_used_at else {
            return false;
        };

        let idle = last_used.elapsed();
        if idle >= idle_for {
            cache.embedder = None;
            cache.loaded_at = None;
            cache.unload_count = cache.unload_count.saturating_add(1);
            cache.embedding_lru.clear();
            unloaded = true;
            idle_secs = idle.as_secs();
        }
    }

    if unloaded {
        crate::logging::info(&format!(
            "Unloaded embedding model after {}s idle",
            idle_secs
        ));

        #[cfg(all(target_os = "linux", not(feature = "jemalloc")))]
        {
            unsafe extern "C" {
                fn malloc_trim(pad: usize) -> i32;
            }
            let trimmed = unsafe { malloc_trim(0) };
            crate::logging::info(&format!(
                "malloc_trim after model unload: {}",
                if trimmed == 1 {
                    "released pages"
                } else {
                    "no pages to release"
                }
            ));
        }
    }

    unloaded
}

/// Force unload the global embedder and clear its embedding cache.
pub fn unload_now() -> bool {
    let mut unloaded = false;
    if let Ok(mut cache) = embedder_cache().lock() {
        if cache.embedder.is_some() {
            cache.embedder = None;
            cache.loaded_at = None;
            cache.unload_count = cache.unload_count.saturating_add(1);
            cache.embedding_lru.clear();
            unloaded = true;
        }
    }

    if unloaded {
        crate::logging::info("Embedding model force-unloaded");

        #[cfg(all(target_os = "linux", not(feature = "jemalloc")))]
        {
            unsafe extern "C" {
                fn malloc_trim(pad: usize) -> i32;
            }
            let _ = unsafe { malloc_trim(0) };
        }
    }

    unloaded
}

/// Snapshot runtime statistics for the global embedder cache.
pub fn stats() -> EmbedderStats {
    let now = Instant::now();
    let (model_artifact_bytes, tokenizer_artifact_bytes) = artifact_sizes();
    let total_artifact_bytes = model_artifact_bytes.saturating_add(tokenizer_artifact_bytes);
    match embedder_cache().lock() {
        Ok(cache) => {
            let avg_embed_ms = if cache.embed_calls == 0 {
                None
            } else {
                Some(cache.total_embed_ms as f64 / cache.embed_calls as f64)
            };
            let idle_secs = cache
                .last_used_at
                .map(|last| now.saturating_duration_since(last).as_secs());
            let loaded_secs = cache
                .loaded_at
                .map(|loaded| now.saturating_duration_since(loaded).as_secs());

            let cache_bytes_estimate = cache
                .embedding_lru
                .values()
                .map(|(embedding, _)| embedding.len().saturating_mul(std::mem::size_of::<f32>()))
                .sum::<usize>() as u64;

            EmbedderStats {
                loaded: cache.embedder.is_some(),
                model_artifact_bytes,
                tokenizer_artifact_bytes,
                total_artifact_bytes,
                load_count: cache.load_count,
                unload_count: cache.unload_count,
                embed_calls: cache.embed_calls,
                embed_failures: cache.embed_failures,
                total_embed_ms: cache.total_embed_ms,
                avg_embed_ms,
                idle_secs,
                loaded_secs,
                cache_hits: cache.cache_hits,
                cache_size: cache.embedding_lru.len(),
                cache_bytes_estimate,
                embedding_dim: embedding_dim(),
            }
        }
        Err(_) => EmbedderStats {
            loaded: false,
            model_artifact_bytes,
            tokenizer_artifact_bytes,
            total_artifact_bytes,
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
        },
    }
}

fn artifact_sizes() -> (u64, u64) {
    let Ok(dir) = models_dir() else {
        return (0, 0);
    };
    let model_bytes = std::fs::metadata(dir.join("model.onnx"))
        .ok()
        .map(|meta| meta.len())
        .unwrap_or(0);
    let tokenizer_bytes = std::fs::metadata(dir.join("tokenizer.json"))
        .ok()
        .map(|meta| meta.len())
        .unwrap_or(0);
    (model_bytes, tokenizer_bytes)
}

/// Compute cosine similarity between two embeddings.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    backend::cosine_similarity(a, b)
}

/// Compute cosine similarities between a query and many candidates.
pub fn batch_cosine_similarity(query: &[f32], candidates: &[&[f32]]) -> Vec<f32> {
    backend::batch_cosine_similarity(query, candidates)
}

/// Find the top-k most similar embeddings from a list.
pub fn find_similar(
    query: &[f32],
    candidates: &[EmbeddingVec],
    threshold: f32,
    top_k: usize,
) -> Vec<(usize, f32)> {
    backend::find_similar(query, candidates, threshold, top_k)
}

/// Get the models directory path.
pub fn models_dir() -> Result<PathBuf> {
    let dir = jcode_dir()?.join("models").join(backend::MODEL_NAME);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Check if the embedding model is available.
pub fn is_model_available() -> bool {
    if let Ok(dir) = models_dir() {
        backend::is_model_available(&dir)
    } else {
        false
    }
}

/// Get embedding dimension.
pub const fn embedding_dim() -> usize {
    backend::embedding_dim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.001);

        let c = vec![0.0, 1.0, 0.0];
        assert!((cosine_similarity(&a, &c) - 0.0).abs() < 0.001);

        let d = vec![-1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &d) - (-1.0)).abs() < 0.001);
    }

    #[test]
    fn test_find_similar() {
        let query = vec![1.0, 0.0, 0.0];
        let candidates = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.9, 0.1, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![-1.0, 0.0, 0.0],
        ];

        let candidates: Vec<Vec<f32>> = candidates
            .into_iter()
            .map(|v| {
                let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                v.into_iter().map(|x| x / norm).collect()
            })
            .collect();

        let results = find_similar(&query, &candidates, 0.5, 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, 0);
    }

    #[test]
    fn test_idle_unload_noop_when_not_loaded() {
        if let Ok(mut cache) = embedder_cache().lock() {
            *cache = EmbedderCache::default();
        }
        assert!(!maybe_unload_if_idle(Duration::from_secs(1)));
    }
}
