#![allow(
    unknown_lints,
    clippy::collapsible_match,
    clippy::manual_checked_ops,
    clippy::unnecessary_sort_by,
    clippy::useless_conversion
)]

//! Application core for jcode (upper layer).
//!
//! This crate holds the server/tool/agent layer and its presentation-adjacent
//! leaves. The foundational layer it builds on (provider, auth, config, session,
//! message, memory, telemetry, ...) lives in the `jcode-base` crate and is
//! re-exported here via `pub use jcode_base::*`, so every existing
//! `crate::<module>` path (e.g. `crate::config`, `crate::provider`) keeps
//! resolving unchanged across this crate and the root `jcode` crate, which in
//! turn re-exports this crate via `pub use jcode_app_core::*`.

// Foundational layer: re-export every `jcode-base` module so `crate::<module>`
// paths resolve here exactly as they did before the split.
pub use jcode_base::*;

// Upper layer (server / tool / agent and supporting leaves).
pub mod agent;
pub mod ambient;
pub mod ambient_runner;
pub mod ambient_scheduler;
pub mod build;
pub mod catchup;
pub mod channel;
pub mod external_auth;
pub mod mission;
pub mod network_retry;
pub mod notifications;
pub mod overnight;
pub mod perf;
pub mod replay;
pub mod restart_snapshot;
pub mod server;
pub mod server_spawn;
pub mod session_launch;
pub mod session_rebuild;
pub mod setup_hints;
pub mod ssh_remote;
pub mod startup_profile;
pub mod tool;
pub mod update;

use std::sync::Mutex;

static CURRENT_SESSION_ID: Mutex<Option<String>> = Mutex::new(None);

pub fn set_current_session(session_id: &str) {
    if let Ok(mut guard) = CURRENT_SESSION_ID.lock() {
        *guard = Some(session_id.to_string());
    }
}

pub fn get_current_session() -> Option<String> {
    CURRENT_SESSION_ID.lock().ok()?.clone()
}
