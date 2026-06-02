//! Issue #122 follow-up: skill disable list.
//!
//! Tracks user-disabled skills in a single TOML file at
//! `<JCODE_HOME>/disabled_skills.toml`. Persisted across sessions.
//!
//! Format:
//! ```toml
//! disabled = ["qa-checklist", "grill-me"]
//! ```
//!
//! Used by:
//!   - `jcode skills disable <name>`  → add to list
//!   - `jcode skills enable <name>`   → remove from list
//!   - `jcode skills list`            → annotate disabled entries
//!   - `SkillRegistry::is_disabled()` → activation gate

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Debug, Default, Serialize, Deserialize)]
struct DisabledFile {
    #[serde(default)]
    disabled: BTreeSet<String>,
}

fn disabled_file_path() -> Result<PathBuf> {
    Ok(jcode_storage::jcode_dir()?.join("disabled_skills.toml"))
}

/// Load the disabled-skills set from disk. Empty when the file
/// doesn't exist; missing file is not an error.
pub fn load() -> Result<BTreeSet<String>> {
    let path = disabled_file_path()?;
    if !path.exists() {
        return Ok(BTreeSet::new());
    }
    let raw = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let parsed: DisabledFile =
        toml::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    Ok(parsed.disabled)
}

/// Persist a new disabled-skills set.
fn save(disabled: &BTreeSet<String>) -> Result<()> {
    let path = disabled_file_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create dir {}", parent.display()))?;
    }
    let file = DisabledFile {
        disabled: disabled.clone(),
    };
    let toml = toml::to_string_pretty(&file).context("serialize disabled_skills.toml")?;
    std::fs::write(&path, toml).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Add `name` to the disabled set. Returns whether the set changed.
pub fn disable(name: &str) -> Result<bool> {
    let mut set = load()?;
    let inserted = set.insert(name.to_string());
    if inserted {
        save(&set)?;
    }
    Ok(inserted)
}

/// Remove `name` from the disabled set. Returns whether the set
/// changed.
pub fn enable(name: &str) -> Result<bool> {
    let mut set = load()?;
    let removed = set.remove(name);
    if removed {
        save(&set)?;
    }
    Ok(removed)
}

/// Whether the named skill is currently disabled.
pub fn is_disabled(name: &str) -> bool {
    load().map(|s| s.contains(name)).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_isolated_home<F, T>(f: F) -> T
    where
        F: FnOnce() -> T,
    {
        let _lock = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().unwrap();
        let prev = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());
        let result = f();
        if let Some(p) = prev {
            crate::env::set_var("JCODE_HOME", p);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
        result
    }

    #[test]
    fn load_returns_empty_when_file_missing() {
        with_isolated_home(|| {
            let set = load().unwrap();
            assert!(set.is_empty());
        });
    }

    #[test]
    fn disable_adds_and_persists() {
        with_isolated_home(|| {
            assert!(disable("grill-me").unwrap());
            assert!(load().unwrap().contains("grill-me"));
            // Idempotent.
            assert!(!disable("grill-me").unwrap());
        });
    }

    #[test]
    fn enable_removes_and_persists() {
        with_isolated_home(|| {
            disable("grill-me").unwrap();
            disable("qa-checklist").unwrap();
            assert!(enable("grill-me").unwrap());
            let set = load().unwrap();
            assert!(!set.contains("grill-me"));
            assert!(set.contains("qa-checklist"));
            // Removing a missing name is a no-op.
            assert!(!enable("not-disabled").unwrap());
        });
    }

    #[test]
    fn is_disabled_reads_current_state() {
        with_isolated_home(|| {
            assert!(!is_disabled("review"));
            disable("review").unwrap();
            assert!(is_disabled("review"));
            enable("review").unwrap();
            assert!(!is_disabled("review"));
        });
    }

    #[test]
    fn parse_handles_malformed_toml_gracefully() {
        with_isolated_home(|| {
            let path = disabled_file_path().unwrap();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "[[[ not toml").unwrap();
            let res = load();
            assert!(res.is_err(), "malformed TOML should error so user can fix");
        });
    }
}
