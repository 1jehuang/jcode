//! Memory recall benchmark (Mode 1 / no-LLM).
//!
//! Faithful offline harness for measuring memory retrieval accuracy. Reuses the
//! REAL jcode retrieval primitives:
//!   - `jcode::memory_graph::MemoryGraph` deserialization (real on-disk graphs)
//!   - `jcode::embedding::embed` (real all-MiniLM-L6-v2 ONNX model)
//!   - `jcode::memory::format_context_for_relevance` (real live query window)
//!   - a faithful re-implementation of `score_and_filter` (cosine + gap filter)
//!
//! Privacy: all data lives OUTSIDE the repo (default `~/jcode-memory-bench`).
//! Nothing here writes into the repo tree.
//!
//! Subcommands:
//!   queries  - replay sessions -> emit per-turn query windows (labels/queries.jsonl)
//!   pool     - run retrievers over queries -> emit candidate pool (labels/pool.jsonl)
//!   metrics  - read cached gold labels -> emit recall@k/MRR/nDCG (results/*.json)
//!
//! Run via: cargo run --profile selfdev --features dev-bins --bin memory_recall_bench -- <subcmd> ...

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use jcode::embedding;
use jcode::memory::format_context_for_relevance;
use jcode::memory_graph::MemoryGraph;
use jcode::session::Session;
use serde::{Deserialize, Serialize};

// ---- Tunables that mirror production retrieval (memory.rs) ----
const EMBEDDING_SIMILARITY_THRESHOLD: f32 = 0.5;
const EMBEDDING_MAX_HITS: usize = 10;
const GAP_FACTOR: f32 = 0.25;
const MIN_KEEP: usize = 1;
// Memory agent context window (memory_prompt.rs constants are private; the
// production path calls format_context_for_relevance over the full message list).

// Cost accounting for LLM configs, written by the rerank precompute and read
// when emitting the result JSON. 0 for no-LLM configs.
static LLM_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static LLM_PROMPT_TOK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static LLM_OUTPUT_TOK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn bench_root() -> PathBuf {
    std::env::var("MEMORY_BENCH_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs_home().join("jcode-memory-bench"))
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_default()
}

/// A queued extraction job: (session id, transcript, expected (id, content) pairs).
type ExtractionJob = (String, String, Vec<(String, String)>);
/// A raw recall result row: (query, ranked (id, score) candidates, hits, total).
type RecallResultRow = (String, Vec<(String, f32)>, usize, usize);

// ---------------- Corpus ----------------

#[derive(Clone)]
struct CorpusMemory {
    id: String,
    content: String,
    // Parsed from the corpus fixture for completeness; not used by the current
    // recall scoring path.
    #[allow(dead_code)]
    category: String,
    embedding: Option<Vec<f32>>,
    #[allow(dead_code)]
    graph: String,
    source: Option<String>,
    active: bool,
    confidence: f32,
    strength: u32,
    age_days: f32,
}

struct Corpus {
    memories: Vec<CorpusMemory>,
    /// 1-hop expansion adjacency: memory id -> related memory ids reachable via
    /// recall-useful edges (relates_to / derived_from / supersedes).
    expand_edges: HashMap<String, Vec<String>>,
}

impl Corpus {
    /// Load a single graph file as the search corpus.
    fn load_graph_file(path: &Path) -> Result<Corpus> {
        let graph = load_graph(path)?;
        let graph_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
        let now = chrono::Utc::now();
        let memories = graph
            .memories
            .values()
            .map(|m| CorpusMemory {
                id: m.id.clone(),
                content: m.content.clone(),
                category: m.category.to_string(),
                embedding: m.embedding.clone(),
                graph: graph_name.clone(),
                source: m.source.clone(),
                active: m.active,
                confidence: m.confidence,
                strength: m.strength,
                age_days: (now - m.updated_at).num_seconds().max(0) as f32 / 86_400.0,
            })
            .collect();

        // Build 1-hop expansion adjacency from recall-useful edge kinds only
        // (skip has_tag / in_cluster which fan out to huge sets).
        let active_ids: HashSet<String> = graph
            .memories
            .values()
            .filter(|m| m.active)
            .map(|m| m.id.clone())
            .collect();
        let mut expand_edges: HashMap<String, Vec<String>> = HashMap::new();
        for (src, edges) in &graph.edges {
            if !active_ids.contains(src) {
                continue;
            }
            for e in edges {
                let kind = format!("{:?}", e.kind).to_lowercase();
                let useful = kind.contains("relatesto")
                    || kind.contains("derivedfrom")
                    || kind.contains("supersedes");
                if useful && active_ids.contains(&e.target) {
                    expand_edges
                        .entry(src.clone())
                        .or_default()
                        .push(e.target.clone());
                }
            }
        }

        Ok(Corpus {
            memories,
            expand_edges,
        })
    }

    fn active(&self) -> impl Iterator<Item = &CorpusMemory> {
        self.memories.iter().filter(|m| m.active)
    }
}

fn load_graph(path: &Path) -> Result<MemoryGraph> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let graph: MemoryGraph =
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))?;
    Ok(graph)
}

// ---------------- Retrievers ----------------

/// Faithful re-implementation of MemoryManager::score_and_filter for the dense
/// (embedding) path, including the score-distribution gap filter.
fn dense_retrieve(
    query_emb: &[f32],
    corpus: &Corpus,
    threshold: f32,
    limit: usize,
    apply_gap: bool,
) -> Vec<(String, f32)> {
    let entries: Vec<&CorpusMemory> = corpus.active().filter(|m| m.embedding.is_some()).collect();
    let emb_refs: Vec<&[f32]> = entries
        .iter()
        .filter_map(|m| m.embedding.as_deref())
        .collect();
    let scores = embedding::batch_cosine_similarity(query_emb, &emb_refs);

    let mut scored: Vec<(String, f32)> = entries
        .iter()
        .zip(scores)
        .filter(|(_, s)| *s >= threshold)
        .map(|(m, s)| (m.id.clone(), s))
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored.truncate(limit);

    if apply_gap {
        scored = apply_gap_filter(scored, threshold);
    }
    scored
}

fn apply_gap_filter(scored: Vec<(String, f32)>, threshold: f32) -> Vec<(String, f32)> {
    if scored.len() <= 1 {
        return scored;
    }
    let top = scored[0].1;
    let range = (top - threshold).max(0.01);
    let max_gap = range * GAP_FACTOR;
    let mut keep = scored.len();
    for i in 1..scored.len() {
        let drop = scored[i - 1].1 - scored[i].1;
        if drop > max_gap && i >= MIN_KEEP {
            keep = i;
            break;
        }
    }
    scored.into_iter().take(keep).collect()
}

/// Simple BM25 lexical retriever over memory content (for hybrid experiments
/// and to widen the candidate pool so pooled gold labels are less biased).
struct Bm25 {
    docs: Vec<(String, Vec<String>)>, // (id, tokens)
    df: HashMap<String, usize>,
    avgdl: f32,
    n: usize,
}

impl Bm25 {
    fn build(corpus: &Corpus) -> Bm25 {
        let mut docs = Vec::new();
        let mut df: HashMap<String, usize> = HashMap::new();
        let mut total_len = 0usize;
        for m in corpus.active() {
            let toks = tokenize(&m.content);
            total_len += toks.len();
            let unique: HashSet<&String> = toks.iter().collect();
            for t in unique {
                *df.entry(t.clone()).or_insert(0) += 1;
            }
            docs.push((m.id.clone(), toks));
        }
        let n = docs.len().max(1);
        Bm25 {
            avgdl: total_len as f32 / n as f32,
            n,
            docs,
            df,
        }
    }

    fn search(&self, query: &str, limit: usize) -> Vec<(String, f32)> {
        const K1: f32 = 1.2;
        const B: f32 = 0.75;
        let q = tokenize(query);
        let qset: HashSet<&String> = q.iter().collect();
        let mut out: Vec<(String, f32)> = Vec::new();
        for (id, toks) in &self.docs {
            let dl = toks.len() as f32;
            let mut tf: HashMap<&String, f32> = HashMap::new();
            for t in toks {
                *tf.entry(t).or_insert(0.0) += 1.0;
            }
            let mut score = 0.0f32;
            for term in &qset {
                let Some(&f) = tf.get(*term) else { continue };
                let n_q = *self.df.get(*term).unwrap_or(&0) as f32;
                if n_q == 0.0 {
                    continue;
                }
                let idf = (((self.n as f32 - n_q + 0.5) / (n_q + 0.5)) + 1.0).ln();
                let denom = f + K1 * (1.0 - B + B * dl / self.avgdl);
                score += idf * (f * (K1 + 1.0)) / denom;
            }
            if score > 0.0 {
                out.push((id.clone(), score));
            }
        }
        out.sort_by(|a, b| b.1.total_cmp(&a.1));
        out.truncate(limit);
        out
    }
}

fn tokenize(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

/// Gentle importance prior from confidence/strength/recency. Multiplicative
/// tiebreaker on the fused relevance score; never dominates relevance.
fn memory_prior(m: &CorpusMemory) -> f32 {
    let conf = 0.9 + 0.2 * m.confidence.clamp(0.0, 1.0);
    let strength = 1.0 + 0.05 * (m.strength as f32 + 1.0).ln().min(3.0);
    let recency = 1.0 + 0.1 * (-(m.age_days.max(0.0)) / 120.0).exp();
    conf * strength * recency
}

/// Build a focused query from the raw context window: drop system-reminder
/// blocks and tool-call markers, keep human/assistant prose, and over-weight the
/// most recent user message (the strongest signal of current intent) by
/// repeating it. Mirrors what a production recall-3 query builder would do.
fn focus_query(raw: &str) -> String {
    let mut kept: Vec<String> = Vec::new();
    let mut last_user: Option<String> = None;
    let mut in_reminder = false;
    let mut current_role = "";

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("<system-reminder>") {
            in_reminder = true;
            continue;
        }
        if trimmed.ends_with("</system-reminder>") {
            in_reminder = false;
            continue;
        }
        if in_reminder {
            continue;
        }
        if trimmed == "User:" {
            current_role = "user";
            continue;
        }
        if trimmed == "Assistant:" {
            current_role = "assistant";
            continue;
        }
        // Drop tool markers and result dumps (noise for intent).
        if trimmed.starts_with("[Tool:")
            || trimmed.starts_with("[Tool error:")
            || trimmed.starts_with("[Result:")
            || trimmed.starts_with("[Image]")
        {
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        kept.push(trimmed.to_string());
        if current_role == "user" {
            last_user = Some(trimmed.to_string());
        }
    }

    let mut out = kept.join("\n");
    // Over-weight the most recent user intent.
    if let Some(u) = last_user {
        out = format!("{u}\n{out}");
    }
    if out.trim().is_empty() {
        raw.to_string()
    } else {
        out
    }
}

/// Reciprocal Rank Fusion of multiple ranked lists.
fn rrf(lists: &[Vec<(String, f32)>], k: f32, limit: usize) -> Vec<(String, f32)> {
    let mut fused: HashMap<String, f32> = HashMap::new();
    for list in lists {
        for (rank, (id, _)) in list.iter().enumerate() {
            *fused.entry(id.clone()).or_insert(0.0) += 1.0 / (k + rank as f32 + 1.0);
        }
    }
    let mut out: Vec<(String, f32)> = fused.into_iter().collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1));
    out.truncate(limit);
    out
}

/// Dynamic variable-k gate over a ranked, score-descending candidate list.
///
/// Produces a DYNAMIC number of items per query (including 0) instead of a fixed
/// top-k. This is the precision lever the no-LLM path needs: most turns have only
/// 0-2 relevant memories among hundreds, so a fixed top-5 injects mostly noise.
///
/// Rules (all relative to the top score, so scale-free across RRF/dense/bm25):
///   - `rel_floor`: keep item i only while `score[i] >= score[0] * rel_floor`.
///   - `drop_ratio`: stop as soon as an item is `< prev * drop_ratio` (a cliff in
///     the score curve marks the relevant/irrelevant boundary).
///   - `max_k`: hard upper bound (backstop).
///
/// Returns at least 1 item when the input is non-empty (the top candidate always
/// clears its own floor), so recall of a present top-1 is never lost.
#[allow(dead_code)] // Convenience wrapper; bench paths call dynamic_gate_abs directly.
fn dynamic_gate(
    ranked: &[(String, f32)],
    rel_floor: f32,
    drop_ratio: f32,
    max_k: usize,
) -> Vec<String> {
    dynamic_gate_abs(ranked, rel_floor, drop_ratio, max_k, 0.0)
}

