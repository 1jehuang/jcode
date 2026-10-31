//! Last user transport closure, independent of transcript activity and agent lifetime.
//!
//! Runtime markers intentionally do not survive reboot. Attach and close updates
//! are serialized by the server's client-connections lock.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Serialize, Deserialize)]
struct Closure {
    session_id: String,
    closed_at: DateTime<Utc>,
}

fn marker_path(dir: &Path, session_id: &str) -> Option<PathBuf> {
    // Native session IDs use ASCII identifiers. Never allow a session ID to
    // escape the runtime directory, even if it came from a protocol request.
    (!session_id.is_empty()
        && session_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
    .then(|| dir.join(format!("{session_id}.json")))
}

fn directory() -> PathBuf {
    crate::storage::runtime_dir().join("recent-window-closures")
}

/// Called only when the final attached transport actually disconnects.
pub fn record_window_closed(session_id: &str) {
    let dir = directory();
    if let Some(path) = marker_path(&dir, session_id) {
        let result = crate::storage::ensure_dir(&dir).and_then(|()| {
            crate::storage::write_json_fast(
                &path,
                &Closure {
                    session_id: session_id.to_owned(),
                    closed_at: Utc::now(),
                },
            )
        });
        if let Err(error) = result {
            crate::logging::debug(&format!("Could not record window closure: {error}"));
        }
    }
}

/// Reattaching invalidates the previous closure, even if no new message is sent.
pub fn clear_window_closed(session_id: &str) {
    if let Some(path) = marker_path(&directory(), session_id) {
        let _ = std::fs::remove_file(path);
    }
}

fn snapshot(dir: &Path, now: DateTime<Utc>, window: Duration) -> HashMap<String, DateTime<Utc>> {
    if window.is_zero() {
        return HashMap::new();
    }
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            // Old sessions should cost a stat, not an open + JSON parse on
            // every picker refresh. Do not unlink here: the server could
            // replace an expired marker concurrently with a new close.
            if let Some(age) = entry
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|modified| modified.elapsed().ok())
                && age > window
            {
                return None;
            }
            let closure: Closure =
                serde_json::from_slice(&std::fs::read(entry.path()).ok()?).ok()?;
            let age = now.signed_duration_since(closure.closed_at).to_std().ok()?;
            (age <= window).then_some((closure.session_id, closure.closed_at))
        })
        .collect()
}

/// Snapshot once per presence refresh, never once per rendered session row.
pub fn recent_window_closures(window: Duration) -> HashMap<String, DateTime<Utc>> {
    snapshot(&directory(), Utc::now(), window)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closure_window_uses_real_event_time_and_rejects_future_or_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();
        for (id, seconds) in [
            ("just_closed", 5),
            ("boundary", 30),
            ("old", 31),
            ("future", -1),
        ] {
            crate::storage::write_json_fast(
                &marker_path(dir.path(), id).unwrap(),
                &Closure {
                    session_id: id.into(),
                    closed_at: now - chrono::Duration::seconds(seconds),
                },
            )
            .unwrap();
        }
        let found = snapshot(dir.path(), now, Duration::from_secs(30));
        assert_eq!(found.len(), 2);
        assert!(found.contains_key("just_closed"));
        assert!(found.contains_key("boundary"));
        assert!(snapshot(dir.path(), now, Duration::ZERO).is_empty());
        assert_eq!(snapshot(dir.path(), now, Duration::from_secs(60)).len(), 3);
        std::fs::write(dir.path().join("bad.json"), "not json").unwrap();
        assert_eq!(snapshot(dir.path(), now, Duration::from_secs(30)).len(), 2);
    }

    #[test]
    fn identifiers_cannot_escape_runtime_directory() {
        assert!(marker_path(Path::new("/runtime"), "../../session").is_none());
        assert!(marker_path(Path::new("/runtime"), "").is_none());
        assert!(marker_path(Path::new("/runtime"), "session_test-123").is_some());
    }
}
