//! Local embedding generation using all-MiniLM-L6-v2
//!
//! Provides fast, free, consistent embeddings for memory similarity search.
//! Uses tract for pure-Rust ONNX inference (no external dependencies).

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::OnceLock;
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

/// Global cached model (loaded once, reused)
static EMBEDDER: OnceLock<Result<Embedder, String>> = OnceLock::new();

/// Embedding vector type
pub type EmbeddingVec = Vec<f32>;

/// The embedder handles model loading and inference
pub struct Embedder {
    model: SimplePlan<TypedFact, Box<dyn TypedOp>, Graph<TypedFact, Box<dyn TypedOp>>>,
    tokenizer: Tokenizer,
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
            let valid_tokens = len;

            for i in 0..valid_tokens {
                for j in 0..hidden_dim {
                    embedding[j] += output[[0, i, j]];
                }
            }

            // Average
            for val in &mut embedding {
                *val /= valid_tokens as f32;
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

/// Get or create the global embedder instance
pub fn get_embedder() -> Result<&'static Embedder> {
    EMBEDDER
        .get_or_init(|| Embedder::load().map_err(|e| e.to_string()))
        .as_ref()
        .map_err(|e| anyhow::anyhow!("{}", e))
}

/// Generate embedding for text using the global embedder
pub fn embed(text: &str) -> Result<EmbeddingVec> {
    get_embedder()?.embed(text)
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
}
