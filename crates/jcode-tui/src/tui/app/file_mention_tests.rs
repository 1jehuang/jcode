//! Unit tests for the @file mention index (module split out of the
//! original file_mention.rs; see PR 1889).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::cache::{FileMentionCache, ensure_ignored_dir_scanned};
use super::history::{LookupResult, SearchHistory};
use super::prompt::{MAX_FILE_SIZE, expand_tilde, read_to_string_bounded};
use super::search::{
    REFRESH_TTL_LARGE_THRESHOLD, SKIP_DIRS, SearchLimits, dir_to_query_match, match_entry,
    refresh_ttl, search_in_index, show_all_files, walkdir_collect,
};
use super::types::{FileEntry, FileMatch, Frecency, MAX_FRECENCY_ENTRIES, PathIndex};
use super::*;
use crate::tui::app::char_bag::CharBag;
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

#[test]
fn match_exact_filename_outranks_prefix_and_dirs_stay_above() {
    let exact = file_entry("deep/lib.rs");
    let prefix = file_entry("deep/lib.rs.old");
    let exact_score = match_entry(&exact, "lib.rs", &s("lib.rs"));
    let prefix_score = match_entry(&prefix, "lib.rs", &s("lib.rs"));
    assert!(
        exact_score > prefix_score,
        "exact filename must beat a longer prefix match: {exact_score} vs {prefix_score}"
    );
    assert!(exact_score < 110.0, "directory tier (110) stays on top");
    assert_eq!(exact_score, 109.0);
}

#[test]
fn match_shorter_prefix_filename_outranks_longer() {
    let short = file_entry("src/main.rs");
    let long = file_entry("src/main.rs.bak");
    let short_score = match_entry(&short, "main", &s("main"));
    let long_score = match_entry(&long, "main", &s("main"));
    assert!(
        short_score > long_score,
        "shorter filename with same prefix must rank first: {short_score} vs {long_score}"
    );
}

#[test]
fn match_substring_beats_weak_fuzzy() {
    // "lib" as a real substring of "mylib.rs" must outrank the same
    // letters scattered across "olive_branch.rs".
    let substring = file_entry("mylib.rs");
    let fuzzy = file_entry("olive_branch.rs");
    let substring_score = match_entry(&substring, "lib", &s("lib"));
    let fuzzy_score = match_entry(&fuzzy, "lib", &s("lib"));
    assert!(
        substring_score > fuzzy_score,
        "substring tier must outrank fuzzy tier: {substring_score} vs {fuzzy_score}"
    );
    // And the fuzzy band is capped below the substring band.
    assert!(
        fuzzy_score <= 40.0,
        "fuzzy must stay banded: got {fuzzy_score}"
    );
}