/// Like `dynamic_gate`, but with an absolute-score floor on the TOP candidate:
/// if even the best hybrid score is below `abs_floor`, inject nothing. This is
/// what lets the zero-cost (no-LLM) path return 0 memories on no-memory turns
/// (the empty-gold queries), instead of always keeping top-1.
fn dynamic_gate_abs(
    ranked: &[(String, f32)],
    rel_floor: f32,
    drop_ratio: f32,
    max_k: usize,
    abs_floor: f32,
) -> Vec<String> {
    if ranked.is_empty() {
        return Vec::new();
    }
    let top = ranked[0].1.max(f32::MIN_POSITIVE);
    if top < abs_floor {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut prev = top;
    for (id, score) in ranked.iter().take(max_k) {
        if out.is_empty() {
            out.push(id.clone());
            prev = *score;
            continue;
        }
        if *score < top * rel_floor {
            break;
        }
        if *score < prev * drop_ratio {
            break;
        }
        out.push(id.clone());
        prev = *score;
    }
    out
}

// ---------------- Query generation (replay sessions) ----------------

#[derive(Serialize, Deserialize, Clone)]
struct QueryRecord {
    qid: String,
    session: String,
    turn: usize,
    query: String,
    /// Memories whose `source` == this session (excluded from gold to avoid
    /// extraction leakage).
    origin_memory_ids: Vec<String>,
}

fn cmd_queries(args: &[String]) -> Result<()> {
    let opts = parse_kv(args);
    let graph_file = opts.get("corpus").cloned().unwrap_or_else(|| {
        bench_root()
            .join("corpus/projects/7fe469b5e6e471c1.json")
            .display()
            .to_string()
    });
    let sessions_dir = opts
        .get("sessions")
        .cloned()
        .unwrap_or_else(|| format!("{}/.jcode/sessions", dirs_home().display()));
    let max_sessions: usize = opts
        .get("max_sessions")
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);
    let per_session: usize = opts
        .get("per_session")
        .and_then(|s| s.parse().ok())
        .unwrap_or(6);
    let working_dir_filter = opts.get("working_dir").cloned();

    let corpus = Corpus::load_graph_file(Path::new(&graph_file))?;
    // Map source-session -> memory ids, for leakage exclusion.
    let mut by_source: HashMap<String, Vec<String>> = HashMap::new();
    for m in &corpus.memories {
        if let Some(src) = &m.source {
            by_source.entry(src.clone()).or_default().push(m.id.clone());
        }
    }

    // Pick recent sessions (optionally filtered by working_dir).
    let mut sessions: Vec<(PathBuf, std::time::SystemTime)> = std::fs::read_dir(&sessions_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|s| s.to_str()) == Some("json")
                && p.file_name()
                    .and_then(|s| s.to_str())
                    .map(|n| n.starts_with("session_"))
                    .unwrap_or(false)
        })
        .filter_map(|p| {
            let mtime = std::fs::metadata(&p).ok()?.modified().ok()?;
            Some((p, mtime))
        })
        .collect();
    sessions.sort_by_key(|s| std::cmp::Reverse(s.1));

    let out_path = bench_root().join("labels/queries.jsonl");
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut out = String::new();
    let mut count = 0usize;
    let mut used_sessions = 0usize;

    for (path, _) in sessions {
        if used_sessions >= max_sessions {
            break;
        }
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let session: Session = match serde_json::from_slice(&bytes) {
            Ok(s) => s,
            Err(_) => continue,
        };
        if let Some(filter) = &working_dir_filter
            && session.working_dir.as_deref() != Some(filter.as_str())
        {
            continue;
        }
        let sid = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let origin_ids = by_source.get(&sid).cloned().unwrap_or_default();

        let messages: Vec<_> = session.messages.iter().map(|m| m.to_message()).collect();
        // Sample turns: user messages spread through the session.
        let user_turns: Vec<usize> = messages
            .iter()
            .enumerate()
            .filter(|(_, m)| matches!(m.role, jcode::message::Role::User))
            .map(|(i, _)| i)
            // Skip the first two user turns: they are dominated by the session
            // bootstrap (system reminder + opening ask) and carry little
            // working context to retrieve against.
            .skip(2)
            .collect();
        if user_turns.is_empty() {
            continue;
        }
        let step = (user_turns.len() / per_session).max(1);
        let mut taken = 0;
        // Start sampling from the middle so we capture turns with accumulated
        // working context rather than only the earliest turns.
        let start = step / 2;
        for &turn in user_turns.iter().skip(start).step_by(step) {
            if taken >= per_session {
                break;
            }
            // Reconstruct the live query window exactly as production would.
            let window = &messages[..=turn];
            let query = format_context_for_relevance(window);
            if query.len() < 30 {
                continue;
            }
            out.push_str(&serde_json::to_string(&QueryRecord {
                qid: format!("q{:05}", count),
                session: sid.clone(),
                turn,
                query,
                origin_memory_ids: origin_ids.clone(),
            })?);
            out.push('\n');
            count += 1;
            taken += 1;
        }
        if taken > 0 {
            used_sessions += 1;
        }
    }

    std::fs::write(&out_path, out)?;
    println!(
        "Wrote {} queries from {} sessions -> {}",
        count,
        used_sessions,
        out_path.display()
    );
    Ok(())
}

// ---------------- Pool generation ----------------

#[derive(Serialize, Deserialize)]
struct PoolRecord {
    qid: String,
    /// candidate memory id -> {content, retrievers that surfaced it}
    candidates: Vec<PoolCandidate>,
}

#[derive(Serialize, Deserialize)]
struct PoolCandidate {
    id: String,
    content: String,
    #[serde(default)]
    retrievers: Vec<String>,
}

fn cmd_pool(args: &[String]) -> Result<()> {
    let opts = parse_kv(args);
    let graph_file = opts.get("corpus").cloned().unwrap_or_else(|| {
        bench_root()
            .join("corpus/projects/7fe469b5e6e471c1.json")
            .display()
            .to_string()
    });
    let pool_n: usize = opts
        .get("pool_n")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);

    let corpus = Corpus::load_graph_file(Path::new(&graph_file))?;
    let content_by_id: HashMap<String, String> = corpus
        .memories
        .iter()
        .map(|m| (m.id.clone(), m.content.clone()))
        .collect();
    let bm25 = Bm25::build(&corpus);

    let queries = read_queries()?;
    let out_path = bench_root().join("labels/pool.jsonl");
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut out = String::new();

    for q in &queries {
        let q_emb = embedding::embed(&q.query)?;
        // Multiple diverse retrievers widen the pool (reduces pooling bias).
        let dense = dense_retrieve(&q_emb, &corpus, 0.0, pool_n, false);
        let lexical = bm25.search(&q.query, pool_n);
        let fused = rrf(&[dense.clone(), lexical.clone()], 60.0, pool_n);

        let mut retrievers_by_id: HashMap<String, Vec<String>> = HashMap::new();
        for (id, _) in &dense {
            retrievers_by_id
                .entry(id.clone())
                .or_default()
                .push("dense".into());
        }
        for (id, _) in &lexical {
            retrievers_by_id
                .entry(id.clone())
                .or_default()
                .push("bm25".into());
        }
        for (id, _) in &fused {
            retrievers_by_id
                .entry(id.clone())
                .or_default()
                .push("rrf".into());
        }
        // Exclude origin-session memories to avoid extraction leakage.
        let origin: HashSet<&String> = q.origin_memory_ids.iter().collect();
        let candidates: Vec<PoolCandidate> = retrievers_by_id
            .into_iter()
            .filter(|(id, _)| !origin.contains(id))
            .map(|(id, retrievers)| PoolCandidate {
                content: content_by_id.get(&id).cloned().unwrap_or_default(),
                id,
                retrievers,
            })
            .collect();

        out.push_str(&serde_json::to_string(&PoolRecord {
            qid: q.qid.clone(),
            candidates,
        })?);
        out.push('\n');
    }

    std::fs::write(&out_path, out)?;
    println!(
        "Wrote pool for {} queries -> {}",
        queries.len(),
        out_path.display()
    );
    Ok(())
}

// ---------------- LLM judge (direct Anthropic via jcode Sidecar) ----------------

#[derive(Deserialize)]
struct JudgeInput {
    qid: String,
    query: String,
    candidates: Vec<PoolCandidate>,
}

const JUDGE_SYSTEM: &str = "You judge whether stored MEMORIES would be genuinely useful to surface to an AI coding agent given the CURRENT conversation context. \
Be strict and prefer precision: a memory is relevant ONLY if a competent engineer would say \"yes, knowing this specifically helps respond here.\" \
Mark relevant when the memory is a fact, user preference, correction, or procedure that applies to what is happening right now. \
Mark NOT relevant when it is off-topic, generic/obvious, only shares surface keywords, or would be noise. When unsure, exclude it. \
The context contains boilerplate (system reminders, tool output); focus on what is actually being worked on. \
Reply with ONLY a JSON array of the relevant candidate numbers, e.g. [1,4] or []. No prose.";

fn build_judge_prompt(input: &JudgeInput) -> String {
    let query = truncate_for_judge(&input.query, 6000);
    let mut p = String::new();
    p.push_str("CURRENT CONTEXT:\n");
    p.push_str(&query);
    p.push_str("\n\nCANDIDATE MEMORIES:\n");
    for (i, c) in input.candidates.iter().enumerate() {
        p.push_str(&format!("{}. {}\n", i + 1, c.content.replace('\n', " ")));
    }
    p.push_str("\nReturn the numbers of the relevant memories as a JSON array.");
    p
}

fn truncate_for_judge(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    // Keep the TAIL: the most recent context is the most informative for recall.
    let chars: Vec<char> = s.chars().collect();
    chars[chars.len() - max..].iter().collect()
}

fn parse_judge_response(resp: &str, n: usize) -> Vec<usize> {
    // Extract the first JSON array of integers from the response.
    let start = resp.find('[');
    let end = resp.rfind(']');
    let (Some(s), Some(e)) = (start, end) else {
        return Vec::new();
    };
    if e < s {
        return Vec::new();
    }
    let slice = &resp[s..=e];
    let nums: Vec<i64> = serde_json::from_str(slice).unwrap_or_default();
    nums.into_iter()
        .filter_map(|x| {
            let idx = x as usize;
            if idx >= 1 && idx <= n {
                Some(idx - 1)
            } else {
                None
            }
        })
        .collect()
}

// ---- Listwise LLM reranker (recall-5, Mode-2) ----------------------------
//
// The prompt/parse/rerank logic lives in the shared `jcode::memory_rerank`
// module so the benchmark and the live memory agent use ONE implementation
// (bench == prod). This file only orchestrates running it over the gold set.
use jcode::memory_rerank::{LLM_RERANK_SYSTEM, build_rerank_prompt, parse_rerank_response};

/// STRICT precision variant of the listwise rerank system prompt. Pushes the
/// model to keep ONLY memories it is near-certain are useful for THIS request,
/// and to prefer an empty result over any marginal/keyword-only match. This is
/// the precision lever: the default prompt keeps "clearly useful" items, this one
/// keeps "you would bet the memory is needed to answer well".
const LLM_RERANK_STRICT_SYSTEM: &str = "You decide which stored MEMORIES to surface to an AI coding agent for the CURRENT request. \
Apply a STRICT relevance bar: keep a memory ONLY if a competent engineer, seeing the current request, would clearly want that specific fact/preference/correction/procedure to answer well. \
When in doubt, DROP it. Reject generic, off-topic, stale, or merely keyword-overlapping memories. \
Most requests need 0-2 memories; returning an empty list is correct and expected when nothing clearly applies. \
Never pad to a fixed count. \
Reply with ONLY a JSON array of the kept candidate numbers, best first, e.g. [3,1] (or [] if none qualify). No prose.";

