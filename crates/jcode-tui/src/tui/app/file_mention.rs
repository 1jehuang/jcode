//! File mention (at-file) search and caching for jcode's `@file` completion.
//!
//! This module provides:
//! - `PathIndex` – an in-memory snapshot of the workspace file tree.
//! - `FileIndexManager` – async background refresh with RCU-style atomic swap.
//! - `SearchHistory` – incremental search cache that makes backspace O(1).
//! - `FileMentionCache` – unified public API used by the input UI.

use super::char_bag::CharBag;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
//  Data models
// ---------------------------------------------------------------------------

/// A single file (or directory) entry in the file index.
#[derive(Clone, Debug)]
struct FileEntry {
    /// Relative path from workspace root, e.g. "src/cli/startup.rs".
    pub path: Arc<str>,
    /// Lowercase relative path used for case-insensitive matching.
    lower_path: Arc<str>,
    /// Lowercase filename used for case-insensitive matching.
    lower_filename: Arc<str>,
    pub is_directory: bool,
    /// Extension-based heuristic: false when the extension is in TEXT_EXTENSIONS,
    /// true otherwise. Refined during actual file read (null-byte scan).
    pub is_likely_binary: bool,
    pub char_bag: CharBag,
}

/// An immutable snapshot of the workspace file tree.
///
/// #### Two-layer index strategy
///
/// | Layer | Source | When built |
/// |-------|--------|-----------|
/// | `entries` | `git ls-files --cached --others --exclude-standard` | Background task, adaptive TTL (30 s–2 min) |
/// | `lazy_entries` | `fs::read_dir` on-demand | When user query points to an ignored directory |
///
/// Entries from both layers are chained together in `search_in_index`.
#[derive(Clone, Debug)]
struct PathIndex {
    /// Base entries from git ls-files (excludes gitignored paths).
    pub entries: Vec<FileEntry>,
    /// Lazy entries from on-demand `read_dir` of ignored directories.
    pub lazy_entries: Vec<FileEntry>,
    /// Directories whose files have already been lazy-scanned (dedup).
    pub scanned_ignored_dirs: HashSet<Arc<str>>,
    /// Path → index into `entries` (not lazy_entries).
    pub path_to_index: HashMap<Arc<str>, usize>,
    /// Workspace root directory.
    pub root: PathBuf,
    /// Monotonic timestamp of last build.
    pub built_at: Instant,
    /// Whether this snapshot came from a completed index build.
    ///
    /// An empty entry list is valid for empty or unreadable workspaces, so it
    /// cannot also be used as the "not built yet" sentinel.
    pub built: bool,
    /// Loaded .gitignore patterns for the walkdir fallback.
    #[allow(dead_code)]
    gitignore_patterns: Vec<GitignorePattern>,
}

impl PathIndex {
    pub fn empty(root: PathBuf) -> Self {
        Self {
            entries: Vec::new(),
            lazy_entries: Vec::new(),
            scanned_ignored_dirs: HashSet::new(),
            path_to_index: HashMap::new(),
            root,
            built_at: Instant::now(),
            built: false,
            gitignore_patterns: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
//  Lightweight .gitignore parser (P3-21: walkdir fallback gitignore support)
// ---------------------------------------------------------------------------

/// A single parsed .gitignore pattern.
#[derive(Clone, Debug)]
struct GitignorePattern {
    /// Whether it's a negation (`!pattern`).
    negated: bool,
    /// Whether the pattern is anchored (starts with `/`).
    anchored: bool,
    /// Whether the pattern targets directories only (ends with `/`).
    dir_only: bool,
    /// The glob-like body after stripping `!`, `/`, trailing `/`.
    body: String,
}

/// Parse patterns from a `.gitignore` file.
fn load_gitignore(dir: &Path) -> Vec<GitignorePattern> {
    let path = dir.join(".gitignore");
    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let reader = std::io::BufReader::new(file);
    let mut patterns = Vec::new();
    for line in reader.lines().map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut negated = false;
        let rest = if let Some(stripped) = trimmed.strip_prefix('!') {
            negated = true;
            stripped
        } else {
            trimmed
        };
        let anchored = rest.starts_with('/');
        let body = if anchored { &rest[1..] } else { rest };
        let dir_only = body.ends_with('/');
        let body = if dir_only {
            &body[..body.len() - 1]
        } else {
            body
        };
        patterns.push(GitignorePattern {
            negated,
            anchored,
            dir_only,
            body: body.to_string(),
        });
    }
    patterns
}

/// Check if a relative path matches a gitignore pattern.
fn matches_gitignore(rel_path: &str, is_dir: bool, patterns: &[GitignorePattern]) -> bool {
    let mut ignored = false;
    for p in patterns {
        // Simple matching: check if path contains the pattern body as a segment.
        let matches = if p.anchored {
            rel_path == p.body || rel_path.starts_with(&format!("{}/", p.body))
        } else if p.body.contains('/') {
            // Pattern with slash: match from any directory level.
            rel_path == p.body
                || rel_path.ends_with(&format!("/{}", p.body))
                || rel_path.starts_with(&format!("{}/", p.body))
        } else {
            // Simple name pattern: match filename or any path component.
            if let Some(name) = Path::new(rel_path).file_name().and_then(|n| n.to_str()) {
                glob_match_name(name, &p.body)
            } else {
                rel_path.contains(&p.body)
            }
        };
        // Directory-only patterns only apply to directories.
        if p.dir_only && !is_dir {
            continue;
        }
        if matches {
            ignored = !p.negated;
        }
    }
    ignored
}

/// Simplified glob match for common gitignore patterns like `*.o`, `*.pyc`, `target`.
///
/// # Limitations
///
/// This is a deliberately small matcher that only understands the glob forms
/// commonly seen in `.gitignore` files:
/// - exact name (`target`)
/// - extension glob (`*.o`)
/// - leading/trailing/both wildcard (`*foo`, `foo*`, `*foo*`)
///
/// It does **not** support `?` (single-char wildcard) or character classes
/// (`[abc]`). Such patterns are rare in `.gitignore` files; if full gitignore
/// semantics are needed, use a dedicated crate like the `ignore` crate instead.
fn glob_match_name(name: &str, pattern: &str) -> bool {
    if pattern == name {
        return true;
    }
    if let Some(ext) = pattern.strip_prefix("*.") {
        return name.ends_with(&format!(".{}", ext));
    }
    if pattern.starts_with('*') && pattern.ends_with('*') {
        let inner = &pattern[1..pattern.len() - 1];
        return name.contains(inner);
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return name.starts_with(prefix);
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return name.ends_with(suffix);
    }
    false
}

/// A single file match produced by the search engine.
#[derive(Clone, Debug)]
pub(super) struct FileMatch {
    /// Match score (higher is better).
    pub score: f64,
    /// Relative file path.
    pub path: Arc<str>,
    pub is_directory: bool,
    /// `true` when this file was recently opened by the user.
    pub is_recent: bool,
    /// `true` when the extension is not in the known text whitelist.
    pub is_likely_binary: bool,
}

// ---------------------------------------------------------------------------
//  Frecency (frequency + recency decay) — opencode-style file ranking
// ---------------------------------------------------------------------------

/// One frecency record, serialized as a JSON line to `file_frecency.jsonl`.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct FrecencyEntry {
    path: String,
    /// How many times this file has been selected from the mention UI.
    frequency: u32,
    /// Unix timestamp (seconds) of the last selection.
    last_open: u64,
}

const MAX_FRECENCY_ENTRIES: usize = 1000;

/// Frecency tracking with on-disk persistence (JSONL, append-only with periodic
/// compaction). Inspired by opencode's `frecency.tsx`.
///
/// The score is `frequency / (1 + days_since_last_open)`, so recently and
/// frequently opened files rank higher. A path must have been opened at least
/// once to have a non-zero score.
pub(super) struct Frecency {
    /// `path → (frequency, last_open)` map.
    data: HashMap<String, (u32, u64)>,
    /// Path to `file_frecency.jsonl` under jcode's state dir. `None` when the
    /// dir could not be resolved (tests, sandboxed environments).
    file: Option<PathBuf>,
    /// Monotonic wall-clock anchor for `now_secs` to keep tests deterministic.
    #[allow(dead_code)]
    cwd: PathBuf,
}

impl Frecency {
    pub fn new(cwd: &Path) -> Self {
        // Under cargo test (without an explicit state dir) persistence is
        // disabled so parallel tests never share or clobber a real log.
        let persist = !(cfg!(test)
            && std::env::var_os("JCODE_HOME").is_none()
            && std::env::var_os("JCODE_RUNTIME_DIR").is_none());
        let file = persist.then(|| Self::state_file(cwd)).flatten();
        let mut frecency = Self {
            data: HashMap::new(),
            file,
            cwd: cwd.to_path_buf(),
        };
        frecency.load();
        frecency
    }

