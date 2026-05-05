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
    if let Err(e) = app.session.save() {
        crate::logging::warn(&format!("Failed to save session: {}", e));
    }
}

pub(super) fn persist_swarm_status_snapshot(app: &mut App) {
    if app.is_remote {
        // Avoid clobbering the server-owned session file from a remote client's shadow copy.
        return;
    }
    app.session
        .record_swarm_status_event(app.remote_swarm_members.clone());
    if let Err(e) = app.session.save() {
        crate::logging::warn(&format!("Failed to save session: {}", e));
    }
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
    if let Err(e) = app.session.save() {
        crate::logging::warn(&format!("Failed to save session: {}", e));
    }
}

pub(super) fn persist_remote_session_metadata<F>(app: &mut App, update: F) -> Result<()>
where
    F: FnOnce(&mut crate::session::Session),
{
    let session_id = app
        .remote_session_id
        .as_deref()
        .or(app.resume_session_id.as_deref())
        .unwrap_or(app.session.id.as_str());
    let mut session = crate::session::Session::load(session_id)?;
    update(&mut session);
    session.save()?;
    app.session = session;
    Ok(())
}

pub(super) fn reload_marker_active() -> bool {
    crate::server::reload_marker_active(RELOAD_MARKER_MAX_AGE)
}
