//! Glue between the `/mcp` and `/skills` dialogs and the app: opening them,
//! routing keys, sending `mcp` runtime commands and applying the replies.

use super::*;
use crate::tui::backend::RemoteConnection;
use crate::tui::dialog::{Dialog, DialogKey, SkillRow};

impl App {
    pub(super) fn open_skills_dialog(&mut self) {
        self.refresh_skills_snapshot();
        let working_dir = self.session.working_dir.clone();
        let rows = self
            .current_skills_snapshot()
            .list()
            .iter()
            .map(|skill| {
                let path = skill.path.to_string_lossy();
                let source = if path.contains("/plugins/") {
                    "plugin"
                } else if working_dir
                    .as_deref()
                    .is_some_and(|dir| skill.path.starts_with(dir))
                {
                    "project"
                } else {
                    "user"
                };
                SkillRow {
                    name: skill.name.clone(),
                    description: skill.description.clone(),
                    source: source.to_string(),
                    author_user_only: skill.disable_model_invocation,
                }
            })
            .collect();
        self.dialog = Some(Dialog::skills(rows, crate::skill_overrides::load()));
    }

    /// Route a key to the open dialog. Returns the `mcp` args to send when the
    /// key triggered a server action.
    pub(super) fn handle_dialog_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Option<DialogKey> {
        match self.dialog.as_mut()?.handle_key(code, modifiers) {
            DialogKey::Handled => None,
            DialogKey::Close => {
                self.close_dialog();
                None
            }
            action @ (DialogKey::Mcp(_) | DialogKey::Rewind { .. }) => Some(action),
            action @ DialogKey::AskUser { .. } => {
                if !self.dialog.as_ref().is_some_and(Dialog::is_ask_user) {
                    self.dialog = None;
                }
                Some(action)
            }
        }
    }

    /// Show `ask_user` questions, after any already open.
    pub(super) fn open_ask_user_dialog(
        &mut self,
        request_id: String,
        questions: Vec<crate::protocol::AskUserQuestion>,
    ) {
        match self.dialog.as_mut().filter(|dialog| dialog.is_ask_user()) {
            Some(dialog) => dialog.queue_ask_user(request_id, questions),
            None => self.dialog = Some(Dialog::ask_user(request_id, questions)),
        }
    }

    /// The daemon closed this request; show why when it says.
    pub(super) fn close_ask_user_request(&mut self, request_id: &str, error: Option<String>) {
        if let Some(dialog) = self.dialog.as_mut().filter(|dialog| dialog.is_ask_user())
            && dialog.ask_user_closed(request_id)
        {
            self.dialog = None;
        }
        if let Some(error) = error {
            self.push_display_message(DisplayMessage::error(error));
        }
    }

    /// Open questions belonged to the dead connection; answers cannot reach
    /// the agent, which is told that no user is available.
    pub(in crate::tui::app) fn close_ask_user_dialogs_on_disconnect(&mut self) {
        if self.dialog.as_ref().is_some_and(Dialog::is_ask_user) {
            self.dialog = None;
            self.push_display_message(DisplayMessage::system(
                "Closed the agent's questions: the connection dropped before you answered."
                    .to_string(),
            ));
        }
    }

    /// Local and disconnected clients have no daemon to run MCP actions on.
    pub(super) fn handle_dialog_key_offline(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        match self.handle_dialog_key(code, modifiers) {
            Some(DialogKey::Rewind {
                checkpoint_id,
                mode,
            }) => {
                if self.is_remote {
                    self.rewind_dialog_error(
                        "Reconnect to the server before restoring a checkpoint.".to_string(),
                    );
                } else {
                    self.rewind_ui.local_restore = Some((checkpoint_id, mode));
                }
            }
            Some(DialogKey::AskUser { .. }) => {
                self.push_display_message(DisplayMessage::error(
                    "Not connected to the jcode server, so your answers were not sent.".to_string(),
                ));
            }
            Some(DialogKey::Mcp(_)) => {
                if let Some(dialog) = self.dialog.as_mut() {
                    dialog.pending_mcp = None;
                    dialog.set_status(Some("Not connected to the jcode server".to_string()));
                }
            }
            _ => {}
        }
    }

    fn close_dialog(&mut self) {
        self.rewind_ui.list_request = None;
        self.rewind_ui.last_empty_escape = None;
        if let Some(message) = self.dialog.take().and_then(|dialog| dialog.finish()) {
            self.push_display_message(DisplayMessage::system(message).with_title("Skills"));
        }
    }

    /// Reply to an `mcp` runtime command, matching Claude Code: a reconnect
    /// closes the dialog and reports in the transcript, enable/disable returns
    /// to the server list, a plain refresh just updates the views.
    pub(super) fn apply_mcp_reply(
        &mut self,
        reply: std::result::Result<(serde_json::Value, String), String>,
    ) {
        let Some(dialog) = self.dialog.as_mut().filter(|dialog| dialog.is_mcp()) else {
            match reply {
                Ok((_, message)) if !message.is_empty() => {
                    self.push_display_message(DisplayMessage::system(message).with_title("MCP"))
                }
                // The startup check: ask about project servers, like Claude Code.
                Ok((data, _)) => {
                    if self.dialog.is_none()
                        && let Some(server) = crate::tui::dialog::first_pending_approval(&data)
                    {
                        self.dialog = Some(Dialog::mcp_at_server(data, &server));
                    }
                }
                Err(error) => self.push_display_message(DisplayMessage::error(error)),
            }
            return;
        };
        let action = dialog.pending_mcp.take();
        match reply {
            // Claude Code reports a failed action in the transcript and closes;
            // a failed refresh stays in the dialog.
            Err(error) if action.is_some() => {
                self.dialog = None;
                self.push_display_message(DisplayMessage::error(error));
            }
            Err(error) => dialog.set_status(Some(error)),
            Ok((data, message)) => {
                let next_pending = crate::tui::dialog::first_pending_approval(&data);
                dialog.set_mcp(data);
                match action
                    .as_ref()
                    .and_then(|args| args.first())
                    .map(String::as_str)
                {
                    Some("reconnect" | "authenticate" | "clear-auth") => {
                        self.dialog = None;
                        self.push_display_message(
                            DisplayMessage::system(message).with_title("MCP"),
                        );
                    }
                    // Answered one prompt: ask about the next server, or finish.
                    Some("approve" | "approve-all" | "reject") => match next_pending {
                        Some(server) => {
                            dialog.open_server(&server);
                            self.set_status_notice(message);
                        }
                        None => {
                            self.dialog = None;
                            self.push_display_message(
                                DisplayMessage::system(message).with_title("MCP"),
                            );
                        }
                    },
                    Some(_) => {
                        dialog.pop_to_root();
                        self.set_status_notice(message);
                    }
                    None => {}
                }
            }
        }
    }
}

/// Send `/mcp [args]` to the daemon. With no args and no open dialog, opens
/// the dialog in its loading state first.
pub(super) async fn send_mcp_command(
    app: &mut App,
    remote: &mut RemoteConnection,
    args: Vec<String>,
) {
    match remote.runtime_command("mcp", args).await {
        Ok(id) => app.mcp_request_id = Some(id),
        Err(error) => {
            app.apply_mcp_reply(Err(format!("Failed to reach the jcode server: {error}")))
        }
    }
}