/// Confidence-SCORED rerank: the model assigns each candidate a 0-100 usefulness
/// score for the current request. We cache the full score map, then a cheap
/// threshold sweep picks the dynamic injected set (count varies per query,
/// including 0). Scoring (vs binary keep/drop) gives a calibrated knob to push
/// precision toward 1.0 by raising the threshold, with recall/cost reported.
const LLM_SCORED_SYSTEM: &str = "You score how useful each stored MEMORY would be to surface to an AI coding agent for the CURRENT request. \
For each candidate, output an integer 0-100: 100 = clearly essential to answer this request well (a directly-applicable fact/preference/correction/procedure); \
0 = irrelevant, generic, stale, or only shares surface keywords. Use the full range and be calibrated: most candidates in a pool are NOT relevant and should score low. \
Reply with ONLY a JSON object mapping candidate number to score, e.g. {\"1\":5,\"2\":95,\"3\":0}. Include EVERY candidate. No prose.";

// ---- Synthesis ("fork writes what to inject") experiment -----------------
//
// Instead of selecting relevant memories (llm_rerank), the model reads the
// high-recall pool + focused context and SYNTHESIZES a tight injection note,
// citing which memory ids it drew from. Faithfulness is constrained: use ONLY
// the provided memories, invent nothing. We score recall/precision on the cited
// `used` ids (comparable to llm_rerank) and save the notes for quality review.
const LLM_SYNTH_SYSTEM: &str = "You prepare a memory context note for an AI coding agent. \
You are given the CURRENT request and a pool of candidate stored MEMORIES (high recall, mixed relevance). \
Write a concise note containing ONLY the facts/preferences/corrections from the candidates that are genuinely useful for the current request. \
STRICT RULES: use ONLY information present in the candidate memories; invent NOTHING; omit anything irrelevant; if nothing is relevant, return an empty note and empty used list. \
Merge related points and drop irrelevant halves of partially-relevant memories. \
Reply with ONLY a JSON object: {\"used\":[candidate numbers you drew from],\"note\":\"the synthesized context, or empty string\"}. No prose outside the JSON.";

fn build_synth_prompt(query: &str, candidates: &[(String, String)]) -> String {
    let q = if query.chars().count() > 4000 {
        query
            .chars()
            .skip(query.chars().count() - 4000)
            .collect::<String>()
    } else {
        query.to_string()
    };
    let mut p = String::new();
    p.push_str("CURRENT REQUEST:\n");
    p.push_str(&q);
    p.push_str("\n\nCANDIDATE MEMORIES:\n");
    for (i, (_id, content)) in candidates.iter().enumerate() {
        p.push_str(&format!("{}. {}\n", i + 1, content.replace('\n', " ")));
    }
    p.push_str(
        "\nReturn {\"used\":[...],\"note\":\"...\"} drawing ONLY from the candidates above.",
    );
    p
}

/// Parse the synth JSON, returning (used 0-based indices, note text). Tolerant of
/// surrounding prose by extracting the first {..} object.
fn parse_synth_response(resp: &str, n: usize) -> (Vec<usize>, String) {
    let (Some(s), Some(e)) = (resp.find('{'), resp.rfind('}')) else {
        return (Vec::new(), String::new());
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&resp[s..=e]) else {
        return (Vec::new(), String::new());
    };
    let note = v
        .get("note")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let mut seen = std::collections::HashSet::new();
    let used = v
        .get("used")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_i64())
                .filter_map(|x| {
                    let idx = usize::try_from(x).ok()?;
                    if idx >= 1 && idx <= n && seen.insert(idx) {
                        Some(idx - 1)
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    (used, note)
}

/// Parse a `{"1":95,"2":0,...}` score map into `(candidate_index, score)` pairs
/// (0-based index). Tolerates surrounding prose and missing candidates.
fn parse_scored_response(resp: &str, n: usize) -> Vec<(usize, f32)> {
    let (Some(s), Some(e)) = (resp.find('{'), resp.rfind('}')) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&resp[s..=e]) else {
        return Vec::new();
    };
    let Some(obj) = v.as_object() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (k, val) in obj {
        let Ok(idx1) = k.trim().parse::<usize>() else {
            continue;
        };
        if idx1 < 1 || idx1 > n {
            continue;
        }
        let score = val
            .as_f64()
            .or_else(|| val.as_str().and_then(|s| s.parse().ok()));
        if let Some(sc) = score {
            out.push((idx1 - 1, sc as f32));
        }
    }
    out
}

fn cmd_judge(args: &[String]) -> Result<()> {
    let opts = parse_kv(args);
    let model = opts
        .get("model")
        .cloned()
        .unwrap_or_else(|| "claude-sonnet-4-5-20250929".to_string());
    let concurrency: usize = opts
        .get("concurrency")
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);
    // Backend: explicit --backend, else infer from model name.
    let backend = opts.get("backend").cloned().unwrap_or_else(|| {
        if model.starts_with("gpt") || model.starts_with("o1") || model.starts_with("o3") {
            "openai".to_string()
        } else {
            "claude".to_string()
        }
    });
    // Reasoning effort override (OpenAI only); default "none" for no-thinking.
    let reasoning = opts
        .get("reasoning")
        .cloned()
        .unwrap_or_else(|| "none".to_string());

    let input_path = bench_root().join("labels/judge_ready.jsonl");
    let text = std::fs::read_to_string(&input_path)
        .with_context(|| format!("reading {}", input_path.display()))?;
    let inputs: Vec<JudgeInput> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    eprintln!(
        "Judging {} queries with model {} backend={} (concurrency {})",
        inputs.len(),
        model,
        backend,
        concurrency
    );

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let results = rt.block_on(async {
        use futures::stream::{self, StreamExt};
        stream::iter(inputs)
            .map(|input| {
                let model = model.clone();
                let backend = backend.clone();
                let reasoning = reasoning.clone();
                async move {
                    let sidecar = if backend == "openai" {
                        let eff = if reasoning == "default" {
                            None
                        } else {
                            Some(reasoning)
                        };
                        jcode::sidecar::Sidecar::with_openai_model(&model, eff)
                    } else {
                        jcode::sidecar::Sidecar::with_claude_model(&model)
                    };
                    let prompt = build_judge_prompt(&input);
                    let n = input.candidates.len();
                    let mut relevant_ids = Vec::new();
                    // Retry once on transient failure.
                    for attempt in 0..2 {
                        match sidecar.complete(JUDGE_SYSTEM, &prompt).await {
                            Ok(resp) => {
                                let idxs = parse_judge_response(&resp, n);
                                relevant_ids = idxs
                                    .into_iter()
                                    .map(|i| input.candidates[i].id.clone())
                                    .collect();
                                break;
                            }
                            Err(e) => {
                                if attempt == 1 {
                                    eprintln!("judge failed for {}: {}", input.qid, e);
                                } else {
                                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                                }
                            }
                        }
                    }
                    GoldRecord {
                        qid: input.qid,
                        relevant_ids,
                    }
                }
            })
            .buffer_unordered(concurrency)
            .collect::<Vec<_>>()
            .await
    });

    let out_path = bench_root().join("labels/gold.jsonl");
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut out = String::new();
    let mut with_rel = 0usize;
    let mut total = 0usize;
    for g in &results {
        if !g.relevant_ids.is_empty() {
            with_rel += 1;
        }
        total += g.relevant_ids.len();
        out.push_str(&serde_json::to_string(g)?);
        out.push('\n');
    }
    std::fs::write(&out_path, out)?;
    println!(
        "Judged {} queries -> {} ({} with >=1 relevant, {} total labels)",
        results.len(),
        out_path.display(),
        with_rel,
        total
    );
    Ok(())
}

// ---------------- Metrics ----------------

#[derive(Serialize, Deserialize)]
struct GoldRecord {
    qid: String,
    relevant_ids: Vec<String>,
}

/// Dense ranking over a precomputed alt-embedding corpus.
fn alt_dense_rank(
    query_emb: &[f32],
    corpus_emb: &[(String, Vec<f32>)],
    limit: usize,
) -> Vec<(String, f32)> {
    let refs: Vec<&[f32]> = corpus_emb.iter().map(|(_, v)| v.as_slice()).collect();
    let scores = embedding::batch_cosine_similarity(query_emb, &refs);
    let mut scored: Vec<(String, f32)> = corpus_emb
        .iter()
        .zip(scores)
        .map(|((id, _), s)| (id.clone(), s))
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored.truncate(limit);
    scored
}

