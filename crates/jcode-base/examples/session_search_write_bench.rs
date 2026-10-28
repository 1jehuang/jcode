//! Measure SQLite commit cost on a disposable copy, never a live session store.
use jcode_base::message::{ContentBlock, Role};
use jcode_base::session::{SessionBackend, SessionStore};
use std::time::Instant;

fn stats(mut values: Vec<f64>) -> serde_json::Value {
    values.sort_by(f64::total_cmp);
    serde_json::json!({"samples":values.len(),"median_ms":values[values.len()/2],
        "min_ms":values[0],"max_ms":values[values.len()-1]})
}

fn main() -> anyhow::Result<()> {
    let root = std::env::var("JCODE_HOME")?;
    let store = SessionStore::open(std::path::Path::new(&root), SessionBackend::Sqlite)?;
    let mut candidates = store.list_search_candidates(usize::MAX)?;
    candidates.sort_by_key(|candidate| candidate.size_bytes);
    anyhow::ensure!(!candidates.is_empty(), "empty benchmark store");
    let count = candidates.len().min(8);
    let mut metadata = Vec::new();
    let mut append = Vec::new();
    for index in 0..count {
        let slot = if count == 1 {
            0
        } else {
            index * (candidates.len() - 1) / (count - 1)
        };
        let mut session = store.load(&candidates[slot].id)?;
        if session.messages.is_empty() {
            continue;
        }
        for iteration in 0..3 {
            session.custom_title = Some(format!("index-write-benchmark-{iteration}"));
            let started = Instant::now();
            store.save(&mut session)?;
            metadata.push(started.elapsed().as_secs_f64() * 1000.0);
            session.add_message(
                Role::User,
                vec![ContentBlock::Text {
                    text: format!("index write benchmark append {iteration}"),
                    cache_control: None,
                }],
            );
            let started = Instant::now();
            store.save(&mut session)?;
            append.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        let loaded = store.load(&session.id)?;
        anyhow::ensure!(
            serde_json::to_value(&loaded)? == serde_json::to_value(&session)?,
            "write read-back mismatch; private values withheld"
        );
    }
    anyhow::ensure!(!metadata.is_empty(), "no nonempty benchmark sessions");
    println!(
        "{}",
        serde_json::json!({"sessions":metadata.len()/3,"metadata":stats(metadata),
                                    "append":stats(append),"read_back_equal":true})
    );
    Ok(())
}
