//! Fingerprint-validated resume lookups backed by the shared SQLite index.
//! Session persistence populates the index before the first title lookup.
//! Older/unindexed snapshots are backfilled on demand or by the daemon worker.
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::Session;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FileFingerprint {
    len: u64,
    modified: SystemTime,
}

fn fingerprint(path: &Path) -> std::io::Result<Option<FileFingerprint>> {
    match std::fs::metadata(path) {
        Ok(meta) => Ok(Some(FileFingerprint {
            len: meta.len(),
            modified: meta.modified()?,
        })),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SessionFingerprint {
    snapshot: FileFingerprint,
    journal: Option<FileFingerprint>,
}

impl SessionFingerprint {
    pub(crate) fn read(path: &Path) -> Option<Self> {
        Some(Self {
            snapshot: fingerprint(path).ok()??,
            journal: fingerprint(&super::session_journal_path_from_snapshot(path)).ok()?,
        })
    }
}

pub(crate) struct Entry {
    pub fingerprint: SessionFingerprint,
    pub short_name: Option<String>,
    pub title: Option<String>,
    pub updated_at: DateTime<Utc>,
}

pub(super) struct Cache {
    entries: HashMap<String, Entry>,
    seen: HashSet<String>,
    writable: bool,
}

impl Cache {
    pub fn load() -> Self {
        let entries = crate::recent_session_index::resume_entries();
        let writable = entries.is_ok();
        Self {
            entries: entries.unwrap_or_default(),
            seen: HashSet::new(),
            writable,
        }
    }

    pub fn get(&mut self, id: &str, path: &Path) -> Option<&Entry> {
        self.seen.insert(id.to_string());
        let before = SessionFingerprint::read(path)?;
        let valid = self
            .entries
            .get(id)
            .is_some_and(|entry| entry.fingerprint == before);
        if !valid {
            self.entries.remove(id);
            let session = Session::load_startup_stub(id)
                .or_else(|_| Session::load(id))
                .ok()?;
            // Never associate older metadata with a concurrently written file.
            if SessionFingerprint::read(path).as_ref() != Some(&before) {
                return None;
            }
            // Index errors must not stop a valid session from being resumed.
            // Supply the fingerprint captured around the read, not a later stat.
            if self.writable {
                self.writable =
                    crate::recent_session_index::backfill_resume_session(&session, &before).is_ok();
            }
            self.entries.insert(
                id.to_string(),
                Entry {
                    fingerprint: before,
                    title: session.display_title().map(str::to_owned),
                    short_name: session.short_name,
                    updated_at: session.updated_at,
                },
            );
        }
        self.entries.get(id)
    }

    pub fn finish(self) {
        if !self.writable {
            return;
        }
        let missing: Vec<_> = self
            .entries
            .keys()
            .filter(|id| !self.seen.contains(*id))
            .map(String::as_str)
            .collect();
        let _ = crate::recent_session_index::invalidate_resume_entries(&missing);
    }
}

/// Run off the startup critical path. A lookup that races the backfill still
/// scans unindexed files itself, so a partial index cannot hide older matches.
pub(crate) fn backfill() -> anyhow::Result<()> {
    let directory = crate::storage::jcode_dir()?.join("sessions");
    if !directory.is_dir() {
        return Ok(());
    }
    let mut cache = Cache::load();
    anyhow::ensure!(cache.writable, "session metadata index is unavailable");
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        if let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) {
            let _ = cache.get(id, &path);
        }
    }
    cache.finish();
    Ok(())
}