    /// Per-project frecency log: `state_dir/file_frecency/<hash>/file_frecency.jsonl`.
    ///
    /// Ranking signals from one repository are noise in another (identical
    /// relative paths like `src/main.rs` exist everywhere), so the store is
    /// keyed by the project working dir.
    fn state_file(cwd: &Path) -> Option<PathBuf> {
        let dir = crate::storage::durable_state_dir();
        // Stable, filesystem-safe project key (first 16 hex chars of
        // the SHA-256 of the cwd string).
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(cwd.to_string_lossy().as_bytes());
        let key: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
        Some(
            dir.join("file_frecency")
                .join(key)
                .join("file_frecency.jsonl"),
        )
    }

    /// Current unix timestamp in seconds.
    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    /// Compute the frecency score for a path. Returns 0.0 for unknown paths.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn score(&self, path: &str) -> f64 {
        self.score_at(path, Self::now_secs())
    }

    /// Compute the frecency score for a path using a caller-provided timestamp.
    fn score_at(&self, path: &str, now: u64) -> f64 {
        let Some(&(frequency, last_open)) = self.data.get(path) else {
            return 0.0;
        };
        let days = (now.saturating_sub(last_open)) as f64 / 86_400.0;
        frequency as f64 / (1.0 + days)
    }

    /// Record a file selection and append to the JSONL log.
    pub fn record(&mut self, path: &str) {
        let now = Self::now_secs();
        let entry = self.data.entry(path.to_string()).or_insert((0, 0));
        entry.0 += 1;
        entry.1 = now;

        // Append to log (best-effort).
        if let Some(file) = &self.file {
            if let Some(parent) = file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let record = FrecencyEntry {
                path: path.to_string(),
                frequency: entry.0,
                last_open: entry.1,
            };
            if let Ok(line) = serde_json::to_string(&record) {
                let _ = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(file)
                    .and_then(|mut f| writeln!(f, "{line}"));
            }
        }

        // Compact if we exceed the cap.
        if self.data.len() > MAX_FRECENCY_ENTRIES * 2 {
            self.compact();
        }
    }

    /// Load frecency entries from the JSONL log (last-write-wins per path).
    fn load(&mut self) {
        let Some(file) = &self.file else {
            return;
        };
        let Ok(reader) = std::fs::File::open(file).and_then(|f| {
            std::io::BufReader::new(f)
                .lines()
                .collect::<std::io::Result<Vec<_>>>()
        }) else {
            return;
        };
        for line in reader {
            let Ok(entry) = serde_json::from_str::<FrecencyEntry>(&line) else {
                continue;
            };
            // Last-write-wins: later appends override earlier records.
            self.data
                .insert(entry.path, (entry.frequency, entry.last_open));
        }
        // Trim to cap on load.
        if self.data.len() > MAX_FRECENCY_ENTRIES {
            self.compact();
        }
    }

    /// Compact the in-memory map and rewrite the log file with only the most
    /// recent `MAX_FRECENCY_ENTRIES` records.
    fn compact(&mut self) {
        let mut entries: Vec<(String, (u32, u64))> = self.data.drain().collect();
        entries.sort_by(|a, b| b.1.1.cmp(&a.1.1)); // by last_open desc
        entries.truncate(MAX_FRECENCY_ENTRIES);
        self.data = entries.iter().cloned().collect();

        if let Some(file) = &self.file {
            if let Some(parent) = file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let lines: Vec<String> = self
                .data
                .iter()
                .map(|(path, (freq, last))| {
                    serde_json::to_string(&FrecencyEntry {
                        path: path.clone(),
                        frequency: *freq,
                        last_open: *last,
                    })
                    .unwrap_or_default()
                })
                .collect();
            let _ = std::fs::write(file, lines.join("\n") + "\n");
        }
    }

    /// Return paths sorted by frecency score descending, for empty-query display.
    pub fn recent_paths(&self) -> Vec<String> {
        let now = Self::now_secs();
        let mut paths: Vec<(String, f64)> = self
            .data
            .keys()
            .map(|p| (p.clone(), self.score_at(p, now)))
            .collect();
        paths.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        paths.into_iter().map(|(p, _)| p).collect()
    }
}

// ---------------------------------------------------------------------------
//  Search history
// ---------------------------------------------------------------------------

struct HistoryEntry {
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
struct SearchHistory {
    entries: Vec<HistoryEntry>,
    max_entries: usize,
}

enum LookupResult {
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

// ---------------------------------------------------------------------------
//  Search engine
// ---------------------------------------------------------------------------

/// Effective per-query limits for the mention picker, derived from
/// [`FileMentionSettings`] with zero values normalized to built-in defaults.
#[derive(Clone, Copy, Debug)]
struct SearchLimits {
    max_results: usize,
}

impl SearchLimits {
    /// Read the limits from the cached global config.
    ///
    /// Uses `config()` (the cached accessor) because this runs on the sync UI
    /// thread while the @file popover is open; `Config::load()` would re-read
    /// and re-parse config.toml on every call.
    fn from_config() -> Self {
        Self::from_settings(crate::config::config().file_mention.clone())
    }

