use anyhow::{Context, Result};
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::io::Write;
use std::path::Path;
use tokenizers::Tokenizer;
use tract_hir::infer::Factoid as _;
use tract_hir::prelude::*;

pub const MODEL_NAME: &str = "all-MiniLM-L6-v2";
// tract 0.23 renamed the runnable plan alias and dropped its third (model)
// type parameter. `into_runnable` now hands back an `Arc<RunnableModel<..>>`.
type RunnableEmbeddingModel = std::sync::Arc<RunnableModel<TypedFact, Box<dyn TypedOp>>>;

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
    /// Per-input binding plan: (role, dtype) in the model's DECLARED input order.
    /// Exporters differ in both input ORDER (MiniLM puts input_ids first; e5/bge
    /// put attention_mask first) and DTYPE (f32 vs i64), so we bind by name and
    /// feed each input its model-declared dtype instead of assuming a position.
    input_plan: Vec<(InputRole, DatumType)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum InputRole {
    InputIds,
    AttentionMask,
    TokenTypeIds,
}

fn classify_input(name: &str) -> InputRole {
    let n = name.to_ascii_lowercase();
    if n.contains("attention") || n.contains("mask") {
        InputRole::AttentionMask
    } else if n.contains("token_type") || n.contains("segment") {
        InputRole::TokenTypeIds
    } else {
        InputRole::InputIds
    }
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

        let raw = tract_onnx::onnx()
            .model_for_path(&model_path)
            .context("Failed to load ONNX model")?;

        // Determine each input's role (by name) and dtype (declared, else i64).
        let input_outlets = raw
            .input_outlets()
            .context("Failed to read model input outlets")?
            .to_vec();
        let mut input_plan: Vec<(InputRole, DatumType)> = Vec::with_capacity(input_outlets.len());
        for (ix, outlet) in input_outlets.iter().enumerate() {
            let role = classify_input(&raw.node(outlet.node).name);
            let dt = raw
                .input_fact(ix)
                .ok()
                .and_then(|f| f.datum_type.concretize())
                .unwrap_or(DatumType::I64);
            input_plan.push((role, dt));
        }

        let mut model = raw;
        for (ix, (_, dt)) in input_plan.iter().enumerate() {
            model = model.with_input_fact(ix, InferenceFact::dt_shape(*dt, [1, MAX_SEQ_LENGTH]))?;
        }
        let model = model
            .into_optimized()
            .context("Failed to optimize model")?
            .into_runnable()
            .context("Failed to make model runnable")?;

        Ok(Self {
            model,
            tokenizer,
            input_plan,
        })
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

        // Build each input tensor by role, cast to the model's declared dtype.
        let make = |data: &[i64], dt: DatumType| -> Result<Tensor> {
            let t: Tensor =
                tract_ndarray::Array2::from_shape_vec((1, MAX_SEQ_LENGTH), data.to_vec())?.into();
            Ok(t.cast_to_dt(dt)?.into_owned())
        };

        let mut inputs: TVec<TValue> = tvec![];
        for (role, dt) in &self.input_plan {
            let data: &[i64] = match role {
                InputRole::InputIds => &input_ids,
                InputRole::AttentionMask => &attention_mask,
                InputRole::TokenTypeIds => &token_type_ids,
            };
            inputs.push(make(data, *dt)?.into());
        }

        let outputs = self.model.run(inputs)?;

        let output = outputs[0].to_plain_array_view::<f32>()?.to_owned();

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

/// Build the run() inputs for a BERT-style model from token ids, honoring the
/// model's declared input order/role/dtype. Shared by Embedder and CrossEncoder.
fn build_bert_inputs(
    input_plan: &[(InputRole, DatumType)],
    ids: &[u32],
    type_ids: &[u32],
) -> Result<TVec<TValue>> {
    let mut input_ids = vec![0i64; MAX_SEQ_LENGTH];
    let mut attention_mask = vec![0i64; MAX_SEQ_LENGTH];
    let mut token_type_ids = vec![0i64; MAX_SEQ_LENGTH];
    let len = ids.len().min(MAX_SEQ_LENGTH);
    for i in 0..len {
        input_ids[i] = ids[i] as i64;
        attention_mask[i] = 1;
        if i < type_ids.len() {
            token_type_ids[i] = type_ids[i] as i64;
        }
    }
    let make = |data: &[i64], dt: DatumType| -> Result<Tensor> {
        let t: Tensor =
            tract_ndarray::Array2::from_shape_vec((1, MAX_SEQ_LENGTH), data.to_vec())?.into();
        Ok(t.cast_to_dt(dt)?.into_owned())
    };
    let mut inputs: TVec<TValue> = tvec![];
    for (role, dt) in input_plan {
        let data: &[i64] = match role {
            InputRole::InputIds => &input_ids,
            InputRole::AttentionMask => &attention_mask,
            InputRole::TokenTypeIds => &token_type_ids,
        };
        inputs.push(make(data, *dt)?.into());
    }
    Ok(inputs)
}

/// A cross-encoder reranker: scores a (query, passage) pair jointly and returns
/// a single relevance logit. Used to reorder a candidate set after first-stage
/// retrieval (recall-5). Higher score = more relevant.
pub struct CrossEncoder {
    model: RunnableEmbeddingModel,
    tokenizer: Tokenizer,
    input_plan: Vec<(InputRole, DatumType)>,
}

impl CrossEncoder {
    pub fn load_from_dir(model_dir: &Path) -> Result<Self> {
        let model_path = model_dir.join("model.onnx");
        let tokenizer_path = model_dir.join("tokenizer.json");
        let tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {}", e))?;

        let raw = tract_onnx::onnx()
            .model_for_path(&model_path)
            .context("Failed to load cross-encoder ONNX model")?;
        let input_outlets = raw.input_outlets().context("read input outlets")?.to_vec();
        let mut input_plan: Vec<(InputRole, DatumType)> = Vec::with_capacity(input_outlets.len());
        for (ix, outlet) in input_outlets.iter().enumerate() {
            let role = classify_input(&raw.node(outlet.node).name);
            let dt = raw
                .input_fact(ix)
                .ok()
                .and_then(|f| f.datum_type.concretize())
                .unwrap_or(DatumType::I64);
            input_plan.push((role, dt));
        }
        let mut model = raw;
        for (ix, (_, dt)) in input_plan.iter().enumerate() {
            model = model.with_input_fact(ix, InferenceFact::dt_shape(*dt, [1, MAX_SEQ_LENGTH]))?;
        }
        let model = model
            .into_optimized()
            .context("optimize cross-encoder")?
            .into_runnable()
            .context("make cross-encoder runnable")?;
        Ok(Self {
            model,
            tokenizer,
            input_plan,
        })
    }

    /// Relevance score for a (query, passage) pair. Higher = more relevant.
    pub fn score(&self, query: &str, passage: &str) -> Result<f32> {
        let encoding = self
            .tokenizer
            .encode((query, passage), true)
            .map_err(|e| anyhow::anyhow!("Tokenization failed: {}", e))?;
        let inputs = build_bert_inputs(
            &self.input_plan,
            encoding.get_ids(),
            encoding.get_type_ids(),
        )?;
        let outputs = self.model.run(inputs)?;
        let view = outputs[0].to_plain_array_view::<f32>()?;
        // logits shape is [1, 1] (relevance) or [1, N]; take the first/primary.
        view.iter()
            .next()
            .copied()
            .ok_or_else(|| anyhow::anyhow!("empty cross-encoder output"))
    }

    /// Rerank `(id, text)` candidates by cross-encoder score against `query`.
    /// Returns ids sorted by descending relevance.
    pub fn rerank(
        &self,
        query: &str,
        candidates: &[(String, String)],
    ) -> Result<Vec<(String, f32)>> {
        let mut scored: Vec<(String, f32)> = Vec::with_capacity(candidates.len());
        for (id, text) in candidates {
            let s = self.score(query, text)?;
            scored.push((id.clone(), s));
        }
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        Ok(scored)
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

    fn related_beats_unrelated(model_dir: &Path) {
        let e = Embedder::load_from_dir(model_dir).expect("load model");
        let q = e.embed("how do I set the cargo build profile").unwrap();
        let related = e.embed("The build uses the selfdev cargo profile").unwrap();
        let unrelated = e.embed("Bees pollinate flowers in spring").unwrap();
        let sim_rel = cosine_similarity(&q, &related);
        let sim_unrel = cosine_similarity(&q, &unrelated);
        assert!(
            sim_rel > sim_unrel + 0.05,
            "expected related ({sim_rel:.3}) >> unrelated ({sim_unrel:.3}) for {}",
            model_dir.display()
        );
    }

    /// Guard the numeric stability of the embedding output across inference-engine
    /// upgrades.
    ///
    /// Persisted memory embeddings are keyed by `model_id`, which stays
    /// `all-MiniLM-L6-v2` across a `tract` bump, so `memory.rs`'s
    /// model-mismatch check cannot notice a numeric change. If a new tract
    /// produced different vectors for the same text, every stored embedding
    /// would silently become incomparable with freshly computed ones, and
    /// "related still beats unrelated" would not detect it.
    ///
    /// These reference values were captured on tract 0.21.10 and reproduced on
    /// 0.23.4, where the largest per-component difference was 1e-6 (float32
    /// rounding), i.e. a worst-case cosine deviation around 2e-10. The tolerance
    /// below is deliberately far tighter than any retrieval threshold, so a real
    /// change in model behaviour fails while rounding does not.
    ///
    /// Skipped when the model is not present locally, like the tests above.
    #[test]
    fn minilm_embedding_is_numerically_stable_across_inference_engines() {
        let dir = std::env::var_os("HOME")
            .map(|h| std::path::PathBuf::from(h).join(".jcode/models/all-MiniLM-L6-v2"))
            .filter(|d| is_model_available(d));
        let Some(dir) = dir else {
            eprintln!("skip: MiniLM model not present locally");
            return;
        };
        let e = Embedder::load_from_dir(&dir).expect("load model");

        // Captured on tract 0.21.10, first 8 of 384 dimensions.
        let expected: [f32; 8] = [
            0.040730, 0.103227, 0.065710, 0.032524, -0.012075, 0.005709, 0.029490, -0.038528,
        ];
        let got = e
            .embed("how do I set the cargo build profile")
            .expect("embed");

        assert_eq!(got.len(), 384, "MiniLM must stay 384-dimensional");
        let norm: f32 = got.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-4,
            "output must stay L2-normalised, got norm {norm}"
        );
        for (i, want) in expected.iter().enumerate() {
            let diff = (got[i] - want).abs();
            assert!(
                diff < 1e-4,
                "dim {i} drifted beyond float32 rounding: expected {want}, got {}, diff {diff:e}",
                got[i]
            );
        }
    }

    /// Regression test for the input-binding fix: the real MiniLM model must
    /// produce meaningfully higher similarity for related vs unrelated text.
    /// Skipped automatically if the model isn't present locally.
    #[test]
    fn minilm_related_beats_unrelated_if_present() {
        let dir = std::env::var_os("HOME")
            .map(|h| std::path::PathBuf::from(h).join(".jcode/models/all-MiniLM-L6-v2"))
            .filter(|d| is_model_available(d));
        match dir {
            Some(d) => related_beats_unrelated(&d),
            None => eprintln!("skip: MiniLM model not present locally"),
        }
    }

    /// Exploratory check for an alternate model with a DIFFERENT input
    /// order/dtype (e5-small-v2: attention_mask declared first). Reports the
    /// related vs unrelated gap but does NOT hard-fail: some models need
    /// model-specific pooling/normalization (e.g. CLS pooling) that the shared
    /// mean-pool path does not yet provide. Skipped if not present locally.
    #[test]
    fn alt_model_related_beats_unrelated_if_present() {
        let dir = std::env::var_os("HOME")
            .map(|h| std::path::PathBuf::from(h).join("jcode-memory-bench/models/e5-small-v2"))
            .filter(|d| is_model_available(d));
        let Some(d) = dir else {
            eprintln!("skip: e5-small-v2 model not present locally");
            return;
        };
        let e = Embedder::load_from_dir(&d).expect("load model");
        let q = e.embed("how do I set the cargo build profile").unwrap();
        let related = e.embed("The build uses the selfdev cargo profile").unwrap();
        let unrelated = e.embed("Bees pollinate flowers in spring").unwrap();
        let sim_rel = cosine_similarity(&q, &related);
        let sim_unrel = cosine_similarity(&q, &unrelated);
        eprintln!(
            "e5-small-v2: related={sim_rel:.4} unrelated={sim_unrel:.4} gap={:.4} (informational; mean-pool may need CLS for this family)",
            sim_rel - sim_unrel
        );
    }

    /// Cross-encoder reranker must score a relevant (query, passage) pair higher
    /// than an irrelevant one. Skipped if the model isn't present locally.
    #[test]
    fn cross_encoder_scores_relevant_higher_if_present() {
        let dir = std::env::var_os("HOME")
            .map(|h| std::path::PathBuf::from(h).join("jcode-memory-bench/models/ce-minilm-l6"))
            .filter(|d| d.join("model.onnx").exists() && d.join("tokenizer.json").exists());
        let Some(d) = dir else {
            eprintln!("skip: cross-encoder model not present locally");
            return;
        };
        let ce = CrossEncoder::load_from_dir(&d).expect("load cross-encoder");
        let q = "how do I set the cargo build profile";
        let rel = ce
            .score(q, "The build uses the selfdev cargo profile")
            .unwrap();
        let unrel = ce.score(q, "Bees pollinate flowers in spring").unwrap();
        eprintln!("cross-encoder: relevant={rel:.3} irrelevant={unrel:.3}");
        assert!(
            rel > unrel,
            "cross-encoder must score relevant ({rel:.3}) > irrelevant ({unrel:.3})"
        );
    }
}
