//! Search engine for the @file mention picker: tiered matching, fuzzy
//! fallback, regex/glob handling, and the background index builder.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use regex::{Regex, RegexBuilder};

use crate::tui::app::char_bag::CharBag;

use super::types::{FileEntry, FileMatch, Frecency, PathIndex, TEXT_EXTENSIONS};

/// Effective per-query limits for the mention picker, derived from
/// [`FileMentionSettings`] with zero values normalized to built-in defaults.
#[derive(Clone, Copy, Debug)]
pub(super) struct SearchLimits {
    pub(super) max_results: usize,
}

impl SearchLimits {
    /// Read the limits from the cached global config.
    ///
    /// Uses `config()` (the cached accessor) because this runs on the sync UI
    /// thread while the @file popover is open; `Config::load()` would re-read
    /// and re-parse config.toml on every call.
    pub(super) fn from_config() -> Self {
        Self::from_settings(crate::config::config().file_mention.clone())
    }

    pub(super) fn from_settings(settings: crate::config::FileMentionConfig) -> Self {
        let max_results = if settings.max_results == 0 {
            crate::config::DEFAULT_FILE_MENTION_MAX_RESULTS
        } else {
            settings.max_results
        };
        Self { max_results }
    }
}

/// Check if `query` matches a directory path at a path-segment boundary.
///
/// Matches when the last component of `dir_path` begins with `query`,
/// or when `query/` appears as an exact prefix somewhere in `dir_path`.
/// Examples for query "src":
///   "src/"              → last component "src" starts_with "src" → true
///   "crates/jcode-tui/src/" → contains "/src/" → true
///   "scripts/"          → last component is "scripts", not "src" → false
pub(super) fn dir_to_query_match(dir_path: &str, query: &str) -> bool {
    // Direct match: dir starts with query (e.g. "src/" for query "src")
    if dir_path.starts_with(query) {
        return true;
    }
    // Segment match: "/query/" appears in path (e.g. "/src/" in "crates/.../src/")
    let segment = format!("/{}/", query);
    if dir_path.contains(&segment) {
        return true;
    }
    // Trailing segment: path ends with "/query/" (already covered by contains)
    // Also: last component of dir starts with query
    if let Some(last_slash) = dir_path.trim_end_matches('/').rfind('/') {
        let last = &dir_path[last_slash + 1..];
        if last.starts_with(query) {
            return true;
        }
    }
    false
}

