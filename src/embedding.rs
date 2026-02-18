//! Local embedding generation using all-MiniLM-L6-v2
//!
//! Provides fast, free, consistent embeddings for memory similarity search.
//! Uses tract for pure-Rust ONNX inference (no external dependencies).

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokenizers::Tokenizer;
use tract_hir::prelude::*;
use tract_onnx::prelude::*;

use crate::storage::jcode_dir;

/// Model configuration
const MODEL_NAME: &str = "all-MiniLM-L6-v2";
const EMBEDDING_DIM: usize = 384;
const MAX_SEQ_LENGTH: usize = 256;

/// Download URLs for model files
const MODEL_URL: &str =
    "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/onnx/model.onnx";
const TOKENIZER_URL: &str =
    "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/tokenizer.json";

/// Global embedder cache and runtime stats.
///
/// This is process-wide: all server sessions share one embedding model.
static EMBEDDER_CACHE: OnceLock<Mutex<EmbedderCache>> = OnceLock::new();

/// Embedding vector type
pub type EmbeddingVec = Vec<f32>;

/// The embedder handles model loading and inference
pub struct Embedder {
    model: SimplePlan<TypedFact, Box<dyn TypedOp>, Graph<TypedFact, Box<dyn TypedOp>>>,
    tokenizer: Tokenizer,
}

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
}

impl Default for EmbedderCache {
    fn default() -> Self {
        Self {
            embedder: None,
            load_error: None,
            loaded_at: None,
            last_used_at: None,
            load_count: 0,
            unload_count: 0,
            embed_calls: 0,
            embed_failures: 0,
            total_embed_ms: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EmbedderStats {
    pub loaded: bool,
    pub load_count: u64,
    pub unload_count: u64,
    pub embed_calls: u64,
    pub embed_failures: u64,
    pub total_embed_ms: u64,
    pub avg_embed_ms: Option<f64>,
    pub idle_secs: Option<u64>,
    pub loaded_secs: Option<u64>,
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
        let model_path = model_dir.join("model.onnx");
        let tokenizer_path = model_dir.join("tokenizer.json");

        // Check if files exist, download if not
        if !model_path.exists() || !tokenizer_path.exists() {
            download_model(&model_dir)?;
        }

        // Load tokenizer
        let tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {}", e))?;

        // Load ONNX model with tract
        let model = tract_onnx::onnx()
            .model_for_path(&model_path)
            .context("Failed to load ONNX model")?
            .with_input_fact(0, f32::fact([1, MAX_SEQ_LENGTH]).into())? // input_ids
            .with_input_fact(1, i64::fact([1, MAX_SEQ_LENGTH]).into())? // attention_mask
            .with_input_fact(2, i64::fact([1, MAX_SEQ_LENGTH]).into())? // token_type_ids
            .into_optimized()
            .context("Failed to optimize model")?
            .into_runnable()
            .context("Failed to make model runnable")?;

        Ok(Self { model, tokenizer })
    }

    /// Generate embedding for a single text
    pub fn embed(&self, text: &str) -> Result<EmbeddingVec> {
        // Tokenize
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| anyhow::anyhow!("Tokenization failed: {}", e))?;

        // Prepare inputs (pad to MAX_SEQ_LENGTH)
        let mut input_ids = vec![0i64; MAX_SEQ_LENGTH];
        let mut attention_mask = vec![0i64; MAX_SEQ_LENGTH];
        let mut token_type_ids = vec![0i64; MAX_SEQ_LENGTH];

        let ids = encoding.get_ids();
        let len = ids.len().min(MAX_SEQ_LENGTH);

        for i in 0..len {
            input_ids[i] = ids[i] as i64;
            attention_mask[i] = 1;
            // token_type_ids stays 0 for single sentence
        }

        // Convert to tensors
        let input_ids_tensor: Tensor =
            tract_ndarray::Array2::from_shape_vec((1, MAX_SEQ_LENGTH), input_ids)?
                .into_tensor()
                .cast_to::<f32>()?
                .into_owned();

        let attention_mask_tensor: Tensor =
            tract_ndarray::Array2::from_shape_vec((1, MAX_SEQ_LENGTH), attention_mask)?.into();

        let token_type_ids_tensor: Tensor =
            tract_ndarray::Array2::from_shape_vec((1, MAX_SEQ_LENGTH), token_type_ids)?.into();

        // Run inference
        let outputs = self.model.run(tvec![
            input_ids_tensor.into(),
            attention_mask_tensor.into(),
            token_type_ids_tensor.into(),
        ])?;

        // Extract embedding (mean pooling over sequence)
        let output = outputs[0].to_array_view::<f32>()?.to_owned();

        // Mean pooling: average over sequence dimension (axis 1)
        // Output shape is [1, seq_len, 384], we want [384]
        let shape = output.shape();
        if shape.len() == 3 {
            let seq_len = shape[1];
            let hidden_dim = shape[2];
            let mut embedding = vec![0f32; hidden_dim];

            // Count non-padded tokens for proper averaging
            let valid_tokens = len.min(seq_len);

            for i in 0..valid_tokens {
                for j in 0..hidden_dim {
                    embedding[j] += output[[0, i, j]];
                }
            }

            // Average
            for val in &mut embedding {
                *val /= valid_tokens.max(1) as f32;
            }

            // L2 normalize
            let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                for val in &mut embedding {
                    *val /= norm;
                }
            }

            Ok(embedding)
        } else {
            anyhow::bail!("Unexpected output shape: {:?}", shape);
        }
    }