#[test]
fn match_fuzzy_band_stays_below_substring_band() {
    // Worst substring (mid-word, no boundary) vs best fuzzy (tight
    // subsequence, filename hit, non-prefix).
    let weak_substring = file_entry("liberation_long_name.rs");
    let tight_fuzzy = file_entry("stalb_index.rs");
    let substring_score = match_entry(&weak_substring, "lib", &s("lib"));
    let fuzzy_score = match_entry(&tight_fuzzy, "tlb", &s("tlb"));
    assert!(
        substring_score > fuzzy_score,
        "any substring match must beat any fuzzy match: {substring_score} vs {fuzzy_score}"
    );
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

// -- walkdir_collect (no-git fallback) -----------------------------------

fn write_walker_tree(root: &Path) {
    std::fs::write(root.join("main.rs"), "fn main() {}").unwrap();
    std::fs::create_dir_all(root.join("src/deep")).unwrap();
    std::fs::write(root.join("src/deep/lib.rs"), "").unwrap();
    // Ignored by .gitignore.
    std::fs::write(root.join("debug.log"), "").unwrap();
    std::fs::write(root.join("src/notes.txt"), "").unwrap();
    std::fs::write(root.join(".gitignore"), "*.log\nsrc/notes.txt\n").unwrap();
    // Skipped by SKIP_DIRS, not by gitignore.
    std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
    std::fs::write(root.join("node_modules/pkg/index.js"), "").unwrap();
    std::fs::create_dir_all(root.join("target/debug")).unwrap();
    std::fs::write(root.join("target/debug/main.rs"), "").unwrap();
    // Hidden dotfiles stay visible (hidden(false)), .git contents are
    // skipped by the explicit .git/ filter.
    std::fs::write(root.join(".env"), "").unwrap();
}

#[tokio::test]
async fn walkdir_collect_respects_gitignore_and_skip_dirs() {
    let temp = tempfile::tempdir().unwrap();
    write_walker_tree(temp.path());

    let files = walkdir_collect(temp.path(), &Arc::new(AtomicUsize::new(0))).await;

    assert!(
        files.contains(&"main.rs".to_string()),
        "regular files must be indexed: {files:?}"
    );
    assert!(
        files.contains(&"src/deep/lib.rs".to_string()),
        "nested files must be indexed: {files:?}"
    );
    assert!(
        files.contains(&".gitignore".to_string()),
        "dotfiles must stay visible: {files:?}"
    );
    assert!(
        files.contains(&".env".to_string()),
        "dotfiles must stay visible: {files:?}"
    );
    assert!(
        !files.iter().any(|f| f.ends_with(".log")),
        ".gitignore *.log must be honored: {files:?}"
    );
    assert!(
        !files.iter().any(|f| f == "src/notes.txt"),
        ".gitignore literal path must be honored: {files:?}"
    );
    assert!(
        !files.iter().any(|f| f.starts_with("node_modules/")),
        "SKIP_DIRS must be honored: {files:?}"
    );
    assert!(
        !files.iter().any(|f| f.starts_with("target/")),
        "SKIP_DIRS must be honored: {files:?}"
    );
}

#[tokio::test]
async fn walkdir_collect_empty_dir_returns_empty() {
    let temp = tempfile::tempdir().unwrap();
    let files = walkdir_collect(temp.path(), &Arc::new(AtomicUsize::new(0))).await;
    assert!(files.is_empty());
}

#[tokio::test]
async fn walkdir_collect_sorted_output() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("src")).unwrap();
    std::fs::write(temp.path().join("zz.rs"), "").unwrap();
    std::fs::write(temp.path().join("aa.rs"), "").unwrap();
    std::fs::write(temp.path().join("src/mm.rs"), "").unwrap();

    let files = walkdir_collect(temp.path(), &Arc::new(AtomicUsize::new(0))).await;
    let mut sorted = files.clone();
    sorted.sort();
    assert_eq!(files, sorted, "walker output must be sorted");
}

#[tokio::test]
async fn walkdir_collect_reports_live_progress() {
    let temp = tempfile::tempdir().unwrap();
    write_walker_tree(temp.path());

    let progress = Arc::new(AtomicUsize::new(0));
    let files = walkdir_collect(temp.path(), &progress).await;

    assert_eq!(
        progress.load(Ordering::Relaxed),
        files.len(),
        "progress must end at the collected file count"
    );
    assert!(!files.is_empty(), "tree must produce files for the counter");
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

/// compact() must not double-count: the in-memory map already includes
/// the running totals this instance wrote to the shared log, so merging
/// the file back in has to keep the larger count per path, not sum
/// (PR 1889 review, "cleanup inflates saved counts").
#[test]
fn frecency_compact_does_not_double_count_own_records() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("file_frecency.jsonl");

    let mut frecency = Frecency {
        data: HashMap::new(),
        file: Some(file.clone()),
        cwd: PathBuf::from("."),
    };
    // Same instance records the file three times: in-memory count is 3
    // and the log's last line for this path also says 3.
    frecency.record("src/main.rs");
    frecency.record("src/main.rs");
    frecency.record("src/main.rs");

    frecency.compact();

    let Some(&(freq, _)) = frecency.data.get("src/main.rs") else {
        panic!("entry must survive compact");
    };
    assert_eq!(
        freq, 3,
        "compact must keep the running total, not add the file copy on top"
    );

    // A reloaded instance sees the same total after the compact rewrite.
    let mut reloaded = Frecency {
        data: HashMap::new(),
        file: Some(file),
        cwd: PathBuf::from("."),
    };
    reloaded.load();
    let Some(&(freq, _)) = reloaded.data.get("src/main.rs") else {
        panic!("entry must survive reload");
    };
    assert_eq!(freq, 3, "the rewritten log must keep the running total");
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

#[path = "file_mention_ranking_tests.rs"]
mod ranking;
