//! Rank the repositories a user works in most, from the working directories
//! recorded across their agent sessions (jcode + imported Claude/Codex/etc.).
//!
//! The motivating use case is one-time keybinding setup: during auto-import we
//! want to guess a user's top project directories so we can offer to bind global
//! launch hotkeys (e.g. `Cmd+[`, `Cmd+]`) to "open jcode here". The ranking is a
//! pure function over `(working_dir, last_used)` observations so it can be unit
//! tested without touching the filesystem, and a thin [`resolve_git_root`]
//! helper folds subdirectories into their repository root.
//!
//! Ranking weights raw frequency by recency: a repo touched heavily last week
//! beats one touched a year ago. The score uses an exponential recency decay so
//! the result reflects where the user *currently* works, not their lifetime
//! history.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};

/// One observed session location: the working directory it ran in, and when it
/// was last active. `last_used` is optional because some imported sources lack a
/// reliable timestamp; those sessions still contribute frequency, just with the
/// most-decayed recency weight.
#[derive(Debug, Clone)]
pub struct SessionLocation {
    pub working_dir: String,
    pub last_used: Option<DateTime<Utc>>,
}

impl SessionLocation {
    pub fn new(working_dir: impl Into<String>, last_used: Option<DateTime<Utc>>) -> Self {
        Self {
            working_dir: working_dir.into(),
            last_used,
        }
    }
}

/// A ranked repository candidate produced by [`rank_repositories`].
#[derive(Debug, Clone, PartialEq)]
pub struct RankedRepo {
    /// Absolute path to the repository root (or the raw working dir when no git
    /// root could be resolved).
    pub path: String,
    /// Number of sessions that mapped to this repo (after folding subdirs).
    pub session_count: usize,
    /// Recency-weighted score used for ordering. Higher is "more active".
    pub score: f64,
    /// Most recent session timestamp seen for this repo, if any.
    pub last_used: Option<DateTime<Utc>>,
}

/// Tunables for [`rank_repositories`]. Defaults are chosen for the keybinding
/// use case (a handful of top repos, recency-biased).
#[derive(Debug, Clone)]
pub struct RankOptions {
    /// Recency half-life: a session this many days old contributes half the
    /// weight of a brand-new one. Sessions without a timestamp are treated as
    /// `floor_weight`.
    pub half_life_days: f64,
    /// Minimum recency weight for a session (also used for timestamp-less
    /// sessions) so old-but-frequent repos still register.
    pub floor_weight: f64,
    /// Paths to exclude entirely (exact match after normalization). Typically
    /// the user's home directory, which is noise from launching jcode at `$HOME`.
    pub excluded_paths: Vec<PathBuf>,
    /// When true, only keep candidates whose resolved path is an actual git
    /// root (i.e. [`resolve_git_root`] found a `.git`). Raw, non-repo working
    /// dirs are dropped. The resolver is injected via `rank_repositories_with`.
    pub require_git_root: bool,
}

impl Default for RankOptions {
    fn default() -> Self {
        Self {
            half_life_days: 21.0,
            floor_weight: 0.05,
            excluded_paths: Vec::new(),
            require_git_root: true,
        }
    }
}

/// Resolve `dir` to the root of the git repository that contains it by walking
/// upward until a `.git` entry is found. Returns `None` if no ancestor contains
/// `.git` (the directory is not inside a repository).
///
/// This touches the filesystem; the ranking core takes the resolver as a
/// parameter so it stays pure and testable.
pub fn resolve_git_root(dir: &Path) -> Option<PathBuf> {
    let mut cur = Some(dir);
    while let Some(p) = cur {
        if p.join(".git").exists() {
            return Some(p.to_path_buf());
        }
        cur = p.parent();
    }
    None
}

