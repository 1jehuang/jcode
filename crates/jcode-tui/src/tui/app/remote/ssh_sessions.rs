//! SSH session commands use the attached daemon exclusively.
use super::*;

#[derive(Default)]
pub(in crate::tui::app) struct State {
    back: Vec<String>,
    navigation: Option<Navigation>,
    child: Option<Child>,
    ready_prompt: Option<String>,
}

struct Navigation {
    request_id: u64,
    target: String,
    previous: Option<String>,
    returning: bool,
    prompt: Option<String>,
}

struct Child {
    request_id: u64,
    prompt: Option<String>,
}

fn token(input: &str) -> Option<(&str, &str)> {
    let input = input.trim();
    let end = input.find(char::is_whitespace).unwrap_or(input.len());
    let command = &input[..end];
    matches!(
        command,
        "/resume"
            | "/sessions"
            | "/session"
            | "/active"
            | "/catchup"
            | "/back"
            | "/save"
            | "/unsave"
            | "/fork"
            | "/split"
            | "/btw"
            | "/transfer"
            | "/transcript"
    )
    .then_some((command, input[end..].trim()))
}

async fn navigate(
    app: &mut App,
    remote: &mut RemoteConnection,
    target: &str,
    returning: bool,
    prompt: Option<String>,
) -> Result<()> {
    let previous = remote.session_id().map(str::to_owned);
    let request_id = remote.resume_ssh_session(target).await?;
    app.ssh_sessions.navigation = Some(Navigation {
        request_id,
        target: target.to_owned(),
        previous,
        returning,
        prompt,
    });
    app.set_status_notice(format!("Resuming remote session {target}"));
    Ok(())
}

/// Claim session commands before any local picker/filesystem fallback.
pub(in crate::tui::app) async fn handle_command(
    app: &mut App,
    remote: &mut RemoteConnection,
    input: &str,
) -> Result<bool> {
    if !crate::tui::is_ssh_remote() {
        return Ok(false);
    }
    let Some((command, args)) = token(input) else {
        return Ok(false);
    };
    if app.ssh_sessions.navigation.is_some() || app.ssh_sessions.child.is_some() {
        app.push_display_message(DisplayMessage::system(
            "A remote session switch is pending. Wait for its history or error response.",
        ));
        return Ok(true);
    }
    let switching = matches!(
        command,
        "/resume" | "/back" | "/fork" | "/split" | "/btw" | "/transfer"
    ) || (command == "/session" && !args.is_empty());
    if switching && app.is_processing {
        app.push_display_message(DisplayMessage::system(
            "Wait for this turn to finish or cancel it before switching remote sessions.",
        ));
        return Ok(true);
    }
    match command {
        "/resume" | "/session" if !args.is_empty() && args != "saved" && args != "all" => {
            if args.split_whitespace().count() != 1 {
                anyhow::bail!("Usage: {command} <remote-session-id>");
            }
            navigate(app, remote, args, false, None).await?;
        }
        "/resume" | "/sessions" => {
            remote
                .runtime_command(
                    "sessions",
                    args.split_whitespace().map(str::to_owned).collect(),
                )
                .await?;
        }
        "/session" | "/active" | "/transcript" => {
            remote
                .runtime_command(
                    &command[1..],
                    args.split_whitespace().map(str::to_owned).collect(),
                )
                .await?;
        }
        "/catchup" if args == "current" => {
            remote.refresh_ssh_history().await?;
            app.set_status_notice("Refreshing remote history");
        }
        "/catchup" => {
            remote
                .runtime_command(
                    "catchup",
                    args.split_whitespace().map(str::to_owned).collect(),
                )
                .await?;
        }
        "/back" => {
            if !args.is_empty() {
                anyhow::bail!("Usage: /back");
            }
            if let Some(target) = app.ssh_sessions.back.last().cloned() {
                navigate(app, remote, &target, true, None).await?;
            } else {
                app.push_display_message(DisplayMessage::system(
                    "No previous remote session in this connection. Use /resume to list sessions.",
                ));
            }
        }
        "/save" => {
            remote
                .set_session_saved(true, (!args.is_empty()).then(|| args.to_owned()))
                .await?;
            app.set_status_notice("Requested remote session bookmark");
        }
        "/unsave" => {
            if !args.is_empty() {
                anyhow::bail!("Usage: /unsave");
            }
            remote.set_session_saved(false, None).await?;
            app.set_status_notice("Requested remote bookmark removal");
        }
        "/fork" | "/split" | "/btw" | "/transfer" => {
            if command == "/btw" && args.is_empty() {
                anyhow::bail!("Usage: /btw <side-question>");
            }
            if command == "/transfer" && !args.is_empty() {
                anyhow::bail!("Usage: /transfer");
            }
            let request_id = if command == "/transfer" {
                remote.transfer().await?
            } else {
                remote.split().await?
            };
            app.ssh_sessions.child = Some(Child {
                request_id,
                prompt: (!args.is_empty()).then(|| args.to_owned()),
            });
            app.set_status_notice("Creating remote child session");
        }
        _ => unreachable!("token only accepts session commands"),
    }
    Ok(true)
}

