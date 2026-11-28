//! Unified public API wiring indexing, search, history, and lazy
//! ignored-directory scanning together.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::tui::app::char_bag::CharBag;

use super::history::{LookupResult, SearchHistory};
use super::index::FileIndexManager;
use super::search::{SKIP_DIRS, SearchLimits, refresh_ttl, search_in_index};
use super::types::{FileEntry, FileMatch, Frecency, PathIndex, TEXT_EXTENSIONS};

/// Top-level cache that wires together indexing, search, history, and lazy
/// ignored-directory scanning.
///
/// Supports multiple workspace directories (P3-18). The first manager (index 0)
/// is the primary workspace. Search results from secondary workspaces are
/// prefixed with the workspace name.
pub(in crate::tui::app) struct FileMentionCache {
    /// Per-workspace index managers. Index 0 is the primary (cwd).
    pub(super) managers: Vec<FileIndexManager>,
    /// Workspace display names (last path component) for result prefixing.
    workspace_names: Vec<String>,
    history: SearchHistory,
    /// Frecency tracking (frequency + recency decay) with on-disk persistence.
    /// Replaces the old fixed-cap `recent_files` list with opencode-style scoring.
    frecency: Frecency,
}

impl FileMentionCache {
    pub fn new() -> Self {
        Self::with_cwd(Path::new("."))
    }

    /// Create with an explicit cwd for frecency initialization.
    pub fn with_cwd(cwd: &Path) -> Self {
        Self {
            managers: vec![FileIndexManager::new(PathBuf::new())],
            workspace_names: vec![String::new()],
            history: SearchHistory::new(),
            frecency: Frecency::new(cwd),
        }
    }

    /// Test-only constructor binding the primary manager to `cwd` with a
    /// fixed index: `with_cwd` only feeds the frecency log, so a cache built
    /// that way still recreates its manager on the first `check_refresh`
    /// cwd comparison. Tests need the manager bound so the seeded index
    /// survives that comparison instead of being discarded for a rebuild.
    #[cfg(test)]
    pub fn with_cwd_and_test_index(cwd: &Path, paths: &[&str]) -> Self {
        let manager = FileIndexManager::new_headless_with_index(
            cwd.to_path_buf(),
            PathIndex::test_index(cwd.to_path_buf(), paths),
        );
        Self {
            managers: vec![manager],
            workspace_names: vec![String::new()],
            history: SearchHistory::new(),
            frecency: Frecency::new(cwd),
        }
    }

    /// Add an additional workspace directory to search.
    #[allow(dead_code)]
    pub fn add_workspace(&mut self, path: &Path) {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(".")
            .to_string();
        self.managers
            .push(FileIndexManager::new(path.to_path_buf()));
        self.workspace_names.push(name);
    }

