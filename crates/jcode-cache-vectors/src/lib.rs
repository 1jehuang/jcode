//! # jcode-cache-vectors
//!
//! Prompt-cache vector hashing and cache-break detection for Jcode.
//!
//! Prompt caching (e.g. Anthropic's `cache_control`) keeps the provider-side
//! cache alive across turns. The cache breaks whenever any of the **14 cache
//! vectors** change between requests:
//!
//! 1. System prompt hash
//! 2. Tool schemas hash (overall)
//! 3. Per-tool hashes (one per tool)
//! 4. Model name
//! 5. `fast_mode` flag
//! 6. `global_cache_strategy`
//! 7. `betas` set
//! 8. …and diffable content used for debugging
//!
//! This crate provides:
//!
//! - **`cache_hash`** – djb2-based hash computation for each vector
//! - **`prompt_state`** – `PromptStateSnapshot` capturing all 14 vectors at a
//!   point in time
//! - **`cache_break`** – detection of which vectors changed between snapshots
//! - **`diffable_content`** – human-readable diff payloads for debugging cache
//!   breaks

pub mod cache_break;
pub mod cache_hash;
pub mod diffable_content;
pub mod prompt_state;

pub use cache_break::{CacheBreak, CacheBreakKind};
pub use cache_hash::{compute_hash, djb2_hash, hash_bytes, hash_str, VectorHash};
pub use diffable_content::DiffableContent;
pub use prompt_state::{snapshot_current_state, PromptStateSnapshot};