/// Run before normal server-event handling. Only a child response belonging to
/// this workflow is consumed. History must still flow through the replay path.
pub(in crate::tui::app) async fn handle_event(
    app: &mut App,
    remote: &mut RemoteConnection,
    event: &ServerEvent,
) -> Result<bool> {
    if !crate::tui::is_ssh_remote() {
        return Ok(false);
    }
    match event {
        ServerEvent::SplitResponse {
            id, new_session_id, ..
        } if app
            .ssh_sessions
            .child
            .as_ref()
            .is_some_and(|child| child.request_id == *id) =>
        {
            let child = app.ssh_sessions.child.take().expect("matched child");
            navigate(app, remote, new_session_id, false, child.prompt).await?;
            return Ok(true);
        }
        ServerEvent::History { session_id, .. }
            if app
                .ssh_sessions
                .navigation
                .as_ref()
                .is_some_and(|nav| nav.target == *session_id) =>
        {
            let navigation = app
                .ssh_sessions
                .navigation
                .take()
                .expect("matched navigation");
            if navigation.returning {
                app.ssh_sessions.back.pop();
            } else if let Some(previous) = navigation
                .previous
                .filter(|previous| previous != session_id)
            {
                app.ssh_sessions.back.push(previous);
            }
            app.ssh_sessions.ready_prompt = navigation.prompt;
        }
        ServerEvent::Error { id, .. } => {
            if app
                .ssh_sessions
                .child
                .as_ref()
                .is_some_and(|child| child.request_id == *id)
            {
                app.ssh_sessions.child = None;
            }
            if app
                .ssh_sessions
                .navigation
                .as_ref()
                .is_some_and(|nav| nav.request_id == *id)
            {
                app.ssh_sessions.navigation = None;
                // A failed switch leaves the parent attached. Refresh explicitly
                // so a rejected resume cannot leave has_loaded_history false.
                remote.refresh_ssh_history().await?;
            }
        }
        _ => {}
    }
    Ok(false)
}

/// Submit a side question after the normal History handler has replaced replay
/// state. The standard send path initializes timing, retry and usage counters.
pub(in crate::tui::app) async fn after_event(
    app: &mut App,
    remote: &mut RemoteConnection,
) -> Result<()> {
    if let Some(prompt) = app.ssh_sessions.ready_prompt.take() {
        begin_remote_send(app, remote, prompt.clone(), vec![], false, None, false, 0).await?;
        app.push_display_message(DisplayMessage::user(prompt));
    }
    Ok(())
}

#[cfg(test)]
#[path = "ssh_sessions_tests.rs"]
mod tests;