    fn from_settings(settings: crate::config::FileMentionConfig) -> Self {
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
fn dir_to_query_match(dir_path: &str, query: &str) -> bool {
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
fn match_entry(entry: &FileEntry, query_lower: &str, slash_query: &str) -> f64 {
    let filename = entry.lower_filename.as_ref();
    let path = entry.lower_path.as_ref();

    // L1 – filename prefix  (~30 ns)  ──────────────────────────────
    if filename.starts_with(query_lower) {
        return 100.0 + (filename.len() as f64).sqrt();
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
    if let Some(pos) = filename.find(query_lower) {
        return 65.0 * (1.0 - (pos as f64 / filename.len().max(1) as f64));
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
    if !query_lower.contains('/') {
        let filename_score = jcode_fuzzy::fuzzy_score(query_lower, filename).unwrap_or(0) as f64;
        let path_score = jcode_fuzzy::fuzzy_score(query_lower, path).unwrap_or(0) as f64;

        if filename_score > 0.0 || path_score > 0.0 {
            return 20.0
                + filename_score.max(path_score) * 1.0
                + if filename_score > 0.0 { 10.0 } else { 0.0 };
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

/// Show recent files + root-level files when the query is empty.
fn show_all_files(index: &PathIndex, frecency: &Frecency, max_results: usize) -> Vec<FileMatch> {
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
fn search_in_index(
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
                    score: score * (1.0 + frecency_score),
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
                score: score * (1.0 + frecency_score),
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

// ---------------------------------------------------------------------------
//  Index builder
// ---------------------------------------------------------------------------

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
const REFRESH_TTL_LARGE_THRESHOLD: usize = 2_000;

/// Return the refresh TTL appropriate for an index of `entry_count` files.
fn refresh_ttl(entry_count: usize, base_secs: u64) -> Duration {
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
const SKIP_DIRS: &[&str] = &[
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
/// `.gitignore` and `.git/info/exclude`.
async fn git_ls_files(cwd: &Path) -> Option<Vec<String>> {
    let output = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::process::Command::new("git")
            .args(["ls-files", "--cached", "--others", "--exclude-standard"])
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

    let mut files: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| {
            let l = l.trim();
            !l.is_empty() && !l.starts_with(".git/")
        })
        .map(|l| l.trim().to_string())
        .collect();

    files.sort();
    files.truncate(effective_max_files());
    Some(files)
}

/// Walkdir fallback when `git ls-files` is unavailable.
///
/// Uses `symlink_metadata` to avoid following symlinks, skips directories
/// listed in `SKIP_DIRS`, and respects `.gitignore` patterns (P3-21).
async fn walkdir_collect(cwd: &Path) -> Vec<String> {
    let cwd = cwd.to_path_buf();
    let max_files = effective_max_files();
    tokio::task::spawn_blocking(move || {
        let gitignore = load_gitignore(&cwd);
        let mut files: Vec<String> = Vec::new();
        let mut stack: Vec<PathBuf> = vec![cwd.clone()];
        // Track per-directory gitignore patterns.
        let mut dir_gitignores: Vec<(PathBuf, Vec<GitignorePattern>)> = Vec::new();

        while let Some(dir) = stack.pop() {
            let entries = match std::fs::read_dir(&dir) {
                Ok(e) => e,
                Err(_) => continue,
            };

            // Load .gitignore for this directory.
            let local_patterns = load_gitignore(&dir);
            if !local_patterns.is_empty()
                && let Ok(rel_dir) = dir.strip_prefix(&cwd)
                && !rel_dir.as_os_str().is_empty()
            {
                dir_gitignores.push((rel_dir.to_path_buf(), local_patterns));
            }

            for entry in entries.filter_map(|e| e.ok()) {
                let path = entry.path();

                // Skip symlinks.
                if entry.file_type().is_ok_and(|ft| ft.is_symlink()) {
                    continue;
                }

                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

                if path.is_dir() {
                    if !SKIP_DIRS.contains(&name) {
                        // Check gitignore for directory.
                        if let Ok(rel) = path.strip_prefix(&cwd)
                            && let Some(rel_str) = rel.to_str()
                            && !matches_any_gitignore(rel_str, true, &gitignore, &dir_gitignores)
                        {
                            stack.push(path);
                        }
                    }
                    continue;
                }

                if let Ok(rel) = path.strip_prefix(&cwd)
                    && let Some(rel_str) = rel.to_str()
                {
                    if matches_any_gitignore(rel_str, false, &gitignore, &dir_gitignores) {
                        continue;
                    }
                    files.push(rel_str.to_string());
                    if files.len() >= max_files {
                        return files;
                    }
                }
            }
        }

        files
    })
    .await
    .unwrap_or_default()
}

/// Check a relative path against root and per-directory gitignore patterns.
fn matches_any_gitignore(
    rel: &str,
    is_dir: bool,
    root_patterns: &[GitignorePattern],
    dir_patterns: &[(PathBuf, Vec<GitignorePattern>)],
) -> bool {
    if matches_gitignore(rel, is_dir, root_patterns) {
        return true;
    }
    for (dir, patterns) in dir_patterns {
        // Only apply if the path is under this directory.
        let dir_str = dir.to_string_lossy();
        let dir_prefix = format!("{dir_str}/");
        if rel.starts_with(&dir_prefix) || rel == dir_str.as_ref() {
            let sub_path = rel.strip_prefix(&dir_prefix).unwrap_or(rel);
            if matches_gitignore(sub_path, is_dir, patterns) {
                return true;
            }
        }
    }
    false
}

/// Build a fresh `PathIndex` for the workspace.
///
/// Prefers `git ls-files` (fast, .gitignore-aware) with a walkdir fallback.
async fn build_path_index(cwd: &Path) -> PathIndex {
    // Strategy 1: git ls-files.
    let file_paths = match git_ls_files(cwd).await {
        Some(paths) => paths,
        None => walkdir_collect(cwd).await,
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
        gitignore_patterns: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
//  FileIndexManager (RCU-style atomic index swap)
// ---------------------------------------------------------------------------

/// Manages background index builds with lock-free reads.
///
/// - Reads (`snapshot`) use `std::sync::RwLock::read` — the read lock is
///   never contended because the write lock is held for nanoseconds (an
///   `Arc` pointer swap).
/// - Writes happen in `tokio::spawn` tasks; `std::sync::RwLock::write`
///   blocks the worker thread briefly, which is acceptable for a sub-µs
///   critical section.
///
/// We deliberately use `std::sync::RwLock`, **not** `tokio::sync::RwLock`.
/// `tokio::sync::RwLock::blocking_read()` panics when called from inside a
/// tokio runtime, and `snapshot()` is called from the sync hot path which
/// runs on the tokio main thread.
struct FileIndexManager {
    current: Arc<RwLock<Arc<PathIndex>>>,
    cwd: PathBuf,
    refreshing: Arc<AtomicBool>,
    /// Flag set by the background file watcher (notify) when FS changes occur.
    /// Checked in `check_refresh` to trigger re-index without TTL polling.
    dirty: Arc<AtomicBool>,
    /// Holds the notify watcher thread alive. Dropping this stops watching.
    #[allow(dead_code)]
    _watcher_handle: Arc<Mutex<Option<notify::RecommendedWatcher>>>,
}

struct RefreshingFlagGuard(Arc<AtomicBool>);

impl Drop for RefreshingFlagGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl FileIndexManager {
    pub fn new(cwd: PathBuf) -> Self {
        let dirty = Arc::new(AtomicBool::new(false));
        let watcher = Self::start_watcher(cwd.clone(), dirty.clone());

        Self {
            current: Arc::new(RwLock::new(Arc::new(PathIndex::empty(cwd.clone())))),
            cwd,
            refreshing: Arc::new(AtomicBool::new(false)),
            dirty,
            _watcher_handle: Arc::new(Mutex::new(watcher)),
        }
    }

    /// Spawn a background thread that watches `cwd` for filesystem changes
    /// and sets the `dirty` flag. Returns the watcher handle.
    fn start_watcher(cwd: PathBuf, dirty: Arc<AtomicBool>) -> Option<notify::RecommendedWatcher> {
        use notify::{Event, EventKind, RecursiveMode, Watcher};
        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = notify::RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    // Ignore access-only events; only care about creates, modifies,
                    // deletes, and renames.
                    match event.kind {
                        EventKind::Create(_)
                        | EventKind::Modify(_)
                        | EventKind::Remove(_)
                        | EventKind::Any => {}
                        _ => return,
                    }
                    let _ = tx.send(());
                }
            },
            notify::Config::default(),
        )
        .ok()?;

        let _ = watcher.watch(&cwd, RecursiveMode::NonRecursive);
        // Drain events; set dirty on any relevant filesystem change.
        // Debounce: only set dirty at most once per second.
        std::thread::spawn(move || {
            let mut last_set = std::time::Instant::now()
                .checked_sub(Duration::from_secs(2))
                .unwrap_or_else(|| std::time::Instant::now() - Duration::from_secs(2));
            for _ in rx {
                let now = std::time::Instant::now();
                if now.duration_since(last_set) >= Duration::from_secs(1) {
                    dirty.store(true, Ordering::Release);
                    last_set = now;
                }
            }
        });

        Some(watcher)
    }

    /// Obtain a lightweight snapshot of the current index.
    ///
    /// Uses `std::sync::RwLock::read` — safe on the sync hot path because
    /// the write lock is only held for an `Arc` pointer swap (~ns).
    pub fn snapshot(&self) -> Arc<PathIndex> {
        self.current
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Borrow the current index without cloning the `Arc<PathIndex>`.
    fn with_index<R>(&self, f: impl FnOnce(&PathIndex) -> R) -> R {
        let guard = self.current.read().unwrap_or_else(|e| e.into_inner());
        f(&guard)
    }

    /// Lazily scan ignored directories by mutating only the lazy layer of the
    /// current index. The caller must not hold a snapshot `Arc`, so `Arc::make_mut`
    /// can usually borrow the current index mutably without cloning the full file
    /// list. If another reader briefly holds a snapshot, this still falls back to
    /// clone-on-write for correctness instead of blocking the UI path.
    fn ensure_ignored_dir_scanned(&self, query: &str) -> bool {
        let mut guard = self.current.write().unwrap_or_else(|e| e.into_inner());
        let index = Arc::make_mut(&mut guard);
        let before_lazy = index.lazy_entries.len();
        let before_scanned = index.scanned_ignored_dirs.len();
        ensure_ignored_dir_scanned(query, index);
        index.lazy_entries.len() != before_lazy
            || index.scanned_ignored_dirs.len() != before_scanned
    }

    /// Kick off an async background refresh.
    ///
    /// Takes `&self` (not `&mut self`) because state is shared through `Arc`.
    pub fn refresh_async(&self) {
        if self.refreshing.swap(true, Ordering::Acquire) {
            return; // already refreshing
        }

        let cwd = self.cwd.clone();
        let current = self.current.clone();
        let refreshing = self.refreshing.clone();

        tokio::spawn(async move {
            let _refreshing_guard = RefreshingFlagGuard(refreshing);
            let new_index = build_path_index(&cwd).await;

            // Write lock held for sub-µs: just an Arc pointer swap.
            {
                let mut guard = current.write().unwrap_or_else(|e| e.into_inner());
                *guard = Arc::new(new_index);
            }
        });
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn is_refreshing(&self) -> bool {
        self.refreshing.load(Ordering::Acquire)
    }
}

// ---------------------------------------------------------------------------
//  FileMentionCache — unified public API
// ---------------------------------------------------------------------------

/// Top-level cache that wires together indexing, search, history, and lazy
/// ignored-directory scanning.
///
/// Supports multiple workspace directories (P3-18). The first manager (index 0)
/// is the primary workspace. Search results from secondary workspaces are
/// prefixed with the workspace name.
pub(super) struct FileMentionCache {
    /// Per-workspace index managers. Index 0 is the primary (cwd).
    managers: Vec<FileIndexManager>,
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
    fn filesystem_candidates(query: &str, limits: &SearchLimits) -> Vec<FileMatch> {
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
}

// ---------------------------------------------------------------------------
//  Lazy ignored-directory scanning
// ---------------------------------------------------------------------------

/// Check whether `query` targets an ignored directory and, if so, populate
/// `index.lazy_entries` from `fs::read_dir`.
fn ensure_ignored_dir_scanned(query: &str, index: &mut PathIndex) {
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

// ---------------------------------------------------------------------------
//  File content loading (wired into the send path)
// ---------------------------------------------------------------------------

/// Maximum size of a single file before truncation.
const MAX_FILE_SIZE: usize = 100 * 1024; // 100 KB

/// Maximum total content loaded across all @file references.
const MAX_FILE_TOTAL_BUDGET: usize = 500 * 1024; // 500 KB

/// Known text file extensions (whitelist). Files not in this list are still
/// read, but a null-byte check decides whether to treat them as binary.
const TEXT_EXTENSIONS: &[&str] = &[
    "rs",
    "py",
    "js",
    "ts",
    "jsx",
    "tsx",
    "go",
    "java",
    "c",
    "cpp",
    "h",
    "hpp",
    "rb",
    "php",
    "swift",
    "kt",
    "scala",
    "clj",
    "el",
    "lua",
    "r",
    "R",
    "toml",
    "yaml",
    "yml",
    "json",
    "xml",
    "ini",
    "cfg",
    "conf",
    "md",
    "txt",
    "log",
    "csv",
    "sh",
    "bash",
    "zsh",
    "fish",
    "sql",
    "css",
    "scss",
    "html",
    "htm",
    "svg",
    "vue",
    "svelte",
    "Makefile",
    "Dockerfile",
    "gitignore",
    "env",
    "lock",
    "gradle",
    "cmake",
    "meson",
];

/// Fast binary check: whitelisted extension → text; otherwise null-byte scan.
fn is_likely_binary(path: &Path) -> bool {
    if let Some(ext) = path.extension().and_then(|e| e.to_str())
        && TEXT_EXTENSIONS.contains(&ext)
    {
        return false;
    }
    // Read the first 8 KB and check for null bytes.
    let mut buf = [0u8; 8192];
    if let Ok(mut file) = std::fs::File::open(path) {
        match file.read(&mut buf) {
            Ok(n) => buf[..n].contains(&0u8),
            Err(_) => false,
        }
    } else {
        false
    }
}

/// Recursively collect files from a directory, respecting SKIP_DIRS blacklist.
/// Stops after `max_files` entries.
fn collect_dir_files(
    dir: &Path,
    _cwd: &Path,
    out: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
    max_files: usize,
) {
    if out.len() >= max_files || !dir.is_dir() {
        return;
    }
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.filter_map(|e| e.ok()) {
            let path = entry.path();
            // Skip symlinks.
            if entry.file_type().is_ok_and(|ft| ft.is_symlink()) {
                continue;
            }
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if SKIP_DIRS.contains(&name) {
                    continue;
                }
                collect_dir_files(&path, _cwd, out, seen, max_files);
            } else if seen.insert(path.clone()) {
                out.push(path);
                if out.len() >= max_files {
                    return;
                }
            }
        }
    }
}

/// Load file contents referenced by `file_chips` and prepend them to the
/// user's prompt. This is called from the send path (`submit_input`).
///
/// `file_chips` contains relative paths (as they appear in the input text).
/// Paths are resolved against the current directory. Directories are
/// recursively walked (up to 50 files per directory, respecting SKIP_DIRS).
/// The same binary detection, size truncation, and budget management applies.
/// Expand a leading `~` in a chip path to `$HOME`, leaving other paths as-is.
///
/// Chips store display strings (e.g. `~/notes/todo.txt`), so that
/// `prune_orphan_chips` can keep matching them against the input. The tilde is
/// resolved here, just before the path is used to read file contents.
fn expand_tilde(path: &Path) -> PathBuf {
    let Some(s) = path.to_str() else {
        return path.to_path_buf();
    };
    if s == "~" {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| path.to_path_buf());
    }
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    path.to_path_buf()
}

pub(super) fn build_prompt_with_files(input: &str, file_chips: &[PathBuf], cwd: &Path) -> String {
    if file_chips.is_empty() {
        return input.to_string();
    }

    let cwd = cwd.to_path_buf();

    // Expand directories into individual file paths.
    let mut expanded_files: Vec<PathBuf> = Vec::new();
    const MAX_FILES_PER_DIR: usize = 50;
    let mut seen: HashSet<PathBuf> = HashSet::new();

    for chip in file_chips {
        let abs = cwd.join(expand_tilde(chip));
        if abs.is_dir() {
            // Recursively collect files, respecting skip dirs.
            collect_dir_files(
                &abs,
                &cwd,
                &mut expanded_files,
                &mut seen,
                MAX_FILES_PER_DIR,
            );
        } else {
            if seen.insert(abs.clone()) {
                expanded_files.push(abs);
            }
        }
    }

    // Collect files synchronously (send path runs on the main thread).
    let mut file_blocks: Vec<(String, String)> = Vec::with_capacity(expanded_files.len());

    for path in &expanded_files {
        let rel_path = path
            .strip_prefix(&cwd)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| path.to_string_lossy().to_string());

        if is_likely_binary(path) {
            let rp = rel_path.clone();
            file_blocks.push((rp, format!("[skipped: binary file {}]", rel_path)));
            continue;
        }

        // Check the size from metadata *before* reading: a chip pointing at a
        // huge file must not be fully loaded into memory just to be rejected.
        let file_size = std::fs::metadata(path).map(|m| m.len()).ok();
        if file_size.is_some_and(|size| size > MAX_FILE_SIZE as u64) {
            let size = file_size.unwrap_or(0);
            let preview = read_prefix_lines(path, 200).unwrap_or_default();
            let block = if preview.trim().is_empty() {
                format!("[file too large: {} bytes, showing first 200 lines]", size)
            } else {
                format!(
                    "{}\n\n[... file too large: {} bytes, showing first 200 lines]",
                    preview, size
                )
            };
            file_blocks.push((rel_path, block));
            continue;
        }

        match read_to_string_bounded(path, MAX_FILE_SIZE) {
            Ok(content) => {
                let block = if content.len() <= MAX_FILE_SIZE {
                    content
                } else {
                    format!(
                        "{}\n\n[... file too large: {} bytes, showing first 200 lines]",
                        content,
                        content.len(),
                    )
                };
                file_blocks.push((rel_path, block));
            }
            Err(e) => {
                let rp = rel_path.clone();
                file_blocks.push((rp, format!("[read failed: {} → {}]", rel_path, e)));
            }
        }
    }

    // Context budget: cumulative cap.
    let mut context = String::new();
    let mut total = 0usize;
    for (path, content) in &file_blocks {
        if total + content.len() > MAX_FILE_TOTAL_BUDGET {
            context.push_str(&format!(
                "\n--- {} ---\n[skipped: context budget exhausted ({} KB total)]\n",
                path,
                MAX_FILE_TOTAL_BUDGET / 1024,
            ));
        } else {
            total += content.len();
            context.push_str(&format!("\n--- {} ---\n{}\n", path, content));
        }
    }

    format!("{}{}", input, context)
}

/// Read at most `limit` bytes of a file as a string.
///
/// Metadata can race with concurrent writes, so a file that grew past the
/// limit between the stat and the read is still capped here instead of
/// ballooning memory.
///
/// UTF-8 validation matches `read_to_string` with one carve-out: when the
/// read stopped exactly at the byte limit and the *only* problem is a
/// multi-byte character sliced in half at that boundary, the file itself is
/// valid and the truncated prefix is kept. Any invalid byte before the
/// limit is genuine corruption and returns `InvalidData` so the caller can
/// surface a read-failure marker instead of silently embedding a partial
/// file.
fn read_to_string_bounded(path: &Path, limit: usize) -> std::io::Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut buf = Vec::with_capacity(limit.min(64 * 1024));
    (&mut file).take(limit as u64).read_to_end(&mut buf)?;
    let read_len = buf.len();
    match String::from_utf8(buf) {
        Ok(s) => Ok(s),
        Err(e) => {
            let valid_up_to = e.utf8_error().valid_up_to();
            match e.utf8_error().error_len() {
                // No error_len means the input ended mid-character: the byte
                // limit sliced a multi-byte char exactly at the cut.
                None if read_len == limit => {
                    let bytes = e.into_bytes();
                    let s = String::from_utf8_lossy(&bytes[..valid_up_to]).into_owned();
                    Ok(s)
                }
                _ => Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "stream did not contain valid UTF-8",
                )),
            }
        }
    }
}

/// Read the first `max_lines` lines of a file (bounded by MAX_FILE_SIZE).
fn read_prefix_lines(path: &Path, max_lines: usize) -> std::io::Result<String> {
    let content = read_to_string_bounded(path, MAX_FILE_SIZE)?;
    Ok(content
        .lines()
        .take(max_lines)
        .collect::<Vec<_>>()
        .join("\n"))
}

// ---------------------------------------------------------------------------
//  Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Mutex;

    /// Serializes tests that mutate `HOME`: the default test runner executes
    /// tests in parallel threads, and an unset or foreign `HOME` mid-test makes
    /// `~`-query assertions read another test's fixtures.
    static HOME_LOCK: Mutex<()> = Mutex::new(());

    fn set_home(dir: Option<&Path>) -> Option<std::ffi::OsString> {
        let saved = std::env::var_os("HOME");
        match dir {
            Some(p) => crate::env::set_var("HOME", p.as_os_str()),
            None => crate::env::remove_var("HOME"),
        }
        saved
    }

    fn restore_home(saved: Option<std::ffi::OsString>) {
        match saved {
            Some(v) => crate::env::set_var("HOME", v),
            None => crate::env::remove_var("HOME"),
        }
    }

    fn limits(n: usize) -> SearchLimits {
        SearchLimits { max_results: n }
    }

    fn sorted_paths(matches: &[FileMatch]) -> Vec<String> {
        let mut v: Vec<String> = matches.iter().map(|m| m.path.to_string()).collect();
        v.sort();
        v
    }

    #[test]
    fn filesystem_candidates_home_root_lists_entries() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        let saved = set_home(Some(home.path()));
        std::fs::write(home.path().join("alpha.txt"), "a").unwrap();
        std::fs::write(home.path().join("beta.md"), "b").unwrap();
        let m = FileMentionCache::filesystem_candidates("~", &limits(10));
        restore_home(saved);
        assert_eq!(sorted_paths(&m), vec!["~/alpha.txt", "~/beta.md"]);
        assert!(m.iter().all(|f| !f.is_recent && f.score == 0.0));
    }

    #[test]
    fn filesystem_candidates_home_root_no_home_returns_empty() {
        let _guard = HOME_LOCK.lock().unwrap();
        let saved = set_home(None);
        let m = FileMentionCache::filesystem_candidates("~", &limits(10));
        restore_home(saved);
        assert!(m.is_empty());
    }

    #[test]
    fn filesystem_candidates_home_partial_prefix_filters() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        let saved = set_home(Some(home.path()));
        std::fs::write(home.path().join("alpha.txt"), "a").unwrap();
        std::fs::write(home.path().join("beta.md"), "b").unwrap();
        let m = FileMentionCache::filesystem_candidates("~/al", &limits(10));
        restore_home(saved);
        assert_eq!(sorted_paths(&m), vec!["~/alpha.txt"]);
    }

    #[test]
    fn filesystem_candidates_home_subdir_and_partial() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        let saved = set_home(Some(home.path()));
        std::fs::create_dir(home.path().join("proj")).unwrap();
        std::fs::write(home.path().join("proj/main.rs"), "fn main() {}").unwrap();
        std::fs::write(home.path().join("proj/util.rs"), "pub fn f() {}").unwrap();
        let m = FileMentionCache::filesystem_candidates("~/proj/ma", &limits(10));
        restore_home(saved);
        assert_eq!(sorted_paths(&m), vec!["~/proj/main.rs"]);
        assert!(!m[0].is_directory && !m[0].is_likely_binary);
    }

    #[test]
    fn filesystem_candidates_absolute_root_lists_entries() {
        let dir = tempfile::tempdir().unwrap();
        let abs = format!("{}/", dir.path().to_string_lossy());
        std::fs::write(dir.path().join("zeta.txt"), "z").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let m = FileMentionCache::filesystem_candidates(&abs, &limits(10));
        assert_eq!(
            sorted_paths(&m),
            vec![
                format!("{}/sub", dir.path().to_string_lossy()),
                format!("{}/zeta.txt", dir.path().to_string_lossy())
            ]
        );
    }

    #[test]
    fn filesystem_candidates_absolute_partial_filters() {
        let dir = tempfile::tempdir().unwrap();
        let abs = dir.path().to_string_lossy().to_string();
        std::fs::write(dir.path().join("zeta.txt"), "z").unwrap();
        std::fs::write(dir.path().join("keep.md"), "k").unwrap();
        let m = FileMentionCache::filesystem_candidates(&format!("{abs}/ke"), &limits(10));
        assert_eq!(sorted_paths(&m), vec![format!("{abs}/keep.md")]);
    }

    #[test]
    fn filesystem_candidates_skips_symlinks_and_skip_dirs() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        let saved = set_home(Some(home.path()));
        std::fs::create_dir(home.path().join("node_modules")).unwrap();
        let real = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(real.path(), home.path().join("link")).unwrap();
        std::fs::write(home.path().join("plain.txt"), "p").unwrap();
        let m = FileMentionCache::filesystem_candidates("~", &limits(10));
        restore_home(saved);
        assert_eq!(sorted_paths(&m), vec!["~/plain.txt"]);
    }

    #[test]
    fn filesystem_candidates_dotfiles_hidden_unless_partial_dot() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        let saved = set_home(Some(home.path()));
        std::fs::write(home.path().join(".secret"), "s").unwrap();
        std::fs::write(home.path().join("visible.txt"), "v").unwrap();
        let m = FileMentionCache::filesystem_candidates("~", &limits(10));
        restore_home(saved);
        assert_eq!(sorted_paths(&m), vec!["~/visible.txt"]);

        let saved = set_home(Some(home.path()));
        let m = FileMentionCache::filesystem_candidates("~/.", &limits(10));
        restore_home(saved);
        assert_eq!(sorted_paths(&m), vec!["~/.secret"]);
    }

    #[test]
    fn filesystem_candidates_truncates_at_max_results() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        let saved = set_home(Some(home.path()));
        for i in 0..5 {
            std::fs::write(home.path().join(format!("f{i}.txt")), "x").unwrap();
        }
        let m = FileMentionCache::filesystem_candidates("~", &limits(3));
        restore_home(saved);
        assert_eq!(m.len(), 3);
    }

