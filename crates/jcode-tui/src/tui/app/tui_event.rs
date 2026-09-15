//! TUI event types for cross-component communication.
//!
//! [`TuiEvent`] carries typed messages from background tasks (tool calls,
//! MCP servers, background agents) into the main TUI event loop. Each
//! variant bundles its payload with an optional response channel so the
//! caller can await the user's interaction result.

use crate::tui::elicitation_types::{ElicitRequest, ElicitResponse};
use tokio::sync::oneshot;

/// Events dispatched from background tasks into the TUI event loop.
pub enum TuiEvent {
    /// Agent requests elicitation from user.
    ///
    /// The `oneshot::Sender` delivers the user's response back to the
    /// waiting tool call. If the overlay is dismissed without a response
    /// (e.g. timeout or cancel), a default `ElicitResponse` with
    /// `ElicitAction::Cancel` is sent.
    Elicit(ElicitRequest, oneshot::Sender<ElicitResponse>),
}