fn cmd_metrics(args: &[String]) -> Result<()> {
    let opts = parse_kv(args);
    let graph_file = opts.get("corpus").cloned().unwrap_or_else(|| {
        bench_root()
            .join("corpus/projects/7fe469b5e6e471c1.json")
            .display()
            .to_string()
    });
    let config = opts
        .get("config")
        .cloned()
        .unwrap_or_else(|| "baseline".into());

    let corpus = Corpus::load_graph_file(Path::new(&graph_file))?;
    let bm25 = Bm25::build(&corpus);
    let queries = read_queries()?;
    let gold = read_gold()?;

    // For `prod_hybrid`: seed a temp JCODE_HOME project graph with the corpus and
    // exercise the REAL shipped MemoryManager::find_similar_hybrid end-to-end.
    let prod_mgr = if config == "prod_hybrid" {
        let tmp = std::env::temp_dir().join(format!("memrecall-prod-{}", std::process::id()));
        std::fs::create_dir_all(&tmp)?;
        // SAFETY: single-threaded setup before any embedding work.
        unsafe { std::env::set_var("JCODE_HOME", &tmp) };
        let project_dir = "/bench/prod-validate";
        let mgr = jcode::memory::MemoryManager::new().with_project_dir(project_dir);
        let graph = load_graph(Path::new(&graph_file))?;
        mgr.save_project_graph(&graph)?;
        Some(mgr)
    } else {
        None
    };

    // Optional alternative embedder for A/B (e.g. bge-small). When set, we
    // re-embed the corpus with this model and embed queries with the query
    // prefix. Used by the *_alt configs.
    let alt_embedder = opts.get("embedder").map(|dir| {
        eprintln!("Loading alt embedder from {dir}");
        jcode::embedding::Embedder::load_from_dir(Path::new(dir)).expect("load alt embedder")
    });
    let query_prefix = opts.get("query_prefix").cloned().unwrap_or_default();
    let passage_prefix = opts.get("passage_prefix").cloned().unwrap_or_default();
    // Precompute alt corpus embeddings (active memories only) once.
    let alt_corpus_emb: Vec<(String, Vec<f32>)> = match alt_embedder.as_ref() {
        Some(emb) => {
            eprintln!(
                "Re-embedding {} active memories with alt model...",
                corpus.active().count()
            );
            corpus
                .active()
                .map(|m| {
                    let text = format!("{passage_prefix}{}", m.content);
                    let v = emb.embed(&text).unwrap_or_else(|e| {
                        panic!("alt embedder failed on a memory: {e} (model ONNX output shape likely incompatible with the shared mean-pool path)")
                    });
                    (m.id.clone(), v)
                })
                .collect()
        }
        None => Vec::new(),
    };

    // Optional remote OpenAI embedding backend for A/B (configs openai_dense /
    // openai_hybrid). Re-embeds the corpus and queries through the REAL shipped
    // `OpenAiEmbeddingBackend` so the bench measures exactly what production
    // would use. Requires OPENAI_API_KEY (or --openai_key=...). Model/base via
    // --openai_model / --openai_base.
    let openai_backend = if config == "openai_dense" || config == "openai_hybrid" {
        use jcode::embedding_backend::{DEFAULT_OPENAI_EMBEDDING_MODEL, OpenAiEmbeddingBackend};
        let model = opts
            .get("openai_model")
            .cloned()
            .unwrap_or_else(|| DEFAULT_OPENAI_EMBEDDING_MODEL.to_string());
        let key = opts
            .get("openai_key")
            .cloned()
            .or_else(|| std::env::var("OPENAI_API_KEY").ok())
            .or_else(|| {
                jcode::provider_catalog::load_api_key_from_env_or_config(
                    "OPENAI_API_KEY",
                    "openai.env",
                )
            })
            .expect("openai_dense/openai_hybrid require OPENAI_API_KEY or --openai_key");
        let base = opts.get("openai_base").cloned();
        let dim = opts.get("openai_dim").and_then(|s| s.parse().ok());
        eprintln!("Using OpenAI embedding backend model={model}");
        Some(OpenAiEmbeddingBackend::new(model, key, base, dim))
    } else {
        None
    };

    // Precompute OpenAI corpus embeddings (active memories only), batched to
    // amortize HTTP round-trips. id -> vector.
    let openai_corpus_emb: Vec<(String, Vec<f32>)> = match openai_backend.as_ref() {
        Some(b) => {
            use jcode::embedding_backend::EmbeddingBackend;
            let items: Vec<(String, String)> = corpus
                .active()
                .map(|m| (m.id.clone(), m.content.clone()))
                .collect();
            eprintln!(
                "Re-embedding {} active memories with OpenAI backend (batched)...",
                items.len()
            );
            let mut out: Vec<(String, Vec<f32>)> = Vec::with_capacity(items.len());
            for chunk in items.chunks(128) {
                let texts: Vec<&str> = chunk.iter().map(|(_, c)| c.as_str()).collect();
                let vecs = b
                    .embed_passages(&texts)
                    .expect("OpenAI corpus embedding batch failed");
                for ((id, _), v) in chunk.iter().zip(vecs) {
                    out.push((id.clone(), v));
                }
            }
            out
        }
        None => Vec::new(),
    };

    // Optional cross-encoder reranker for ce_rerank config.
    let ce_reranker = opts.get("reranker").map(|dir| {
        eprintln!("Loading cross-encoder reranker from {dir}");
        jcode::embedding::CrossEncoder::load_from_dir(Path::new(dir)).expect("load reranker")
    });
    let rerank_pool: usize = opts
        .get("rerank_pool")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);
    // Dynamic variable-k gate tunables (used by *_dyn configs). These control how
    // many memories are injected PER QUERY instead of a fixed top-k.
    let gate_floor: f32 = opts
        .get("gate_floor")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.55);
    let gate_drop: f32 = opts
        .get("gate_drop")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.80);
    let gate_max: usize = opts
        .get("gate_max")
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    // Absolute floor on the TOP hybrid score: if even the best candidate scores
    // below this, the no-LLM gate injects NOTHING. This is the zero-cost lever
    // for cutting bloat on no-memory turns (empty-gold). 0.0 = disabled (legacy).
    let gate_abs: f32 = opts
        .get("gate_abs")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    let content_by_id: HashMap<String, String> = corpus
        .active()
        .map(|m| (m.id.clone(), m.content.clone()))
        .collect();

    // For `llm_rerank`: pre-compute a listwise LLM reranking of the hybrid
    // top-N pool for every judged query (concurrently), cached qid -> ranked
    // ids, so the sync scoring loop just looks it up. Reuses the Sidecar judge
    // plumbing (Claude / OpenAI OAuth) + focused query.
    let llm_rerank_map: HashMap<String, Vec<String>> = if config == "llm_rerank"
        || config == "llm_rerank_padded"
        || config == "llm_strict"
        || config == "llm_judge"
        || config == "llm_synth"
    {
        // `llm_rerank` = precision mode (inject only model-kept ids).
        // `llm_rerank_padded` = emulate the OLD buggy prod path: append the
        //   model-omitted candidates in hybrid order then pad to top-k, so we can
        //   quantify the precision the relevant-only fix recovers.
        // `llm_synth` = the fork SYNTHESIZES an injection note from the pool and
        //   cites which candidates it used; we score on the cited `used` ids and
        //   save the notes to results/synth_notes.jsonl for quality/faithfulness
        //   review.
        let padded = config == "llm_rerank_padded";
        let synth = config == "llm_synth";
        let strict = config == "llm_strict";
        let judge_sel = config == "llm_judge";
        let model = opts
            .get("model")
            .cloned()
            .unwrap_or_else(|| "claude-sonnet-4-6".to_string());
        let backend = opts.get("backend").cloned().unwrap_or_else(|| {
            if model.starts_with("gpt") || model.starts_with("o1") || model.starts_with("o3") {
                "openai".to_string()
            } else {
                "claude".to_string()
            }
        });
        let reasoning = opts
            .get("reasoning")
            .cloned()
            .unwrap_or_else(|| "none".to_string());
        let concurrency: usize = opts
            .get("concurrency")
            .and_then(|s| s.parse().ok())
            .unwrap_or(8);
        // Query view: "focused" (default, what we ship) or "full" (raw transcript
        // window, models the fork-the-judge-off-warm-transcript / prefix-cache idea).
        let query_view = opts
            .get("query_view")
            .cloned()
            .unwrap_or_else(|| "focused".to_string());

        // Build (qid, focused_query, pool_candidates) for judged queries only.
        let mut jobs: Vec<ExtractionJob> = Vec::new();
        for q in &queries {
            let Some(rel) = gold.get(&q.qid) else {
                continue;
            };
            if rel.is_empty() {
                continue;
            }
            let q_emb = embedding::embed(&q.query)?;
            let dense = dense_retrieve(&q_emb, &corpus, 0.0, rerank_pool, false);
            let lex = bm25.search(&q.query, rerank_pool);
            let pool = rrf(&[dense, lex], 60.0, rerank_pool);
            let cands: Vec<(String, String)> = pool
                .into_iter()
                .map(|(id, _)| {
                    (
                        id.clone(),
                        content_by_id.get(&id).cloned().unwrap_or_default(),
                    )
                })
                .collect();
            // Query view fed to the reranker:
            //   focused (default) = noise-stripped latest-user intent (what we ship);
            //   full = the raw transcript window (models the cache/fork prefix; tests
            //     whether feeding the noisy window as the query costs accuracy);
            //   prefix_suffix = full transcript THEN the focused intent appended as a
            //     suffix - models the cache-friendly fork done right: transcript is the
            //     shared (cacheable) prefix, but the model is told to focus on the
            //     latest request at the end. Tests if that recovers quality.
            let rq = match query_view.as_str() {
                "full" => q.query.clone(),
                "prefix_suffix" => {
                    let focused = focus_query(&q.query);
                    format!(
                        "{}\n\n=== LATEST REQUEST (focus your ranking on THIS) ===\n{}",
                        q.query, focused
                    )
                }
                _ => focus_query(&q.query),
            };
            jobs.push((q.qid.clone(), rq, cands));
        }
        eprintln!(
            "llm_rerank: reranking {} queries (pool={}) with {} backend={} (concurrency {})",
            jobs.len(),
            rerank_pool,
            model,
            backend,
            concurrency
        );

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let raw: Vec<(String, Vec<String>, String, usize, usize)> = rt.block_on(async {
            use futures::stream::{self, StreamExt};
            stream::iter(jobs)
                .map(|(qid, query, cands)| {
                    let model = model.clone();
                    let backend = backend.clone();
                    let reasoning = reasoning.clone();
                    async move {
                        let sidecar = if backend == "openai" {
                            let eff = if reasoning == "default" {
                                None
                            } else {
                                Some(reasoning)
                            };
                            jcode::sidecar::Sidecar::with_openai_model(&model, eff)
                        } else {
                            jcode::sidecar::Sidecar::with_claude_model(&model)
                        };
                        let n = cands.len();
                        // Prompt + system for cost accounting (char proxy for tokens).
                        let (system, prompt) = if synth {
                            (LLM_SYNTH_SYSTEM, build_synth_prompt(&query, &cands))
                        } else if strict {
                            (
                                LLM_RERANK_STRICT_SYSTEM,
                                build_rerank_prompt(&query, &cands),
                            )
                        } else if judge_sel {
                            // Use the EXACT gold-judge prompt as the selector: the
                            // gold labels were produced by this judge, so this is
                            // the precision ceiling for an LLM selector (it agrees
                            // with the labeler by construction up to sampling noise).
                            (JUDGE_SYSTEM, build_rerank_prompt(&query, &cands))
                        } else {
                            (LLM_RERANK_SYSTEM, build_rerank_prompt(&query, &cands))
                        };
                        let prompt_chars = system.len() + prompt.len();
                        let mut ranked_ids: Vec<String> = Vec::new();
                        let mut note = String::new();
                        for attempt in 0..2 {
                            match sidecar.complete(system, &prompt).await {
                                Ok(resp) => {
                                    if synth {
                                        let (used, n_text) = parse_synth_response(&resp, n);
                                        ranked_ids =
                                            used.iter().map(|&i| cands[i].0.clone()).collect();
                                        note = n_text;
                                    } else {
                                        let kept = parse_rerank_response(&resp, n);
                                        let kept_set: std::collections::HashSet<usize> =
                                            kept.iter().copied().collect();
                                        ranked_ids =
                                            kept.iter().map(|&i| cands[i].0.clone()).collect();
                                        if padded {
                                            for (i, (id, _)) in cands.iter().enumerate() {
                                                if !kept_set.contains(&i) {
                                                    ranked_ids.push(id.clone());
                                                }
                                            }
                                        }
                                    }
                                    break;
                                }
                                Err(e) => {
                                    if attempt == 1 {
                                        eprintln!("llm_rerank failed for {qid}: {e}");
                                    } else {
                                        tokio::time::sleep(std::time::Duration::from_millis(500))
                                            .await;
                                    }
                                }
                            }
                        }
                        (qid, ranked_ids, note.clone(), prompt_chars, note.len())
                    }
                })
                .buffer_unordered(concurrency)
                .collect::<Vec<_>>()
                .await
        });

        // For synth: save the notes + cost proxy for human review, and report
        // average prompt/output sizes (the real lever is cost/latency, not the
        // circular recall score).
        if synth {
            let notes_path = bench_root().join("results/synth_notes.jsonl");
            let mut out = String::new();
            let mut total_prompt = 0usize;
            let mut total_note = 0usize;
            for (qid, used, note, pchars, nchars) in &raw {
                total_prompt += *pchars;
                total_note += *nchars;
                out.push_str(
                    &serde_json::to_string(&serde_json::json!({
                        "qid": qid,
                        "used": used,
                        "note": note,
                        "prompt_chars": pchars,
                        "note_chars": nchars,
                    }))
                    .unwrap_or_default(),
                );
                out.push('\n');
            }
            let _ = std::fs::write(&notes_path, out);
            let nq = raw.len().max(1);
            eprintln!(
                "llm_synth: {} notes -> {} | avg prompt ~{} chars (~{} tok), avg note ~{} chars (~{} tok)",
                raw.len(),
                notes_path.display(),
                total_prompt / nq,
                total_prompt / nq / 4,
                total_note / nq,
                total_note / nq / 4,
            );
        }

        // Cost accounting (1 LLM call per judged query). Char proxy / 4 ~= tokens.
        {
            let nq = raw.len().max(1);
            let total_prompt: usize = raw.iter().map(|(_, _, _, p, _)| *p).sum();
            let total_out: usize = raw.iter().map(|(_, _, n, _, _)| n.len()).sum();
            eprintln!(
                "{config}: {} LLM calls | avg prompt ~{} tok, avg output ~{} tok (1 call/turn)",
                raw.len(),
                total_prompt / nq / 4,
                total_out / nq / 4,
            );
            LLM_CALLS.store(raw.len(), std::sync::atomic::Ordering::Relaxed);
            LLM_PROMPT_TOK.store(total_prompt / nq / 4, std::sync::atomic::Ordering::Relaxed);
            LLM_OUTPUT_TOK.store(total_out / nq / 4, std::sync::atomic::Ordering::Relaxed);
        }

        raw.into_iter()
            .map(|(qid, ids, _, _, _)| (qid, ids))
            .collect()
    } else if config == "llm_cached" {
        // Replay a previously-saved llm_rerank map (results/llm_rerank_map.json)
        // so cap/gate sweeps are free (no LLM calls). Written by any llm_* run.
        let path = bench_root().join("results/llm_rerank_map.json");
        let txt = std::fs::read_to_string(&path).with_context(|| {
            format!(
                "no cached map at {} (run an llm_* config first)",
                path.display()
            )
        })?;
        serde_json::from_str(&txt)?
    } else {
        HashMap::new()
    };
    // Persist the freshly-computed map for cheap replay via config=llm_cached.
    if !llm_rerank_map.is_empty() && config != "llm_cached" {
        let path = bench_root().join("results/llm_rerank_map.json");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, serde_json::to_string(&llm_rerank_map)?);
    }

    // ---- Confidence-scored rerank (llm_scored / llm_scored_cached) ----
    // `llm_scored` asks the model for a 0-100 usefulness score per candidate and
    // caches the full score map. `llm_scored_cached` replays that cache so a
    // --score_threshold sweep is free. Selection at scoring time: keep candidates
    // with score >= score_threshold (dynamic count incl. 0), capped at gate_max.
    let score_threshold: f32 = opts
        .get("score_threshold")
        .and_then(|s| s.parse().ok())
        .unwrap_or(80.0);
    let llm_score_map: HashMap<String, Vec<(String, f32)>> = if config == "llm_scored" {
        let model = opts
            .get("model")
            .cloned()
            .unwrap_or_else(|| "claude-sonnet-4-6".to_string());
        let backend = opts.get("backend").cloned().unwrap_or_else(|| {
            if model.starts_with("gpt") || model.starts_with("o1") || model.starts_with("o3") {
                "openai".into()
            } else {
                "claude".into()
            }
        });
        let concurrency: usize = opts
            .get("concurrency")
            .and_then(|s| s.parse().ok())
            .unwrap_or(8);
        let mut jobs: Vec<ExtractionJob> = Vec::new();
        for q in &queries {
            let Some(rel) = gold.get(&q.qid) else {
                continue;
            };
            if rel.is_empty() {
                continue;
            }
            let q_emb = embedding::embed(&q.query)?;
            let dense = dense_retrieve(&q_emb, &corpus, 0.0, rerank_pool, false);
            let lex = bm25.search(&q.query, rerank_pool);
            let pool = rrf(&[dense, lex], 60.0, rerank_pool);
            let cands: Vec<(String, String)> = pool
                .into_iter()
                .map(|(id, _)| {
                    (
                        id.clone(),
                        content_by_id.get(&id).cloned().unwrap_or_default(),
                    )
                })
                .collect();
            jobs.push((q.qid.clone(), focus_query(&q.query), cands));
        }
        eprintln!(
            "llm_scored: scoring {} queries (pool={}) with {} (concurrency {})",
            jobs.len(),
            rerank_pool,
            model,
            concurrency
        );
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let raw: Vec<RecallResultRow> = rt.block_on(async {
            use futures::stream::{self, StreamExt};
            stream::iter(jobs)
                .map(|(qid, query, cands)| {
                    let model = model.clone();
                    let backend = backend.clone();
                    async move {
                        let sidecar = if backend == "openai" {
                            jcode::sidecar::Sidecar::with_openai_model(&model, None)
                        } else {
                            jcode::sidecar::Sidecar::with_claude_model(&model)
                        };
                        let n = cands.len();
                        let prompt = build_rerank_prompt(&query, &cands);
                        let prompt_chars = LLM_SCORED_SYSTEM.len() + prompt.len();
                        let mut scored: Vec<(String, f32)> = Vec::new();
                        let mut out_chars = 0usize;
                        for attempt in 0..2 {
                            match sidecar.complete(LLM_SCORED_SYSTEM, &prompt).await {
                                Ok(resp) => {
                                    out_chars = resp.len();
                                    scored = parse_scored_response(&resp, n)
                                        .into_iter()
                                        .map(|(i, s)| (cands[i].0.clone(), s))
                                        .collect();
                                    break;
                                }
                                Err(e) => {
                                    if attempt == 1 {
                                        eprintln!("llm_scored failed for {qid}: {e}");
                                    } else {
                                        tokio::time::sleep(std::time::Duration::from_millis(500))
                                            .await;
                                    }
                                }
                            }
                        }
                        (qid, scored, prompt_chars, out_chars)
                    }
                })
                .buffer_unordered(concurrency)
                .collect::<Vec<_>>()
                .await
        });
        let nq = raw.len().max(1);
        let tp: usize = raw.iter().map(|(_, _, p, _)| *p).sum();
        let to: usize = raw.iter().map(|(_, _, _, o)| *o).sum();
        eprintln!(
            "llm_scored: {} LLM calls | avg prompt ~{} tok, avg output ~{} tok (1 call/turn)",
            raw.len(),
            tp / nq / 4,
            to / nq / 4
        );
        LLM_CALLS.store(raw.len(), std::sync::atomic::Ordering::Relaxed);
        LLM_PROMPT_TOK.store(tp / nq / 4, std::sync::atomic::Ordering::Relaxed);
        LLM_OUTPUT_TOK.store(to / nq / 4, std::sync::atomic::Ordering::Relaxed);
        let map: HashMap<String, Vec<(String, f32)>> =
            raw.into_iter().map(|(q, s, _, _)| (q, s)).collect();
        let path = bench_root().join("results/llm_score_map.json");
        let _ = std::fs::write(&path, serde_json::to_string(&map)?);
        map
    } else if config == "llm_scored_cached" {
        let path = bench_root().join("results/llm_score_map.json");
        let txt = std::fs::read_to_string(&path).with_context(|| {
            format!(
                "no cached score map at {} (run llm_scored first)",
                path.display()
            )
        })?;
        // Cached llm_scored runs ran the LLM, so report 1 call/turn for cost.
        LLM_CALLS.store(1, std::sync::atomic::Ordering::Relaxed);
        serde_json::from_str(&txt)?
    } else if config == "llm_ensemble" {
        // ENSEMBLE voting: run the gold-judge selector `--ensemble=N` times per
        // query (independent samples) and record each candidate's vote count
        // (0..N) as its score. Keeping only high-vote candidates removes single-
        // judge noise -> higher precision. Cached to llm_score_map.json, so
        // `llm_scored_cached --score_threshold=K` sweeps the vote bar K for free.
        let n_ens: usize = opts
            .get("ensemble")
            .and_then(|s| s.parse().ok())
            .unwrap_or(3);
        let model = opts
            .get("model")
            .cloned()
            .unwrap_or_else(|| "claude-sonnet-4-6".to_string());
        let concurrency: usize = opts
            .get("concurrency")
            .and_then(|s| s.parse().ok())
            .unwrap_or(8);
        let qv = opts
            .get("query_view")
            .cloned()
            .unwrap_or_else(|| "focused".into());
        let all_queries = opts
            .get("all_queries")
            .map(|s| s == "1" || s == "true")
            .unwrap_or(false);
        let mut jobs: Vec<ExtractionJob> = Vec::new();
        for q in &queries {
            match gold.get(&q.qid) {
                Some(rel) if !rel.is_empty() => {}
                // Empty-gold (should inject nothing): include only with --all_queries,
                // so we can measure the LLM's clean-rate on no-memory-needed turns.
                _ if all_queries => {}
                _ => continue,
            }
            let q_emb = embedding::embed(&q.query)?;
            let dense = dense_retrieve(&q_emb, &corpus, 0.0, rerank_pool, false);
            let lex = bm25.search(&q.query, rerank_pool);
            let pool = rrf(&[dense, lex], 60.0, rerank_pool);
            let cands: Vec<(String, String)> = pool
                .into_iter()
                .map(|(id, _)| {
                    (
                        id.clone(),
                        content_by_id.get(&id).cloned().unwrap_or_default(),
                    )
                })
                .collect();
            let rq = if qv == "full" {
                q.query.clone()
            } else {
                focus_query(&q.query)
            };
            jobs.push((q.qid.clone(), rq, cands));
        }
        eprintln!(
            "llm_ensemble: {} queries x {} votes (pool={}) with {} (concurrency {})",
            jobs.len(),
            n_ens,
            rerank_pool,
            model,
            concurrency
        );
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let raw: Vec<RecallResultRow> = rt.block_on(async {
            use futures::stream::{self, StreamExt};
            stream::iter(jobs)
                .map(|(qid, query, cands)| {
                    let model = model.clone();
                    async move {
                        let n = cands.len();
                        let prompt = build_rerank_prompt(&query, &cands);
                        let prompt_chars = (JUDGE_SYSTEM.len() + prompt.len()) * n_ens;
                        let mut votes: HashMap<usize, usize> = HashMap::new();
                        let mut out_chars = 0usize;
                        for _ in 0..n_ens {
                            let sidecar = jcode::sidecar::Sidecar::with_claude_model(&model);
                            for attempt in 0..2 {
                                match sidecar.complete(JUDGE_SYSTEM, &prompt).await {
                                    Ok(resp) => {
                                        out_chars += resp.len();
                                        for i in parse_rerank_response(&resp, n) {
                                            *votes.entry(i).or_insert(0) += 1;
                                        }
                                        break;
                                    }
                                    Err(e) => {
                                        if attempt == 1 {
                                            eprintln!("llm_ensemble failed for {qid}: {e}");
                                        } else {
                                            tokio::time::sleep(std::time::Duration::from_millis(
                                                500,
                                            ))
                                            .await;
                                        }
                                    }
                                }
                            }
                        }
                        let scored: Vec<(String, f32)> = votes
                            .into_iter()
                            .map(|(i, v)| (cands[i].0.clone(), v as f32))
                            .collect();
                        (qid, scored, prompt_chars, out_chars)
                    }
                })
                .buffer_unordered(concurrency)
                .collect::<Vec<_>>()
                .await
        });
        let nq = raw.len().max(1);
        let tp: usize = raw.iter().map(|(_, _, p, _)| *p).sum();
        let to: usize = raw.iter().map(|(_, _, _, o)| *o).sum();
        eprintln!(
            "llm_ensemble: {} queries | {} calls/turn | avg prompt ~{} tok, avg output ~{} tok",
            raw.len(),
            n_ens,
            tp / nq / 4,
            to / nq / 4
        );
        LLM_CALLS.store(n_ens, std::sync::atomic::Ordering::Relaxed);
        LLM_PROMPT_TOK.store(tp / nq / 4, std::sync::atomic::Ordering::Relaxed);
        LLM_OUTPUT_TOK.store(to / nq / 4, std::sync::atomic::Ordering::Relaxed);
        let map: HashMap<String, Vec<(String, f32)>> =
            raw.into_iter().map(|(q, s, _, _)| (q, s)).collect();
        let path = bench_root().join("results/llm_score_map.json");
        let _ = std::fs::write(&path, serde_json::to_string(&map)?);
        map
    } else {
        HashMap::new()
    };

    let mut recall5 = 0.0;
    let mut recall10 = 0.0;
    let mut precision5 = 0.0;
    let mut precision10 = 0.0;
    let mut mrr = 0.0;
    let mut ndcg = 0.0;
    let mut judged = 0usize;
    let mut total_injected = 0usize;
    // Full-set "bloat" accounting over EMPTY-gold queries (the ones that should
    // inject NOTHING, e.g. a UI task that needs no memory). These dominate real
    // usage (122/150 here) and are where context bloat actually happens, but the
    // recall/precision@k loop below skips them. We measure them only for configs
    // whose selection is query-adaptive (can return 0): the no-LLM dynamic gate
    // and the LLM selectors. Fixed top-k always injects k here by construction.
    let empty_aware = matches!(
        config.as_str(),
        "hybrid_dyn"
            | "oracle_dyn"
            | "llm_rerank"
            | "llm_strict"
            | "llm_judge"
            | "llm_scored"
            | "llm_scored_cached"
            | "llm_ensemble"
    );
    let mut empty_q = 0usize;
    let mut empty_clean = 0usize; // empty-gold queries where we injected 0 (good)
    let mut empty_injected = 0usize; // total wrongly-injected memories on empty-gold
    if empty_aware {
        for q in &queries {
            let is_empty = gold.get(&q.qid).map(|r| r.is_empty()).unwrap_or(true);
            if !is_empty {
                continue;
            }
            empty_q += 1;
            let q_emb = embedding::embed(&q.query)?;
            let origin: HashSet<&String> = q.origin_memory_ids.iter().collect();
            let injected: Vec<String> = match config.as_str() {
                "hybrid_dyn" => {
                    let dense = dense_retrieve(&q_emb, &corpus, 0.0, 50, false);
                    let lex = bm25.search(&q.query, 50);
                    let pool = rrf(&[dense, lex], 60.0, 50);
                    dynamic_gate_abs(&pool, gate_floor, gate_drop, gate_max, gate_abs)
                }
                "oracle_dyn" => Vec::new(), // gold is empty -> oracle injects nothing
                "llm_scored" | "llm_scored_cached" | "llm_ensemble" => {
                    let mut scored = llm_score_map.get(&q.qid).cloned().unwrap_or_default();
                    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
                    scored
                        .into_iter()
                        .filter(|(_, s)| *s >= score_threshold)
                        .take(gate_max)
                        .map(|(id, _)| id)
                        .collect()
                }
                // LLM keep/drop selectors: not precomputed for empty-gold queries
                // (the rerank map only covers judged queries), so skip scoring them
                // here rather than fire extra LLM calls.
                _ => {
                    empty_q -= 1;
                    continue;
                }
            };
            let injected: Vec<String> = injected
                .into_iter()
                .filter(|id| !origin.contains(id))
                .collect();
            if injected.is_empty() {
                empty_clean += 1;
            }
            empty_injected += injected.len();
        }
    }

    for q in &queries {
        let Some(rel) = gold.get(&q.qid) else {
            continue;
        };
        if rel.is_empty() {
            continue;
        }
        judged += 1;
        let q_emb = embedding::embed(&q.query)?;
        let focused = focus_query(&q.query);
        let q_emb_focused = embedding::embed(&focused)?;
        let q_emb_alt = alt_embedder.as_ref().map(|emb| {
            emb.embed(&format!("{query_prefix}{}", q.query))
                .unwrap_or_default()
        });
        let q_emb_openai = openai_backend.as_ref().map(|b| {
            use jcode::embedding_backend::EmbeddingBackend;
            b.embed_query(&q.query)
                .expect("OpenAI query embedding failed")
        });
        let origin: HashSet<&String> = q.origin_memory_ids.iter().collect();

        let ranked: Vec<String> = match config.as_str() {
            "baseline" => dense_retrieve(
                &q_emb,
                &corpus,
                EMBEDDING_SIMILARITY_THRESHOLD,
                EMBEDDING_MAX_HITS,
                true,
            )
            .into_iter()
            .map(|(id, _)| id)
            .collect(),
            "dense_nogap" => dense_retrieve(
                &q_emb,
                &corpus,
                EMBEDDING_SIMILARITY_THRESHOLD,
                EMBEDDING_MAX_HITS,
                false,
            )
            .into_iter()
            .map(|(id, _)| id)
            .collect(),
            "dense_t0" => dense_retrieve(&q_emb, &corpus, 0.0, EMBEDDING_MAX_HITS, false)
                .into_iter()
                .map(|(id, _)| id)
                .collect(),
            "dense_t35" => dense_retrieve(&q_emb, &corpus, 0.35, EMBEDDING_MAX_HITS, false)
                .into_iter()
                .map(|(id, _)| id)
                .collect(),
            "bm25" => bm25
                .search(&q.query, EMBEDDING_MAX_HITS)
                .into_iter()
                .map(|(id, _)| id)
                .collect(),
            "hybrid" => {
                let dense = dense_retrieve(&q_emb, &corpus, 0.0, 50, false);
                let lex = bm25.search(&q.query, 50);
                rrf(&[dense, lex], 60.0, EMBEDDING_MAX_HITS)
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect()
            }
            "ce_rerank" => {
                // recall-5: hybrid top-N candidate pool, reranked by a local
                // cross-encoder (--reranker=<dir>). The empirical counterpart to
                // oracle_rerank.
                let ce = ce_reranker
                    .as_ref()
                    .expect("--reranker required for ce_rerank");
                let dense = dense_retrieve(&q_emb, &corpus, 0.0, rerank_pool, false);
                let lex = bm25.search(&q.query, rerank_pool);
                let pool = rrf(&[dense, lex], 60.0, rerank_pool);
                let cands: Vec<(String, String)> = pool
                    .into_iter()
                    .map(|(id, _)| {
                        let text = content_by_id.get(&id).cloned().unwrap_or_default();
                        (id, text)
                    })
                    .collect();
                ce.rerank(&q.query, &cands)?
                    .into_iter()
                    .take(EMBEDDING_MAX_HITS)
                    .map(|(id, _)| id)
                    .collect()
            }
            "ce_rerank_focused" => {
                // Same as ce_rerank but feed the cross-encoder the FOCUSED query
                // (boilerplate/tool-output stripped, latest user intent) since
                // cross-encoders are trained on short clean queries.
                let ce = ce_reranker
                    .as_ref()
                    .expect("--reranker required for ce_rerank_focused");
                let dense = dense_retrieve(&q_emb, &corpus, 0.0, rerank_pool, false);
                let lex = bm25.search(&q.query, rerank_pool);
                let pool = rrf(&[dense, lex], 60.0, rerank_pool);
                let cands: Vec<(String, String)> = pool
                    .into_iter()
                    .map(|(id, _)| {
                        (
                            id.clone(),
                            content_by_id.get(&id).cloned().unwrap_or_default(),
                        )
                    })
                    .collect();
                // Use just the most recent user line as the rerank query.
                let rq = focus_query(&q.query);
                let rq = rq.lines().next().unwrap_or(&rq);
                ce.rerank(rq, &cands)?
                    .into_iter()
                    .take(EMBEDDING_MAX_HITS)
                    .map(|(id, _)| id)
                    .collect()
            }
            "oracle_rerank" => {
                // CEILING (fixed-k): take the hybrid top-N candidate POOL, then
                // perfectly reorder it using gold (oracle), padded to top-N. Kept
                // for backwards comparison; precision is capped because it pads
                // irrelevant filler up to N even when only 1-2 are relevant.
                let dense = dense_retrieve(&q_emb, &corpus, 0.0, 50, false);
                let lex = bm25.search(&q.query, 50);
                let pool = rrf(&[dense, lex], 60.0, 50);
                let rel_set: HashSet<&String> = rel.iter().collect();
                let mut ids: Vec<String> = pool.into_iter().map(|(id, _)| id).collect();
                // Stable sort: relevant candidates first, original order otherwise.
                ids.sort_by_key(|id| !rel_set.contains(id));
                ids.into_iter().take(EMBEDDING_MAX_HITS).collect()
            }
            "oracle_dyn" => {
                // TRUE CEILING (dynamic-k): inject EXACTLY the relevant memories
                // present in the hybrid pool, nothing else. This is the oracle for
                // DYNAMIC injection: a perfect system injects the relevant set and
                // 0 irrelevant items, so precision == recall == 1.0 whenever the
                // pool contains all gold (and the only loss is retrieval misses,
                // not padding). Demonstrates the precision headroom that fixed
                // top-k structurally throws away.
                let dense = dense_retrieve(&q_emb, &corpus, 0.0, 50, false);
                let lex = bm25.search(&q.query, 50);
                let pool = rrf(&[dense, lex], 60.0, 50);
                let rel_set: HashSet<&String> = rel.iter().collect();
                pool.into_iter()
                    .map(|(id, _)| id)
                    .filter(|id| rel_set.contains(id))
                    .collect()
            }
            "hybrid_dyn" => {
                // Hybrid retrieval + DYNAMIC variable-k gate (no LLM). Injects a
                // per-query count (incl. 0/1) by cutting the ranked list at a score
                // floor / cliff relative to the top score. The shippable no-sidecar
                // precision improvement. Tune via --gate_floor/--gate_drop/--gate_max.
                let dense = dense_retrieve(&q_emb, &corpus, 0.0, 50, false);
                let lex = bm25.search(&q.query, 50);
                let pool = rrf(&[dense, lex], 60.0, 50);
                dynamic_gate_abs(&pool, gate_floor, gate_drop, gate_max, gate_abs)
            }
            "llm_rerank" | "llm_rerank_padded" | "llm_strict" | "llm_judge" | "llm_synth"
            | "llm_cached" => {
                // recall-5 Mode-2: listwise LLM rerank of the hybrid top-N pool,
                // precomputed into llm_rerank_map above. `llm_rerank` is precision
                // mode (relevant-only); `llm_rerank_padded` emulates the old buggy
                // prod path (pad to 5 with model-omitted candidates).
                llm_rerank_map
                    .get(&q.qid)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .take(gate_max.min(EMBEDDING_MAX_HITS))
                    .collect()
            }
            "llm_scored" | "llm_scored_cached" | "llm_ensemble" => {
                // Threshold the cached per-candidate scores: keep score >=
                // score_threshold, best-first, capped at gate_max. Dynamic count
                // (incl. 0). Raising --score_threshold trades recall for precision.
                let mut scored = llm_score_map.get(&q.qid).cloned().unwrap_or_default();
                scored.sort_by(|a, b| b.1.total_cmp(&a.1));
                scored
                    .into_iter()
                    .filter(|(_, s)| *s >= score_threshold)
                    .take(gate_max)
                    .map(|(id, _)| id)
                    .collect()
            }
            "hybrid_priors" => {
                let dense = dense_retrieve(&q_emb, &corpus, 0.0, 50, false);
                let lex = bm25.search(&q.query, 50);
                let fused = rrf(&[dense, lex], 60.0, 50);
                // Multiply fused RRF score by a gentle prior derived from
                // confidence / strength / recency. Priors only re-order within
                // the already-retrieved set; they never add/remove candidates.
                let prior: HashMap<&String, f32> =
                    corpus.active().map(|m| (&m.id, memory_prior(m))).collect();
                let mut adj: Vec<(String, f32)> = fused
                    .into_iter()
                    .map(|(id, s)| {
                        let p = prior.get(&id).copied().unwrap_or(1.0);
                        (id, s * p)
                    })
                    .collect();
                adj.sort_by(|a, b| b.1.total_cmp(&a.1));
                adj.into_iter()
                    .take(EMBEDDING_MAX_HITS)
                    .map(|(id, _)| id)
                    .collect()
            }
            "hybrid_focused" => {
                let dense = dense_retrieve(&q_emb_focused, &corpus, 0.0, 50, false);
                let lex = bm25.search(&focused, 50);
                rrf(&[dense, lex], 60.0, EMBEDDING_MAX_HITS)
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect()
            }
            "hybrid_expand" => {
                // Hybrid base ranking, then 1-hop graph expansion: each base hit
                // contributes its graph neighbors with a decayed score, fused in.
                let dense = dense_retrieve(&q_emb, &corpus, 0.0, 50, false);
                let lex = bm25.search(&q.query, 50);
                let base = rrf(&[dense, lex], 60.0, 50);
                let mut scored: HashMap<String, f32> = base.iter().cloned().collect();
                // Expansion: add neighbors of the top base hits with 0.5 decay.
                for (id, score) in base.iter().take(10) {
                    if let Some(neighbors) = corpus.expand_edges.get(id) {
                        for nb in neighbors {
                            let add = *score * 0.5;
                            scored
                                .entry(nb.clone())
                                .and_modify(|s| *s = s.max(add))
                                .or_insert(add);
                        }
                    }
                }
                let mut v: Vec<(String, f32)> = scored.into_iter().collect();
                v.sort_by(|a, b| b.1.total_cmp(&a.1));
                v.into_iter()
                    .take(EMBEDDING_MAX_HITS)
                    .map(|(id, _)| id)
                    .collect()
            }
            "bge_dense" => {
                let qe = q_emb_alt
                    .as_ref()
                    .expect("--embedder required for bge_dense");
                alt_dense_rank(qe, &alt_corpus_emb, EMBEDDING_MAX_HITS)
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect()
            }
            "bge_hybrid" => {
                let qe = q_emb_alt
                    .as_ref()
                    .expect("--embedder required for bge_hybrid");
                let dense = alt_dense_rank(qe, &alt_corpus_emb, 50);
                let lex = bm25.search(&q.query, 50);
                rrf(&[dense, lex], 60.0, EMBEDDING_MAX_HITS)
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect()
            }
            "openai_dense" => {
                // Pure dense retrieval using OpenAI embeddings (top-k cosine).
                let qe = q_emb_openai
                    .as_ref()
                    .expect("openai_dense requires the OpenAI backend");
                alt_dense_rank(qe, &openai_corpus_emb, EMBEDDING_MAX_HITS)
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect()
            }
            "openai_hybrid" => {
                // Hybrid: OpenAI dense + BM25 lexical fused with RRF (mirrors the
                // shipped find_similar_hybrid fusion, just with OpenAI vectors).
                let qe = q_emb_openai
                    .as_ref()
                    .expect("openai_hybrid requires the OpenAI backend");
                let dense = alt_dense_rank(qe, &openai_corpus_emb, 50);
                let lex = bm25.search(&q.query, 50);
                rrf(&[dense, lex], 60.0, EMBEDDING_MAX_HITS)
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect()
            }
            "prod_hybrid" => {
                // Validate the ACTUAL shipped production method end-to-end.
                prod_mgr
                    .as_ref()
                    .expect("prod manager")
                    .find_similar_hybrid(&q.query, &q_emb, EMBEDDING_MAX_HITS)?
                    .into_iter()
                    .map(|(e, _)| e.id)
                    .collect()
            }
            other => anyhow::bail!("unknown config: {other}"),
        };
        let ranked: Vec<String> = ranked
            .into_iter()
            .filter(|id| !origin.contains(id))
            .collect();
        let rel_set: HashSet<&String> = rel.iter().collect();
        total_injected += ranked.len();

        recall5 += recall_at(&ranked, &rel_set, 5);
        recall10 += recall_at(&ranked, &rel_set, 10);
        precision5 += precision_at(&ranked, &rel_set, 5);
        precision10 += precision_at(&ranked, &rel_set, 10);
        mrr += reciprocal_rank(&ranked, &rel_set);
        ndcg += ndcg_at(&ranked, &rel_set, 10);
    }

    let n = judged.max(1) as f32;
    let result = serde_json::json!({
        "config": config,
        "corpus": graph_file,
        "queries_judged": judged,
        "recall@5": recall5 / n,
        "recall@10": recall10 / n,
        "precision@5": precision5 / n,
        "precision@10": precision10 / n,
        "mrr": mrr / n,
        "ndcg@10": ndcg / n,
        "avg_injected": total_injected as f32 / n,
        "llm_calls_per_turn": if LLM_CALLS.load(std::sync::atomic::Ordering::Relaxed) > 0 { 1 } else { 0 },
        "llm_prompt_tok": LLM_PROMPT_TOK.load(std::sync::atomic::Ordering::Relaxed),
        "llm_output_tok": LLM_OUTPUT_TOK.load(std::sync::atomic::Ordering::Relaxed),
        // Bloat metrics on EMPTY-gold queries (should inject 0). Only meaningful
        // for query-adaptive configs; 0 counts when not measured.
        "empty_gold_queries": empty_q,
        "empty_gold_clean_rate": if empty_q > 0 { empty_clean as f32 / empty_q as f32 } else { 0.0 },
        "empty_gold_avg_injected": if empty_q > 0 { empty_injected as f32 / empty_q as f32 } else { 0.0 },
    });
    let out_path = bench_root().join(format!("results/{}.json", config));
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&out_path, serde_json::to_string_pretty(&result)?)?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

