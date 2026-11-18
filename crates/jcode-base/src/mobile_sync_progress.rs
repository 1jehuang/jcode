//! Borrowed progress overlay for locked commits. Only updated entries are
//! owned by the caller, so persisting one session never clones the inventory.
use super::{MobileSyncState, SessionProgress};
use serde::Serialize;
use serde::ser::{SerializeMap, Serializer};
use std::collections::BTreeMap;

pub(super) fn replaces(old: Option<&SessionProgress>, progress: &SessionProgress) -> bool {
    let version = |p: &SessionProgress| {
        p.revision
            .map(|r| r[0].max(r[2]))
            .unwrap_or(p.mtime_ms.saturating_mul(1_000_000))
    };
    old.is_none_or(|old| {
        old != progress
            && (version(old) < version(progress)
                || (version(old) == version(progress) && old.uploaded <= progress.uploaded))
    })
}

#[derive(Serialize)]
pub(super) struct ProgressCommit<'a> {
    enabled: bool,
    generation: u64,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    phone_messages_off: bool,
    sessions: MergedSessions<'a>,
}

impl<'a> ProgressCommit<'a> {
    pub(super) fn new(
        state: &'a MobileSyncState,
        generation: u64,
        updates: &'a BTreeMap<String, SessionProgress>,
    ) -> Option<Self> {
        if !state.enabled
            || state.generation != generation
            || !updates
                .iter()
                .any(|(id, progress)| replaces(state.sessions.get(id), progress))
        {
            return None;
        }
        Some(Self {
            enabled: state.enabled,
            generation: state.generation,
            phone_messages_off: state.phone_messages_off,
            sessions: MergedSessions {
                current: &state.sessions,
                updates,
            },
        })
    }
}

struct MergedSessions<'a> {
    current: &'a BTreeMap<String, SessionProgress>,
    updates: &'a BTreeMap<String, SessionProgress>,
}

impl Serialize for MergedSessions<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        let mut current = self.current.iter().peekable();
        let mut updates = self.updates.iter().peekable();
        // Preserve BTreeMap order, including keys inserted between existing ones.
        loop {
            match (current.peek(), updates.peek()) {
                (Some(&(id, old)), Some(&(update_id, progress))) if id == update_id => {
                    map.serialize_entry(
                        id,
                        if replaces(Some(old), progress) {
                            progress
                        } else {
                            old
                        },
                    )?;
                    current.next();
                    updates.next();
                }
                (Some(&(id, progress)), Some(&(update_id, _))) if id < update_id => {
                    map.serialize_entry(id, progress)?;
                    current.next();
                }
                (_, Some(&(id, progress))) => {
                    map.serialize_entry(id, progress)?;
                    updates.next();
                }
                (Some(&(id, progress)), None) => {
                    map.serialize_entry(id, progress)?;
                    current.next();
                }
                (None, None) => break,
            }
        }
        map.end()
    }
}

#[cfg(test)]
#[path = "mobile_sync_progress_tests.rs"]
mod tests;
