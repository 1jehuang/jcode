//! Background cleanup for `~/.jcode/scratch`.
//!
//! The bash tool points `TMPDIR` and `JCODE_SCRATCH_DIR` at this directory and
//! tells agents to put large temporary files there. Agents routinely clone
//! repositories or set `CARGO_TARGET_DIR` under it for isolated builds, and a
//! single cold Rust `target/` directory can be tens of gigabytes. Nothing ever
//! removed those, so long-lived installs accumulated hundreds of gigabytes.
//!
//! This module removes top-level scratch entries whose newest modification
//! (searched recursively) is older than a retention window, skipping anything
//! that a live process is currently using as its working directory.

use crate::storage;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Entries with no file modified within this many days are removed.
pub const DEFAULT_SCRATCH_RETENTION_DAYS: u64 = 7;

/// Override the retention window. `0` disables scratch cleanup entirely.
pub const SCRATCH_RETENTION_ENV: &str = "JCODE_SCRATCH_RETENTION_DAYS";

/// Minimum interval between prune passes across all jcode processes.
const PRUNE_INTERVAL_SECS: u64 = 24 * 60 * 60;

const PRUNE_MARKER: &str = "scratch-prune.stamp";

/// Summary of a prune pass, mostly for logging and tests.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScratchPruneReport {
    pub removed: Vec<PathBuf>,
    pub kept: usize,
}

fn retention_from_env() -> Option<Duration> {
    let days = std::env::var(SCRATCH_RETENTION_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_SCRATCH_RETENTION_DAYS);
    (days > 0).then(|| Duration::from_secs(days * 24 * 60 * 60))
}

/// Remove stale entries from `~/.jcode/scratch`.
///
/// Best-effort and rate limited to one pass per day per machine, so it is safe
/// to call from a background thread on every startup. Only the default scratch
/// directory is cleaned. A custom `JCODE_SCRATCH_DIR` is never touched.
pub fn prune_stale_scratch() {
    let Some(retention) = retention_from_env() else {
        return;
    };
    let Ok(base) = storage::jcode_dir() else {
        return;
    };
    let scratch = base.join("scratch");
    if !scratch.is_dir() || !claim_prune_slot(&base) {
        return;
    }
    // /proc cwd links are canonical, so compare against the canonical path.
    let scratch = std::fs::canonicalize(&scratch).unwrap_or(scratch);
    let busy = busy_process_dirs();
    let report = prune_stale_scratch_in(&scratch, SystemTime::now(), retention, &busy);
    if !report.removed.is_empty() {
        crate::logging::info(&format!(
            "scratch cleanup removed {} stale entr{} from {} (kept {})",
            report.removed.len(),
            if report.removed.len() == 1 {
                "y"
            } else {
                "ies"
            },
            scratch.display(),
            report.kept
        ));
    }
}

fn claim_prune_slot(base: &Path) -> bool {
    let marker = base.join(PRUNE_MARKER);
    if let Ok(metadata) = std::fs::metadata(&marker)
        && let Ok(modified) = metadata.modified()
        && let Ok(age) = SystemTime::now().duration_since(modified)
        && age.as_secs() < PRUNE_INTERVAL_SECS
    {
        return false;
    }
    std::fs::write(&marker, b"").is_ok()
}

/// Core of [`prune_stale_scratch`], parameterized for unit testing.
///
/// `busy` lists directories that live processes are using (for example their
/// current working directory). Any scratch entry containing one is kept.
pub fn prune_stale_scratch_in(
    scratch: &Path,
    now: SystemTime,
    retention: Duration,
    busy: &[PathBuf],
) -> ScratchPruneReport {
    let mut report = ScratchPruneReport::default();
    let Ok(entries) = std::fs::read_dir(scratch) else {
        return report;
    };
    let Some(cutoff) = now.checked_sub(retention) else {
        return report;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let in_use = busy.iter().any(|dir| dir.starts_with(&path));
        if in_use || modified_since(&path, cutoff) {
            report.kept += 1;
            continue;
        }
        let removed = match entry.file_type() {
            Ok(kind) if kind.is_dir() => std::fs::remove_dir_all(&path),
            Ok(_) => std::fs::remove_file(&path),
            Err(err) => Err(err),
        };
        if removed.is_ok() {
            report.removed.push(path);
        } else {
            report.kept += 1;
        }
    }
    report
}

