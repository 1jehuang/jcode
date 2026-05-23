//! Self-dev customization records (#42).
//!
//! When the self-dev agent (or a user manually) modifies jcode source,
//! capture a structured record of the change in `~/.jcode/customizations/`.
//! Foundation for #43 (intent + provenance + rationale) and #44 (update
//! flow integration).
//!
//! ## Schema
//!
//! Each record is one JSON file at
//! `~/.jcode/customizations/<id>.json`:
//!
//! ```json
//! {
//!   "schema_version": 1,
//!   "id": "cust_<unix_ts>_<short_hash>",
//!   "timestamp_unix": 1779504807,
//!   "timestamp_iso": "2026-05-23T18:15:00Z",
//!   "files": ["src/foo.rs"],
//!   "diff_hash": "sha256:abc123...",
//!   "intent": "human-readable reason",
//!   "session_id": "sess_xyz_abc",
//!   "version_at_record": "v0.12.97-dev",
//!   "git_hash_at_record": "65e18d9b"
//! }
//! ```
//!
//! `schema_version: 1` is the current contract; readers should accept any
//! version they were built against. Future fields (provenance #43,
//! validation #43, status #44) are additive.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Current schema version. Bump when introducing breaking changes.
pub const SCHEMA_VERSION: u32 = 1;

/// One customization event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomizationRecord {
    /// Schema version this record was written against.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,

    /// Stable identifier of the form `cust_<unix_ts>_<short_hash>`.
    pub id: String,

    /// Unix epoch seconds when the record was created.
    pub timestamp_unix: u64,

    /// ISO-8601 timestamp (UTC) when the record was created.
    pub timestamp_iso: String,

    /// Files touched by this customization (relative to repo root or
    /// absolute, caller's choice; we don't normalize here).
    pub files: Vec<String>,

    /// SHA-256 hash of the unified diff for this customization. Lets the
    /// update-flow integration (#44) detect whether the customization
    /// still applies cleanly to a new release.
    pub diff_hash: String,

    /// Human-readable reason. May be agent-generated or user-typed.
    pub intent: String,

    /// jcode session id this customization originated from, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,

    /// jcode version at record time (e.g. `v0.12.97-dev`).
    pub version_at_record: String,

    /// Git hash at record time.
    pub git_hash_at_record: String,
}

fn default_schema_version() -> u32 {
    1
}

fn customizations_dir() -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?.join("customizations"))
}

/// Compute the canonical id for a record. Derives a short stable hash from
/// the unified diff so the same customization recorded twice yields the
/// same id (useful for dedup in the update flow).
fn compute_id(timestamp_unix: u64, diff_hash: &str) -> String {
    // diff_hash is already a sha256 — just take its first 12 hex chars.
    let short = diff_hash
        .strip_prefix("sha256:")
        .unwrap_or(diff_hash)
        .chars()
        .take(12)
        .collect::<String>();
    format!("cust_{timestamp_unix}_{short}")
}

/// Hash a unified diff string into `sha256:<64-hex>`.
pub fn hash_diff(diff: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(diff.as_bytes());
    let bytes = hasher.finalize();
    let mut hex = String::with_capacity(7 + 64);
    hex.push_str("sha256:");
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(hex, "{:02x}", byte);
    }
    hex
}

/// Record a new customization. Writes the JSON file under
/// `~/.jcode/customizations/<id>.json` and returns the assigned id.
pub fn record(
    intent: impl Into<String>,
    files: Vec<String>,
    diff: &str,
    session_id: Option<&str>,
) -> Result<String> {
    let dir = customizations_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let diff_hash = hash_diff(diff);
    let id = compute_id(now, &diff_hash);

    let record = CustomizationRecord {
        schema_version: SCHEMA_VERSION,
        id: id.clone(),
        timestamp_unix: now,
        timestamp_iso: chrono::Utc::now().to_rfc3339(),
        files,
        diff_hash,
        intent: intent.into(),
        session_id: session_id.map(ToOwned::to_owned),
        version_at_record: env!("JCODE_VERSION").to_string(),
        git_hash_at_record: env!("JCODE_GIT_HASH").to_string(),
    };

    let path = dir.join(format!("{id}.json"));
    let body = serde_json::to_string_pretty(&record).context("serialize CustomizationRecord")?;
    std::fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
    Ok(id)
}

