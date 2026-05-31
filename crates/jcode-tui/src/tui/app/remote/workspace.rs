use super::{App, DisplayMessage, begin_remote_split_launch};
use crate::tui::backend::RemoteConnection;
use crate::tui::keybind::WorkspaceNavigationDirection;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};

pub(super) async fn handle_workspace_navigation_key(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
    remote: &mut RemoteConnection,
) -> Result<bool> {
    if !crate::tui::workspace_client::is_enabled() {
        return Ok(false);
    }

    let Some(direction) = app.workspace_navigation_keys.direction_for(code, modifiers) else {
        return Ok(false);
    };

    let target = match direction {
        WorkspaceNavigationDirection::Left => crate::tui::workspace_client::navigate_left(),
        WorkspaceNavigationDirection::Right => crate::tui::workspace_client::navigate_right(),
        WorkspaceNavigationDirection::Up => crate::tui::workspace_client::navigate_up(),
        WorkspaceNavigationDirection::Down => crate::tui::workspace_client::navigate_down(),
    };

    if app.is_processing {
        app.set_status_notice("Finish current work before moving workspace focus");
        return Ok(true);
    }

    let Some(target_session_id) = target else {
        app.set_status_notice("No workspace session in that direction");
        return Ok(true);
    };
    remote.resume_session(&target_session_id).await?;
    let label = crate::id::extract_session_name(&target_session_id)
        .map(|name| name.to_string())
        .unwrap_or(target_session_id);
    app.set_status_notice(format!("Workspace → {}", label));
    Ok(true)
}

pub(super) async fn handle_workspace_command(
    app: &mut App,
    remote: &mut RemoteConnection,
    trimmed: &str,
) -> Result<bool> {
    if !trimmed.starts_with("/workspace") {
        return Ok(false);
    }

    let current_session = app
        .remote_session_id
        .as_deref()
        .or(app.resume_session_id.as_deref())
        .or(Some(app.session.id.as_str()));

    match trimmed {
        "/workspace" | "/workspace status" => {
            app.push_display_message(DisplayMessage::system(
                crate::tui::workspace_client::status_summary(),
            ));
            return Ok(true);
        }
        "/workspace on" | "/workspace import" => {
            crate::tui::workspace_client::enable(current_session, &app.remote_sessions);
            app.set_status_notice("Workspace mode enabled");
            app.push_display_message(DisplayMessage::system(
                crate::tui::workspace_client::status_summary(),
            ));
            return Ok(true);
        }
        "/workspace off" => {
            crate::tui::workspace_client::disable();
            app.set_status_notice("Workspace mode disabled");
            app.push_display_message(DisplayMessage::system("Workspace mode: off".to_string()));
            return Ok(true);
        }
        _ => {}
    }

    let target = match trimmed {
        "/workspace add" | "/workspace add right" => {
            Some(crate::tui::workspace_client::WorkspaceSplitTarget::Right)
        }
        "/workspace add up" => Some(crate::tui::workspace_client::WorkspaceSplitTarget::Up),
        "/workspace add down" => Some(crate::tui::workspace_client::WorkspaceSplitTarget::Down),
        _ => None,
    };

    if let Some(target) = target {
        crate::tui::workspace_client::enable(current_session, &app.remote_sessions);
        crate::tui::workspace_client::queue_split_target(target);
        app.pending_split_label = Some("Workspace".to_string());
        if app.is_processing {
            app.pending_split_request = true;
            app.push_display_message(DisplayMessage::system(
                "Workspace add queued - new session will be created when idle.".to_string(),
            ));
            app.set_status_notice("Workspace add queued");
        } else {
            begin_remote_split_launch(app, "Workspace");
            remote.split().await?;
        }
        return Ok(true);
    }

    app.push_display_message(DisplayMessage::system(
        "/workspace\n  Show workspace status.\n\n/workspace on\n  Enable/import workspace mode for current remote sessions.\n\n/workspace off\n  Disable workspace mode.\n\n/workspace add\n  Split current session and add it to the right in the current workspace row.\n\n/workspace add up\n  Split current session into the workspace above.\n\n/workspace add down\n  Split current session into the workspace below."
            .to_string(),
    ));
    Ok(true)
}
