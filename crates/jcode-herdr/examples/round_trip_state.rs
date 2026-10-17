//! Round-trip the `AgentState` enum and read the HERDR environment
//! snapshot the jcode pane would use.
//!
//! `jcode_herdr` is the jcode end of the HERDR cross-harness terminal
//! multiplexer. The interesting public surface for newcomers:
//!
//! - `AgentState` — the three-state lifecycle HERDR panes report:
//!   `Working`, `Idle`, `Blocked`
//! - `HerdrEnv` — the parsed view of the `HERDR_*` env vars the
//!   pane sets when it launches jcode
//!
//! Run with: `cargo run -p jcode-herdr --example round_trip_state`
//!
//! The env snapshot is read at construction time; if you want to
//! exercise it, run the example with the relevant vars set, e.g.:
//!
//! ```text
//! HERDR_BIN_PATH=/usr/local/bin/herdr \
//! HERDR_SOCKET_PATH=/tmp/herdr.sock \
//! HERDR_PANE_ID=pane-1 \
//! HERDR_TAB_ID=tab-1 \
//! HERDR_WORKSPACE_ID=ws-1 \
//!   cargo run -p jcode-herdr --example round_trip_state
//! ```

use jcode_herdr::state::AgentState;
use jcode_herdr::HerdrEnv;

fn main() {
    for state in [AgentState::Working, AgentState::Idle, AgentState::Blocked] {
        let s = state.as_str();
        let back = match s {
            "working" => AgentState::Working,
            "idle" => AgentState::Idle,
            "blocked" => AgentState::Blocked,
            other => panic!("unexpected AgentState str: {other}"),
        };
        assert_eq!(state, back);
        println!("AgentState::{state:?} -> {s:?}");
    }

    println!();

    let env = HerdrEnv::capture();
    println!("HerdrEnv::capture():");
    println!("  is_valid:        {}", env.is_valid());
    println!("  pane_id:         {:?}", env.pane_id());
    println!("  socket_path:     {:?}", env.socket_path());
    println!("  pane_id_or_empty:{:?}", env.pane_id_or_empty());
}
