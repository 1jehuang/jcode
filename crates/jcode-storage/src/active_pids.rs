//! Tracking of active session process IDs under `~/.jcode/active_pids`.
//!
//! This is pure filesystem state keyed by session ID, used to discover which
//! sessions are currently running (and to map a PID back to its session). It
//! lives in the storage crate because it only needs [`jcode_dir`] and is a
//! low-level concern shared by session management, dictation, and crash
//! recovery, none of which should pull the full `session` module into scope.

use crate::jcode_dir;
use std::path::PathBuf;

/// Directory holding one file per active session ID (`~/.jcode/active_pids`).
pub fn active_pids_dir() -> Option<PathBuf> {
    jcode_dir().ok().map(|d| d.join("active_pids"))
}

/// Record that `session_id` is owned by process `pid`.
pub fn register_active_pid(session_id: &str, pid: u32) {
    if let Some(dir) = active_pids_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(session_id), pid.to_string());
    }
}

/// Remove the active-PID record for `session_id`, if present.
pub fn unregister_active_pid(session_id: &str) {
    if let Some(dir) = active_pids_dir() {
        let _ = std::fs::remove_file(dir.join(session_id));
    }
}

/// Find the active session ID currently owned by the given process ID.
pub fn find_active_session_id_by_pid(pid: u32) -> Option<String> {
    let dir = active_pids_dir()?;
    for entry in std::fs::read_dir(dir).ok()? {
        let entry = entry.ok()?;
        let session_id = entry.file_name().to_string_lossy().to_string();
        let stored = std::fs::read_to_string(entry.path()).ok()?;
        if stored.trim().parse::<u32>().ok()? == pid {
            return Some(session_id);
        }
    }
    None
}

/// List active session IDs currently tracked in `~/.jcode/active_pids`.
pub fn active_session_ids() -> Vec<String> {
    let Some(dir) = active_pids_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect()
}
