//! On-demand continuation sweep behind `/continue`: the live-session half of
//! reload recovery, scoped to the caller's project.

use super::SessionAgents;
use crate::agent::Agent;
use crate::protocol::ServerEvent;
use crate::server::{SwarmEvent, SwarmMember, client_session, live_turn, reload_recovery, util};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast, mpsc};

/// Decide whether an idle live session still owes the model a continuation.
///
/// This is the live-session analog of `restored_session_was_interrupted`: a
/// session "would continue if resumed" when it has a pending reload-recovery
/// directive, when it carries reload-interruption markers, or when its last
/// model-visible message is a user/tool turn the assistant never answered
/// (e.g. the turn errored or the process was interrupted mid-generation).
fn live_session_owes_continuation(agent: &Agent) -> bool {
    // Never continue an empty/fresh session.
    if agent.visible_conversation_message_count() == 0 {
        return false;
    }

    if reload_recovery::peek_for_session(agent.session_id())
        .ok()
        .flatten()
        .map(|record| record.status == reload_recovery::ReloadRecoveryStatus::Pending)
        .unwrap_or(false)
    {
        return true;
    }

    if client_session::session_was_interrupted_by_reload(agent) {
        return true;
    }

    matches!(
        agent.last_visible_conversation_role(),
        Some(crate::message::Role::User)
    )
}

/// Continue every live, idle session that would auto-resume on a reload.
///
/// This is the on-demand equivalent of the post-reload recovery sweep: it walks
/// the currently-live sessions, and for each idle one that still owes the model
/// a continuation, injects the standard "continue where you left off" reminder
/// so the session picks back up without the user having to open each one.
///
/// Scope: `caller_working_dir` bounds the sweep to the caller's own project.
/// One daemon serves many projects, so without this a `/continue` typed in
/// project A injects a continuation into every live session in the daemon,
/// including projects the user has open elsewhere and is not looking at.
///
/// The startup sweep in `recover_headless_sessions_on_startup` deliberately
/// stays daemon-wide: no client asked for it, it is the daemon tidying up after
/// its own restart, and narrowing it would strand sessions belonging to projects
/// that have no attached client at all. This one is user-initiated, so it is
/// scoped to the user's project.
#[expect(
    clippy::too_many_arguments,
    reason = "resuming live sessions needs session, swarm membership, and status event state"
)]
pub(in crate::server) async fn handle_resume_all_sessions(
    id: u64,
    sessions: &SessionAgents,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    caller_working_dir: Option<&str>,
) {
    // Snapshot live sessions (those with at least one live client attachment).
    let live_session_ids: Vec<String> = {
        let members = swarm_members.read().await;
        members
            .iter()
            .filter(|(_, member)| !member.event_txs.is_empty() || !member.event_tx.is_closed())
            .map(|(session_id, _)| session_id.clone())
            .collect()
    };

    let mut resumed_sessions: Vec<String> = Vec::new();
    let mut skipped = 0usize;
    let mut out_of_scope = 0usize;

    for session_id in live_session_ids {
        let agent = {
            let guard = sessions.read().await;
            guard.get(&session_id).cloned()
        };
        let Some(agent) = agent else {
            continue;
        };

        // Only act on idle sessions; a busy session is already making progress.
        // The owned guard doubles as the turn reservation (#1152).
        let Ok(agent_guard) = Arc::clone(&agent).try_lock_owned() else {
            skipped += 1;
            continue;
        };

        // Project scope check. Runs on the already-reserved guard rather than
        // taking a second lock: a separate `agent.lock().await` here races with
        // the `try_lock_owned` above and can report an idle session as busy,
        // which would silently drop sessions the user asked to continue.
        if !working_dir_in_scope(agent_guard.working_dir(), caller_working_dir) {
            drop(agent_guard);
            out_of_scope += 1;
            continue;
        }

        if !live_session_owes_continuation(&agent_guard) {
            drop(agent_guard);
            skipped += 1;
            continue;
        }

        let reminder = match reload_recovery::pending_directive_for_session(&session_id) {
            Ok(Some(directive)) => directive.continuation_message,
            _ => crate::tool::selfdev::ReloadContext::interrupted_session_continuation_message(),
        };
        let display_name = agent_guard
            .session_short_name()
            .map(str::to_string)
            .unwrap_or_else(|| session_id[..8.min(session_id.len())].to_string());

        // Best-effort: record that the durable recovery intent was delivered.
        if let Err(error) = reload_recovery::mark_delivered_if_matching_continuation(
            &session_id,
            &reminder,
            "resume_all_sessions",
        ) {
            crate::logging::warn(&format!(
                "resume_all_sessions: failed to mark recovery intent delivered for {}: {}",
                session_id, error
            ));
        }

        live_turn::spawn_tracked_live_turn(
            &session_id,
            agent_guard,
            String::new(),
            Some(reminder),
            None,
            Some("resuming interrupted session".to_string()),
            live_turn::LiveTurnSwarmContext::new(
                swarm_members,
                swarms_by_id,
                event_history,
                event_counter,
                swarm_event_tx,
            ),
        )
        .await;

        resumed_sessions.push(display_name);
    }

    let resumed = resumed_sessions.len();
    let message = if resumed == 0 {
        "No interrupted sessions to resume. All live sessions are idle or already complete."
            .to_string()
    } else if resumed == 1 {
        format!("Resuming 1 interrupted session: {}.", resumed_sessions[0])
    } else {
        format!(
            "Resuming {} interrupted sessions: {}.",
            resumed,
            resumed_sessions.join(", ")
        )
    };

    crate::logging::info(&format!(
        "resume_all_sessions: resumed={} skipped={} out_of_scope={} sessions={:?}",
        resumed, skipped, out_of_scope, resumed_sessions
    ));

    let _ = client_event_tx.send(ServerEvent::ResumeAllResult {
        id,
        resumed,
        skipped,
        resumed_sessions,
        message,
    });
}

/// Whether a session's working directory counts as being in the caller's project.
///
/// Both sides are canonicalized before comparing, so a symlinked checkout, a
/// `..` segment, or a Windows short/long path pair does not read as a different
/// project and silently skip sessions the user asked to continue.
///
/// The `None` cases follow the project-isolation invariants: `working_dir: None`
/// must never mean "the daemon's cwd", because the daemon's cwd is whichever
/// project happened to start it. This function compares the two directories
/// only when both are present; it never falls back to a process-global default.
///
/// When either side is absent the session is treated as in scope, and that is a
/// deliberate choice rather than an oversight:
///
/// - A session with no directory is not attributable to any project, so
///   scoping it away would silently drop it from a sweep the user asked for.
/// - A caller with no directory means the daemon could not attribute the
///   request to a project at all. Scoping nothing would make `/continue`
///   report "no interrupted sessions" while real sessions sat interrupted,
///   which is worse than the behavior this item removes. Note this is not a
///   common path: `Session::create` populates `working_dir` from the process
///   cwd, so a real caller almost always has one.
///
/// In every ordinary case both sides carry a directory, and the comparison is
/// strict: project B never matches a caller in project A.
fn working_dir_in_scope(session_dir: Option<&str>, caller_dir: Option<&str>) -> bool {
    match (session_dir, caller_dir) {
        (Some(session), Some(caller)) => {
            util::canonicalize_or(session.into()) == util::canonicalize_or(caller.into())
        }
        _ => true,
    }
}
