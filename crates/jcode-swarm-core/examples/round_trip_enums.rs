//! Round-trip the swarm role and lifecycle enums through JSON.
//!
//! Demonstrates the wire shape of the two enums that every
//! `jcode_swarm_core` consumer cares about:
//! - `SwarmRole` — Agent / Coordinator / Other(String)
//! - `SwarmLifecycleStatus` — the 10-state lifecycle used by every
//!   swarm member (Spawned, Ready, Running, RunningStale, Completed,
//!   Done, Failed, Stopped, Crashed, Queued)
//!
//! Run with: `cargo run -p jcode-swarm-core --example round_trip_enums`

use jcode_swarm_core::{SwarmLifecycleStatus, SwarmRole};

fn main() {
    for role in [
        SwarmRole::Agent,
        SwarmRole::Coordinator,
        SwarmRole::Other("reviewer".to_string()),
    ] {
        let s = serde_json::to_string(&role).expect("serialize");
        let back: SwarmRole = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(role, back);
        println!("SwarmRole::{role:?} -> {s}");
    }

    println!();

    for status in [
        SwarmLifecycleStatus::Spawned,
        SwarmLifecycleStatus::Ready,
        SwarmLifecycleStatus::Running,
        SwarmLifecycleStatus::Completed,
        SwarmLifecycleStatus::Failed,
        SwarmLifecycleStatus::Stopped,
        SwarmLifecycleStatus::Crashed,
    ] {
        let s = serde_json::to_string(&status).expect("serialize");
        let back: SwarmLifecycleStatus = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(status, back);
        println!("SwarmLifecycleStatus::{status:?} -> {s}");
    }
}