/// True if `path` or anything beneath it was modified at or after `cutoff`.
/// Symlinks are inspected but never followed. Stops at the first recent hit.
fn modified_since(path: &Path, cutoff: SystemTime) -> bool {
    let mut stack = vec![path.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(metadata) = std::fs::symlink_metadata(&current) else {
            continue;
        };
        match metadata.modified() {
            Ok(modified) if modified >= cutoff => return true,
            Ok(_) => {}
            // Unknown mtime: be conservative and keep the entry.
            Err(_) => return true,
        }
        if metadata.is_dir()
            && let Ok(children) = std::fs::read_dir(&current)
        {
            stack.extend(children.flatten().map(|child| child.path()));
        }
    }
    false
}

/// Working directories of live processes, so an in-progress build or shell
/// sitting inside a scratch entry is never deleted out from under it.
#[cfg(target_os = "linux")]
fn busy_process_dirs() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.bytes().all(|b| b.is_ascii_digit()))
        })
        .filter_map(|entry| std::fs::read_link(entry.path().join("cwd")).ok())
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn busy_process_dirs() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    fn age(path: &Path, by: Duration) {
        let when = SystemTime::now() - by;
        File::options()
            .read(true)
            .open(path)
            .and_then(|file| file.set_modified(when))
            .expect("set mtime");
    }

    #[test]
    fn removes_only_entries_with_no_recent_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scratch = temp.path();

        // Old build tree: every file and directory is old.
        let old = scratch.join("old-target/debug/deps");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("libfoo.rlib"), b"x").unwrap();
        age(&old.join("libfoo.rlib"), 30 * DAY);
        age(&old, 30 * DAY);
        age(&scratch.join("old-target/debug"), 30 * DAY);
        age(&scratch.join("old-target"), 30 * DAY);

        // Old top-level dir but a recently touched file deep inside.
        let active = scratch.join("active/deep");
        fs::create_dir_all(&active).unwrap();
        fs::write(active.join("fresh.log"), b"x").unwrap();
        age(&active, 30 * DAY);
        age(&scratch.join("active"), 30 * DAY);

        // Old loose file and a fresh loose file.
        fs::write(scratch.join("old.txt"), b"x").unwrap();
        age(&scratch.join("old.txt"), 30 * DAY);
        fs::write(scratch.join("new.txt"), b"x").unwrap();

        let report = prune_stale_scratch_in(scratch, SystemTime::now(), 7 * DAY, &[]);

        assert!(!scratch.join("old-target").exists());
        assert!(!scratch.join("old.txt").exists());
        assert!(scratch.join("active/deep/fresh.log").exists());
        assert!(scratch.join("new.txt").exists());
        assert_eq!(report.removed.len(), 2);
        assert_eq!(report.kept, 2);
    }

    #[test]
    fn keeps_entries_used_by_live_processes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scratch = temp.path();
        let busy_dir = scratch.join("busy/sub");
        fs::create_dir_all(&busy_dir).unwrap();
        age(&busy_dir, 30 * DAY);
        age(&scratch.join("busy"), 30 * DAY);

        let report =
            prune_stale_scratch_in(scratch, SystemTime::now(), 7 * DAY, &[busy_dir.clone()]);

        assert!(busy_dir.exists());
        assert!(report.removed.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn never_follows_symlinks_out_of_scratch() {
        let temp = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside");
        fs::write(outside.path().join("precious"), b"x").unwrap();
        age(&outside.path().join("precious"), 30 * DAY);

        let scratch = temp.path().join("scratch");
        fs::create_dir_all(&scratch).unwrap();
        let entry = scratch.join("linkdir");
        fs::create_dir_all(&entry).unwrap();
        std::os::unix::fs::symlink(outside.path(), entry.join("link")).unwrap();

        // Evaluate from a year in the future so everything in scratch is
        // stale, then confirm removal did not reach through the symlink.
        let report = prune_stale_scratch_in(&scratch, SystemTime::now() + 365 * DAY, 7 * DAY, &[]);
        assert_eq!(report.removed.len(), 1);
        assert!(!entry.exists());
        assert!(outside.path().join("precious").exists());
    }
}
