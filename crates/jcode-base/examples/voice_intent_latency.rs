//! Measure real Jev voice routing latency with a realistic 20-candidate list.
//! Run: cargo run -p jcode-base --example voice_intent_latency
use jcode_base::voice_intent::{SessionCandidate, classify_with_report};
use std::time::Instant;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let candidates: Vec<_> = (0..20)
        .map(|i| SessionCandidate {
            id: format!("session-{i}"),
            title: format!("fox: fix sidebar spacing and selection bug number {i}"),
            working_dir: Some("/home/jeremy/jcode-desktop".into()),
        })
        .collect();
    let client = jcode_base::jev::JevClient::for_voice()?;
    eprintln!(
        "provider={} model={}",
        client.provider_name(),
        client.model_id()
    );
    let rounds: usize = std::env::var("ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    for n in [20usize, 10, 0] {
        let mut times = Vec::new();
        for round in 0..rounds {
            let text = format!("Can you look into why the transcription is slow? ({round})");
            let start = Instant::now();
            let report = classify_with_report(&text, &candidates[..n]).await;
            times.push(start.elapsed().as_millis());
            if let Err(error) = report {
                eprintln!("error: {error:#}");
            }
        }
        times.sort();
        eprintln!("candidates={n:2} sorted ms: {times:?}");
    }
    Ok(())
}