/// Recency weight for a single session given the reference "now".
fn recency_weight(last_used: Option<DateTime<Utc>>, now: DateTime<Utc>, opts: &RankOptions) -> f64 {
    let Some(ts) = last_used else {
        return opts.floor_weight;
    };
    let age_days = (now - ts).num_seconds().max(0) as f64 / 86_400.0;
    // Exponential decay: weight = 0.5 ^ (age / half_life).
    let decayed = 0.5_f64.powf(age_days / opts.half_life_days.max(0.000_1));
    decayed.max(opts.floor_weight)
}

/// Rank repositories from session locations, resolving each working dir to its
/// repository root via `resolve_root`. `now` anchors the recency decay (pass
/// `Utc::now()` in production; a fixed value in tests).
///
/// `resolve_root` returns the repo root for a working dir, or `None` when the
/// dir is not inside a repository. When `opts.require_git_root` is set, those
/// `None` results are dropped; otherwise the raw working dir is used as its own
/// "repo" so non-git project folders still rank.
pub fn rank_repositories_with<F>(
    locations: &[SessionLocation],
    now: DateTime<Utc>,
    opts: &RankOptions,
    mut resolve_root: F,
) -> Vec<RankedRepo>
where
    F: FnMut(&Path) -> Option<PathBuf>,
{
    struct Acc {
        count: usize,
        score: f64,
        last_used: Option<DateTime<Utc>>,
    }
    let excluded: Vec<PathBuf> = opts
        .excluded_paths
        .iter()
        .map(|p| normalize_path(p))
        .collect();

    // Cache resolver results per distinct working dir so we do not re-stat the
    // same directory once per session.
    let mut resolved_cache: HashMap<String, Option<PathBuf>> = HashMap::new();
    let mut acc: HashMap<PathBuf, Acc> = HashMap::new();

    for loc in locations {
        let wd = loc.working_dir.trim();
        if wd.is_empty() {
            continue;
        }
        let raw = PathBuf::from(wd);
        let root = resolved_cache
            .entry(wd.to_string())
            .or_insert_with(|| resolve_root(&raw))
            .clone();
        let repo = match root {
            Some(r) => normalize_path(&r),
            None => {
                if opts.require_git_root {
                    continue;
                }
                normalize_path(&raw)
            }
        };
        if excluded.iter().any(|e| e == &repo) {
            continue;
        }
        let weight = recency_weight(loc.last_used, now, opts);
        let entry = acc.entry(repo).or_insert(Acc {
            count: 0,
            score: 0.0,
            last_used: None,
        });
        entry.count += 1;
        entry.score += weight;
        entry.last_used = match (entry.last_used, loc.last_used) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) => Some(a),
            (None, b) => b,
        };
    }

    let mut ranked: Vec<RankedRepo> = acc
        .into_iter()
        .map(|(path, a)| RankedRepo {
            path: path.to_string_lossy().into_owned(),
            session_count: a.count,
            score: a.score,
            last_used: a.last_used,
        })
        .collect();

    ranked.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            // Tie-break: more sessions, then most recently used, then path for
            // deterministic ordering.
            .then(b.session_count.cmp(&a.session_count))
            .then(b.last_used.cmp(&a.last_used))
            .then(a.path.cmp(&b.path))
    });
    ranked
}

/// Convenience wrapper that resolves git roots from the real filesystem via
/// [`resolve_git_root`]. Subdirectories of the same repo are folded together.
pub fn rank_repositories(
    locations: &[SessionLocation],
    now: DateTime<Utc>,
    opts: &RankOptions,
) -> Vec<RankedRepo> {
    rank_repositories_with(locations, now, opts, resolve_git_root)
}

/// Normalize a path for comparison: strip a single trailing slash and collapse
/// the macOS `/private` symlink prefix that `std::env::current_dir` sometimes
/// reports for `/tmp` and `/var`, so the same repo seen under both spellings
/// folds together.
fn normalize_path(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    let trimmed = s.trim_end_matches('/');
    let trimmed = if trimmed.is_empty() { "/" } else { trimmed };
    if let Some(rest) = trimmed.strip_prefix("/private/") {
        PathBuf::from(format!("/{rest}"))
    } else {
        PathBuf::from(trimmed)
    }
}

