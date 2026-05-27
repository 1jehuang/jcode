use super::*;

impl App {
    fn client_maintenance_busy_message(
        current: crate::bus::ClientMaintenanceAction,
        requested: crate::bus::ClientMaintenanceAction,
    ) -> String {
        if current == requested {
            format!("{} already running in the background.", current.title())
        } else {
            format!(
                "{} already running in the background. Wait for it to finish before starting {}.",
                current.title(),
                requested.noun()
            )
        }
    }

    fn client_maintenance_card_title(action: crate::bus::ClientMaintenanceAction) -> String {
        action.title().to_string()
    }

    fn client_maintenance_card_message(
        action: crate::bus::ClientMaintenanceAction,
        status: impl Into<String>,
        note: impl Into<String>,
    ) -> String {
        let note = note.into();
        let mut content = format!("**Status:** {}", status.into());
        if !note.is_empty() {
            content.push_str("\n\n");
            content.push_str(&note);
        }
        if action == crate::bus::ClientMaintenanceAction::Rebuild {
            content.push_str(
                "\n\n**Pipeline:** `git pull --ff-only` → `cargo build --release` → `cargo test --release -- --test-threads=1`",
            );
        }
        content
    }

    fn set_client_maintenance_message(
        &mut self,
        action: crate::bus::ClientMaintenanceAction,
        content: String,
    ) {
        let title = Self::client_maintenance_card_title(action);
        if let Some(idx) = self
            .display_messages
            .iter()
            .rposition(|message| Self::is_client_maintenance_message(message, &title))
        {
            let message = &mut self.display_messages[idx];
            let title_changed = message.title.as_deref() != Some(title.as_str());
            if title_changed {
                message.title = Some(title);
            }
            if message.content != content || title_changed {
                message.content = content;
                self.bump_display_messages_version();
            }
        } else {
            self.push_display_message(DisplayMessage::system(content).with_title(title));
        }
    }

    fn remove_client_maintenance_message(
        &mut self,
        action: crate::bus::ClientMaintenanceAction,
    ) -> bool {
        let title = Self::client_maintenance_card_title(action);
        let Some(idx) = self
            .display_messages
            .iter()
            .rposition(|message| Self::is_client_maintenance_message(message, &title))
        else {
            return false;
        };
        self.display_messages.remove(idx);
        self.bump_display_messages_version();
        true
    }

    pub(super) fn start_background_client_rebuild(&mut self, session_id: String) {
        self.start_background_client_maintenance(
            crate::bus::ClientMaintenanceAction::Rebuild,
            session_id,
        );
    }

    pub(super) fn start_background_client_update(&mut self, session_id: String) {
        self.start_background_client_maintenance(
            crate::bus::ClientMaintenanceAction::Update,
            session_id,
        );
    }

    fn start_background_client_maintenance(
        &mut self,
        action: crate::bus::ClientMaintenanceAction,
        session_id: String,
    ) {
        if let Some(current) = self.background_client_action {
            let message = Self::client_maintenance_busy_message(current, action);
            self.set_status_notice(&message);
            self.set_client_maintenance_message(
                current,
                Self::client_maintenance_card_message(current, "already running", message),
            );
            return;
        }

        self.background_client_action = Some(action);
        self.pending_background_client_reload = None;

        match action {
            crate::bus::ClientMaintenanceAction::Update => {
                crate::update::spawn_background_session_update(session_id);
            }
            crate::bus::ClientMaintenanceAction::Rebuild => {
                self.set_status_notice("Starting background rebuild...");
                self.set_client_maintenance_message(
                    action,
                    Self::client_maintenance_card_message(
                        action,
                        "starting background rebuild",
                        "Running in the background. jcode will reload automatically after the rebuild succeeds.",
                    ),
                );
                crate::cli::hot_exec::spawn_background_session_rebuild(session_id);
            }
        }
    }

