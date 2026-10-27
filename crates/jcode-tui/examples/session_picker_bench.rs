//! Time the real `/resume` picker list (`load_sessions`) against a private data directory.
//!
//! Run on a copy of a corpus, never the live `~/.jcode`:
//!   JCODE_HOME=<copy> JCODE_EXTERNAL_SESSIONS=0 [JCODE_SESSION_BACKEND=sqlite] \
//!     cargo run --release -p jcode-tui --example session_picker_bench -- [repeats]
//!
//! Prints only aggregate counts and timings. Warm-cache numbers: a new process is not a cold
//! filesystem cache. The first sample includes one-time process setup, later ones bypass the
//! in-memory list cache but not the OS page cache.
use jcode_tui::tui::session_picker::{invalidate_session_list_cache, load_sessions};
use sha2::{Digest, Sha256};
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let repeats: usize = std::env::args()
        .nth(1)
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(10)
        .max(1);
    let private_rows_path = std::env::args().nth(2);
    let backend = std::env::var("JCODE_SESSION_BACKEND").unwrap_or_else(|_| "files".into());
    let mut samples = Vec::with_capacity(repeats);
    let mut rows = 0usize;
    let mut rows_digest = String::new();
    for _ in 0..repeats {
        invalidate_session_list_cache();
        let start = Instant::now();
        let sessions = load_sessions()?;
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
        rows = sessions.len();
        let mut metadata: Vec<_> = sessions.iter().map(|session| serde_json::json!({
            "id": session.id, "parent_id": session.parent_id,
            "title": session.title, "short_name": session.short_name,
            "messages": session.message_count, "users": session.user_message_count,
            "assistants": session.assistant_message_count, "tokens": session.estimated_tokens,
            "created_at": session.created_at, "updated_at": session.last_message_time,
            "active_at": session.last_active_at, "model": session.model,
            "provider": session.provider_key, "saved": session.saved,
            "save_label": session.save_label, "debug": session.is_debug,
            "canary": session.is_canary,
        })).collect();
        metadata.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        if let Some(path) = &private_rows_path {
            jcode_tui::storage::write_json_secret(std::path::Path::new(path), &metadata)?;
        }
        // Publish only a digest, never session identities or transcript content.
        rows_digest = hex::encode(Sha256::digest(serde_json::to_vec(&metadata)?));
    }
    let first = samples[0];
    let mut sorted = samples.clone();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();
    println!(
        "{}",
        serde_json::json!({
            "operation": "picker_list",
            "backend": backend,
            "rows": rows,
            "metadata_sha256": rows_digest,
            "samples": n,
            "first_ms": first,
            "median_ms": sorted[n / 2],
            "p95_ms": sorted[(n * 95).div_ceil(100).saturating_sub(1)],
            "min_ms": sorted[0],
            "max_ms": sorted[n - 1],
        })
    );
    Ok(())
}
