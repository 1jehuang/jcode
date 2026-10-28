//! Prepare optional FTS indexes offline, with no active session writers.
use jcode_base::session::{SessionBackend, SessionSearchIndexMode, SessionStore};
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let root = std::env::var("JCODE_HOME")?;
    let store = SessionStore::open(std::path::Path::new(&root), SessionBackend::Sqlite)?;
    let started = Instant::now();
    let mode = match std::env::args().nth(1).as_deref() {
        None | Some("positional") => SessionSearchIndexMode::Positional,
        Some("compact") => SessionSearchIndexMode::Compact,
        Some("column") => SessionSearchIndexMode::Column,
        _ => anyhow::bail!("expected compact, column or positional"),
    };
    let prepared = store.prepare_search_index_mode(mode)?;
    println!(
        "{}",
        serde_json::json!({"prepared": prepared, "elapsed_ms": started.elapsed().as_secs_f64() * 1000.0})
    );
    Ok(())
}
