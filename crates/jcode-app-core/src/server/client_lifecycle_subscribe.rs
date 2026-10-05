//! Subscribe working-dir helpers split out of `client_lifecycle.rs` so that
//! file stays within its ratcheting size budget.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::{SessionAgents, SwarmMember};
use crate::protocol::Request;

pub(super) fn required_subscribe_working_dir(
    working_dir: Option<&str>,
) -> std::result::Result<&str, String> {
    let working_dir = working_dir
        .map(str::trim)
        .filter(|dir| !dir.is_empty())
        .ok_or_else(|| "Subscribe requires the client's working directory".to_string())?;
    if !Path::new(working_dir).is_absolute() {
        return Err("Subscribe working_dir must be an absolute path".to_string());
    }
    Ok(working_dir)
}

pub(super) fn initial_subscribe_working_dir(
    request: &Request,
) -> std::result::Result<String, String> {
    match request {
        Request::Subscribe {
            working_dir,
            continue_on_disconnect,
            ..
        } => validated_subscribe_working_dir(working_dir.as_deref(), *continue_on_disconnect)
            .map(str::to_string),
        _ => Err(
            "Client must Subscribe with a working_dir before sending stateful requests".to_string(),
        ),
    }
}

/// A reattachment names an existing session, not a new client working directory.
/// Resolve an omitted cwd before provisional initialization, never from the
/// daemon/bridge process cwd. Idle empty sessions may exist only in memory.
pub(super) async fn resolve_target_subscribe_working_dir(
    request: &mut Request,
    sessions: &SessionAgents,
    members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> std::result::Result<(), String> {
    let Request::Subscribe {
        working_dir,
        target_session_id: Some(target),
        ..
    } = request
    else {
        return Ok(());
    };
    if working_dir.is_some() {
        return Ok(());
    }
    let live = sessions.read().await.get(target).cloned();
    let resolved = if let Some(live) = live {
        let idle_cwd = live
            .try_lock()
            .ok()
            .and_then(|agent| agent.working_dir().map(str::to_string));
        if idle_cwd.is_some() {
            idle_cwd
        } else {
            // A generating Agent owns its mutex. The member records the same
            // session root, so attaching must not wait for the model turn.
            members
                .read()
                .await
                .get(target)
                .and_then(|member| member.working_dir.as_ref())
                .map(|path| path.to_string_lossy().into_owned())
        }
    } else {
        crate::session::Session::load_startup_stub(target)
            .ok()
            .and_then(|session| session.working_dir)
    };
    *working_dir = Some(resolved.ok_or_else(|| {
        format!("Unknown session '{target}' or session has no working directory")
    })?);
    Ok(())
}

pub(super) fn validated_subscribe_working_dir(
    working_dir: Option<&str>,
    remote_continuation: bool,
) -> std::result::Result<&str, String> {
    let working_dir = required_subscribe_working_dir(working_dir)?;
    if remote_continuation && !Path::new(working_dir).is_dir() {
        return Err(format!(
            "Remote working directory must exist and be a directory on the server: {working_dir}"
        ));
    }
    Ok(working_dir)
}

pub(super) fn new_session_system_prompt<'a>(
    provisional_session: bool,
    target_session_id: Option<&str>,
    system_prompt: Option<&'a str>,
) -> Option<&'a str> {
    if provisional_session && target_session_id.is_none() {
        system_prompt
    } else {
        None
    }
}

pub(super) fn initial_subscribe_terminal_env(request: &Request) -> Vec<(String, String)> {
    match request {
        Request::Subscribe { terminal_env, .. } => terminal_env.clone(),
        _ => Vec::new(),
    }
}
