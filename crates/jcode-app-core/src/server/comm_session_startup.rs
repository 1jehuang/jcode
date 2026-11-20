//! Helpers for the headed startup handoff of visible swarm workers.

pub(super) fn persist_headed_startup_message(session_id: &str, message: &str) {
    crate::logging::info(&format!(
        "Headed spawn: persisting startup submission for {session_id} (chars={}) to client-input handoff file",
        message.chars().count(),
    ));
    crate::client_input::save_startup_submission_for_session(
        session_id,
        message.to_string(),
        Vec::new(),
    );
}

pub(super) fn clear_headed_startup_message(session_id: &str) {
    if let Ok(jcode_dir) = crate::storage::jcode_dir() {
        let path = jcode_dir.join(format!("client-input-{}", session_id));
        if let Err(error) = std::fs::remove_file(&path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            crate::logging::warn(&format!(
                "Unable to remove prepared spawn file {}: {error}",
                path.display()
            ));
        }
    }
}

pub(super) fn cleanup_prepared_visible_spawn_session(session_id: &str) {
    crate::session_env::remove_session_env(session_id);
    clear_headed_startup_message(session_id);
    if let Ok(path) = crate::session::session_path(session_id)
        && let Err(error) = std::fs::remove_file(&path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        crate::logging::warn(&format!(
            "Failed to clean prepared session file {}: {error}",
            path.display()
        ));
    }
    if let Ok(path) = crate::session::session_journal_path(session_id)
        && let Err(error) = std::fs::remove_file(&path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        crate::logging::warn(&format!(
            "Failed to clean prepared session file {}: {error}",
            path.display()
        ));
    }
}
