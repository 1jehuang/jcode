//! Permission-bubble integration for subagent session forking.
//!
//! When creating child/coordinator sessions, applies `jcode-permission-bubble`
//! constraints: byte-identical prefix construction, fork depth guards, and
//! the 10-rule child message template.

use jcode_permission_bubble::{
    ChildTemplate, ForkDepthGuard, ForkDepthResult, MAX_FORK_DEPTH,
    build_child_message, build_forked_messages,
};
use jcode_session_types::StoredMessage;

use crate::logging;

/// Result of applying permission-bubble constraints to a new child session.
pub(crate) struct PermissionBubbleResult {
    /// Messages to use in the child session (forked with prefix integrity).
    pub messages: Vec<StoredMessage>,
    /// Child's initial system prompt (with the 10-rule boilerplate).
    pub system_prompt: String,
    /// Whether the fork was denied due to depth limits.
    pub denied: bool,
}

/// Create permission-bubble-constrained messages for a child session.
///
/// Takes the parent's message history and constructs a fork payload with
/// byte-identical prefix sharing for cache efficiency.
pub(crate) fn create_forked_child_messages(
    parent_messages: &[StoredMessage],
    directive: &str,
    fork_id: &str,
    depth_guard: &mut ForkDepthGuard,
) -> PermissionBubbleResult {
    match depth_guard.can_fork() {
        ForkDepthResult::Denied { current_depth, max_depth } => {
            logging::warn(&format!(
                "FORK_DENIED: depth {} would exceed max {} (chain: {:?})",
                current_depth, max_depth, depth_guard.fork_chain(),
            ));
            return PermissionBubbleResult {
                messages: parent_messages.to_vec(),
                system_prompt: build_child_message(directive),
                denied: true,
            };
        }
        ForkDepthResult::Allowed { .. } => {}
    }

    // Advance the guard to record this fork in the chain.
    let _result = depth_guard.advance(fork_id);

    let messages = parent_messages.to_vec();
    logging::info(&format!(
        "FORK_CREATED: fork_id={} depth={}",
        fork_id,
        depth_guard.depth(),
    ));

    PermissionBubbleResult {
        messages,
        system_prompt: build_child_message(directive),
        denied: false,
    }
}

/// Create a new `ForkDepthGuard` for a root-level session.
pub(crate) fn create_root_fork_guard() -> ForkDepthGuard {
    ForkDepthGuard::root()
}

/// Create a `ForkDepthGuard` with a custom max depth.
pub(crate) fn create_fork_guard(max_depth: usize) -> ForkDepthGuard {
    ForkDepthGuard::with_max_depth(max_depth)
}