fn recall_at(ranked: &[String], rel: &HashSet<&String>, k: usize) -> f32 {
    if rel.is_empty() {
        return 0.0;
    }
    let hit = ranked.iter().take(k).filter(|id| rel.contains(id)).count();
    hit as f32 / rel.len() as f32
}

/// Precision@k = fraction of the top-k SURFACED items that are relevant.
/// Denominator is min(k, results) so a config that returns fewer than k items is
/// not unfairly penalized for empty slots.
fn precision_at(ranked: &[String], rel: &HashSet<&String>, k: usize) -> f32 {
    let denom = ranked.len().min(k);
    if denom == 0 {
        return 0.0;
    }
    let hit = ranked.iter().take(k).filter(|id| rel.contains(id)).count();
    hit as f32 / denom as f32
}

fn reciprocal_rank(ranked: &[String], rel: &HashSet<&String>) -> f32 {
    for (i, id) in ranked.iter().enumerate() {
        if rel.contains(id) {
            return 1.0 / (i as f32 + 1.0);
        }
    }
    0.0
}

fn ndcg_at(ranked: &[String], rel: &HashSet<&String>, k: usize) -> f32 {
    let mut dcg = 0.0;
    for (i, id) in ranked.iter().take(k).enumerate() {
        if rel.contains(id) {
            dcg += 1.0 / ((i as f32 + 2.0).ln() / 2f32.ln());
        }
    }
    let ideal_hits = rel.len().min(k);
    let mut idcg = 0.0;
    for i in 0..ideal_hits {
        idcg += 1.0 / ((i as f32 + 2.0).ln() / 2f32.ln());
    }
    if idcg == 0.0 { 0.0 } else { dcg / idcg }
}

