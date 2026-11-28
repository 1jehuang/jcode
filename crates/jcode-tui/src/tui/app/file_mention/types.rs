//! Data models for the @file mention index: entries, the path index
//! snapshot, frecency ranking, and shared classification constants.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::tui::app::char_bag::CharBag;

/// A single file (or directory) entry in the file index.
#[derive(Clone, Debug)]
pub(in crate::tui::app) struct FileEntry {
    /// Relative path from workspace root, e.g. "src/cli/startup.rs".
    pub path: Arc<str>,
    /// Lowercase relative path used for case-insensitive matching.
    pub(super) lower_path: Arc<str>,
    /// Lowercase filename used for case-insensitive matching.
    pub(super) lower_filename: Arc<str>,
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
pub(in crate::tui::app) struct PathIndex {
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
        }
    }

    /// Test-only fixed-entry index: a `built` snapshot whose `FileEntry`
    /// metadata is derived the same way `build_path_index` derives it.
    /// Lets app-level tests exercise the suggestion popover and the Tab
    /// cycle without racing the async builder.
    #[cfg(test)]
    pub fn test_index(root: PathBuf, paths: &[&str]) -> Self {
        let mut entries = Vec::with_capacity(paths.len());
        let mut path_to_index = HashMap::with_capacity(paths.len());
        for (i, path_str) in paths.iter().enumerate() {
            let p = Path::new(path_str);
            let filename: Arc<str> = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(path_str)
                .into();
            let entry = FileEntry {
                path: Arc::from(*path_str),
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
        Self {
            entries,
            lazy_entries: Vec::new(),
            scanned_ignored_dirs: HashSet::new(),
            path_to_index,
            root,
            built_at: Instant::now(),
            built: true,
        }
    }
}

/// A single file match produced by the search engine.
#[derive(Clone, Debug)]
pub(in crate::tui::app) struct FileMatch {
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

/// One frecency record, serialized as a JSON line to `file_frecency.jsonl`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct FrecencyEntry {
    pub(super) path: String,
    pub(super) frequency: u32,
    pub(super) last_open: u64,
}

pub(super) const MAX_FRECENCY_ENTRIES: usize = 1000;

/// Frecency tracking with on-disk persistence (JSONL, append-only with periodic
/// compaction). Inspired by opencode's `frecency.tsx`.
///
/// The score is `frequency / (1 + days_since_last_open)`, so recently and
/// frequently opened files rank higher. A path must have been opened at least
/// once to have a non-zero score.
pub(super) struct Frecency {
    /// `path → (frequency, last_open)` map.
    pub(super) data: HashMap<String, (u32, u64)>,
    /// Path to `file_frecency.jsonl` under jcode's state dir. `None` when the
    /// dir could not be resolved (tests, sandboxed environments).
    pub(super) file: Option<PathBuf>,
    /// Project cwd this frecency log belongs to. Kept for identity/debug
    /// purposes; scoring uses the persisted UNIX-epoch timestamps only.
    #[allow(dead_code)]
    pub(super) cwd: PathBuf,
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
    pub(super) fn state_file(cwd: &Path) -> Option<PathBuf> {
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
    pub(super) fn now_secs() -> u64 {
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
    pub(super) fn score_at(&self, path: &str, now: u64) -> f64 {
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
    pub(super) fn load(&mut self) {
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
    ///
    /// The log file is shared across terminals, so the rewrite cannot just
    /// dump this instance's in-memory copy: another terminal may hold counts
    /// this instance never loaded. Merge-on-write: reload the current file and
    /// keep the larger per-path count (the log stores running totals, and
    /// `load` is last-write-wins, so summing would double-count the entries
    /// this instance already has in memory — PR 1889 review), then write the
    /// merged result.
    pub(super) fn compact(&mut self) {
        // Pull in any entries other terminals appended since our load.
        if let Some(file) = &self.file
            && let Ok(lines) = std::fs::read_to_string(file)
        {
            for line in lines.lines() {
                let Ok(entry) = serde_json::from_str::<FrecencyEntry>(line) else {
                    continue;
                };
                match self.data.get_mut(&entry.path) {
                    // Running totals: keep whichever count is larger and
                    // the newer `last_open`. Summing here double-counts,
                    // because this instance's in-memory counts already
                    // include the records it wrote to the file itself.
                    Some((freq, last_open)) => {
                        if entry.frequency > *freq {
                            *freq = entry.frequency;
                        }
                        if entry.last_open > *last_open {
                            *last_open = entry.last_open;
                        }
                    }
                    None => {
                        self.data
                            .insert(entry.path, (entry.frequency, entry.last_open));
                    }
                }
            }
        }

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

/// Known text file extensions (whitelist). Files not in this list are still
/// read, but a null-byte check decides whether to treat them as binary.
pub(super) const TEXT_EXTENSIONS: &[&str] = &[
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