    /// Generate embeddings for multiple texts (batched)
    pub fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbeddingVec>> {
        // For simplicity, process one at a time (tract doesn't easily support dynamic batching)
        texts.iter().map(|t| self.embed(t)).collect()
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

/// Generate embedding for text using the global embedder
pub fn embed(text: &str) -> Result<EmbeddingVec> {
    let embedder = get_embedder()?;
    let started = Instant::now();
    let result = embedder.embed(text);
    let elapsed_ms = saturating_u64_from_u128(started.elapsed().as_millis());

    if let Ok(mut cache) = embedder_cache().lock() {
        cache.embed_calls = cache.embed_calls.saturating_add(1);
        cache.total_embed_ms = cache.total_embed_ms.saturating_add(elapsed_ms);
        cache.last_used_at = Some(Instant::now());
        if result.is_err() {
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
            unloaded = true;
            idle_secs = idle.as_secs();
        }
    }

    if unloaded {
        crate::logging::info(&format!(
            "Unloaded embedding model after {}s idle",
            idle_secs
        ));
    }

    unloaded
}

/// Snapshot runtime statistics for the global embedder cache.
pub fn stats() -> EmbedderStats {
    let now = Instant::now();
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

            EmbedderStats {
                loaded: cache.embedder.is_some(),
                load_count: cache.load_count,
                unload_count: cache.unload_count,
                embed_calls: cache.embed_calls,
                embed_failures: cache.embed_failures,
                total_embed_ms: cache.total_embed_ms,
                avg_embed_ms,
                idle_secs,
                loaded_secs,
            }
        }
        Err(_) => EmbedderStats {
            loaded: false,
            load_count: 0,
            unload_count: 0,
            embed_calls: 0,
            embed_failures: 0,
            total_embed_ms: 0,
            avg_embed_ms: None,
            idle_secs: None,
            loaded_secs: None,
        },
    }
}

/// Compute cosine similarity between two embeddings
/// Returns value in [-1, 1], higher is more similar
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

/// Find the top-k most similar embeddings from a list
/// Returns indices and similarity scores, sorted by similarity (highest first)
pub fn find_similar(
    query: &[f32],
    candidates: &[EmbeddingVec],
    threshold: f32,
    top_k: usize,
) -> Vec<(usize, f32)> {
    let mut scores: Vec<(usize, f32)> = candidates
        .iter()
        .enumerate()
        .map(|(i, emb)| (i, cosine_similarity(query, emb)))
        .filter(|(_, score)| *score >= threshold)
        .collect();

    scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scores.truncate(top_k);
    scores
}

/// Get the models directory path
pub fn models_dir() -> Result<PathBuf> {
    let dir = jcode_dir()?.join("models").join(MODEL_NAME);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Download the model files if they don't exist
fn download_model(model_dir: &PathBuf) -> Result<()> {
    use std::io::Write;

    crate::logging::info("Downloading embedding model (one-time setup)...");

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()?;

    // Download model.onnx
    let model_path = model_dir.join("model.onnx");
    if !model_path.exists() {
        crate::logging::info(&format!("Downloading {} model...", MODEL_NAME));
        let response = client.get(MODEL_URL).send()?;
        if !response.status().is_success() {
            anyhow::bail!("Failed to download model: {}", response.status());
        }
        let bytes = response.bytes()?;
        let mut file = std::fs::File::create(&model_path)?;
        file.write_all(&bytes)?;
        crate::logging::info(&format!("Model saved to {:?}", model_path));
    }

    // Download tokenizer.json
    let tokenizer_path = model_dir.join("tokenizer.json");
    if !tokenizer_path.exists() {
        crate::logging::info("Downloading tokenizer...");
        let response = client.get(TOKENIZER_URL).send()?;
        if !response.status().is_success() {
            anyhow::bail!("Failed to download tokenizer: {}", response.status());
        }
        let bytes = response.bytes()?;
        let mut file = std::fs::File::create(&tokenizer_path)?;
        file.write_all(&bytes)?;
        crate::logging::info(&format!("Tokenizer saved to {:?}", tokenizer_path));
    }

    Ok(())
}

/// Check if the embedding model is available
pub fn is_model_available() -> bool {
    if let Ok(dir) = models_dir() {
        dir.join("model.onnx").exists() && dir.join("tokenizer.json").exists()
    } else {
        false
    }
}

/// Get embedding dimension
pub const fn embedding_dim() -> usize {
    EMBEDDING_DIM
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
            vec![1.0, 0.0, 0.0],  // identical
            vec![0.9, 0.1, 0.0],  // similar
            vec![0.0, 1.0, 0.0],  // orthogonal
            vec![-1.0, 0.0, 0.0], // opposite
        ];

        // Normalize candidates for proper cosine similarity
        let candidates: Vec<Vec<f32>> = candidates
            .into_iter()
            .map(|v| {
                let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                v.into_iter().map(|x| x / norm).collect()
            })
            .collect();

        let results = find_similar(&query, &candidates, 0.5, 10);
        assert_eq!(results.len(), 2); // Only identical and similar pass threshold
        assert_eq!(results[0].0, 0); // First result is identical
    }

    #[test]
    fn test_idle_unload_noop_when_not_loaded() {
        assert!(!maybe_unload_if_idle(Duration::from_secs(1)));
    }
}