/// Load a single record by id. Returns `Ok(None)` if not found.
pub fn get(id: &str) -> Result<Option<CustomizationRecord>> {
    let dir = customizations_dir()?;
    let path = dir.join(format!("{id}.json"));
    if !path.exists() {
        return Ok(None);
    }
    let body =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let record: CustomizationRecord =
        serde_json::from_str(&body).with_context(|| format!("parse {}", path.display()))?;
    Ok(Some(record))
}

/// List all records, newest first by timestamp.
pub fn list() -> Result<Vec<CustomizationRecord>> {
    let dir = customizations_dir()?;
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut records: Vec<CustomizationRecord> = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if !name.starts_with("cust_") || !name.ends_with(".json") {
            continue;
        }
        let body = match std::fs::read_to_string(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        match serde_json::from_str::<CustomizationRecord>(&body) {
            Ok(record) => records.push(record),
            Err(e) => {
                crate::logging::warn(&format!(
                    "customization: skipping unparseable record {}: {}",
                    path.display(),
                    e
                ));
            }
        }
    }
    records.sort_by(|a, b| b.timestamp_unix.cmp(&a.timestamp_unix));
    Ok(records)
}

/// Remove a record by id. Returns `Ok(true)` if the file existed and was
/// removed, `Ok(false)` if there was no record with that id.
pub fn remove(id: &str) -> Result<bool> {
    let dir = customizations_dir()?;
    let path = dir.join(format!("{id}.json"));
    if !path.exists() {
        return Ok(false);
    }
    std::fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_temp_home<F: FnOnce()>(f: F) {
        let _lock = crate::storage::lock_test_env();
        let prev = std::env::var_os("JCODE_HOME");
        let temp = tempfile::TempDir::new().unwrap();
        crate::env::set_var("JCODE_HOME", temp.path());
        f();
        if let Some(p) = prev {
            crate::env::set_var("JCODE_HOME", p);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[test]
    fn record_then_get_round_trips() {
        with_temp_home(|| {
            let id = record(
                "fix tab handling in editor",
                vec!["src/tui/app/input.rs".to_string()],
                "diff --git a/x b/x\n--- a/x\n+++ b/x\n",
                Some("sess_test"),
            )
            .expect("record");

            assert!(id.starts_with("cust_"));

            let loaded = get(&id).expect("load").expect("present");
            assert_eq!(loaded.id, id);
            assert_eq!(loaded.intent, "fix tab handling in editor");
            assert_eq!(loaded.files, vec!["src/tui/app/input.rs"]);
            assert_eq!(loaded.session_id.as_deref(), Some("sess_test"));
            assert!(loaded.diff_hash.starts_with("sha256:"));
            assert_eq!(loaded.schema_version, SCHEMA_VERSION);
        });
    }

    #[test]
    fn get_returns_none_for_unknown_id() {
        with_temp_home(|| {
            assert!(get("cust_does_not_exist").unwrap().is_none());
        });
    }

    #[test]
    fn list_returns_records_newest_first() {
        with_temp_home(|| {
            // Record three with explicit sleep so timestamps differ.
            record("first", vec![], "d1", None).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(1100));
            record("second", vec![], "d2", None).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(1100));
            record("third", vec![], "d3", None).unwrap();

            let records = list().unwrap();
            assert_eq!(records.len(), 3);
            assert_eq!(records[0].intent, "third");
            assert_eq!(records[1].intent, "second");
            assert_eq!(records[2].intent, "first");
        });
    }

    #[test]
    fn remove_deletes_existing_record_and_reports_missing() {
        with_temp_home(|| {
            let id = record("temp", vec![], "diff", None).unwrap();
            assert!(remove(&id).unwrap(), "first remove returns true");
            assert!(get(&id).unwrap().is_none());
            assert!(!remove(&id).unwrap(), "second remove returns false");
        });
    }

    #[test]
    fn hash_diff_is_stable_and_includes_prefix() {
        let h1 = hash_diff("hello");
        let h2 = hash_diff("hello");
        let h3 = hash_diff("world");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
        assert!(h1.starts_with("sha256:"));
        // 64 hex chars + "sha256:"
        assert_eq!(h1.len(), 7 + 64);
    }

    #[test]
    fn list_skips_unparseable_records_without_failing() {
        with_temp_home(|| {
            // Write a valid record + a malformed one in the dir.
            record("good", vec![], "d", None).unwrap();
            let dir = customizations_dir().unwrap();
            let bad_path = dir.join("cust_garbage.json");
            std::fs::write(&bad_path, "not valid json").unwrap();

            let records = list().expect("list still ok");
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].intent, "good");
        });
    }
}
