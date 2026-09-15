//! HERDR terminal runtime integration for jcode and ForgeCode.
//!
//! Provides lifecycle state reporting (working/idle/blocked) to HERDR's
//! Unix socket API, session identity for restore, and screen manifest
//! generation for agent detection.
//!
//! # Architecture
//!
//! When `HERDR_ENV=1`, the [`HerdrReporter`] connects to HERDR's Unix
//! socket and reports agent state transitions via `pane.report_agent`.
//! A monotonic sequence number ensures stale reports from predecessor
//! sessions are silently dropped by HERDR.
//!
//! When HERDR is not detected, all operations are zero-cost no-ops.

pub mod env;
pub mod manifest;
pub mod reporter;
pub mod socket;
pub mod state;

pub use env::HerdrEnv;
pub use reporter::HerdrReporter;
pub use state::AgentState;
