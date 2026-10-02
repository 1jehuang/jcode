//! stale_rank — per-query ranked retrieval for the stale-competition harness arm.
//!
//! Seeds a temp JCODE_HOME project graph with `--corpus` (a MemoryGraph JSON
//! file) and calls the REAL shipped ranking (`MemoryManager::find_similar_hybrid`
//! by default; `--mode=prefilter96` calls the slot-B `hybrid_prefilter_rank`
//! core at full depth for the 22-prefilter SHIP gate (a)),
//! printing one JSON object: `{query, ranked:[{id, score, active}], count}`.
//!
//! A standalone bin (not a `memory_recall_bench` subcommand) because that
//! file does not compile on this branch lineage (pre-existing breakage in
//! its reranker/alt-embedder paths; out of scope for the stale-writer).
//! Reuses only APIs that build here: `embedding::embed`, `load` via serde,
//! `save_project_graph`, `find_similar_hybrid`, `hybrid_prefilter_rank`.

use std::collections::HashMap;

use anyhow::{Context, Result};
use jcode::embedding;
use jcode::memory_graph::MemoryGraph;

fn parse_kv(args: &[String]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for arg in args {
        if let Some((key, value)) = arg.strip_prefix("--").and_then(|s| s.split_once('=')) {
            out.insert(key.to_string(), value.to_string());
        }
    }
    out
}

/// Embed every memory's content with the real ONNX model and write the graph
/// back out. Reconstructs for harness corpora what the pre-Jev `memory import`
/// did on the write path (`ensure_embedding`): without stored vectors the
/// shipped hybrid pools see nothing, so no retrieval assertion is possible.
/// The writer itself never persists vectors (transient scan only); this step
/// lives in the arm, not the product.
fn cmd_embed_corpus(opts: &HashMap<String, String>) -> Result<()> {
    let input = opts
        .get("embed_corpus")
        .cloned()
        .expect("stale_rank requires --embed-corpus=IN --out=OUT");
    let output = opts.get("out").cloned().expect("missing --out=PATH");
    let bytes = std::fs::read(&input).with_context(|| format!("reading {input}"))?;
    let mut graph: MemoryGraph =
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {input}"))?;
    let model = jcode::embedding_backend::active_model_id();
    let mut embedded = 0usize;
    for entry in graph.memories.values_mut() {
        if entry.embedding.is_some() && entry.effective_embedding_model() == model {
            continue;
        }
        let vec = embedding::embed(&entry.content)?;
        entry.set_embedding(Some(vec), Some(model.clone()));
        embedded += 1;
    }
    let out = serde_json::to_vec_pretty(&graph)?;
    std::fs::write(&output, out)?;
    println!("{}", serde_json::json!({"embedded": embedded, "out": output}));
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opts = parse_kv(&args);
    if opts.contains_key("embed_corpus") {
        return cmd_embed_corpus(&opts);
    }
    let graph_file = opts
        .get("corpus")
        .cloned()
        .expect("stale_rank requires --corpus=PATH");
    let query = opts
        .get("query")
        .cloned()
        .expect("stale_rank requires --query=...");
    let limit: usize = opts
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);

    let tmp = std::env::temp_dir().join(format!("stale-rank-{}", std::process::id()));
    std::fs::create_dir_all(&tmp)?;
    // SAFETY: single-threaded setup before any embedding work.
    unsafe { std::env::set_var("JCODE_HOME", &tmp) };
    let mgr = jcode::memory::MemoryManager::new().with_project_dir("/stale/rank-query");
    let bytes =
        std::fs::read(&graph_file).with_context(|| format!("reading {graph_file}"))?;
    let graph: MemoryGraph =
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {graph_file}"))?;
    mgr.save_project_graph(&graph)?;

    let q_emb = embedding::embed(&query)?;
    let mode = opts.get("mode").cloned().unwrap_or_default();
    let t0 = std::time::Instant::now();
    let ranked = if mode == "prefilter96" {
        // Slot-B gate (a/b): the EXACT shipped prefilter ranking core over
        // the EXACT live input set (bench helper mirrors the
        // get_relevant_parallel collection + active filter), full-depth so
        // recall@96 is measured by truncation, exactly as Jev sees it.
        let entries = mgr.prefilter_bench_entries()?;
        jcode::memory::MemoryManager::hybrid_prefilter_rank(
            entries,
            &query,
            &q_emb,
            usize::MAX,
            usize::MAX,
        )
    } else {
        mgr.find_similar_hybrid(&query, &q_emb, limit)?
    };
    let ms = t0.elapsed().as_millis();
    let ids: Vec<serde_json::Value> = ranked
        .iter()
        .map(|(entry, score)| {
            serde_json::json!({"id": entry.id, "score": score, "active": entry.active})
        })
        .collect();
    println!(
        "{}",
        serde_json::json!({"query": query, "ranked": ids, "count": ranked.len(), "ms": ms})
    );
    Ok(())
}
