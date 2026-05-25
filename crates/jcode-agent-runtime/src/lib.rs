//! Agent runtime primitives: signals, declarative agent definitions, and
//! tier-based model resolution.
//!
//! This crate intentionally stays small and dependency-light. Heavier
//! engine work (loop, programmatic steps, spawn management) lives in
//! `src/agent.rs` and will migrate here incrementally as Phase 0 → Phase 2
//! land.
//!
//! ## Modules
//!
//! - [`signals`] — soft-interrupt + cancellation primitives shared with
//!   the server runtime.
//! - [`definition`] — declarative `AgentDefinition` schema loaded from
//!   `.jcode/agents/*.toml`.
//! - [`tier`] — user-defined model tier slot resolution (extends
//!   `model_routing.rs` #100).
//! - [`output`] — `OutputMode` enum (last_message / all_messages /
//!   structured_output).
//! - [`reasoning`] — `ReasoningEffort` enum (minimal / low / medium / high).
//!
//! ## Re-exports
//!
//! All previous public types stay re-exported at the crate root so existing
//! consumers (`src/agent.rs`) compile unchanged.

pub mod definition;
pub mod output;
pub mod reasoning;
pub mod registry;
pub mod signals;
pub mod tier;

// Backwards-compatible re-exports for existing consumers. Do not remove
// without auditing `src/agent.rs` and other in-tree users.
pub use signals::{
    BackgroundToolSignal, GracefulShutdownSignal, InterruptSignal, SoftInterruptMessage,
    SoftInterruptQueue, SoftInterruptSource, StreamError,
};

// New public surface (Phase 0).
pub use definition::{AgentDefinition, DefinitionError, DEFAULT_AGENT_VERSION};
pub use output::OutputMode;
pub use reasoning::ReasoningEffort;
pub use registry::{AgentRegistry, AgentSource, LoadError, LoadedAgent, SourceKind};
pub use tier::{resolve_model, resolve_model_with_source, ModelTier, ResolutionSource};