    /// Regression (PR review): the cap must apply after the lexical sort, so
    /// a large directory keeps its alphabetically-first entries instead of
    /// whichever readdir happened to yield first.
    #[test]
    fn filesystem_candidates_truncate_keeps_lexically_first_entries() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        let saved = set_home(Some(home.path()));
        // Reverse insertion order so readdir order differs from lexical
        // order for any realistic directory layout.
        for name in ["zulu", "yankee", "xray", "beta", "alpha"] {
            std::fs::write(home.path().join(name), "x").unwrap();
        }
        let m = FileMentionCache::filesystem_candidates("~", &limits(3));
        restore_home(saved);
        assert_eq!(
            sorted_paths(&m),
            vec!["~/alpha", "~/beta", "~/xray"],
            "cap must keep the lexically first entries, not readdir order"
        );
    }

    /// Regression (PR review): a prefix that matches nothing must not stat
    /// every entry in a large directory — filtering happens on the name
    /// before `file_type()`. The observable contract is the empty result.

    #[test]
    fn filesystem_candidates_nonmatching_prefix_returns_empty() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        let saved = set_home(Some(home.path()));
        for i in 0..2000 {
            std::fs::write(home.path().join(format!("file{i:04}.txt")), "x").unwrap();
        }
        let m = FileMentionCache::filesystem_candidates("~/nomatch", &limits(10));
        restore_home(saved);
        assert!(m.is_empty());
    }