    pub(super) fn handle_update_status(&mut self, status: crate::bus::UpdateStatus) {
        use crate::bus::{ClientMaintenanceAction, UpdateStatus};

        let action = ClientMaintenanceAction::Update;
        match status {
            UpdateStatus::Checking => {
                // Background update checks run at startup for normal sessions. Keep the
                // UI quiet unless there is an update to report or work to perform.
            }
            UpdateStatus::Available { current, latest } => {
                self.set_status_notice(format!("Update available: {} → {}", current, latest));
                self.set_client_maintenance_message(
                    action,
                    Self::client_maintenance_card_message(
                        action,
                        format!("{} → {} available", current, latest),
                        format!(
                            "Current: `{}`\nLatest: `{}`\n\nRun `/update` to install, or wait while auto-update continues if enabled.",
                            current, latest
                        ),
                    ),
                );
            }
            UpdateStatus::Downloading { version } => {
                self.background_client_action = Some(action);
                self.set_status_notice(format!("Updating to {}...", version));
                self.set_client_maintenance_message(
                    action,
                    Self::client_maintenance_card_message(
                        action,
                        format!("downloading {}", version),
                        "jcode will restart automatically when the update is ready.",
                    ),
                );
            }
            UpdateStatus::Installing { version } => {
                self.background_client_action = Some(action);
                self.set_status_notice(format!("Installing {}...", version));
                self.set_client_maintenance_message(
                    action,
                    Self::client_maintenance_card_message(
                        action,
                        format!("installing {}", version),
                        "jcode will restart automatically when the update is ready.",
                    ),
                );
            }
            UpdateStatus::Installed { version } => {
                self.background_client_action = None;
                self.set_status_notice(format!("Updated to {}; restarting...", version));
                self.set_client_maintenance_message(
                    action,
                    Self::client_maintenance_card_message(
                        action,
                        format!("updated to {}", version),
                        "Restarting now.",
                    ),
                );
            }
            UpdateStatus::UpToDate => {
                if self.background_client_action == Some(action) {
                    self.background_client_action = None;
                }
                self.pending_background_client_reload = None;
                self.remove_client_maintenance_message(action);
            }
            UpdateStatus::Error(error) => {
                self.background_client_action = None;
                self.pending_background_client_reload = None;
                self.set_status_notice("Update failed; continuing current version");
                self.set_client_maintenance_message(
                    action,
                    Self::client_maintenance_card_message(
                        action,
                        "failed",
                        format!("{}\n\nContinuing with the current version.", error),
                    ),
                );
            }
        }
    }

    pub(super) fn maybe_finish_background_client_reload(&mut self) -> bool {
        if self.is_processing {
            return false;
        }

        let Some((session_id, action)) = self.pending_background_client_reload.take() else {
            return false;
        };

        self.set_client_maintenance_message(
            action,
            Self::client_maintenance_card_message(
                action,
                "reloading client",
                "The new binary is ready, so jcode is switching over now.",
            ),
        );
        self.save_input_for_reload(&session_id);
        self.reload_requested = Some(session_id);
        self.should_quit = true;
        true
    }

    pub(super) fn handle_session_update_status(&mut self, status: crate::bus::SessionUpdateStatus) {
        use crate::bus::{ClientMaintenanceAction, SessionUpdateStatus};

        let Some(active_session_id) = self.active_client_session_id().map(str::to_string) else {
            return;
        };

        match status {
            SessionUpdateStatus::Status {
                session_id,
                action,
                message,
            } => {
                if session_id != active_session_id {
                    return;
                }
                self.background_client_action = Some(action);
                self.set_status_notice(message.clone());
                self.set_client_maintenance_message(
                    action,
                    Self::client_maintenance_card_message(
                        action,
                        message,
                        "Still running in the background. jcode will reload automatically when ready.",
                    ),
                );
            }
            SessionUpdateStatus::NoUpdate {
                session_id,
                current,
            } => {
                if session_id != active_session_id {
                    return;
                }
                self.background_client_action = None;
                self.pending_background_client_reload = None;
                let message = format!("Already up to date ({})", current);
                self.set_status_notice(&message);
                self.set_client_maintenance_message(
                    ClientMaintenanceAction::Update,
                    Self::client_maintenance_card_message(
                        ClientMaintenanceAction::Update,
                        "already up to date",
                        format!("Current version: `{}`", current),
                    ),
                );
            }
            SessionUpdateStatus::ReadyToReload {
                session_id,
                action,
                version,
            } => {
                if session_id != active_session_id {
                    return;
                }
                self.background_client_action = None;
                let ready_message = match action {
                    ClientMaintenanceAction::Update => format!("✅ Updated to {}.", version),
                    ClientMaintenanceAction::Rebuild => {
                        format!("✅ Rebuild finished ({}).", version)
                    }
                };
                if self.is_processing {
                    self.pending_background_client_reload = Some((session_id, action));
                    self.set_status_notice(format!(
                        "{} ready — will reload after the current turn",
                        action.title()
                    ));
                    self.set_client_maintenance_message(
                        action,
                        Self::client_maintenance_card_message(
                            action,
                            ready_message,
                            "Waiting for the current turn to finish before reloading.",
                        ),
                    );
                    return;
                }

                self.set_client_maintenance_message(
                    action,
                    Self::client_maintenance_card_message(action, ready_message, "Reloading now."),
                );
                self.pending_background_client_reload = Some((session_id, action));
                self.maybe_finish_background_client_reload();
            }
            SessionUpdateStatus::Error {
                session_id,
                action,
                message,
            } => {
                if session_id != active_session_id {
                    return;
                }
                self.background_client_action = None;
                self.pending_background_client_reload = None;
                self.set_status_notice(format!("{} failed", action.title()));
                self.set_client_maintenance_message(
                    action,
                    Self::client_maintenance_card_message(action, "failed", message.clone()),
                );
                self.push_display_message(DisplayMessage::error(message));
            }
        }
    }
}
