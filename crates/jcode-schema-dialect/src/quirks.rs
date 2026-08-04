//! Persisting what a provider rejected, so it is only learned once.
//!
//! Recovering inside a turn (see [`crate::rejection`]) fixes the immediate
//! failure but costs a wasted round trip on every subsequent request, and the
//! knowledge dies with the process. Writing it to `~/.jcode/schema-quirks.json`
//! makes the second request the fast path and means the fleet self-heals in
//! front of a provider change instead of waiting for a jcode release.
//!
//! The file is advisory: a corrupt or unreadable store degrades to "learn it
//! again", never to a failure.

use crate::dialect::LearnedQuirks;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct QuirkEntry {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    rejected_keywords: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    rejected_formats: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct QuirkFile {
    #[serde(default)]
    dialects: BTreeMap<String, QuirkEntry>,
}

fn store_path() -> Option<PathBuf> {
    if let Some(override_path) = test_override() {
        return Some(override_path);
    }
    // A test that forgot to isolate itself must not read or write the real
    // store. This is not hypothetical: an earlier version of the test hook used
    // a process-global env var, and because tests run in parallel one of them
    // observed it unset and persisted a learned `minItems` rejection into the
    // developer's real `~/.jcode/schema-quirks.json`, silently stripping that
    // keyword from every OpenAI request on that machine afterwards. Failing
    // closed here makes the whole class impossible rather than relying on every
    // future test remembering to isolate.
    #[cfg(any(test, feature = "test-support"))]
    {
        None
    }
    #[cfg(not(any(test, feature = "test-support")))]
    {
        if let Ok(explicit) = std::env::var("JCODE_SCHEMA_QUIRKS_PATH") {
            return Some(PathBuf::from(explicit));
        }
        let home = if let Ok(jcode_home) = std::env::var("JCODE_HOME") {
            PathBuf::from(jcode_home)
        } else {
            dirs::home_dir()?.join(".jcode")
        };
        Some(home.join("schema-quirks.json"))
    }
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    static TEST_PATH: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

#[cfg(any(test, feature = "test-support"))]
fn test_override() -> Option<PathBuf> {
    TEST_PATH.with(|path| path.borrow().clone())
}

#[cfg(not(any(test, feature = "test-support")))]
fn test_override() -> Option<PathBuf> {
    None
}

/// Point this thread's quirk store at `path`. Tests run in parallel and the
/// store is otherwise process-global, so redirecting per thread is what keeps
/// them from racing each other (and avoids mutating process env, which is
/// `unsafe` in edition 2024).
///
/// Gated behind `test-support` as well as `cfg(test)` so integration tests,
/// which compile against the crate as an external dependency, can isolate
/// themselves too. Without that an integration test would read and write the
/// developer's real `~/.jcode/schema-quirks.json`.
#[cfg(any(test, feature = "test-support"))]
pub fn use_test_path(path: PathBuf) {
    TEST_PATH.with(|slot| *slot.borrow_mut() = Some(path));
    reset_cache_for_tests();
}

/// Cached store contents, keyed by path so that redirected stores (tests, an
/// alternate `JCODE_HOME`) never read each other's state.
fn cache() -> &'static Mutex<BTreeMap<PathBuf, QuirkFile>> {
    static CACHE: OnceLock<Mutex<BTreeMap<PathBuf, QuirkFile>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn load_file(path: &PathBuf) -> QuirkFile {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Run `f` against the cached store for the active path, persisting when it
/// reports a change.
fn with_store<T>(f: impl FnOnce(&mut QuirkFile) -> (T, bool)) -> T {
    let Some(path) = store_path() else {
        return f(&mut QuirkFile::default()).0;
    };
    let mut guard = cache().lock().unwrap_or_else(|e| e.into_inner());
    let file = guard
        .entry(path.clone())
        .or_insert_with(|| load_file(&path));
    let (result, changed) = f(file);
    if changed {
        // Best-effort persistence: an unwritable home must not break the turn
        // that just recovered.
        if let Ok(serialized) = serde_json::to_string_pretty(file) {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&path, serialized);
        }
    }
    result
}

/// Quirks learned so far for `dialect_id`.
pub fn learned_for(dialect_id: &str) -> LearnedQuirks {
    with_store(|file| {
        let learned = file
            .dialects
            .get(dialect_id)
            .map(|entry| LearnedQuirks {
                rejected_keywords: entry.rejected_keywords.clone(),
                rejected_formats: entry.rejected_formats.clone(),
            })
            .unwrap_or_default();
        (learned, false)
    })
}

/// Record a keyword `dialect_id` rejected. Returns `true` when this is new
/// information, which is the caller's signal that a retry is worth attempting.
pub fn record_keyword(dialect_id: &str, keyword: &str) -> bool {
    record(dialect_id, Some(keyword), None)
}

/// Record a `format` value `dialect_id` rejected.
pub fn record_format(dialect_id: &str, format: &str) -> bool {
    record(dialect_id, None, Some(format))
}

fn record(dialect_id: &str, keyword: Option<&str>, format: Option<&str>) -> bool {
    with_store(|file| {
        let entry = file.dialects.entry(dialect_id.to_string()).or_default();
        let mut changed = false;
        if let Some(keyword) = keyword
            && !entry.rejected_keywords.iter().any(|k| k == keyword)
        {
            entry.rejected_keywords.push(keyword.to_string());
            changed = true;
        }
        if let Some(format) = format
            && !entry.rejected_formats.iter().any(|f| f == format)
        {
            entry.rejected_formats.push(format.to_string());
            changed = true;
        }
        (changed, changed)
    })
}

/// Drop every learned quirk for a dialect. Exposed for tests and for a manual
/// reset after a provider fixes its validator.
pub fn forget(dialect_id: &str) {
    with_store(|file| {
        let removed = file.dialects.remove(dialect_id).is_some();
        ((), removed)
    })
}

/// Drop the cached copy of the *active* store so a later read re-reads the
/// file, simulating a restart. Scoped to the active path so a test running in
/// parallel cannot evict another test's (or the real store's) cache.
#[doc(hidden)]
pub fn reset_cache_for_tests() {
    let Some(path) = store_path() else {
        return;
    };
    cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learns_persists_and_forgets() {
        let dir = tempfile::tempdir().unwrap();
        use_test_path(dir.path().join("schema-quirks.json"));

        assert!(learned_for("gemini").is_empty());

        assert!(record_keyword("gemini", "propertyNames"));
        // Learning the same thing twice is not news, so no retry is triggered.
        assert!(!record_keyword("gemini", "propertyNames"));
        assert!(record_format("openai", "uri"));

        let learned = learned_for("gemini");
        assert_eq!(learned.rejected_keywords, vec!["propertyNames".to_string()]);
        assert!(learned.rejected_formats.is_empty());

        // Survives a process restart.
        reset_cache_for_tests();
        assert_eq!(
            learned_for("openai").rejected_formats,
            vec!["uri".to_string()]
        );

        forget("openai");
        reset_cache_for_tests();
        assert!(learned_for("openai").is_empty());
        assert!(!learned_for("gemini").is_empty());
    }

    /// A store that cannot be written must degrade to "learn it again", never
    /// to a failed turn.
    #[test]
    fn an_unwritable_store_still_reports_what_it_learned() {
        use_test_path(PathBuf::from("/proc/definitely-not-writable/quirks.json"));
        assert!(record_keyword("gemini", "somethingNew"));
        assert_eq!(
            learned_for("gemini").rejected_keywords,
            vec!["somethingNew".to_string()]
        );
    }
}