    #[test]
    fn filesystem_candidates_binary_by_extension() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        let saved = set_home(Some(home.path()));
        std::fs::write(home.path().join("data.bin"), "b").unwrap();
        std::fs::write(home.path().join("read.md"), "r").unwrap();
        let m = FileMentionCache::filesystem_candidates("~", &limits(10));
        restore_home(saved);
        let bin = m.iter().find(|f| f.path.ends_with("data.bin")).unwrap();
        let txt = m.iter().find(|f| f.path.ends_with("read.md")).unwrap();
        assert!(bin.is_likely_binary);
        assert!(!txt.is_likely_binary);
    }

    #[test]
    fn filesystem_candidates_missing_dir_returns_empty() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        let saved = set_home(Some(home.path()));
        let m = FileMentionCache::filesystem_candidates("~/nope", &limits(10));
        restore_home(saved);
        assert!(m.is_empty());
    }

    #[test]
    fn filesystem_candidates_no_match_returns_empty() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        let saved = set_home(Some(home.path()));
        std::fs::write(home.path().join("alpha.txt"), "a").unwrap();
        let m = FileMentionCache::filesystem_candidates("~/zz", &limits(10));
        restore_home(saved);
        assert!(m.is_empty());
    }

    #[test]
    fn filesystem_candidates_marks_directories() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        let saved = set_home(Some(home.path()));
        std::fs::create_dir(home.path().join("pkg")).unwrap();
        let m = FileMentionCache::filesystem_candidates("~/p", &limits(10));
        restore_home(saved);
        assert_eq!(sorted_paths(&m), vec!["~/pkg"]);
        assert!(m[0].is_directory);
        assert!(!m[0].is_likely_binary);
    }

    #[test]
    fn expand_tilde_expands_home_and_leaves_others() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        let saved = set_home(Some(home.path()));
        assert_eq!(
            expand_tilde(Path::new("~/a.txt")),
            home.path().join("a.txt")
        );
        assert_eq!(expand_tilde(Path::new("~")), home.path().to_path_buf());
        restore_home(saved);
        assert_eq!(
            expand_tilde(Path::new("/abs/a.txt")),
            PathBuf::from("/abs/a.txt")
        );
        assert_eq!(
            expand_tilde(Path::new("rel/a.txt")),
            PathBuf::from("rel/a.txt")
        );
        assert_eq!(
            expand_tilde(Path::new("not~tilde")),
            PathBuf::from("not~tilde")
        );
    }

    #[test]
    fn expand_tilde_without_home_leaves_path_unchanged() {
        let _guard = HOME_LOCK.lock().unwrap();
        let saved = set_home(None);
        assert_eq!(expand_tilde(Path::new("~/a.txt")), PathBuf::from("~/a.txt"));
        assert_eq!(expand_tilde(Path::new("~")), PathBuf::from("~"));
        restore_home(saved);
    }
    use std::time::Instant;

    fn file_entry(path: &str) -> FileEntry {
        let filename: Arc<str> = Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(path)
            .into();

        FileEntry {
            path: Arc::from(path),
            lower_path: Arc::from(path.to_lowercase()),
            lower_filename: Arc::from(filename.to_lowercase()),
            is_directory: false,
            is_likely_binary: false,
            char_bag: CharBag::from(path),
        }
    }

    fn file_match(path: &str) -> FileMatch {
        FileMatch {
            score: 50.0,
            path: Arc::from(path),
            is_directory: false,
            is_recent: false,
            is_likely_binary: false,
        }
    }

    fn build_test_index(paths: &[&str]) -> PathIndex {
        let mut entries = Vec::new();
        for p in paths {
            entries.push(file_entry(p));
        }
        let path_to_index = entries
            .iter()
            .enumerate()
            .map(|(i, entry)| (entry.path.clone(), i))
            .collect();
        PathIndex {
            entries,
            lazy_entries: Vec::new(),
            scanned_ignored_dirs: HashSet::new(),
            path_to_index,
            root: PathBuf::from("."),
            built_at: Instant::now(),
            built: true,
            gitignore_patterns: Vec::new(),
        }
    }

    fn empty_frecency() -> Frecency {
        Frecency {
            data: HashMap::new(),
            file: None,
            cwd: PathBuf::from("."),
        }
    }

    fn frecency_with(paths: &[&str]) -> Frecency {
        let mut frecency = empty_frecency();
        for path in paths {
            frecency.record(path);
        }
        frecency
    }

    #[test]
    fn frecency_persists_jsonl_and_loads_last_record() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("file_frecency.jsonl");

        let mut frecency = Frecency {
            data: HashMap::new(),
            file: Some(file.clone()),
            cwd: PathBuf::from("."),
        };
        frecency.record("src/main.rs");
        frecency.record("src/main.rs");

        let mut loaded = Frecency {
            data: HashMap::new(),
            file: Some(file),
            cwd: PathBuf::from("."),
        };
        loaded.load();

        assert!(loaded.score("src/main.rs") >= 2.0);
        assert_eq!(loaded.recent_paths(), vec!["src/main.rs".to_string()]);
    }

    /// Frecency state must be keyed per project: two different cwds map to
    /// two different log files, so one project's ranking cannot leak into
    /// another (identical relative paths like `src/main.rs` are common).
    #[test]
    fn frecency_state_file_is_keyed_by_cwd() {
        // `state_file` builds a path under the *real* home when the state
        // dir helper is unavailable; a parallel `~` test mutating `HOME`
        // would change the resolved path mid-assertion.
        let _guard = HOME_LOCK.lock().unwrap();
        let a = Frecency::state_file(Path::new("/repo/alpha"))
            .expect("state file must resolve outside cfg(test) helper dirs");
        let b = Frecency::state_file(Path::new("/repo/beta"))
            .expect("state file must resolve outside cfg(test) helper dirs");
        let a2 = Frecency::state_file(Path::new("/repo/alpha"))
            .expect("state file must resolve outside cfg(test) helper dirs");
        assert_ne!(a, b, "different projects must use different log files");
        assert_eq!(a, a2, "the same project must map to a stable path");
        assert!(
            a.to_string_lossy().contains("file_frecency"),
            "logs live under a file_frecency subdir: {}",
            a.display()
        );
    }

    // -- match_entry ---------------------------------------------------------

    fn s(q: &str) -> String {
        format!("/{}", q)
    }

    #[test]
    fn match_filename_prefix_wins() {
        let entry = file_entry("deep/nested/src/lib.rs");
        let score = match_entry(&entry, "lib.rs", &s("lib.rs"));
        assert!(
            score > 90.0,
            "filename prefix should score high, got {score}"
        );
    }

    #[test]
    fn match_path_prefix() {
        let entry = file_entry("src/cli/args.rs");
        let score = match_entry(&entry, "src/cli", &s("src/cli"));
        assert!(score > 70.0, "path prefix should score high, got {score}");
    }

    #[test]
    fn match_fuzzy_subsequence() {
        let entry = file_entry("src/cli/startup.rs");
        let score = match_entry(&entry, "scli", &s("scli"));
        assert!(score > 0.0, "fuzzy match should work for 'scli'");
    }

    #[test]
    fn match_no_match() {
        let entry = file_entry("src/lib.rs");
        assert_eq!(match_entry(&entry, "xyz", &s("xyz")), 0.0);
    }

    // -- search_in_index ----------------------------------------------------

    #[test]
    fn search_returns_results() {
        let index = build_test_index(&["src/main.rs", "Cargo.toml"]);
        let results = search_in_index(
            "main",
            &index,
            &empty_frecency(),
            SearchLimits::from_settings(crate::config::FileMentionConfig::default()),
        );
        assert!(!results.is_empty());
        assert!(results[0].path.contains("main"));
    }

    #[test]
    fn search_empty_query() {
        let index = build_test_index(&["README.md", "src/main.rs"]);
        let results = search_in_index(
            "",
            &index,
            &empty_frecency(),
            SearchLimits::from_settings(crate::config::FileMentionConfig::default()),
        );
        // README.md is root-level → should appear.
        assert!(results.iter().any(|r| r.path.as_ref() == "README.md"));
    }

    #[test]
    fn search_prioritizes_filename() {
        let index = build_test_index(&["deep/nested/args.rs", "src/args.rs"]);
        let results = search_in_index(
            "args",
            &index,
            &empty_frecency(),
            SearchLimits::from_settings(crate::config::FileMentionConfig::default()),
        );
        // Both files match; "deep/nested/args.rs" has "args" in filename and shorter path
        // portion, so either ordering is valid as long as both appear.
        assert!(!results.is_empty());
        let paths: Vec<&str> = results.iter().map(|r| r.path.as_ref()).collect();
        assert!(paths.contains(&"src/args.rs"));
        assert!(paths.contains(&"deep/nested/args.rs"));
    }

    #[test]
    fn search_recent_file_boost() {
        let index = build_test_index(&["Cargo.toml", "src/main.rs"]);
        let frecency = frecency_with(&["Cargo.toml"]);
        let results = search_in_index(
            "Cargo",
            &index,
            &frecency,
            SearchLimits::from_settings(crate::config::FileMentionConfig::default()),
        );
        assert!(results.iter().any(|r| r.is_recent));
    }

    #[test]
    fn search_regex_matches_full_relative_path() {
        let index = build_test_index(&[
            "src/tui/app/file_mention.rs",
            "crates/jcode-base/src/message.rs",
        ]);

        let results = search_in_index(
            r"src/.*/file_mention\.rs",
            &index,
            &empty_frecency(),
            SearchLimits::from_settings(crate::config::FileMentionConfig::default()),
        );

        assert!(
            results
                .iter()
                .any(|r| r.path.as_ref() == "src/tui/app/file_mention.rs"),
            "regex should match against the full relative path: {results:?}"
        );
    }

    #[test]
    fn search_regex_matches_filename() {
        let index = build_test_index(&["docs/file_mention.md", "src/tui/app/input.rs"]);

        let results = search_in_index(
            r"file_.*\.md",
            &index,
            &empty_frecency(),
            SearchLimits::from_settings(crate::config::FileMentionConfig::default()),
        );

        assert!(
            results
                .iter()
                .any(|r| r.path.as_ref() == "docs/file_mention.md"),
            "regex should still match against filename: {results:?}"
        );
    }

    #[test]
    fn search_wildcard_path_input_matches_like_regex() {
        let index = build_test_index(&["src/main.rs", "docs/main.md"]);

        let results = search_in_index(
            "*.rs",
            &index,
            &empty_frecency(),
            SearchLimits::from_settings(crate::config::FileMentionConfig::default()),
        );

        assert!(
            results.iter().any(|r| r.path.as_ref() == "src/main.rs"),
            "common wildcard path input should match file paths: {results:?}"
        );
    }

    #[tokio::test]
    async fn empty_workspace_finishes_initial_index_without_building_forever() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path();
        let mut cache = FileMentionCache::new();
        let deadline = Instant::now() + Duration::from_secs(2);

        loop {
            cache.check_refresh(cwd);
            let _ = cache.candidates("missing");
            if !cache.is_initial_building()
                && cache
                    .managers
                    .iter()
                    .any(|manager| manager.snapshot().built)
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "empty workspace index never reached a built terminal state"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        assert!(cache.candidates("missing").is_empty());
        assert!(!cache.is_initial_building());
    }

    // -- SearchHistory (exact-match only) -----------------------------------

    #[test]
    fn history_exact_hit_after_save() {
        let mut history = SearchHistory::new();
        history.save("src", &[file_match("src/main.rs")]);
        match history.lookup("src") {
            LookupResult::Hit(results) => assert_eq!(results.len(), 1),
            _ => panic!("exact match should hit"),
        }
    }

    #[test]
    fn history_miss_on_prefix_extension() {
        // Incremental input like "src" → "src/" falls through to full search.
        let mut history = SearchHistory::new();
        history.save("src", &[file_match("src/main.rs")]);
        assert!(matches!(history.lookup("src/cli"), LookupResult::Miss));
    }

    #[test]
    fn history_backspace_to_exact() {
        let mut history = SearchHistory::new();
        history.save("src/cli", &[file_match("src/cli/args.rs")]);
        history.save("src/cli/s", &[file_match("src/cli/startup.rs")]);
        // Backspace to "src/cli" — exact match, hit.
        match history.lookup("src/cli") {
            LookupResult::Hit(results) => assert_eq!(results.len(), 1),
            _ => panic!("backspace to exact should hit"),
        }
        // Backspace further to "src" — not exact, miss, triggers full search.
        assert!(matches!(history.lookup("src"), LookupResult::Miss));
    }

    #[test]
    fn history_empty_clears_and_misses() {
        let mut history = SearchHistory::new();
        history.save("abc", &[file_match("x")]);
        assert!(matches!(history.lookup(""), LookupResult::Miss));
        assert!(matches!(history.lookup("abc"), LookupResult::Miss));
    }

    #[test]
    fn search_is_case_insensitive_without_per_candidate_lowercase() {
        let index = build_test_index(&["Src/Main.RS", "Cargo.toml"]);
        let results = search_in_index(
            "main",
            &index,
            &empty_frecency(),
            SearchLimits::from_settings(crate::config::FileMentionConfig::default()),
        );
        assert!(results.iter().any(|r| r.path.as_ref() == "Src/Main.RS"));
    }

    #[test]
    fn build_prompt_with_files_uses_supplied_workspace_cwd() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path();
        std::fs::write(cwd.join("context.md"), "hello from workspace").unwrap();

        let prompt = build_prompt_with_files("Question", &[PathBuf::from("context.md")], cwd);

        assert!(prompt.contains("Question"));
        assert!(prompt.contains("--- context.md ---"));
        assert!(prompt.contains("hello from workspace"));
    }

    /// An oversized file is truncated without ever loading it fully into
    /// memory: size comes from metadata and the preview from a bounded read.
    #[test]
    fn build_prompt_with_files_truncates_oversized_file_without_full_read() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path();
        let big = cwd.join("big.log");
        // 200 distinct lines plus padding past the 100 KB limit.
        let line = "x".repeat(600);
        let mut content = String::new();
        for i in 0..200 {
            content.push_str(&format!("{:04} {}\n", i, line));
        }
        content.push_str(&"pad ".repeat(40000));
        std::fs::write(&big, content).unwrap();
        assert!(std::fs::metadata(&big).unwrap().len() > MAX_FILE_SIZE as u64);

        let prompt = build_prompt_with_files("Q", &[PathBuf::from("big.log")], cwd);

        assert!(
            prompt.contains("[... file too large:"),
            "must be marked truncated"
        );
        assert!(
            prompt.contains("0000 "),
            "the preview must contain the first lines of the file"
        );
        assert!(
            !prompt.contains("pad pad pad"),
            "the tail padding must not be read into the prompt"
        );
        assert!(
            prompt.matches("1999 ").count() <= 1,
            "preview is capped at 200 lines"
        );
    }

    /// The bounded reader caps reads at the limit even when the file grows
    /// between the metadata check and the read.
    #[test]
    fn read_to_string_bounded_caps_at_limit() {
        let temp = tempfile::tempdir().unwrap();
        let p = temp.path().join("bounded.txt");
        std::fs::write(&p, "abcdef").unwrap();

        let full = read_to_string_bounded(&p, 100).unwrap();
        assert_eq!(full, "abcdef");

        let capped = read_to_string_bounded(&p, 3).unwrap();
        assert_eq!(capped, "abc", "read must stop at the byte limit");
    }

    /// A genuinely corrupt file (invalid byte before the read limit) must be
    /// reported as a read failure so the prompt shows the failure marker
    /// instead of silently embedding a partial file.
    #[test]
    fn read_to_string_bounded_rejects_invalid_utf8_mid_file() {
        let temp = tempfile::tempdir().unwrap();
        let p = temp.path().join("corrupt.txt");
        let mut bytes = b"valid prefix".to_vec();
        bytes.push(0xff); // invalid UTF-8 byte in the middle
        bytes.extend_from_slice(b" trailing");
        std::fs::write(&p, bytes).unwrap();

        let err = read_to_string_bounded(&p, 100).unwrap_err();
        assert_eq!(
            err.kind(),
            std::io::ErrorKind::InvalidData,
            "corrupt content must surface as a read failure"
        );
    }

    /// A valid file whose multi-byte character happens to be sliced at the
    /// byte limit is not corruption: the truncated prefix is kept.
    #[test]
    fn read_to_string_bounded_keeps_prefix_sliced_at_char_boundary() {
        let temp = tempfile::tempdir().unwrap();
        let p = temp.path().join("sliced.txt");
        // 'é' is 2 bytes: a 1-byte cut lands mid-character.
        std::fs::write(&p, "caf\u{e9}s").unwrap();

        let s = read_to_string_bounded(&p, 4).unwrap();
        assert_eq!(s, "caf", "sliced char at the limit keeps the valid prefix");
    }

    /// Corrupt content must reach the prompt as a read-failure marker, never
    /// as silently truncated content.
    #[test]
    fn build_prompt_with_files_reports_read_failure_for_corrupt_file() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path();
        let p = cwd.join("corrupt.txt");
        let mut bytes = b"valid prefix".to_vec();
        bytes.push(0xff);
        std::fs::write(&p, bytes).unwrap();

        let prompt = build_prompt_with_files("Q", &[PathBuf::from("corrupt.txt")], cwd);

        assert!(
            prompt.contains("[read failed: corrupt.txt"),
            "corrupt file must surface the read-failure marker: {prompt}"
        );
        assert!(
            !prompt.contains("valid prefix"),
            "partial content must not be silently embedded: {prompt}"
        );
    }

    // -- load_gitignore ------------------------------------------------------

    fn gitignore_pattern(
        body: &str,
        negated: bool,
        anchored: bool,
        dir_only: bool,
    ) -> GitignorePattern {
        GitignorePattern {
            negated,
            anchored,
            dir_only,
            body: body.to_string(),
        }
    }

    #[test]
    fn load_gitignore_reads_patterns_from_file() {
        let temp = tempfile::tempdir().unwrap();
        let gitignore = temp.path().join(".gitignore");
        std::fs::write(
            &gitignore,
            "# comment line\n*.log\nnode_modules/\n/build\n!important.log\n",
        )
        .unwrap();

        let patterns = load_gitignore(temp.path());
        assert_eq!(patterns.len(), 4, "comment + blank lines are skipped");

        // *.log — plain extension glob.
        assert_eq!(patterns[0].body, "*.log");
        assert!(!patterns[0].negated);
        assert!(!patterns[0].anchored);
        assert!(!patterns[0].dir_only);

        // node_modules/ — directory-only, trailing slash stripped.
        assert_eq!(patterns[1].body, "node_modules");
        assert!(patterns[1].dir_only);
        assert!(!patterns[1].anchored);

        // /build — anchored at root, leading slash stripped.
        assert_eq!(patterns[2].body, "build");
        assert!(patterns[2].anchored);
        assert!(!patterns[2].dir_only);

        // !important.log — negation.
        assert_eq!(patterns[3].body, "important.log");
        assert!(patterns[3].negated);
    }

    #[test]
    fn load_gitignore_empty_dir_returns_empty() {
        let temp = tempfile::tempdir().unwrap();
        let patterns = load_gitignore(temp.path());
        assert!(patterns.is_empty());
    }

    #[test]
    fn load_gitignore_nonexistent_dir_returns_empty() {
        let patterns = load_gitignore(Path::new(
            "/nonexistent/jcode/test/path/that/does/not/exist",
        ));
        assert!(patterns.is_empty());
    }

    // -- matches_gitignore ---------------------------------------------------

    #[test]
    fn matches_gitignore_extension_glob() {
        let patterns = vec![gitignore_pattern("*.log", false, false, false)];
        assert!(matches_gitignore("debug.log", false, &patterns));
        assert!(matches_gitignore("logs/error.log", false, &patterns));
        assert!(!matches_gitignore("debug.txt", false, &patterns));
    }

    #[test]
    fn matches_gitignore_dir_only_skips_files() {
        let patterns = vec![gitignore_pattern("node_modules", false, false, true)];
        // A file literally named "node_modules" is not ignored (dir-only).
        assert!(!matches_gitignore("node_modules", false, &patterns));
        // A directory named "node_modules" is ignored.
        assert!(matches_gitignore("node_modules", true, &patterns));
        // A nested "node_modules" directory is ignored by name match.
        assert!(matches_gitignore("packages/node_modules", true, &patterns));
    }

    #[test]
    fn matches_gitignore_anchored_pattern_only_at_root() {
        let patterns = vec![gitignore_pattern("build", false, true, false)];
        assert!(matches_gitignore("build", false, &patterns));
        assert!(matches_gitignore("build/output.o", false, &patterns));
        // A nested "build" path is not matched by an anchored pattern.
        assert!(!matches_gitignore("sub/build", false, &patterns));
    }

    #[test]
    fn matches_gitignore_negation_unignores() {
        let patterns = vec![
            gitignore_pattern("*.log", false, false, false),
            gitignore_pattern("important.log", true, false, false),
        ];
        // Negated pattern overrides the earlier *.log match.
        assert!(!matches_gitignore("important.log", false, &patterns));
        // Other .log files stay ignored.
        assert!(matches_gitignore("debug.log", false, &patterns));
    }

    // -- glob_match_name -----------------------------------------------------

    #[test]
    fn glob_match_exact_name() {
        assert!(glob_match_name("target", "target"));
        assert!(glob_match_name("main.rs", "main.rs"));
        assert!(!glob_match_name("target", "cargo"));
    }

    #[test]
    fn glob_match_extension_wildcard() {
        assert!(glob_match_name("debug.o", "*.o"));
        assert!(glob_match_name("app.log", "*.log"));
        assert!(!glob_match_name("debug.rs", "*.o"));
    }

    #[test]
    fn glob_match_prefix_wildcard() {
        assert!(glob_match_name("debug.log", "debug*"));
        assert!(glob_match_name("debug.txt", "debug*"));
        assert!(!glob_match_name("error.log", "debug*"));
    }

    #[test]
    fn glob_match_suffix_wildcard() {
        assert!(glob_match_name("test_main.rs", "*main.rs"));
        // endswith matches the bare name too.
        assert!(glob_match_name("main.rs", "*main.rs"));
        assert!(!glob_match_name("main.ts", "*main.rs"));
    }

    #[test]
    fn glob_match_contains_wildcard() {
        assert!(glob_match_name("myfoo.txt", "*foo*"));
        assert!(glob_match_name("foobar", "*foo*"));
        assert!(glob_match_name("foo", "*foo*"));
        assert!(!glob_match_name("bar", "*foo*"));
    }

    #[test]
    fn glob_match_question_mark_is_not_supported() {
        // `?` is intentionally not a single-char wildcard in this matcher.
        assert!(!glob_match_name("a.txt", "?.txt"));
        assert!(!glob_match_name("ab", "a?"));
    }

    // -- dir_to_query_match -------------------------------------------------

    #[test]
    fn dir_to_query_match_exact_prefix() {
        assert!(dir_to_query_match("src/", "src"));
    }

    #[test]
    fn dir_to_query_match_nested_segment() {
        assert!(dir_to_query_match("crates/jcode-tui/src/", "src"));
    }

    #[test]
    fn dir_to_query_match_last_component_partial() {
        // Last component "src" starts with "sr".
        assert!(dir_to_query_match("packages/src/", "sr"));
    }

    #[test]
    fn dir_to_query_match_no_match() {
        assert!(!dir_to_query_match("docs/", "src"));
        assert!(!dir_to_query_match("tests/unit/", "src"));
    }

    // -- refresh_ttl --------------------------------------------------------

    #[test]
    fn refresh_ttl_small_count_uses_base_interval() {
        let base = Duration::from_secs(crate::config::DEFAULT_FILE_MENTION_REFRESH_TTL_SECS);
        assert_eq!(
            refresh_ttl(0, crate::config::DEFAULT_FILE_MENTION_REFRESH_TTL_SECS),
            base
        );
        assert_eq!(
            refresh_ttl(100, crate::config::DEFAULT_FILE_MENTION_REFRESH_TTL_SECS),
            base
        );
        assert_eq!(
            refresh_ttl(
                REFRESH_TTL_LARGE_THRESHOLD - 1,
                crate::config::DEFAULT_FILE_MENTION_REFRESH_TTL_SECS,
            ),
            base
        );
    }

    #[test]
    fn refresh_ttl_large_count_uses_large_interval() {
        let large = Duration::from_secs(crate::config::DEFAULT_FILE_MENTION_REFRESH_TTL_SECS * 4);
        assert_eq!(
            refresh_ttl(
                REFRESH_TTL_LARGE_THRESHOLD,
                crate::config::DEFAULT_FILE_MENTION_REFRESH_TTL_SECS,
            ),
            large
        );
        assert_eq!(
            refresh_ttl(10_000, crate::config::DEFAULT_FILE_MENTION_REFRESH_TTL_SECS),
            large
        );
    }

    #[test]
    fn refresh_ttl_changes_at_threshold() {
        assert_ne!(
            refresh_ttl(100, crate::config::DEFAULT_FILE_MENTION_REFRESH_TTL_SECS),
            refresh_ttl(10_000, crate::config::DEFAULT_FILE_MENTION_REFRESH_TTL_SECS)
        );
        assert_eq!(
            refresh_ttl(0, crate::config::DEFAULT_FILE_MENTION_REFRESH_TTL_SECS),
            refresh_ttl(100, crate::config::DEFAULT_FILE_MENTION_REFRESH_TTL_SECS)
        );
    }

    #[test]
    fn refresh_ttl_zero_config_uses_default() {
        assert_eq!(
            refresh_ttl(0, 0),
            Duration::from_secs(crate::config::DEFAULT_FILE_MENTION_REFRESH_TTL_SECS)
        );
    }

    // -- show_all_files -----------------------------------------------------

    #[test]
    fn show_all_files_sorted_by_frecency_score() {
        let index = build_test_index(&["Cargo.toml", "src/main.rs"]);
        let mut frecency = empty_frecency();
        // Cargo.toml: high frequency, just opened.
        frecency
            .data
            .insert("Cargo.toml".to_string(), (10, Frecency::now_secs()));
        // src/main.rs: low frequency, very old.
        frecency.data.insert("src/main.rs".to_string(), (1, 1));

        let results = show_all_files(&index, &frecency, 10);
        assert_eq!(results.len(), 2);
        // Higher frecency score ranks first.
        assert_eq!(results[0].path.as_ref(), "Cargo.toml");
        assert!(results[0].is_recent);
        assert_eq!(results[1].path.as_ref(), "src/main.rs");
    }

    #[test]
    fn show_all_files_recent_file_ranks_above_root_files() {
        let index = build_test_index(&["Cargo.toml", "src/main.rs"]);
        let mut frecency = empty_frecency();
        frecency
            .data
            .insert("src/main.rs".to_string(), (5, Frecency::now_secs()));

        let results = show_all_files(&index, &frecency, 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].path.as_ref(), "src/main.rs");
        assert!(results[0].is_recent);
        assert_eq!(results[1].path.as_ref(), "Cargo.toml");
        assert!(!results[1].is_recent);
    }

    #[test]
    fn show_all_files_respects_max_results() {
        let index = build_test_index(&["a.rs", "b.rs", "c.rs"]);
        let results = show_all_files(&index, &empty_frecency(), 2);
        assert_eq!(results.len(), 2);
    }

    // -- Config-driven limits ------------------------------------------------

    #[test]
    fn search_limits_default_when_unset() {
        let limits = SearchLimits::from_settings(crate::config::FileMentionConfig::default());
        assert_eq!(
            limits.max_results,
            crate::config::DEFAULT_FILE_MENTION_MAX_RESULTS
        );
    }

    #[test]
    fn search_limits_zero_values_fall_back_to_defaults() {
        let settings = crate::config::FileMentionConfig {
            max_results: 0,
            ..Default::default()
        };
        let limits = SearchLimits::from_settings(settings);
        assert_eq!(
            limits.max_results,
            crate::config::DEFAULT_FILE_MENTION_MAX_RESULTS,
            "max_results = 0 must mean 'use the default', not 'return nothing'"
        );
    }

    #[test]
    fn search_in_index_respects_configured_max_results() {
        let index = build_test_index(&["a1.rs", "a2.rs", "a3.rs", "a4.rs"]);
        let settings = crate::config::FileMentionConfig {
            max_results: 2,
            ..Default::default()
        };
        let limits = SearchLimits::from_settings(settings);
        let results = search_in_index("a", &index, &empty_frecency(), limits);
        assert_eq!(
            results.len(),
            2,
            "configured max_results must cap the suggestion list"
        );
    }

    // -- Frecency::compact / recent_paths ----------------------------------

    #[test]
    fn frecency_compact_trims_to_cap() {
        let mut frecency = empty_frecency();
        // Insert more than the cap, with monotonically increasing last_open so
        // the surviving set is deterministic (most recent entries kept).
        for i in 0..(MAX_FRECENCY_ENTRIES + 500) as u64 {
            frecency.data.insert(format!("file_{i}.rs"), (1, i));
        }
        assert_eq!(
            frecency.data.len(),
            MAX_FRECENCY_ENTRIES + 500,
            "precondition: all entries inserted"
        );

        frecency.compact();

        assert_eq!(frecency.data.len(), MAX_FRECENCY_ENTRIES);
        // last_open 0..500 are the oldest → dropped; 500..1500 survive.
        assert!(!frecency.data.contains_key("file_499.rs"));
        assert!(frecency.data.contains_key("file_500.rs"));
        assert!(frecency.data.contains_key("file_1499.rs"));
    }

    #[test]
    fn frecency_recent_paths_returns_recorded_paths_in_score_order() {
        let mut frecency = empty_frecency();
        frecency.record("src/main.rs");
        frecency.record("Cargo.toml");
        frecency.record("src/main.rs"); // second open → higher frequency

        let recent = frecency.recent_paths();
        assert_eq!(recent.len(), 2);
        // src/main.rs was opened twice → higher score → ranks first.
        assert_eq!(recent[0], "src/main.rs");
        assert_eq!(recent[1], "Cargo.toml");
    }

    // -- SearchHistory::new ------------------------------------------------

    #[test]
    fn search_history_new_is_empty() {
        let history = SearchHistory::new();
        assert!(history.entries.is_empty());
        assert_eq!(history.max_entries, 20);

        let mut history = SearchHistory::new();
        assert!(matches!(history.lookup("anything"), LookupResult::Miss));
        assert!(matches!(history.lookup(""), LookupResult::Miss));
    }
}
