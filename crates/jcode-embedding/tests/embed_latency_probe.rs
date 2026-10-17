//! Ignored embed-latency probe for the tract inference stack.
//!
//! Motivation: at opt-level 0 (plain dev/selfdev profiles before the
//! workspace pinned tract-* and tokenizers to opt-level 3) a single MiniLM
//! embed measured ~666 ms of interpreter overhead on the shared server. That
//! latency kept the embedding model "busy" through recurring memory
//! maintenance, so the 15-minute idle unloader never fired and the model's
//! ~100 MB stayed resident indefinitely.
//!
//! Run with:
//!   cargo test -p jcode-embedding --test embed_latency_probe -- --ignored --nocapture
//!
//! Requires the MiniLM model to be installed (~/.kcode/models/all-MiniLM-L6-v2).

use std::path::PathBuf;

fn model_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let dir = home
        .join(".kcode")
        .join("models")
        .join(jcode_embedding::MODEL_NAME);
    dir.join("model.onnx").exists().then_some(dir)
}

#[test]
#[ignore = "perf probe; requires installed model; run with --ignored --nocapture"]
fn embed_latency_probe() {
    let Some(dir) = model_dir() else {
        eprintln!("model not installed; skipping");
        return;
    };
    let load_start = std::time::Instant::now();
    let embedder = jcode_embedding::Embedder::load_from_dir(&dir).expect("load model");
    let load = load_start.elapsed();

    // Warm once (first run pays one-time plan setup).
    let _ = embedder
        .embed("warmup sentence for the embedding probe")
        .expect("warm embed");

    const ITERS: usize = 10;
    let texts: Vec<String> = (0..ITERS)
        .map(|i| format!("memory recall probe sentence number {i} with a few extra tokens"))
        .collect();
    let start = std::time::Instant::now();
    for text in &texts {
        let v = embedder.embed(text).expect("embed");
        assert_eq!(v.len(), 384);
    }
    let per_embed_ms = start.elapsed().as_secs_f64() * 1000.0 / ITERS as f64;

    println!("embed latency probe:");
    println!("  model load: {load:?}");
    println!("  per-embed:  {per_embed_ms:.1} ms (over {ITERS} iters)");

    // Regression guard: with the tract stack pinned to opt-level 3 this
    // measures ~300 ms on this hardware (the model always runs a full
    // 256-token forward pass); the opt-level 0 regression measured ~666 ms.
    // The bound sits between the two so profile regressions fail loudly
    // without flaking on normal variance.
    assert!(
        per_embed_ms < 450.0,
        "embed took {per_embed_ms:.1} ms; tract opt-level regression? \
         (check [profile.*.package.tract-*] pins in the workspace Cargo.toml)"
    );
}
