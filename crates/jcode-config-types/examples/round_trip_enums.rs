//! Round-trip the `CompactionMode` and `AgentMode` enums through JSON.
//!
//! These are the two enums the harness reads from runtime config to
//! decide *when* to compact the transcript and *what role* the agent
//! plays. They also serialize to lowercase strings so they can live in
//! the `JCODE_OPENAI_NATIVE_COMPACTION_MODE` and `JCODE_AGENT_MODE`
//! env vars (see `docs/ENVIRONMENT.md`).
//!
//! Run with: `cargo run -p jcode-config-types --example round_trip_enums`

use jcode_config_types::{AgentMode, CompactionMode};

fn main() {
    for mode in [CompactionMode::Reactive, CompactionMode::Proactive] {
        let s = serde_json::to_string(&mode).expect("serialize");
        let back: CompactionMode = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(mode, back);
        println!("CompactionMode::{mode:?} -> {s}");
    }

    println!();

    for mode in [AgentMode::Execute, AgentMode::Manager] {
        let s = serde_json::to_string(&mode).expect("serialize");
        let back: AgentMode = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(mode, back);
        println!("AgentMode::{mode:?} -> {s}");
    }
}