/// Helper for callers that have a [`Duration`]-based half-life preference.
pub fn half_life_from_duration(d: Duration) -> f64 {
    d.num_seconds() as f64 / 86_400.0
}

/// A planned global launch hotkey: a chord plus the directory it should open
/// jcode in. Produced by [`build_launch_hotkey_plan`] from a ranking, then
/// persisted to config so the mapping is baked once and does not move around.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedHotkey {
    /// jcode-style chord string, e.g. `cmd+;` or `cmd+[`.
    pub chord: String,
    /// Absolute directory the hotkey opens jcode in.
    pub dir: String,
    /// Short human label (usually the repo's directory name) for notices.
    pub label: String,
}

/// The default chord assigned to each launch-hotkey slot, in slot order.
///
/// Slot meaning (see [`build_launch_hotkey_plan`]):
/// 0 = top repo, 1 = home, 2 = repo #2, 3 = repo #3, 4 = repo #4.
pub const DEFAULT_LAUNCH_HOTKEY_CHORDS: [&str; 5] = ["cmd+;", "cmd+'", "cmd+[", "cmd+]", "cmd+\\"];

/// Build the default launch-hotkey plan from a ranking and the user's home dir.
///
/// The layout follows the product spec: the most-active repo gets `Cmd+;`, home
/// gets `Cmd+'`, and the next three repos get `Cmd+[`, `Cmd+]`, `Cmd+\`. `home`
/// is always slot 1 even if it also appears in the ranking, and it is skipped
/// from the repo slots so we never bind two chords to the same directory.
///
/// `chords` lets callers override the default chord sequence (e.g. from config);
/// pass [`DEFAULT_LAUNCH_HOTKEY_CHORDS`] for the standard layout. Only as many
/// hotkeys as there are available chords and repos are produced.
pub fn build_launch_hotkey_plan(
    home: &Path,
    ranked: &[RankedRepo],
    chords: &[&str],
) -> Vec<PlannedHotkey> {
    let home_norm = normalize_path(home);
    // Top repos, excluding home itself, in rank order.
    let repos: Vec<&RankedRepo> = ranked
        .iter()
        .filter(|r| normalize_path(Path::new(&r.path)) != home_norm)
        .collect();

    // Slot order interleaves the top repo, then home, then the remaining repos.
    // dirs[i] is the directory for chord slot i.
    let mut dirs: Vec<(String, String)> = Vec::new();
    if let Some(top) = repos.first() {
        dirs.push((top.path.clone(), dir_label(&top.path)));
    }
    dirs.push((home.to_string_lossy().into_owned(), "home".to_string()));
    for repo in repos.iter().skip(1) {
        dirs.push((repo.path.clone(), dir_label(&repo.path)));
    }

    chords
        .iter()
        .zip(dirs)
        .map(|(chord, (dir, label))| PlannedHotkey {
            chord: (*chord).to_string(),
            dir,
            label,
        })
        .collect()
}

/// Final path component as a short label, falling back to the full path.
fn dir_label(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| path.to_string())
}

/// Scan a jcode sessions directory and extract one [`SessionLocation`] per
/// session file that records a `working_dir`.
///
/// This is deliberately lightweight: it reads each `*.json` (skipping `.bak`
/// siblings), pulls just the `working_dir` string and uses the file's mtime as
/// the recency timestamp, and skips anything it cannot parse. Returns an empty
/// vec if the directory is missing. The function is filesystem-facing but kept
/// here so callers get session collection + ranking from one module.
pub fn collect_jcode_session_locations(sessions_dir: &Path) -> Vec<SessionLocation> {
    use std::fs;

    let Ok(entries) = fs::read_dir(sessions_dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !name.ends_with(".json") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let Some(wd) = value.get("working_dir").and_then(|v| v.as_str()) else {
            continue;
        };
        if wd.trim().is_empty() {
            continue;
        }
        let last_used = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|d| DateTime::from_timestamp(d.as_secs() as i64, 0));
        out.push(SessionLocation::new(wd, last_used));
    }
    out
}

