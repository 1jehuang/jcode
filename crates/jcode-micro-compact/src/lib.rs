//! # jcode-micro-compact
//!
//! Micro-compaction clears old tool results before each API call without
//! performing full compaction (summarization).
//!
//! ## Purpose
//!
//! When the context window is large but not yet at the compaction threshold,
//! old tool results (file reads, bash output, grep results) consume tokens
//! that the assistant is unlikely to reference again. Micro-compaction
//! replaces those results with short placeholder messages, reclaiming tokens
//! without the cost of an LLM summarization call.
//!
//! ## How it works
//!
//! 1. **Token estimation** -- estimate how many tokens each message consumes.
//! 2. **Compactable tool identification** -- find tool results that are safe
//!    to replace (Read, Bash, Grep, etc.).
//! 3. **Time-based trigger** -- skip MC if a fresh cache already exists.
//! 4. **Cached MC path** -- remember which blocks were compacted so the next
//!    turn can skip already-processed content.

pub mod cached_mc;
pub mod compactable_tools;
pub mod time_based;
pub mod token_estimate;

// Re-export primary types for convenience.
pub use cached_mc::{CachedMcConfig, CachedMcPath};
pub use compactable_tools::{COMPACTABLE_TOOLS, get_compactable_tools, is_compactable};
pub use time_based::TimeBasedTrigger;
pub use token_estimate::{estimate_message_tokens, estimate_tokens, rough_token_count};
