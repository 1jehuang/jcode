#![allow(
    unknown_lints,
    clippy::collapsible_match,
    clippy::manual_checked_ops,
    clippy::unnecessary_sort_by,
    clippy::useless_conversion
)]

//! Root `jcode` crate: the entrypoint + cli layer on top of the `jcode-tui`
//! presentation crate (which in turn re-exports `jcode-app-core` and
//! `jcode-base`).
//!
//! The presentation modules (`tui`, `video_export`) live in `jcode-tui` and the
//! non-presentation modules live in `jcode-app-core`; both are re-exported here
//! via `pub use jcode_tui::*`, so existing `crate::<module>` paths (e.g.
//! `crate::config`, `crate::server`, `crate::tui`) keep resolving unchanged
//! across the cli code that was not moved.

// Re-export the presentation layer (and, transitively, the application core)
// so `crate::tui`, `crate::video_export`, and `crate::<app-core module>` paths
// resolve.
pub use jcode_tui::*;

// Cli + entrypoint layer (kept in the root crate).
pub mod cli;
pub mod crash_log;
pub mod customization;
pub mod extension_policy;
pub mod floating_diagram;
pub mod model_failover;
pub mod model_routing;
pub mod orchestration_api;
pub mod prefix_cache_stable;
pub mod skill_disable;
pub mod skill_distillation;
pub mod theme;
pub mod turborag;

use anyhow::Result;

pub async fn run() -> Result<()> {
    cli::startup::run().await
}