/// Tiered matching: try the fastest strategies first, only falling back to
/// the expensive DP fuzzy matcher when nothing else matches.
///
/// Tier bands (descending): exact filename 109 > filename prefix ~100 >
/// path prefix 85 > segment prefix <=80 > filename substring <=70 >
/// path substring <=48 > fuzzy (20..=40). Every tier stays below the one
/// above it even at its best position factor, so a weak fuzzy match can
/// never outrank a real substring hit. 109 keeps directory entries (110)
/// above every file match as documented in `search_in_index`.
pub(super) fn match_entry(entry: &FileEntry, query_lower: &str, slash_query: &str) -> f64 {
    let filename = entry.lower_filename.as_ref();
    let path = entry.lower_path.as_ref();

    // L0 – exact filename match. Must outrank any prefix tier: without it,
    // "lib.rs.old" (longer filename, also a prefix hit) scored above the
    // file the query names exactly.
    if filename == query_lower {
        return 109.0;
    }

    // L1 – filename prefix  (~30 ns)  ──────────────────────────────
    // Shorter filenames rank first: they sit closest to the query, while
    // sqrt(len) used to reward longer names that merely start with it.
    if filename.starts_with(query_lower) {
        return 100.0 + (20.0 - filename.len() as f64).max(0.0) * 0.25;
    }

    // L2 – full-path prefix  (~50 ns) ──────────────────────────────
    if path.starts_with(query_lower) {
        return 85.0;
    }

    // L2b – path-segment prefix (query appears after / in path).
    // slash_query is precomputed as "/{query_lower}" once in the caller.
    if let Some(pos) = path.find(slash_query) {
        return 80.0 * (1.0 - (pos as f64 / path.len().max(1) as f64));
    }

    // L3 – filename substring  (~80 ns) ─────────────────────────────
    // A word-boundary hit (after / _ - .) is what the user most likely
    // means, so it gets a flat bonus over mid-word hits.
    if let Some(pos) = filename.find(query_lower) {
        let boundary =
            if pos > 0 && matches!(filename.as_bytes()[pos - 1], b'/' | b'_' | b'-' | b'.') {
                5.0
            } else {
                0.0
            };
        return 65.0 * (1.0 - (pos as f64 / filename.len().max(1) as f64)) + boundary;
    }

    // L4 – full-path substring  (~100 ns) ───────────────────────────
    // Lower-priority catch-all. L2b above already gives higher scores
    // to segment-boundary matches like "/src".
    if let Some(pos) = path.find(query_lower) {
        return 45.0 * (1.0 - (pos as f64 / path.len().max(1) as f64));
    }

    // L5 – DP fuzzy subsequence  (~500 ns) ─────────────────────────
    // Uses jcode-fuzzy which already treats `/`, `-`, `_`, `.`, `:`
    // as boundary characters, giving path-like queries a natural boost.
    //
    // Skip fuzzy when query contains '/' — for structured path queries
    // like "src/lib.rs", a weak fuzzy match against "src/cli/debug.rs"
    // produces noise.  L1-L4 above are the correct minimum bar for
    // path-like input.
    //
    // The raw DP score is unbounded (it grows with query length), so it
    // is normalized against the perfect-match ceiling before use.
    // Without that, a scattered "lib" inside "olive_branch.rs" scored
    // higher than the real substring hit in "lib.rs".
    if !query_lower.contains('/') {
        let filename_score = jcode_fuzzy::fuzzy_score(query_lower, filename).unwrap_or(0) as f64;
        let path_score = jcode_fuzzy::fuzzy_score(query_lower, path).unwrap_or(0) as f64;

        let best = filename_score.max(path_score);
        if best > 0.0 {
            // Perfect-match ceiling: jcode-fuzzy awards MATCH (16) per
            // character plus CONSECUTIVE (8) per follow-on, i.e. 24*len
            // for a tight run, plus boundary bonuses. Clamp absorbs those.
            let ceiling = (query_lower.len() as f64 * 24.0).max(1.0);
            let normalized = (best / ceiling).clamp(0.0, 1.0);
            return 20.0 + 20.0 * normalized;
        }
    }

    0.0
}

fn query_looks_like_regex(query: &str) -> bool {
    query.chars().any(|c| {
        matches!(
            c,
            '*' | '+' | '?' | '[' | ']' | '(' | ')' | '{' | '}' | '|' | '^' | '$' | '\\'
        )
    })
}

fn compile_path_regex(query: &str) -> Option<Regex> {
    if !query_looks_like_regex(query) {
        return None;
    }
    RegexBuilder::new(query)
        .case_insensitive(true)
        .build()
        .ok()
        .or_else(|| compile_glob_like_path_pattern(query))
}

fn compile_glob_like_path_pattern(query: &str) -> Option<Regex> {
    if !query.contains(['*', '?']) {
        return None;
    }
    let mut pattern = String::with_capacity(query.len() * 2);
    for ch in query.chars() {
        match ch {
            '*' => pattern.push_str(".*"),
            '?' => pattern.push('.'),
            _ => pattern.push_str(&regex::escape(&ch.to_string())),
        }
    }
    RegexBuilder::new(&pattern)
        .case_insensitive(true)
        .build()
        .ok()
}

fn regex_match_entry(entry: &FileEntry, regex: &Regex) -> f64 {
    let path = entry.path.as_ref();
    let filename = Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path);

    let filename_score = regex.find(filename).map(|m| {
        if m.start() == 0 {
            95.0
        } else {
            72.0 * (1.0 - (m.start() as f64 / filename.len().max(1) as f64))
        }
    });

    let path_score = regex.find(path).map(|m| {
        if m.start() == 0 {
            88.0
        } else {
            68.0 * (1.0 - (m.start() as f64 / path.len().max(1) as f64))
        }
    });

    filename_score
        .into_iter()
        .chain(path_score)
        .fold(0.0, f64::max)
}

/// Stay below one score point to preserve match-tier and directory boundaries.
fn frecency_bonus(score: f64) -> f64 {
    let score = score.max(0.0);
    0.9 * score / (1.0 + score)
}

