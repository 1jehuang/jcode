//! Exact-query search-history cache for the @file mention picker.
//!
//! Stores results only for exact matches (same query string). Used for
//! O(1) backspace recovery; every other case falls through to a full
//! `search_in_index` scan.

use super::types::FileMatch;

pub(super) struct HistoryEntry {
    query: String,
    results: Vec<FileMatch>,
}

/// Exact-query search-history cache.
///
/// Stores results only for exact matches (same query string).  Used for
/// O(1) backspace recovery.  All other cases (prefix extension, partial
/// match) fall through to a full `search_in_index` scan — the engine is
/// fast enough (~0.3 ms for 5000 files) that incremental history filtering
/// is not needed.
///
/// Capacity is capped at `max_entries` (default 20).
pub(super) struct SearchHistory {
    pub(super) entries: Vec<HistoryEntry>,
    pub(super) max_entries: usize,
}

pub(super) enum LookupResult {
    /// Cache hit — return these results immediately.
    Hit(Vec<FileMatch>),
    /// Cache miss — caller must perform a full search.
    Miss,
}

impl SearchHistory {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            max_entries: 20,
        }
    }

    /// Returns `Hit` when `query` matches any cached entry exactly.
    /// All other cases → `Miss` → full search in `search_in_index`.
    pub fn lookup(&mut self, query: &str) -> LookupResult {
        if query.is_empty() {
            self.entries.clear();
            return LookupResult::Miss;
        }
        if let Some(entry) = self.entries.iter().rev().find(|e| e.query == query) {
            return LookupResult::Hit(entry.results.clone());
        }
        LookupResult::Miss
    }

    /// Save a full-search result.  Capacity-controlled.
    pub fn save(&mut self, query: &str, results: &[FileMatch]) {
        self.entries.push(HistoryEntry {
            query: query.to_string(),
            results: results.to_vec(),
        });
        while self.entries.len() > self.max_entries {
            self.entries.remove(0);
        }
    }
}