// ---------------- helpers ----------------

fn cmd_probe(args: &[String]) -> Result<()> {
    let opts = parse_kv(args);
    let dir = opts.get("embedder").cloned().expect("--embedder required");
    let qp = opts.get("query_prefix").cloned().unwrap_or_default();
    let pp = opts.get("passage_prefix").cloned().unwrap_or_default();
    let e = jcode::embedding::Embedder::load_from_dir(Path::new(&dir))?;
    let a = e.embed(&format!("{qp}cargo build profile"))?;
    let b = e.embed(&format!("{pp}The build uses cargo profile selfdev"))?;
    let c = e.embed(&format!("{pp}coffee brewing temperature guide"))?;
    let norm = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
    let dot = |x: &[f32], y: &[f32]| x.iter().zip(y).map(|(p, q)| p * q).sum::<f32>();
    println!(
        "dim={} normA={:.4} normB={:.4} normC={:.4}",
        a.len(),
        norm(&a),
        norm(&b),
        norm(&c)
    );
    println!(
        "cos(query,relevant)={:.4} cos(query,unrelated)={:.4}",
        dot(&a, &b),
        dot(&a, &c)
    );
    println!("a[0..8]={:?}", &a[0..8.min(a.len())]);
    Ok(())
}

/// Measure the topic-stability gate: replay real sessions turn-by-turn, embed
/// each live query window, and decide whether the expensive LLM rerank would
/// fire. The gate skips the rerank when the current context is highly similar
/// (cosine >= threshold) to the embedding captured the last time the rerank
/// actually ran. This quantifies the activation-reduction the gate buys on real
/// traffic. We sweep several thresholds and report fire-rate per threshold.
fn cmd_gate(args: &[String]) -> Result<()> {
    let opts = parse_kv(args);
    let sessions_dir = opts
        .get("sessions")
        .cloned()
        .unwrap_or_else(|| format!("{}/.jcode/sessions", dirs_home().display()));
    let max_sessions: usize = opts
        .get("max_sessions")
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    let working_dir_filter = opts.get("working_dir").cloned();
    // Thresholds to sweep. A turn FIRES the rerank when cosine(current, last_fired)
    // < threshold (topic moved enough) OR it is the first turn of the session.
    let thresholds: Vec<f32> = opts
        .get("thresholds")
        .map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![0.80, 0.85, 0.90, 0.93, 0.95]);

    // Candidate-pool stability gate: retrieve the hybrid top-k pool per turn and
    // skip the rerank when the pool id-set is unchanged (or Jaccard >= pool_thr)
    // versus the last fired turn. This is the stronger signal: if retrieval
    // surfaces the same memories, the rerank answer cannot change. Requires
    // --corpus to load the same graph production retrieves from.
    let pool_k: usize = opts
        .get("pool_k")
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    let pool_thr: f32 = opts
        .get("pool_thr")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0);
    let corpus_pool = match opts.get("corpus") {
        Some(p) => {
            let c = Corpus::load_graph_file(Path::new(p))?;
            let bm = Bm25::build(&c);
            Some((c, bm))
        }
        None => None,
    };
    // Pool-gate tallies.
    let mut pool_fires = 0usize;
    let mut pool_total = 0usize;
    // Novelty-gate tallies: fire only when the top-k hybrid pool contains at
    // least one memory NOT already surfaced this session (the natural trigger,
    // since already-injected memories are filtered out before reranking). Also
    // apply an optional min-cadence (>= cadence turns since last fire).
    let cadence: usize = opts
        .get("cadence")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut nov_fires = 0usize;
    let mut nov_total = 0usize;
    // Production-faithful gate: mirrors memory_agent::should_run_rerank exactly.
    // Fires when first-turn OR topic_changed (cosine < TOPIC_CHANGE_THRESHOLD vs
    // last embedding) OR turns_since_last_rerank >= prod_cadence. This captures
    // the real fire-rate (above 1/N because topic jumps + first turn fire extra).
    let prod_cadence: usize = opts
        .get("prod_cadence")
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);
    const TOPIC_CHANGE_THRESHOLD: f32 = 0.3;
    let mut prod_fires = 0usize;
    let mut prod_total = 0usize;
    let mut prod_topic_fires = 0usize;
    let mut prod_cadence_fires = 0usize;

    let mut sessions: Vec<(PathBuf, std::time::SystemTime)> = std::fs::read_dir(&sessions_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|s| s.to_str()) == Some("json")
                && p.file_name()
                    .and_then(|s| s.to_str())
                    .map(|n| n.starts_with("session_"))
                    .unwrap_or(false)
        })
        .filter_map(|p| {
            let mtime = std::fs::metadata(&p).ok()?.modified().ok()?;
            Some((p, mtime))
        })
        .collect();
    sessions.sort_by_key(|s| std::cmp::Reverse(s.1));

    // Per-threshold tallies.
    let mut fires = vec![0usize; thresholds.len()];
    let mut total_turns = 0usize;
    let mut used_sessions = 0usize;
    // Collect consecutive-turn similarities for a distribution summary.
    let mut consec_sims: Vec<f32> = Vec::new();

    for (path, _) in sessions {
        if used_sessions >= max_sessions {
            break;
        }
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let session: Session = match serde_json::from_slice(&bytes) {
            Ok(s) => s,
            Err(_) => continue,
        };
        if let Some(filter) = &working_dir_filter
            && session.working_dir.as_deref() != Some(filter.as_str())
        {
            continue;
        }
        let messages: Vec<_> = session.messages.iter().map(|m| m.to_message()).collect();
        // Every user turn is a relevance-check opportunity (production runs the
        // check after each user message). Skip the first two bootstrap turns to
        // match the query-sampling convention.
        let user_turns: Vec<usize> = messages
            .iter()
            .enumerate()
            .filter(|(_, m)| matches!(m.role, jcode::message::Role::User))
            .map(|(i, _)| i)
            .skip(2)
            .collect();
        if user_turns.len() < 2 {
            continue;
        }
        // Cap turns per session to keep CPU-embedding time bounded; sample evenly.
        let max_turns: usize = opts
            .get("max_turns")
            .and_then(|s| s.parse().ok())
            .unwrap_or(20);
        let user_turns: Vec<usize> = if user_turns.len() > max_turns {
            let step = user_turns.len() / max_turns;
            user_turns
                .into_iter()
                .step_by(step.max(1))
                .take(max_turns)
                .collect()
        } else {
            user_turns
        };

        // Embed each turn's live window once.
        let mut embs: Vec<Vec<f32>> = Vec::with_capacity(user_turns.len());
        for &turn in &user_turns {
            let window = &messages[..=turn];
            let query = format_context_for_relevance(window);
            if query.len() < 30 {
                embs.push(Vec::new());
                continue;
            }
            embs.push(embedding::embed(&query).unwrap_or_default());
        }

        // Consecutive similarity distribution.
        for w in embs.windows(2) {
            if !w[0].is_empty() && !w[1].is_empty() {
                consec_sims.push(embedding::cosine_similarity(&w[0], &w[1]));
            }
        }

        // Production-faithful gate sim: mirror should_run_rerank over the turn
        // sequence. turn_count increments each turn; topic_changed compares to
        // the PREVIOUS turn's embedding (matches prod's last_context_embedding,
        // which is updated every turn before the gate).
        {
            let mut last_rerank_turn: Option<usize> = None;
            let mut prev_emb: Option<&Vec<f32>> = None;
            for (turn_count, emb) in embs.iter().enumerate() {
                if emb.is_empty() {
                    continue;
                }
                let topic_changed = match prev_emb {
                    Some(p) => embedding::cosine_similarity(emb, p) < TOPIC_CHANGE_THRESHOLD,
                    None => false,
                };
                prod_total += 1;
                let fire = if topic_changed {
                    prod_topic_fires += 1;
                    true
                } else {
                    match last_rerank_turn {
                        None => true,
                        Some(last) => {
                            let due = prod_cadence <= 1
                                || turn_count.saturating_sub(last) >= prod_cadence;
                            if due {
                                prod_cadence_fires += 1;
                            }
                            due
                        }
                    }
                };
                if fire {
                    prod_fires += 1;
                    last_rerank_turn = Some(turn_count);
                }
                prev_emb = Some(emb);
            }
        }

        // Simulate the gate per threshold over this session's turn sequence.
        for (ti, &thr) in thresholds.iter().enumerate() {
            let mut last_fired: Option<&Vec<f32>> = None;
            for emb in &embs {
                if emb.is_empty() {
                    continue;
                }
                let fire = match last_fired {
                    None => true, // first real turn always fires
                    Some(prev) => embedding::cosine_similarity(emb, prev) < thr,
                };
                if fire {
                    fires[ti] += 1;
                    last_fired = Some(emb);
                }
            }
        }
        // Total turns counted once (use first threshold's traversal count basis:
        // every non-empty embedding is one opportunity).
        total_turns += embs.iter().filter(|e| !e.is_empty()).count();

        // Pool-stability gate: for each turn, retrieve the hybrid top-k pool and
        // compare its id-set to the last FIRED pool. Fire when Jaccard < pool_thr.
        if let Some((corpus, bm25)) = corpus_pool.as_ref() {
            let mut last_pool: Option<std::collections::HashSet<String>> = None;
            // Novelty state: ids already surfaced this session, and turns since
            // the last novelty-fire (for the cadence floor).
            let mut surfaced: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut since_fire = usize::MAX;
            for (idx, &turn) in user_turns.iter().enumerate() {
                let emb = &embs[idx];
                if emb.is_empty() {
                    continue;
                }
                let window = &messages[..=turn];
                let query = format_context_for_relevance(window);
                let dense = dense_retrieve(emb, corpus, 0.0, pool_k, false);
                let lex = bm25.search(&query, pool_k);
                let pool = rrf(&[dense, lex], 60.0, pool_k);
                let ids: std::collections::HashSet<String> =
                    pool.into_iter().map(|(id, _)| id).collect();
                pool_total += 1;

                // Novelty gate: is there any candidate not yet surfaced? Combined
                // with the cadence floor. Computed before `ids` is moved below.
                nov_total += 1;
                let has_new = ids.iter().any(|id| !surfaced.contains(id));
                let cadence_ok = since_fire == usize::MAX || since_fire >= cadence;
                if has_new && cadence_ok {
                    nov_fires += 1;
                    since_fire = 0;
                    for id in &ids {
                        surfaced.insert(id.clone());
                    }
                } else if since_fire != usize::MAX {
                    since_fire += 1;
                }

                let fire = match &last_pool {
                    None => true,
                    Some(prev) => {
                        let inter = ids.intersection(prev).count() as f32;
                        let uni = ids.union(prev).count().max(1) as f32;
                        (inter / uni) < pool_thr
                    }
                };
                if fire {
                    pool_fires += 1;
                    last_pool = Some(ids);
                }
            }
        }

        used_sessions += 1;
        if used_sessions.is_multiple_of(5) {
            eprintln!("  ...{used_sessions} sessions, {total_turns} turns embedded");
        }
    }

    consec_sims.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pct = |p: f32| {
        if consec_sims.is_empty() {
            return f32::NAN;
        }
        let idx = ((consec_sims.len() as f32 - 1.0) * p).round() as usize;
        consec_sims[idx]
    };
    let mean = if consec_sims.is_empty() {
        f32::NAN
    } else {
        consec_sims.iter().sum::<f32>() / consec_sims.len() as f32
    };

    println!(
        "Gate simulation over {used_sessions} sessions, {total_turns} relevance-check opportunities"
    );
    println!(
        "Consecutive-turn cosine: mean={:.3} p10={:.3} p25={:.3} p50={:.3} p75={:.3} p90={:.3}",
        mean,
        pct(0.10),
        pct(0.25),
        pct(0.50),
        pct(0.75),
        pct(0.90),
    );
    println!("\nthreshold  fire_rate  fires/total  amortization (1/fire_rate)");
    for (ti, &thr) in thresholds.iter().enumerate() {
        let rate = fires[ti] as f32 / total_turns.max(1) as f32;
        println!(
            "  {:.2}      {:6.1}%   {:>5}/{:<5}  {:.2}x fewer calls",
            thr,
            rate * 100.0,
            fires[ti],
            total_turns,
            if rate > 0.0 { 1.0 / rate } else { 0.0 }
        );
    }

    if corpus_pool.is_some() && pool_total > 0 {
        let rate = pool_fires as f32 / pool_total as f32;
        println!(
            "\nPool-stability gate (top-{pool_k}, Jaccard>={pool_thr:.2} -> skip):\n  \
             fire_rate {:.1}%  {pool_fires}/{pool_total}  {:.2}x fewer calls",
            rate * 100.0,
            if rate > 0.0 { 1.0 / rate } else { 0.0 }
        );
        let nrate = nov_fires as f32 / nov_total.max(1) as f32;
        println!(
            "Novelty gate (fire only if pool has an un-surfaced memory; cadence>={cadence}):\n  \
             fire_rate {:.1}%  {nov_fires}/{nov_total}  {:.2}x fewer calls",
            nrate * 100.0,
            if nrate > 0.0 { 1.0 / nrate } else { 0.0 }
        );
    }

    // Production-faithful gate result (always printed; needs no corpus).
    if prod_total > 0 {
        let rate = prod_fires as f32 / prod_total as f32;
        println!(
            "\nPRODUCTION gate (should_run_rerank, cadence={prod_cadence}, topic-override @cos<{:.1}):\n  \
             fire_rate {:.1}%  {prod_fires}/{prod_total}  {:.2}x fewer calls\n  \
             breakdown: {prod_topic_fires} topic-override fires + {prod_cadence_fires} cadence-due fires (+ first-turn)",
            TOPIC_CHANGE_THRESHOLD,
            rate * 100.0,
            if rate > 0.0 { 1.0 / rate } else { 0.0 },
        );
    }
    Ok(())
}

