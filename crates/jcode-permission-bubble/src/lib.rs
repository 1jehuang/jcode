//! Permission bubble: subagents inherit parent context and bubble permission
//! prompts back to the parent.
//!
//! # Why this exists
//!
//! When jcode spawns a subagent (child), the child needs the parent's conversation
//! context to produce useful work. However, blindly copying messages breaks cache
//! sharing because different conversations have different prefixes.
//!
//! This crate solves the permission-bubble problem:
//!
//! 1. **Fork messages** are constructed with byte-identical prefixes so that the
//!    parent and child share prompt cache hits on the overlapping leading messages.
//! 2. **Child message templates** encode 10 strict rules that constrain what a
//!    child may do, preventing permission escalation and scope creep.
//! 3. **Fork guards** detect recursive fork attempts (child spawning grandchild
//!    spawning great-grandchild) and cut off the chain at a configurable depth.
//!
//! # Design
//!
//! The crate is purely data-oriented: it builds message payloads and checks
//! invariants, but performs no I/O, network calls, or model invocations. The
//! caller (typically `jcode-app-core`'s agent runtime) is responsible for
//! actually sending the constructed messages to a provider.

mod child_template;
mod fork_guard;
mod fork_message;

pub use child_template::{
    build_child_message, ChildMessage, ChildTemplate, FORK_BOILERPLATE_TAG,
    FORK_DIRECTIVE_PREFIX,
};
pub use fork_guard::{ForkDepthGuard, ForkDepthResult, MAX_FORK_DEPTH};
pub use fork_message::{
    build_forked_messages, is_in_fork_child, AssistantMessage, ContentBlock,
    ForkMessage, ForkMessageBuilder, ForkPayload, Message, FORK_PLACEHOLDER_RESULT,
};
