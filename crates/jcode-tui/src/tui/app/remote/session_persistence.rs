use super::*;

pub(super) fn persist_replay_display_message(
    app: &mut App,
    role: &str,
    title: Option<String>,
    content: &str,
) {
    if app.is_remote {
        // In remote mode, the server owns authoritative session history. Persisting the
        // client's stale shadow copy can roll back newer turns after reconnect/reload.
        return;
    }
    app.session
        .record_replay_display_message(role.to_string(), title, content.to_string());
    let _ = app.session.save();
}

pub(super) fn persist_swarm_status_snapshot(app: &mut App) {
    if app.is_remote {
        // Avoid clobbering the server-owned session file from a remote client's shadow copy.
        return;
    }
    app.session
        .record_swarm_status_event(app.remote_swarm_members.clone());
    let _ = app.session.save();
}

pub(super) fn persist_swarm_plan_snapshot(
    app: &mut App,
    swarm_id: String,
    version: u64,
    items: Vec<crate::plan::PlanItem>,
    participants: Vec<String>,
    reason: Option<String>,
) {
    if app.is_remote {
        // Avoid clobbering the server-owned session file from a remote client's shadow copy.
        return;
    }
    app.session
        .record_swarm_plan_event(swarm_id, version, items, participants, reason);
    let _ = app.session.save();
}

pub(super) fn persist_remote_session_metadata<F>(app: &mut App, update: F) -> Result<()>
where
    F: FnOnce(&mut crate::session::Session),
{
    if crate::tui::is_ssh_remote() {
        anyhow::bail!("Session metadata belongs to the SSH server; local persistence is disabled");
    }
    let session_id = app
        .remote_session_id
        .as_deref()
        .or(app.resume_session_id.as_deref())
        .unwrap_or(app.session.id.as_str());

    // A remote session the server has not flushed yet (brand new, or idle and
    // unsaved) has no snapshot on disk, so `Session::load` returns ENOENT.
    // That is an ordinary state, not a failure: update the in-memory copy and
    // let the server own the file when it first writes one. Writing a client
    // shadow copy here would race the authoritative server snapshot.
    if !crate::session::session_exists(session_id) {
        update(&mut app.session);
        return Ok(());
    }

    let mut session = crate::session::Session::load(session_id)?;
    update(&mut session);
    session.save()?;
    app.session = session;
    Ok(())
}

/// Persist improve/refactor mode without letting a storage failure end the session.
///
/// `/improve` and `/refactor` are ordinary slash commands. Propagating an IO
/// error from here aborts the whole TUI event loop and drops the user back to
/// the shell, which is a far worse outcome than a mode flag that survives only
/// in memory. Report the problem and carry on.
pub(super) fn persist_improve_mode_or_warn(
    app: &mut App,
    mode: Option<crate::session::SessionImproveMode>,
) {
    if let Err(error) = persist_remote_session_metadata(app, |session| {
        session.improve_mode = mode;
    }) {
        crate::logging::warn(&format!(
            "Failed to persist improve mode for this session: {}",
            error
        ));
        app.session.improve_mode = mode;
        app.push_display_message(DisplayMessage::system(format!(
            "⚠️ Could not save the improve/refactor mode for this session ({}). Continuing for this session only.",
            error
        )));
    }
}

pub(super) fn reload_marker_active() -> bool {
    crate::server::reload_marker_active(RELOAD_MARKER_MAX_AGE)
}