/// Show recent files + root-level files when the query is empty.
pub(super) fn show_all_files(
    index: &PathIndex,
    frecency: &Frecency,
    max_results: usize,
) -> Vec<FileMatch> {
    let now = Frecency::now_secs();
    let mut results: Vec<FileMatch> = frecency
        .recent_paths()
        .iter()
        .filter_map(|path| {
            let idx = index.path_to_index.get(path.as_str())?;
            let entry = &index.entries[*idx];
            Some(FileMatch {
                score: 100.0 * (1.0 + frecency.score_at(path, now)),
                path: entry.path.clone(),
                is_directory: false,
                is_recent: true,
                is_likely_binary: entry.is_likely_binary,
            })
        })
        .collect();
    let mut seen_paths: HashSet<Arc<str>> = results.iter().map(|r| r.path.clone()).collect();

    // Root-level entries: directories first, then visible files, skip hidden.
    // Shows recent files + the top-level directory structure rather than
    // every loose file.
    let mut root_dirs: Vec<FileMatch> = Vec::new();
    let mut root_files: Vec<FileMatch> = Vec::new();
    for entry in index.entries.iter().chain(index.lazy_entries.iter()) {
        let is_root_level = !entry.path.contains('/') && !entry.path.contains('\\');
        if !is_root_level || !seen_paths.insert(entry.path.clone()) {
            continue;
        }
        if entry.is_directory {
            root_dirs.push(FileMatch {
                score: 30.0,
                path: entry.path.clone(),
                is_directory: true,
                is_recent: false,
                is_likely_binary: false,
            });
        } else if !entry.path.starts_with(".git/") && entry.path.as_ref() != ".git" {
            // Skip only .git contents, keep other dotfiles (.gitignore, .env, etc.).
            root_files.push(FileMatch {
                score: 0.0,
                path: entry.path.clone(),
                is_directory: false,
                is_recent: false,
                is_likely_binary: entry.is_likely_binary,
            });
        }
    }
    results.extend(root_dirs);
    results.extend(root_files);
    results.truncate(max_results);
    results
}

/// Core file-search function (synchronous hot path; must return < 5 ms).
///
/// Merges both `entries` (git ls-files) and `lazy_entries` (on-demand
/// ignored-directory scan) so that `@ai-memory/` finds gitignored files.
pub(super) fn search_in_index(
    query: &str,
    index: &PathIndex,
    frecency: &Frecency,
    limits: SearchLimits,
) -> Vec<FileMatch> {
    if query.is_empty() {
        return show_all_files(index, frecency, limits.max_results);
    }

    // Normalize query: strip leading "/" so "@src" and "@/src" behave
    // identically (both compare against relative paths).
    let query_raw = query.trim_start_matches('/');
    if query_raw.is_empty() {
        return show_all_files(index, frecency, limits.max_results);
    }
    let query_lower = query_raw.to_lowercase();
    let slash_query = format!("/{}", query_lower);
    let query_bag = CharBag::from(&query_lower);
    let query_regex = compile_path_regex(query_raw);
    let now = Frecency::now_secs();
    let mut results: Vec<FileMatch> = Vec::with_capacity(64);

    // Chain base + lazy entries (two-layer index, see PathIndex docs).
    for entry in index.entries.iter().chain(index.lazy_entries.iter()) {
        if let Some(regex) = &query_regex {
            let score = regex_match_entry(entry, regex);
            if score > 0.0 {
                let frecency_score = frecency.score_at(entry.path.as_ref(), now);
                let is_recent = frecency_score > 0.0;
                results.push(FileMatch {
                    score: score + frecency_bonus(frecency_score),
                    path: entry.path.clone(),
                    is_directory: entry.is_directory,
                    is_recent,
                    is_likely_binary: entry.is_likely_binary,
                });
                continue;
            }
        }

        // CharBag pre-filter: O(1), eliminates 60-80% of candidates.
        if !entry.char_bag.is_superset(query_bag) {
            continue;
        }

        let score = match_entry(entry, &query_lower, &slash_query);
        if score > 0.0 {
            let frecency_score = frecency.score_at(entry.path.as_ref(), now);
            let is_recent = frecency_score > 0.0;
            results.push(FileMatch {
                score: score + frecency_bonus(frecency_score),
                path: entry.path.clone(),
                is_directory: entry.is_directory,
                is_recent,
                is_likely_binary: entry.is_likely_binary,
            });
        }
    }

    // Build directory entries from matching ancestors of file results.
    // Score 110 ensures directories rank above all file matches.
    if !query.is_empty() {
        let mut dirs = HashSet::new();
        for r in &results {
            let mut remaining = r.path.as_ref();
            while let Some(slash) = remaining.rfind('/') {
                let dir = &r.path[..slash + 1];
                if dir_to_query_match(dir, &query_lower) {
                    dirs.insert(Arc::from(dir));
                }
                remaining = &r.path[..slash];
            }
        }
        for dir in dirs {
            results.push(FileMatch {
                score: 110.0,
                path: dir,
                is_directory: true,
                is_recent: false,
                is_likely_binary: false,
            });
        }
    }

    // Sort: score descending, then shorter paths first, then path ascending.
    results.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.path.len().cmp(&b.path.len()))
            .then_with(|| a.path.cmp(&b.path))
    });

    // Ensure a mix of directories and files.  Directories are useful for
    // navigation, but files are what the user ultimately wants.  Keep at most
    // 4 directory entries so files from deep inside the target directory
    // still appear even when many ancestor directories match.
    let mut final_results = Vec::with_capacity(limits.max_results.min(64));
    let mut dirs_seen = 0usize;
    const MAX_DIRS: usize = 4;
    for r in results {
        if r.is_directory {
            if dirs_seen >= MAX_DIRS {
                continue;
            }
            dirs_seen += 1;
        }
        final_results.push(r);
        if final_results.len() >= limits.max_results {
            break;
        }
    }
    final_results
}