    /// Main entry-point: return candidate files for `query`.
    ///
    /// Must return in < 5 ms on the synchronous UI thread.
    /// Filesystem completion for `@~` (from `$HOME`) and `@/` (from `/`) queries.
    ///
    /// `query` is the `@` token without the leading `@`. Everything up to the
    /// final component names a directory to list; the final component (when
    /// the query has no trailing `/`) is a prefix filter over entry names.
    /// Returned paths are display strings (`~/...` or absolute) suitable for
    /// chips; `~` is expanded later, at prompt-build time.
    pub(super) fn filesystem_candidates(query: &str, limits: &SearchLimits) -> Vec<FileMatch> {
        let (prefix, base_dir, rest) = if let Some(rest) = query.strip_prefix('~') {
            let Some(home) = std::env::var_os("HOME") else {
                return Vec::new();
            };
            (
                "~/",
                std::path::PathBuf::from(home),
                rest.strip_prefix('/').unwrap_or(rest),
            )
        } else {
            (
                "/",
                std::path::PathBuf::from("/"),
                query.strip_prefix('/').unwrap_or(""),
            )
        };
        let (sub, partial) = match rest.rfind('/') {
            Some(idx) => (&rest[..idx], &rest[idx + 1..]),
            None => ("", rest),
        };
        let mut dir = base_dir;
        for segment in sub.split('/').filter(|segment| !segment.is_empty()) {
            dir.push(segment);
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        // Collect every matching entry name first: truncating before the
        // sort would keep an arbitrary readdir-order subset instead of the
        // lexically first results. Name filtering is a cheap string check
        // and runs before the `stat` so a non-matching prefix never stats
        // the whole directory.
        let mut names: Vec<(String, bool)> = Vec::new();
        for entry in entries {
            let Ok(entry) = entry else {
                continue;
            };
            let os_name = entry.file_name();
            let file_name = os_name.to_string_lossy();
            let name: &str = file_name.as_ref();
            if !partial.is_empty() && !name.starts_with(partial) {
                continue;
            }
            if name.starts_with('.') && !partial.starts_with('.') {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            // Only regular files and directories: FIFOs and device nodes
            // would block the later content read (greptile A1).
            if !file_type.is_file() && !file_type.is_dir() {
                continue;
            }
            let is_directory = file_type.is_dir();
            if is_directory && SKIP_DIRS.contains(&name) {
                continue;
            }
            names.push((name.to_string(), is_directory));
        }
        names.sort();
        names.truncate(limits.max_results);

        let mut matches: Vec<FileMatch> = Vec::with_capacity(names.len());
        for (name, is_directory) in names {
            let display = if sub.is_empty() {
                format!("{prefix}{name}")
            } else {
                format!("{prefix}{sub}/{name}")
            };
            let is_likely_binary = if is_directory {
                false
            } else {
                match std::path::Path::new(&name)
                    .extension()
                    .and_then(|ext| ext.to_str())
                {
                    Some(ext) => !TEXT_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()),
                    None => false,
                }
            };
            matches.push(FileMatch {
                score: 0.0,
                path: display.into(),
                is_directory,
                is_recent: false,
                is_likely_binary,
            });
        }
        matches.sort_by(|a, b| a.path.cmp(&b.path));
        matches
    }

    pub fn candidates(&mut self, query: &str) -> Vec<FileMatch> {
        let limits = SearchLimits::from_config();
        if query.starts_with('~') || query.starts_with('/') {
            return Self::filesystem_candidates(query, &limits);
        }

        // 1. Check search history.
        match self.history.lookup(query) {
            LookupResult::Hit(results) => return results,
            LookupResult::Miss => {}
        }

        let mut all_results: Vec<FileMatch> = Vec::new();
        let mut searched_any_index = false;

        for (wi, manager) in self.managers.iter().enumerate() {
            let Some(mut results) = manager.with_index(|index| {
                if !index.built {
                    None
                } else {
                    Some(search_in_index(query, index, &self.frecency, limits))
                }
            }) else {
                manager.refresh_async();
                continue;
            };
            searched_any_index = true;

            let q = query.trim_start_matches('/');
            if !q.is_empty()
                && results.len() < limits.max_results
                && manager.ensure_ignored_dir_scanned(q)
            {
                results = manager
                    .with_index(|index| search_in_index(query, index, &self.frecency, limits));
            }

            // Prefix paths from secondary workspaces.
            if wi > 0 {
                let prefix = &self.workspace_names[wi];
                all_results.extend(results.into_iter().map(|mut m| {
                    m.path = Arc::from(format!("{}/{}", prefix, m.path).as_str());
                    m
                }));
            } else {
                all_results.extend(results);
            }
        }

        // Sort merged results by score, truncate.
        all_results.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.path.cmp(&b.path))
        });
        all_results.truncate(limits.max_results);

        // Persist only real searches.  The first call often sees an empty index
        // while the async builder is starting; caching that empty list would
        // poison subsequent lookups after the index finishes.
        if searched_any_index {
            self.history.save(query, &all_results);
        }
        all_results
    }

    /// Record a file open so it ranks higher in future searches.
    pub fn record_file_open(&mut self, path: Arc<str>) {
        self.frecency.record(path.as_ref());
        self.history = SearchHistory::new();
    }

    /// Ensure the index is still valid for the current working directory.
    pub fn check_refresh(&mut self, cwd: &Path) {
        // Primary workspace: recreate if cwd changed.
        if self.managers[0].cwd() != cwd {
            self.managers[0] = FileIndexManager::new(cwd.to_path_buf());
            self.workspace_names[0] = cwd
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(".")
                .to_string();
            self.history = SearchHistory::new();
            // Frecency is keyed per project: switch to the new project's log.
            self.frecency = Frecency::new(cwd);
        }

        for manager in &self.managers {
            let index = manager.snapshot();
            let dirty = manager.dirty.swap(false, Ordering::Acquire);
            let refresh_ttl_secs = crate::config::config().file_mention.refresh_ttl_secs;
            let ttl = refresh_ttl(index.entries.len(), refresh_ttl_secs);
            let ttl_expired = index.built_at.elapsed() > ttl;
            let needs_refresh = !index.built || dirty || ttl_expired;

            if dirty || ttl_expired {
                self.history = SearchHistory::new();
            }

            if needs_refresh && !manager.is_refreshing() {
                manager.refresh_async();
            }
        }
    }

    pub fn is_initial_building(&self) -> bool {
        let any_ready = self.managers.iter().any(|manager| manager.snapshot().built);
        !any_ready && self.managers.iter().any(FileIndexManager::is_refreshing)
    }

    /// Files scanned so far by the in-flight initial build, summed across
    /// workspaces. Only meaningful while `is_initial_building()` is true.
    pub fn indexed_so_far(&self) -> usize {
        self.managers
            .iter()
            .filter(|manager| manager.is_refreshing())
            .map(FileIndexManager::scanned_files)
            .sum()
    }
}

