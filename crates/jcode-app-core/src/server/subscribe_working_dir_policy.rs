//! Working-directory policy for subscribe and target attachments.
//!
//! One daemon serves sessions for many projects, and the `working_dir` a client
//! reports describes that *client's* project, not the project its target session
//! belongs to. These functions decide, in one place, which directory a session is
//! actually allowed to bind to, so the agent, its project-local MCP config and its
//! swarm grouping cannot each resolve a different answer (issue #481).

use std::path::Path;
use std::sync::Arc;

use tokio::sync::Mutex;

use super::Agent;

/// Resolve the working directory a subscribe should actually bind to.
///
/// Returns the reported dir when it is acceptable, or the session's existing
/// dir when the report is rejected by [`subscribe_working_dir_replacement`].
/// Every consumer of a subscribe cwd (agent state, swarm id, project-local MCP
/// resolution) must agree on this one answer, otherwise the session's tools,
/// swarm grouping, and MCP config can each resolve against a different
/// directory (issue #481).
pub(super) fn effective_subscribe_working_dir(
    current: Option<&str>,
    reported: &str,
    home: Option<&Path>,
) -> String {
    match subscribe_working_dir_replacement(current, reported, home) {
        Some(accepted) => accepted,
        None => current
            .map(str::to_string)
            .unwrap_or_else(|| reported.trim().to_string()),
    }
}

/// Decide whether a client-reported subscribe cwd may replace the session's
/// current working directory.
///
/// Requiring a subscribe cwd to be non-empty and absolute (the earlier
/// require-cwd change) is necessary but not sufficient: a client that launches
/// with an inherited environment can report the user's *home* directory even
/// though the real project lives elsewhere. Accepting that silently re-pins the
/// session to home, so bash/file tools run against home while the header still
/// shows the project path (issue #481).
///
/// The rule is deliberately narrow so it cannot break legitimate directory
/// changes: a reported cwd that is exactly the home directory is ignored *only*
/// when the session already has a different working directory. Working in home
/// on purpose (no prior cwd, or a session already pinned to home) still works,
/// and every other path is accepted as before.
pub(super) fn subscribe_working_dir_replacement(
    current: Option<&str>,
    reported: &str,
    home: Option<&Path>,
) -> Option<String> {
    let reported_trimmed = reported.trim();
    if reported_trimmed.is_empty() {
        return None;
    }
    let current = current.map(str::trim).filter(|dir| !dir.is_empty());
    if current == Some(reported_trimmed) {
        return None;
    }
    if let (Some(current), Some(home)) = (current, home)
        && Path::new(reported_trimmed) == home
        && Path::new(current) != home
    {
        return None;
    }
    Some(reported_trimmed.to_string())
}

pub(super) fn log_ignored_subscribe_working_dir(session_id: &str, current: &str, reported: &str) {
    crate::logging::warn(&format!(
        "Ignoring subscribe working_dir {} for session {}: it is the home directory while the session is already bound to {} (issue #481)",
        reported, session_id, current
    ));
}