/// Maximum files collected into the index (safety cap).
/// Built-in cap on indexed files per workspace. The effective cap comes from
/// `file_mention.max_files` in config.toml; zero falls back to this default.
const DEFAULT_MAX_FILES: usize = 5_000;

/// Resolve the effective index-file cap from the cached global config.
fn effective_max_files() -> usize {
    let configured = crate::config::config().file_mention.max_files;
    if configured == 0 {
        DEFAULT_MAX_FILES
    } else {
        configured
    }
}

/// Background index refresh interval.
///
/// Adapts to workspace size so larger trees (which cost more to rebuild) are
/// re-scanned less frequently. Small workspaces refresh at the configured base
/// interval; once the index passes a few thousand entries the interval stretches
/// to 4x that base interval.
/// The file watcher's `dirty` flag still triggers an immediate rebuild on any
/// detected change, so the TTL is only a fallback for missed watch events.
pub(super) const REFRESH_TTL_LARGE_THRESHOLD: usize = 2_000;

/// Return the refresh TTL appropriate for an index of `entry_count` files.
pub(super) fn refresh_ttl(entry_count: usize, base_secs: u64) -> Duration {
    let base_secs = if base_secs == 0 {
        crate::config::DEFAULT_FILE_MENTION_REFRESH_TTL_SECS
    } else {
        base_secs
    };
    if entry_count >= REFRESH_TTL_LARGE_THRESHOLD {
        Duration::from_secs(base_secs.saturating_mul(4))
    } else {
        Duration::from_secs(base_secs)
    }
}

/// Directories to skip during walkdir fallback. Unrelated to .gitignore;
/// purely avoids indexing huge dependency / build directories.
pub(super) const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    "__pycache__",
    "vendor",
    ".venv",
    "venv",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    "bower_components",
    ".next",
    ".nuxt",
    "coverage",
    ".terraform",
    ".serverless",
    ".netlify",
];

