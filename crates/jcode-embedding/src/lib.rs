use anyhow::{Context, Result};
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::io::Write;
use std::path::Path;
use tokenizers::Tokenizer;
use tract_hir::prelude::*;

pub const MODEL_NAME: &str = "all-MiniLM-L6-v2";
type RunnableEmbeddingModel =
    SimplePlan<TypedFact, Box<dyn TypedOp>, Graph<TypedFact, Box<dyn TypedOp>>>;

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
const EMBEDDING_DIM: usize = 384;
const MAX_SEQ_LENGTH: usize = 256;

const MODEL_URL: &str =
    "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/onnx/model.onnx";
const TOKENIZER_URL: &str =
    "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/tokenizer.json";

pub type EmbeddingVec = Vec<f32>;

pub struct Embedder {
    model: RunnableEmbeddingModel,
    tokenizer: Tokenizer,
}

impl Embedder {
    pub fn load_from_dir(model_dir: &Path) -> Result<Self> {
        let model_path = model_dir.join("model.onnx");
        let tokenizer_path = model_dir.join("tokenizer.json");

        if !model_path.exists() || !tokenizer_path.exists() {
            download_model(model_dir)?;
        }

        let tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {}", e))?;

        let model = tract_onnx::onnx()
            .model_for_path(&model_path)
            .context("Failed to load ONNX model")?
            .with_input_fact(0, f32::fact([1, MAX_SEQ_LENGTH]).into())?
            .with_input_fact(1, i64::fact([1, MAX_SEQ_LENGTH]).into())?
            .with_input_fact(2, i64::fact([1, MAX_SEQ_LENGTH]).into())?
            .into_optimized()
            .context("Failed to optimize model")?
            .into_runnable()
            .context("Failed to make model runnable")?;

        Ok(Self { model, tokenizer })
    }

    pub fn embed(&self, text: &str) -> Result<EmbeddingVec> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| anyhow::anyhow!("Tokenization failed: {}", e))?;

        let mut input_ids = vec![0i64; MAX_SEQ_LENGTH];
        let mut attention_mask = vec![0i64; MAX_SEQ_LENGTH];
        let token_type_ids = vec![0i64; MAX_SEQ_LENGTH];

        let ids = encoding.get_ids();
        let len = ids.len().min(MAX_SEQ_LENGTH);

        for i in 0..len {
            input_ids[i] = ids[i] as i64;
            attention_mask[i] = 1;
        }

        let input_ids_tensor: Tensor =
            tract_ndarray::Array2::from_shape_vec((1, MAX_SEQ_LENGTH), input_ids)?
                .into_tensor()
                .cast_to::<f32>()?
                .into_owned();

        let attention_mask_tensor: Tensor =
            tract_ndarray::Array2::from_shape_vec((1, MAX_SEQ_LENGTH), attention_mask)?.into();

        let token_type_ids_tensor: Tensor =
            tract_ndarray::Array2::from_shape_vec((1, MAX_SEQ_LENGTH), token_type_ids)?.into();

        let outputs = self.model.run(tvec![
            input_ids_tensor.into(),
            attention_mask_tensor.into(),
            token_type_ids_tensor.into(),
        ])?;

        let output = outputs[0].to_array_view::<f32>()?.to_owned();

        let shape = output.shape();
        if shape.len() == 3 {
            let seq_len = shape[1];
            let hidden_dim = shape[2];
            let mut embedding = vec![0f32; hidden_dim];

            let valid_tokens = len.min(seq_len);

            for i in 0..valid_tokens {
                for j in 0..hidden_dim {
                    embedding[j] += output[[0, i, j]];
                }
            }

            for val in &mut embedding {
                *val /= valid_tokens.max(1) as f32;
            }

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

    pub fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbeddingVec>> {
        texts.iter().map(|t| self.embed(t)).collect()
    }
}

pub const fn embedding_dim() -> usize {
    EMBEDDING_DIM
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

pub fn is_model_available(model_dir: &Path) -> bool {
    model_dir.join("model.onnx").exists() && model_dir.join("tokenizer.json").exists()
}

fn download_model(model_dir: &Path) -> Result<()> {
    let model_dir = model_dir.to_path_buf();
    match std::thread::spawn(move || download_model_blocking(&model_dir)).join() {
        Ok(result) => result,
        Err(panic) => {
            let panic_msg = if let Some(msg) = panic.downcast_ref::<&str>() {
                (*msg).to_string()
            } else if let Some(msg) = panic.downcast_ref::<String>() {
                msg.clone()
            } else {
                "unknown panic payload".to_string()
            };
            anyhow::bail!("Embedding model download thread panicked: {}", panic_msg);
        }
    }
}

fn download_model_blocking(model_dir: &Path) -> Result<()> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("jcode-embedding/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(300))
        .build()?;

    std::fs::create_dir_all(model_dir)?;

    let model_path = model_dir.join("model.onnx");
    if !model_path.exists() {
        let response = client.get(MODEL_URL).send()?;
        if !response.status().is_success() {
            anyhow::bail!("Failed to download model: {}", response.status());
        }
        let bytes = response.bytes()?;
        let mut file = std::fs::File::create(&model_path)?;
        file.write_all(&bytes)?;
    }

    let tokenizer_path = model_dir.join("tokenizer.json");
    if !tokenizer_path.exists() {
        let response = client.get(TOKENIZER_URL).send()?;
        if !response.status().is_success() {
            anyhow::bail!("Failed to download tokenizer: {}", response.status());
        }
        let bytes = response.bytes()?;
        let mut file = std::fs::File::create(&tokenizer_path)?;
        file.write_all(&bytes)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_similarity_handles_basic_cases() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        let c = vec![0.0, 1.0, 0.0];
        let d = vec![-1.0, 0.0, 0.0];

        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.001);
        assert!((cosine_similarity(&a, &c) - 0.0).abs() < 0.001);
        assert!((cosine_similarity(&a, &d) - (-1.0)).abs() < 0.001);
    }

    #[test]
    fn find_similar_returns_only_top_k_sorted_hits() {
        let query = vec![1.0, 0.0, 0.0];
        let candidates = vec![
            vec![0.2, 0.0, 0.0],
            vec![0.9, 0.0, 0.0],
            vec![0.7, 0.0, 0.0],
            vec![0.8, 0.0, 0.0],
        ];

        let hits = find_similar(&query, &candidates, 0.1, 2);

        assert_eq!(hits, vec![(1, 0.9), (3, 0.8)]);
    }
}