pub(super) fn apply_or_defer_subscribe_working_dir(
    agent: &Arc<Mutex<Agent>>,
    working_dir: &str,
    session_id: &str,
) {
    let home = dirs::home_dir();
    if let Ok(mut agent_guard) = agent.try_lock() {
        match subscribe_working_dir_replacement(
            agent_guard.working_dir(),
            working_dir,
            home.as_deref(),
        ) {
            Some(accepted) => {
                // An existing project directory always wins over a client-reported
                // one. See `session_working_dir_for_client`: a client's directory
                // is creation-only, and a target attachment reports the attaching
                // client's project, not this session's.
                let accepted = match agent_guard.working_dir() {
                    Some(existing) => {
                        session_working_dir_for_client(Some(existing), Some(&accepted), false)
                    }
                    None => Some(accepted),
                };
                if let Some(accepted) = accepted {
                    agent_guard.set_working_dir(&accepted);
                } else if let Some(current) = agent_guard.working_dir()
                    && current != working_dir
                {
                    log_ignored_subscribe_working_dir(session_id, current, working_dir);
                }
            }
            None => {
                if let Some(current) = agent_guard.working_dir()
                    && current != working_dir
                {
                    log_ignored_subscribe_working_dir(session_id, current, working_dir);
                }
            }
        }
        return;
    }

    let agent = Arc::clone(agent);
    let working_dir = working_dir.to_string();
    let session_id = session_id.to_string();
    tokio::spawn(async move {
        let mut agent_guard = agent.lock().await;
        match subscribe_working_dir_replacement(
            agent_guard.working_dir(),
            &working_dir,
            home.as_deref(),
        ) {
            Some(accepted) => {
                // Same rule as the synchronous branch above. Without it the guard
                // would hold only while the session was idle and quietly lapse
                // mid-turn, which is exactly when a desktop attaches to a live
                // session.
                let accepted = match agent_guard.working_dir() {
                    Some(existing) => {
                        session_working_dir_for_client(Some(existing), Some(&accepted), false)
                    }
                    None => Some(accepted),
                };
                match accepted {
                    Some(accepted) => {
                        agent_guard.set_working_dir(&accepted);
                        crate::logging::info(&format!(
                            "Applied deferred subscribe working directory for session {}",
                            session_id
                        ));
                    }
                    None => {
                        if let Some(current) = agent_guard.working_dir() {
                            log_ignored_subscribe_working_dir(&session_id, current, &working_dir);
                        }
                    }
                }
            }
            None => {
                if let Some(current) = agent_guard.working_dir()
                    && current != working_dir
                {
                    log_ignored_subscribe_working_dir(&session_id, current, &working_dir);
                }
            }
        }
    });
}

/// Decide which working directory a client-reported directory may bind an
/// existing session to.
///
/// This is the working-dir counterpart of the rule the request handler already
/// applies to a system prompt: "overrides are creation-only. In particular, never
/// apply one to a target attachment." The system-prompt path got that rule; the
/// working-dir path did not, so a client attaching to a session could still move
/// it.
///
/// The rule is scoped to session *creation* on purpose. `create_working_dir`
/// distinguishes the two cases: at creation the reported directory is the only
/// description of the project the user is working in and must be adopted, while a
/// directory reported later describes whichever client happened to reconnect. A
/// client cannot deliberately retarget an existing session's project, because the
/// wire protocol has no request that means "move this session to another project"
/// (the only requests carrying a `working_dir` are Subscribe, CommSpawn, and
/// spawn-agent, and all three either create a session or attach to one). Making a
/// deliberate project move possible later means adding an explicit request for it,
/// not letting an ordinary reconnect imply one.
///
/// A session belongs to a project. A client's directory describes the project
/// that *client* is sitting in, not the project the session belongs to.
/// Overwriting re-points that session's bash and file tools, its project-local
/// MCP config, its memory scope, and its swarm grouping at the client's project,
/// so a later message in the original project would read and write the wrong tree
/// while the header still showed the original path.
///
/// The session's stored directory therefore wins whenever it has one. A client's
/// directory is adopted only when the session has none to lose, which is the
/// creation case (including remote continuation, where the client has no local
/// copy of the session it is resuming).
///
/// Both sides are canonicalized before comparing, so a symlinked checkout, a
/// `..` segment, or a Windows short/long path pair is recognized as the same
/// project instead of looking like a cross-project attach.
pub(super) fn session_working_dir_for_client(
    session_dir: Option<&str>,
    client_dir: Option<&str>,
    create_working_dir: bool,
) -> Option<String> {
    let session = session_dir.map(str::trim).filter(|dir| !dir.is_empty());
    let client = client_dir.map(str::trim).filter(|dir| !dir.is_empty());
    match (session, client) {
        (Some(session), _) => Some(session.to_string()),
        (None, Some(client)) if create_working_dir => Some(client.to_string()),
        (None, _) => None,
    }
}
