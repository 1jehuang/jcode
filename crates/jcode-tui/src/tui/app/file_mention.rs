//! File mention (at-file) search and caching for jcode's `@file` completion.
//!
//! This module provides:
//! - `PathIndex` – an in-memory snapshot of the workspace file tree.
//! - `FileIndexManager` – async background refresh with RCU-style atomic swap.
//! - `SearchHistory` – incremental search cache that makes backspace O(1).
//! - `FileMentionCache` – unified public API used by the input UI.

mod cache;
mod history;
mod index;
mod prompt;
mod search;
mod types;

pub(super) use cache::FileMentionCache;
pub(super) use prompt::build_prompt_with_files;

#[cfg(test)]
#[path = "file_mention_tests.rs"]
mod tests;
