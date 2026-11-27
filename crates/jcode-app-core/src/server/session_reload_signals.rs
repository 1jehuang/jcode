//! Reload and active-turn signal bookkeeping for session changes.

use super::*;

pub(super) fn mark_remote_reload_started(request_id: &str) {
    crate::server::write_reload_state(
        request_id,
        jcode_build_meta::version(),
        crate::server::ReloadPhase::Starting,
        None,
    );
}

pub(super) async fn rename_shutdown_signal(
    shutdown_signals: &Arc<RwLock<HashMap<String, InterruptSignal>>>,
    old_session_id: &str,
    new_session_id: &str,
) {
    if old_session_id == new_session_id {
        return;
    }

    let mut signals = shutdown_signals.write().await;
    if let Some(signal) = signals.remove(old_session_id) {
        signals.insert(new_session_id.to_string(), signal);
    }
    drop(signals);
    rename_background_tool_signal(old_session_id, new_session_id);
    // In-flight turns are registered in the process-global cancel registry by
    // session id. Attaching to / resuming a session renames it underneath a
    // still-streaming turn, so the registration must follow, or a later Esc
    // finds no active-turn signal for the new id and the model keeps
    // generating (issue #732, regression of issue #428).
    crate::turn_cancel_registry::rename_active_turns(old_session_id, new_session_id);
}