/// Run `git ls-files` and return sorted relative paths.
///
/// The command `--cached --others --exclude-standard` respects both
/// `.gitignore` and `.git/info/exclude`. `-z` makes git emit NUL-separated
/// raw paths: the default C-quoted form mangles non-ASCII bytes
/// (`core.quotepath`, e.g. `"caf\303\251.rs"` for `café.rs`) and loses
/// leading/trailing spaces, so the indexed path would not match the real
/// file and reads failed at send time (greptile A4).
async fn git_ls_files(cwd: &Path, progress: &Arc<AtomicUsize>) -> Option<Vec<String>> {
    let output = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::process::Command::new("git")
            .args([
                "ls-files",
                "-z",
                "--cached",
                "--others",
                "--exclude-standard",
            ])
            .current_dir(cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;

    if !output.status.success() {
        return None;
    }

    // NUL-separated entries are never quoted, so no trim/unquote is needed;
    // preserving exact spacing matters for files like ` lead.txt`.
    let mut files: Vec<String> = output
        .stdout
        .split(|b| *b == 0)
        .map(|entry| String::from_utf8_lossy(entry).into_owned())
        .filter(|l| !l.is_empty() && !l.starts_with(".git/"))
        .collect();

    files.sort();
    progress.store(files.len().min(effective_max_files()), Ordering::Relaxed);
    files.truncate(effective_max_files());
    Some(files)
}

/// Parallel filesystem walk when `git ls-files` is unavailable.
///
/// Uses the `ignore` crate's `WalkBuilder`, which parallelizes directory
/// traversal across cores and honors `.gitignore`, `.ignore`, and global
/// excludes natively. `SKIP_DIRS` are still skipped explicitly: those trees
/// are never worth indexing even when not gitignored (dependencies, build
/// output, caches). The walk does not require a git repository, so plain
/// directories are covered too.
pub(super) async fn walkdir_collect(cwd: &Path, progress: &Arc<AtomicUsize>) -> Vec<String> {
    let cwd = cwd.to_path_buf();
    // The walker's per-thread closures require 'static data; sharing the
    // counter through a cloneable Arc lets it cross into spawn_blocking.
    let progress = progress.clone();
    let max_files = effective_max_files();
    tokio::task::spawn_blocking(move || {
        let mut files: Vec<String> = Vec::new();
        {
            let sink = Arc::new(Mutex::new(&mut files));
            let root = cwd.clone();
            let mut walker = ignore::WalkBuilder::new(root);
            let walker = walker
                .hidden(false)
                .follow_links(false)
                .require_git(false)
                .git_ignore(true)
                .git_global(true)
                .git_exclude(true)
                .ignore(true)
                .filter_entry(|entry| {
                    let is_dir = entry.file_type().is_some_and(|ft| ft.is_dir());
                    if is_dir {
                        let name = entry.file_name().to_string_lossy();
                        !SKIP_DIRS.contains(&name.as_ref())
                    } else {
                        true
                    }
                });
            let cwd_for_threads = cwd.clone();
            walker.build_parallel().run(move || {
                let sink = sink.clone();
                let cwd = cwd_for_threads.clone();
                let progress = progress.clone();
                Box::new(move |entry| {
                    let Ok(entry) = entry else {
                        return ignore::WalkState::Continue;
                    };
                    // Only regular files. FIFOs, sockets, and device nodes
                    // are read by the later File::open-based content scan;
                    // opening a FIFO without a writer blocks that thread
                    // forever (greptile A1).
                    if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                        return ignore::WalkState::Continue;
                    }
                    let Ok(rel) = entry.path().strip_prefix(&cwd) else {
                        return ignore::WalkState::Continue;
                    };
                    let Some(rel_str) = rel.to_str() else {
                        return ignore::WalkState::Continue;
                    };
                    if rel_str.is_empty() || rel_str.starts_with(".git/") {
                        return ignore::WalkState::Continue;
                    }
                    let mut guard = sink.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    if guard.len() < max_files {
                        guard.push(rel_str.to_string());
                        progress.store(guard.len(), Ordering::Relaxed);
                    }
                    if guard.len() >= max_files {
                        ignore::WalkState::Quit
                    } else {
                        ignore::WalkState::Continue
                    }
                })
            });
        }
        files.sort();
        files
    })
    .await
    .unwrap_or_default()
}

/// Build a fresh `PathIndex` for the workspace.
///
/// Prefers `git ls-files` (fast, .gitignore-aware) with a walkdir fallback.
pub(super) async fn build_path_index(cwd: &Path, progress: &Arc<AtomicUsize>) -> PathIndex {
    progress.store(0, Ordering::Relaxed);
    // Strategy 1: git ls-files.
    let file_paths = match git_ls_files(cwd, progress).await {
        Some(paths) => paths,
        None => walkdir_collect(cwd, progress).await,
    };

    let mut entries = Vec::with_capacity(file_paths.len());
    let mut path_to_index = HashMap::with_capacity(file_paths.len());

    for (i, path_str) in file_paths.iter().enumerate() {
        let p = Path::new(path_str);
        let filename: Arc<str> = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(path_str)
            .into();

        let entry = FileEntry {
            path: Arc::from(path_str.as_str()),
            lower_path: Arc::from(path_str.to_lowercase()),
            lower_filename: Arc::from(filename.to_lowercase()),
            is_directory: false,
            is_likely_binary: p
                .extension()
                .and_then(|e| e.to_str())
                .map(|ext| !TEXT_EXTENSIONS.contains(&ext))
                .unwrap_or(true),
            char_bag: CharBag::from(path_str),
        };

        path_to_index.insert(entry.path.clone(), i);
        entries.push(entry);
    }

    PathIndex {
        entries,
        lazy_entries: Vec::new(),
        scanned_ignored_dirs: HashSet::new(),
        path_to_index,
        root: cwd.to_path_buf(),
        built_at: Instant::now(),
        built: true,
    }
}
