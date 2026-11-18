//! Snapshot preparation for session split and transfer actions.

use super::*;

pub(super) fn clone_split_session(
    parent_session_id: &str,
    live_parent: Option<&Session>,
) -> anyhow::Result<(String, String)> {
    // Keep the persisted snapshot authoritative, including while the parent is
    // busy. A brand-new Agent may not have saved anything yet, however. Only a
    // missing snapshot permits an in-memory fallback, never corrupt/unreadable
    // history or a session belonging to a different client.
    let parent = Session::load(parent_session_id).or_else(|error| {
        let missing = error
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound);
        match live_parent.filter(|parent| missing && parent.id == parent_session_id) {
            Some(parent) => Ok(parent.clone()),
            None => Err(error),
        }
    })?;

    let mut child = Session::create(Some(parent_session_id.to_string()), None);
    child.replace_messages(parent.messages.clone());
    child.compaction = parent.compaction.clone();
    child.system_prompt = parent.system_prompt.clone();
    child.working_dir = parent.working_dir.clone();
    child.model = parent.model.clone();
    // Keep the same provider account. Replayed provider-native blocks (Anthropic
    // web search `encrypted_content`) are bound to the account that produced
    // them, and another account rejects the whole request with a 400.
    child.provider_key = parent.provider_key.clone();
    child.route_api_method = parent.route_api_method.clone();
    child.status = crate::session::SessionStatus::Closed;
    // The parent agent keeps ownership of any in-flight request; tell the
    // forked agent so it treats the next prompt as fresh work instead of
    // continuing (and duplicating) the parent's current turn.
    child.append_fork_notice(parent_session_id, parent.display_name());
    child.save()?;

    crate::session_env::inherit_session_env(parent_session_id, &child.id);
    let name = child.display_name().to_string();
    Ok((child.id.clone(), name))
}

pub(super) fn transfer_active_messages(session: &Session) -> Vec<crate::message::Message> {
    let start = session
        .compaction
        .as_ref()
        .map(|state| state.compacted_count.min(session.messages.len()))
        .unwrap_or(0);
    session.messages[start..]
        .iter()
        .map(crate::session::StoredMessage::to_message)
        .collect()
}

pub(super) fn create_transfer_child_session(
    parent_session_id: &str,
    parent: &Session,
    compaction: Option<Arc<crate::session::StoredCompactionState>>,
) -> anyhow::Result<(String, String)> {
    let mut todos = crate::todo::load_todos(parent_session_id)?;
    // Background tasks belong to the source session. A fresh session must
    // reassess those results, not inherit foreign task IDs or their wakes.
    for todo in &mut todos {
        todo.waiting_on.clear();
        todo.waiting_reason = None;
    }
    let mut child = Session::create(Some(parent_session_id.to_string()), None);
    child.messages.clear();
    child.compaction = compaction;
    child.system_prompt = parent.system_prompt.clone();
    child.working_dir = parent.working_dir.clone();
    child.model = parent.model.clone();
    child.provider_key = parent.provider_key.clone();
    child.route_api_method = parent.route_api_method.clone();
    child.subagent_model = parent.subagent_model.clone();
    child.improve_mode = parent.improve_mode;
    child.autoreview_enabled = parent.autoreview_enabled;
    child.autojudge_enabled = parent.autojudge_enabled;
    child.is_canary = parent.is_canary;
    child.testing_build = parent.testing_build.clone();
    child.provider_session_id = None;
    child.status = crate::session::SessionStatus::Closed;
    child.save()?;
    crate::todo::save_todos(&child.id, &todos)?;
    crate::session_env::inherit_session_env(parent_session_id, &child.id);
    Ok((child.id.clone(), child.display_name().to_string()))
}