/// Compute the baked launch-hotkey plan from a jcode sessions directory.
///
/// Convenience wrapper that scans `sessions_dir`, ranks the repos (excluding
/// `home`, requiring real git roots), and assigns the default chord layout. Pass
/// `now` for the recency anchor. Returns the planned chord -> directory entries
/// (empty if there is nothing to bind beyond home, which still gets `Cmd+;`).
pub fn plan_launch_hotkeys_from_sessions(
    sessions_dir: &Path,
    home: &Path,
    now: DateTime<Utc>,
) -> Vec<PlannedHotkey> {
    let locations = collect_jcode_session_locations(sessions_dir);
    let opts = RankOptions {
        excluded_paths: vec![home.to_path_buf()],
        ..RankOptions::default()
    };
    let ranked = rank_repositories(&locations, now, &opts);
    build_launch_hotkey_plan(home, &ranked, &DEFAULT_LAUNCH_HOTKEY_CHORDS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(y: i32, m: u32, d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, 12, 0, 0).unwrap()
    }

    /// Resolver that pretends every path under one of `roots` belongs to that
    /// root, mimicking git-root folding without touching disk.
    fn fake_resolver(roots: &'static [&'static str]) -> impl FnMut(&Path) -> Option<PathBuf> {
        move |p: &Path| {
            let s = p.to_string_lossy().to_string();
            roots
                .iter()
                .filter(|r| s == **r || s.starts_with(&format!("{r}/")))
                .max_by_key(|r| r.len())
                .map(|r| PathBuf::from(*r))
        }
    }

    #[test]
    fn folds_subdirectories_into_repo_root() {
        let now = ts(2026, 6, 25);
        let locs = vec![
            SessionLocation::new("/u/jeremy/proj", Some(ts(2026, 6, 24))),
            SessionLocation::new("/u/jeremy/proj/crates/a", Some(ts(2026, 6, 24))),
            SessionLocation::new("/u/jeremy/proj/crates/b", Some(ts(2026, 6, 23))),
        ];
        let ranked = rank_repositories_with(
            &locs,
            now,
            &RankOptions::default(),
            fake_resolver(&["/u/jeremy/proj"]),
        );
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].path, "/u/jeremy/proj");
        assert_eq!(ranked[0].session_count, 3);
    }

    #[test]
    fn recency_outranks_raw_frequency() {
        let now = ts(2026, 6, 25);
        // `old` has many sessions but all a year old; `fresh` has fewer but
        // recent. With a 21-day half-life, fresh should win.
        let mut locs = vec![];
        for _ in 0..20 {
            locs.push(SessionLocation::new("/repo/old", Some(ts(2025, 6, 25))));
        }
        for _ in 0..4 {
            locs.push(SessionLocation::new("/repo/fresh", Some(ts(2026, 6, 24))));
        }
        let ranked = rank_repositories_with(
            &locs,
            now,
            &RankOptions::default(),
            fake_resolver(&["/repo/old", "/repo/fresh"]),
        );
        assert_eq!(ranked[0].path, "/repo/fresh");
        assert_eq!(ranked[1].path, "/repo/old");
    }

    #[test]
    fn excluded_paths_are_dropped() {
        let now = ts(2026, 6, 25);
        let locs = vec![
            SessionLocation::new("/home/jeremy", Some(ts(2026, 6, 24))),
            SessionLocation::new("/home/jeremy/work", Some(ts(2026, 6, 24))),
        ];
        let opts = RankOptions {
            excluded_paths: vec![PathBuf::from("/home/jeremy")],
            ..RankOptions::default()
        };
        let ranked = rank_repositories_with(
            &locs,
            now,
            &opts,
            fake_resolver(&["/home/jeremy", "/home/jeremy/work"]),
        );
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].path, "/home/jeremy/work");
    }

    #[test]
    fn non_git_dirs_dropped_when_required() {
        let now = ts(2026, 6, 25);
        let locs = vec![
            SessionLocation::new("/repo/a", Some(ts(2026, 6, 24))),
            SessionLocation::new("/not/a/repo", Some(ts(2026, 6, 24))),
        ];
        let ranked = rank_repositories_with(
            &locs,
            now,
            &RankOptions::default(),
            fake_resolver(&["/repo/a"]),
        );
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].path, "/repo/a");
    }

    #[test]
    fn non_git_dirs_kept_when_not_required() {
        let now = ts(2026, 6, 25);
        let locs = vec![SessionLocation::new("/not/a/repo", Some(ts(2026, 6, 24)))];
        let opts = RankOptions {
            require_git_root: false,
            ..RankOptions::default()
        };
        let ranked = rank_repositories_with(&locs, now, &opts, |_| None);
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].path, "/not/a/repo");
    }

    #[test]
    fn private_prefix_folds_with_plain_path() {
        let now = ts(2026, 6, 25);
        let locs = vec![
            SessionLocation::new("/private/tmp/x", Some(ts(2026, 6, 24))),
            SessionLocation::new("/tmp/x", Some(ts(2026, 6, 24))),
        ];
        let opts = RankOptions {
            require_git_root: false,
            ..RankOptions::default()
        };
        let ranked = rank_repositories_with(&locs, now, &opts, |_| None);
        assert_eq!(ranked.len(), 1, "private/ and plain path should fold");
        assert_eq!(ranked[0].path, "/tmp/x");
        assert_eq!(ranked[0].session_count, 2);
    }

    #[test]
    fn timestampless_sessions_still_count_with_floor_weight() {
        let now = ts(2026, 6, 25);
        let locs = vec![
            SessionLocation::new("/repo/a", None),
            SessionLocation::new("/repo/a", None),
        ];
        let opts = RankOptions {
            require_git_root: false,
            ..RankOptions::default()
        };
        let ranked = rank_repositories_with(&locs, now, &opts, |_| None);
        assert_eq!(ranked[0].session_count, 2);
        assert!(ranked[0].score > 0.0);
    }

    #[test]
    fn empty_working_dirs_are_ignored() {
        let now = ts(2026, 6, 25);
        let locs = vec![
            SessionLocation::new("", Some(ts(2026, 6, 24))),
            SessionLocation::new("   ", Some(ts(2026, 6, 24))),
        ];
        let ranked = rank_repositories_with(&locs, now, &RankOptions::default(), |_| {
            Some(PathBuf::from("/x"))
        });
        assert!(ranked.is_empty());
    }

    fn repo(path: &str, score: f64) -> RankedRepo {
        RankedRepo {
            path: path.to_string(),
            session_count: 1,
            score,
            last_used: None,
        }
    }

    #[test]
    fn plan_assigns_top_repo_home_then_next_repos() {
        let ranked = vec![
            repo("/u/jeremy/jcode", 600.0),
            repo("/u/jeremy/scrollwm", 100.0),
            repo("/u/jeremy/sideproj", 50.0),
            repo("/u/jeremy/fourth", 10.0),
            repo("/u/jeremy/fifth", 5.0),
        ];
        let plan = build_launch_hotkey_plan(
            Path::new("/u/jeremy"),
            &ranked,
            &DEFAULT_LAUNCH_HOTKEY_CHORDS,
        );
        // Slots: top, home, #2, #3, #4 -> 5 chords total.
        assert_eq!(plan.len(), 5);
        assert_eq!(plan[0].chord, "cmd+;");
        assert_eq!(plan[0].dir, "/u/jeremy/jcode");
        assert_eq!(plan[1].chord, "cmd+'");
        assert_eq!(plan[1].dir, "/u/jeremy");
        assert_eq!(plan[1].label, "home");
        assert_eq!(plan[2].chord, "cmd+[");
        assert_eq!(plan[2].dir, "/u/jeremy/scrollwm");
        assert_eq!(plan[3].chord, "cmd+]");
        assert_eq!(plan[3].dir, "/u/jeremy/sideproj");
        assert_eq!(plan[4].chord, "cmd+\\");
        assert_eq!(plan[4].dir, "/u/jeremy/fourth");
    }

    #[test]
    fn plan_skips_home_if_it_appears_in_ranking() {
        let ranked = vec![
            repo("/u/jeremy", 999.0), // home ranked #1, should not take a repo slot
            repo("/u/jeremy/jcode", 600.0),
        ];
        let plan = build_launch_hotkey_plan(
            Path::new("/u/jeremy"),
            &ranked,
            &DEFAULT_LAUNCH_HOTKEY_CHORDS,
        );
        // Top repo slot is jcode (home filtered out), then home gets cmd+'.
        assert_eq!(plan[0].dir, "/u/jeremy/jcode");
        assert_eq!(plan[1].dir, "/u/jeremy");
        // No duplicate dir bound to two chords.
        let mut dirs: Vec<&str> = plan.iter().map(|p| p.dir.as_str()).collect();
        dirs.sort();
        dirs.dedup();
        assert_eq!(dirs.len(), plan.len());
    }

    #[test]
    fn plan_truncates_to_available_repos() {
        let ranked = vec![repo("/u/jeremy/only", 600.0)];
        let plan = build_launch_hotkey_plan(
            Path::new("/u/jeremy"),
            &ranked,
            &DEFAULT_LAUNCH_HOTKEY_CHORDS,
        );
        // top repo + home only.
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].dir, "/u/jeremy/only");
        assert_eq!(plan[1].dir, "/u/jeremy");
    }

    #[test]
    fn plan_with_no_repos_still_binds_home() {
        let plan =
            build_launch_hotkey_plan(Path::new("/u/jeremy"), &[], &DEFAULT_LAUNCH_HOTKEY_CHORDS);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].chord, "cmd+;");
        assert_eq!(plan[0].dir, "/u/jeremy");
        assert_eq!(plan[0].label, "home");
    }

    #[test]
    fn collect_reads_working_dirs_and_skips_junk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let write = |name: &str, body: &str| {
            std::fs::write(dir.path().join(name), body).unwrap();
        };
        write(
            "session_a.json",
            r#"{"working_dir":"/Users/jeremy/jcode-github","id":"a"}"#,
        );
        write(
            "session_b.json",
            r#"{"working_dir":"/Users/jeremy/scrollwm","id":"b"}"#,
        );
        // No working_dir -> skipped.
        write("session_c.json", r#"{"id":"c"}"#);
        // .bak sibling -> skipped (not .json suffix match).
        write("session_a.bak", r#"{"working_dir":"/should/not/count"}"#);
        // Not JSON -> skipped.
        write("notes.json", "this is not json");

        let mut locs = collect_jcode_session_locations(dir.path());
        locs.sort_by(|a, b| a.working_dir.cmp(&b.working_dir));
        let dirs: Vec<&str> = locs.iter().map(|l| l.working_dir.as_str()).collect();
        assert_eq!(
            dirs,
            vec!["/Users/jeremy/jcode-github", "/Users/jeremy/scrollwm"]
        );
        // mtime-derived timestamp is populated.
        assert!(locs.iter().all(|l| l.last_used.is_some()));
    }

    #[test]
    fn collect_missing_dir_is_empty() {
        let locs = collect_jcode_session_locations(Path::new("/nonexistent/jcode/sessions"));
        assert!(locs.is_empty());
    }
}
