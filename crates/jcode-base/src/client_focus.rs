//! Which jcode session each local client is showing.
//!
//! TUI clients record the session they display when they gain focus. The server
//! reads this to route `jcode transcript` text to the session the user is
//! looking at, and desktop-side tools (dictation, the voice companion) map a
//! focused window to its client's session. Plain files under `~/.jcode`, so
//! readers never need a live connection.

use anyhow::{Context, Result};
use std::sync::{Mutex, OnceLock};

fn last_focused_session_write_cache() -> &'static Mutex<Option<String>> {
    static CACHE: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

pub fn remember_last_focused_session(session_id: &str) -> Result<()> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Ok(());
    }

    // Always rewrite the shared file (callers debounce): another client may
    // have taken it since this process last wrote the same session.
    let path = last_focused_session_path()?;
    if let Some(parent) = path.parent() {
        crate::storage::ensure_dir(parent)?;
    }
    std::fs::write(&path, session_id).context("failed to persist last focused jcode session")?;

    if let Ok(cache) = last_focused_session_write_cache().lock()
        && cache.as_deref() == Some(session_id)
    {
        return Ok(());
    }
    // Best effort: lets a global voice hold map a focused terminal window to
    // exactly this client's session, even with several CLIs open.
    if let Err(error) = remember_client_session(std::process::id(), session_id) {
        crate::logging::warn(&format!("Cannot record focused client session: {error:#}"));
    }

    if let Ok(mut cache) = last_focused_session_write_cache().lock() {
        *cache = Some(session_id.to_string());
    }

    Ok(())
}

pub fn last_focused_session() -> Result<Option<String>> {
    let path = last_focused_session_path()?;
    let session_id = match std::fs::read_to_string(path) {
        Ok(text) => text.trim().to_string(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).context("failed to read last focused jcode session"),
    };
    if session_id.is_empty() {
        return Ok(None);
    }

    if crate::storage::active_session_ids()
        .iter()
        .any(|id| id == &session_id)
    {
        Ok(Some(session_id))
    } else {
        Ok(None)
    }
}

fn client_sessions_dir() -> Result<std::path::PathBuf> {
    Ok(crate::storage::jcode_dir()?.join("client_sessions"))
}

/// Record which session a TUI client process is showing. Keyed by client PID.
fn remember_client_session(pid: u32, session_id: &str) -> Result<()> {
    let dir = client_sessions_dir()?;
    crate::storage::ensure_dir(&dir)?;
    // Opportunistically drop markers of exited clients.
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let alive = entry
                .file_name()
                .to_str()
                .and_then(|name| match name.parse::<u32>() {
                    Ok(pid) => Some(pid),
                    Err(error) => {
                        crate::logging::debug(&format!(
                            "Invalid client session marker filename: {error}"
                        ));
                        None
                    }
                })
                .is_some_and(|pid| std::path::Path::new(&format!("/proc/{pid}")).exists());
            if !alive
                && cfg!(target_os = "linux")
                && let Err(error) = std::fs::remove_file(entry.path())
                && error.kind() != std::io::ErrorKind::NotFound
            {
                crate::logging::debug(&format!(
                    "Cannot remove stale client session marker: {error}"
                ));
            }
        }
    }
    std::fs::write(dir.join(pid.to_string()), session_id)?;
    Ok(())
}

pub fn registered_client_session(pid: u32) -> Option<String> {
    let path = match client_sessions_dir() {
        Ok(dir) => dir.join(pid.to_string()),
        Err(error) => {
            crate::logging::warn(&format!(
                "Cannot resolve client session directory: {error:#}"
            ));
            return None;
        }
    };
    let session_id = match std::fs::read_to_string(path) {
        Ok(text) => text.trim().to_string(),
        // A client that has not focused a session has no marker yet.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            crate::logging::warn(&format!("Cannot read registered client session: {error}"));
            return None;
        }
    };
    (!session_id.is_empty()).then_some(session_id)
}

fn last_focused_session_path() -> Result<std::path::PathBuf> {
    Ok(crate::storage::jcode_dir()?.join("last_focused_client_session"))
}