fn read_queries() -> Result<Vec<QueryRecord>> {
    let path = bench_root().join("labels/queries.jsonl");
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {} (run `queries` first)", path.display()))?;
    Ok(text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect())
}

fn read_gold() -> Result<HashMap<String, Vec<String>>> {
    let path = bench_root().join("labels/gold.jsonl");
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {} (run judge first)", path.display()))?;
    let mut map = HashMap::new();
    for line in text.lines() {
        if let Ok(g) = serde_json::from_str::<GoldRecord>(line) {
            map.insert(g.qid, g.relevant_ids);
        }
    }
    Ok(map)
}

fn parse_kv(args: &[String]) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for a in args {
        if let Some(rest) = a.strip_prefix("--")
            && let Some((k, v)) = rest.split_once('=')
        {
            m.insert(k.to_string(), v.to_string());
        }
    }
    m
}

/// Diagnostic: does the RAW top-1 dense cosine (NOT RRF, which normalizes away
/// absolute magnitude) separate empty-gold queries (should inject nothing) from
/// non-empty ones? If so, a zero-cost cosine floor can cut no-memory-turn bloat.
/// Prints percentiles for each group so we can pick a floor.
fn cmd_cosdiag(args: &[String]) -> Result<()> {
    let opts = parse_kv(args);
    let graph_file = opts.get("corpus").cloned().unwrap_or_else(|| {
        bench_root()
            .join("corpus/projects/7fe469b5e6e471c1.json")
            .display()
            .to_string()
    });
    let corpus = Corpus::load_graph_file(Path::new(&graph_file))?;
    let queries = read_queries()?;
    let gold = read_gold()?;

    let mut empty_top: Vec<f32> = Vec::new();
    let mut rel_top: Vec<f32> = Vec::new();
    for q in &queries {
        let Some(rel) = gold.get(&q.qid) else {
            continue;
        };
        let q_emb = embedding::embed(&q.query)?;
        let dense = dense_retrieve(&q_emb, &corpus, 0.0, 5, false);
        let top1 = dense.first().map(|(_, s)| *s).unwrap_or(0.0);
        if rel.is_empty() {
            empty_top.push(top1);
        } else {
            rel_top.push(top1);
        }
    }
    let pct = |v: &mut Vec<f32>, p: f32| -> f32 {
        if v.is_empty() {
            return f32::NAN;
        }
        v.sort_by(|a, b| a.total_cmp(b));
        let idx = ((v.len() as f32 - 1.0) * p).round() as usize;
        v[idx]
    };
    println!(
        "empty-gold (n={}) top1-cosine: p10={:.3} p25={:.3} p50={:.3} p75={:.3} p90={:.3} max={:.3}",
        empty_top.len(),
        pct(&mut empty_top, 0.10),
        pct(&mut empty_top, 0.25),
        pct(&mut empty_top, 0.50),
        pct(&mut empty_top, 0.75),
        pct(&mut empty_top, 0.90),
        empty_top.iter().cloned().fold(0.0_f32, f32::max),
    );
    println!(
        "relevant  (n={}) top1-cosine: p10={:.3} p25={:.3} p50={:.3} p75={:.3} p90={:.3} max={:.3}",
        rel_top.len(),
        pct(&mut rel_top, 0.10),
        pct(&mut rel_top, 0.25),
        pct(&mut rel_top, 0.50),
        pct(&mut rel_top, 0.75),
        pct(&mut rel_top, 0.90),
        rel_top.iter().cloned().fold(0.0_f32, f32::max),
    );
    // For a sweep of floors, show how many empty-gold turns go clean vs how many
    // relevant turns we would wrongly suppress (lose recall entirely).
    println!("\nfloor  empty_clean%  rel_kept%");
    for f in [0.30_f32, 0.35, 0.40, 0.45, 0.50, 0.55, 0.60, 0.65, 0.70] {
        let ec =
            empty_top.iter().filter(|&&s| s < f).count() as f32 / empty_top.len().max(1) as f32;
        let rk = rel_top.iter().filter(|&&s| s >= f).count() as f32 / rel_top.len().max(1) as f32;
        println!("{:.2}   {:.3}         {:.3}", f, ec, rk);
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().cloned().unwrap_or_default();
    let rest = if args.len() > 1 { &args[1..] } else { &[] };
    match cmd.as_str() {
        "queries" => cmd_queries(rest),
        "pool" => cmd_pool(rest),
        "judge" => cmd_judge(rest),
        "metrics" => cmd_metrics(rest),
        "probe" => cmd_probe(rest),
        "gate" => cmd_gate(rest),
        "cosdiag" => cmd_cosdiag(rest),
        _ => {
            eprintln!(
                "usage: memory_recall_bench <queries|pool|metrics> [--key=value ...]\n\
                 \n\
                 queries  --corpus=PATH --sessions=DIR --max_sessions=N --per_session=N [--working_dir=DIR]\n\
                 pool     --corpus=PATH --pool_n=50\n\
                 metrics  --corpus=PATH --config=baseline|dense_nogap|bm25|hybrid\n\
                 \n\
                 Bench dir: {} (override with MEMORY_BENCH_DIR)",
                bench_root().display()
            );
            Ok(())
        }
    }
}
