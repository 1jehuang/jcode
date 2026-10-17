//! Background maintenance for the on-disk session store.
//!
//! Session transcripts (`<id>.json`) are kept forever, but the atomic-write
//! layer also leaves a single rolling `<id>.bak` next to each file as a
//! crash-recovery copy (see `jcode_storage::write_bytes_inner`). That backup is
//! only ever consulted when the primary `.json` is found to be corrupt on the
//! very next read. For sessions that have not been touched in weeks the primary
//! is stable, so the stale `.bak` is pure disk overhead (these accumulate into
//! gigabytes over time).
//!
//! This module prunes `.bak` files that are older than a conservative window.
//! It never touches the `.json` transcripts themselves, so no session data is
//! lost; at worst a very old, already-stable session loses its redundant
//! recovery copy.

use crate::storage;
use chrono::{DateTime, Duration, Local};
use std::path::Path;

/// Backups older than this are considered safe to remove. Chosen conservatively
/// so any realistic "crashed mid-write, reopened later" scenario still has its
/// recovery copy.
const BACKUP_RETENTION_DAYS: i64 = 30;

/// Minimum interval between prune passes across all jcode processes.
///
/// The prune walks the entire sessions directory (easily 100k+ entries on a
/// long-lived install), which profiles as the single largest CPU cost of TUI
/// startup when it runs unconditionally. Backups only need to be reclaimed
/// eventually, so one pass per interval per machine is plenty; a marker file's
/// mtime coordinates that across concurrently spawned processes.
const PRUNE_INTERVAL_SECS: u64 = 24 * 60 * 60;

/// Remove stale `<id>.bak` files from the sessions directory.
///
/// Best-effort: any I/O error is ignored so this can run on a background thread
/// at startup without ever affecting launch. Skips cheaply (one stat) unless
/// the machine-wide prune interval has elapsed, so spawning many jcode
/// processes at once does not trigger many full directory walks.
pub fn prune_old_session_backups() {
    if let Ok(base) = storage::jcode_dir() {
        let sessions_dir = base.join("sessions");
        if !claim_prune_slot(&base) {
            return;
        }
        prune_old_session_backups_in(&sessions_dir, Local::now());
    }
}

/// Returns true when this process should run the prune pass now, updating the
/// marker so other processes (and future spawns) skip until the next interval.
///
/// The marker touch happens before the walk, so a burst of simultaneous spawns
/// resolves to at most a couple of walkers (racing between the stat and the
/// touch) instead of one per process, and steady-state spawns do a single stat.
fn claim_prune_slot(base: &Path) -> bool {
    let marker = base.join("sessions-bak-prune.stamp");
    if let Ok(metadata) = std::fs::metadata(&marker)
        && let Ok(modified) = metadata.modified()
        && let Ok(age) = std::time::SystemTime::now().duration_since(modified)
        && age.as_secs() < PRUNE_INTERVAL_SECS
    {
        return false;
    }
    // Touch (create or refresh) the marker to claim the slot.
    std::fs::write(&marker, b"").is_ok()
}

/// Core of [`prune_old_session_backups`], parameterized on the directory and
/// "now" for unit testing.
fn prune_old_session_backups_in(sessions_dir: &Path, now: DateTime<Local>) {
    let Ok(entries) = std::fs::read_dir(sessions_dir) else {
        return;
    };
    let cutoff = now - Duration::days(BACKUP_RETENTION_DAYS);
    for entry in entries.flatten() {
        let path = entry.path();
        // Only prune the atomic-write backup files; never the .json transcripts
        // or anything else (journals, tmp files, etc.).
        if path.extension().map(|e| e == "bak").unwrap_or(false)
            && let Ok(metadata) = entry.metadata()
            && metadata.is_file()
            && let Ok(modified) = metadata.modified()
        {
            let modified: DateTime<Local> = modified.into();
            if modified < cutoff {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::io::Write;
    use std::time::{Duration as StdDuration, SystemTime};

    #[test]
    fn claim_prune_slot_rate_limits_within_interval_and_reclaims_after() {
        let dir = std::env::temp_dir().join(format!(
            "kcode-bak-claim-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).expect("create temp dir");

        // First claim wins and creates the marker.
        assert!(claim_prune_slot(&dir), "first claim should win");
        let marker = dir.join("sessions-bak-prune.stamp");
        assert!(marker.exists(), "marker should be created");

        // A concurrent/subsequent spawn within the interval is rejected.
        assert!(
            !claim_prune_slot(&dir),
            "second claim within interval should be skipped"
        );

        // Once the marker is older than the interval the slot opens again.
        let old = SystemTime::now() - StdDuration::from_secs(PRUNE_INTERVAL_SECS + 60);
        File::options()
            .write(true)
            .open(&marker)
            .and_then(|f| f.set_modified(old))
            .expect("age the marker");
        assert!(
            claim_prune_slot(&dir),
            "claim should succeed after the interval elapses"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn prunes_only_old_bak_files() {
        let dir = std::env::temp_dir().join(format!(
            "kcode-bak-prune-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).expect("create temp dir");

        let write = |name: &str, age_days: u64| {
            let path = dir.join(name);
            let mut f = File::create(&path).expect("create");
            f.write_all(b"{}").ok();
            if age_days > 0 {
                let mtime = SystemTime::now() - StdDuration::from_secs(age_days * 24 * 60 * 60);
                f.set_modified(mtime).expect("set mtime");
            }
            path
        };

        // 60-day-old backup: should be pruned.
        let old_bak = write("session_old.bak", 60);
        // 5-day-old backup: within window, should survive.
        let recent_bak = write("session_recent.bak", 5);
        // Transcripts must never be removed, regardless of age.
        let old_json = write("session_old.json", 60);
        let recent_json = write("session_recent.json", 0);
        // Other artifacts must be left alone.
        let journal = write("session_old.journal.jsonl", 60);

        prune_old_session_backups_in(&dir, Local::now());

        assert!(!old_bak.exists(), "old .bak should be pruned");
        assert!(recent_bak.exists(), "recent .bak must survive");
        assert!(
            old_json.exists(),
            "old .json transcript must never be removed"
        );
        assert!(recent_json.exists(), "recent .json transcript must survive");
        assert!(journal.exists(), "journals are out of scope");

        fs::remove_dir_all(&dir).ok();
    }
}