/// Check whether `query` targets an ignored directory and, if so, populate
/// `index.lazy_entries` from `fs::read_dir`.
pub(super) fn ensure_ignored_dir_scanned(query: &str, index: &mut PathIndex) {
    // Walk the query's directory prefixes, deepest first.
    let mut candidate = query.to_string();
    while let Some(slash_pos) = candidate.rfind('/') {
        candidate.truncate(slash_pos);
        try_scan_ignored_dir(&candidate, index);
    }
    // Also check the leaf (e.g. "ai-memory" from "ai-memory/de").
    try_scan_ignored_dir(query, index);
}

fn try_scan_ignored_dir(dir_path: &str, index: &mut PathIndex) {
    let dir_key: Arc<str> = Arc::from(dir_path);

    // Already scanned or is in the base index (i.e. not ignored).
    if index.scanned_ignored_dirs.contains(&dir_key) {
        return;
    }
    let dir_prefix = format!("{}/", dir_path.trim_end_matches('/'));
    if index
        .entries
        .iter()
        .any(|e| e.path.as_ref() == dir_path || e.path.starts_with(&dir_prefix))
    {
        return;
    }

    let abs_dir = index.root.join(dir_path);
    if !abs_dir.is_dir() {
        // Prefix fallback: when the query is a root-level name and the
        // exact directory does not exist, scan the workspace root for
        // directory names that start with `dir_path` (e.g. "ai" →
        // "ai-memory/").  This is how users discover gitignored
        // directories without typing their full name.
        if !dir_path.contains('/')
            && let Ok(rd) = std::fs::read_dir(&index.root)
        {
            for entry in rd.filter_map(|e| e.ok()) {
                if entry.file_type().is_ok_and(|ft| ft.is_dir()) {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if name_str.starts_with(dir_path) && !name_str.starts_with('.') {
                        // Recursively scan the matched directory.
                        try_scan_ignored_dir(&name_str, index);
                    }
                }
            }
        }
        return;
    }

    let mut new_files = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&abs_dir) {
        for entry in rd.filter_map(|e| e.ok()) {
            // Skip symlinks (design §5.2.8).
            if entry.file_type().is_ok_and(|ft| ft.is_symlink()) {
                continue;
            }
            // Only regular files and directories: FIFOs and device nodes
            // would block the later content read (greptile A1).
            if !entry
                .file_type()
                .is_ok_and(|ft| ft.is_file() || ft.is_dir())
            {
                continue;
            }

            let abs_path = entry.path();
            if let Ok(rel) = abs_path.strip_prefix(&index.root)
                && let Some(rel_str) = rel.to_str()
                && !rel_str.starts_with(".git/")
            {
                let filename: Arc<str> = abs_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(rel_str)
                    .into();

                new_files.push(FileEntry {
                    path: Arc::from(rel_str),
                    lower_path: Arc::from(rel_str.to_lowercase()),
                    lower_filename: Arc::from(filename.to_lowercase()),
                    is_directory: abs_path.is_dir(),
                    is_likely_binary: abs_path
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|ext| !TEXT_EXTENSIONS.contains(&ext))
                        .unwrap_or(!abs_path.is_dir()),
                    char_bag: CharBag::from(rel_str),
                });
            }
        }
    }

    index.lazy_entries.extend(new_files);
    index.scanned_ignored_dirs.insert(dir_key);
}
