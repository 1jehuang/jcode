//! Disposable, fingerprint-validated metadata for title-based resume lookup.
//! Never cache transcripts or failed loads. Snapshot and journal changes both
//! invalidate entries, including changes made by older Jcode processes.
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::Session;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Fingerprint {
    len: u64,
    modified: SystemTime,
}

fn fingerprint(path: &Path) -> std::io::Result<Option<Fingerprint>> {
    match std::fs::metadata(path) {
        Ok(meta) => Ok(Some(Fingerprint {
            len: meta.len(),
            modified: meta.modified()?,
        })),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct Entry {
    snapshot: Option<Fingerprint>,
    journal: Option<Fingerprint>,
    pub short_name: Option<String>,
    pub title: Option<String>,
    pub updated_at: DateTime<Utc>,
}

pub(super) struct Cache {
    entries: HashMap<String, Entry>,
    dirty: bool,
    seen: HashSet<String>,
}

impl Cache {
    pub fn load(directory: &Path) -> Self {
        let entries = std::fs::read(directory.join("resume-metadata-v1.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            entries,
            dirty: false,
            seen: HashSet::new(),
        }
    }

    pub fn get(&mut self, id: &str, path: &Path) -> Option<&Entry> {
        self.seen.insert(id.to_string());
        let journal_path = super::session_journal_path_from_snapshot(path);
        let before = (fingerprint(path).ok()?, fingerprint(&journal_path).ok()?);
        let valid = self
            .entries
            .get(id)
            .is_some_and(|entry| entry.snapshot == before.0 && entry.journal == before.1);
        if !valid {
            self.entries.remove(id);
            self.dirty = true;
            let session = Session::load_startup_stub(id)
                .or_else(|_| Session::load(id))
                .ok()?;
            // A concurrent writer must not leave metadata cached under a newer
            // fingerprint than the data we actually read.
            let after = (fingerprint(path).ok()?, fingerprint(&journal_path).ok()?);
            if before != after {
                return None;
            }
            self.entries.insert(
                id.to_string(),
                Entry {
                    snapshot: before.0,
                    journal: before.1,
                    title: session.display_title().map(str::to_owned),
                    short_name: session.short_name,
                    updated_at: session.updated_at,
                },
            );
        }
        self.entries.get(id)
    }

    pub fn save(mut self, directory: &Path) {
        let before = self.entries.len();
        self.entries.retain(|id, _| self.seen.contains(id));
        self.dirty |= before != self.entries.len();
        if self.dirty {
            // Best effort: cache errors must never prevent resuming a session.
            let _ = crate::storage::write_json_fast(
                &directory.join("resume-metadata-v1.json"),
                &self.entries,
            );
        }
    }
}
