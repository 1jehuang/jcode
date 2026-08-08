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
        let mut content = format!("Status: {}", status.into());
        if !note.is_empty() {
            content.push_str("\n\n");
            content.push_str(&note);
        }
        if action == crate::bus::ClientMaintenanceAction::Rebuild {
            content.push_str(
                "\n\nPipeline: git pull --ff-only → cargo build --release → cargo test --release -- --test-threads=1",
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
                crate::session_rebuild::spawn_background_session_rebuild(session_id);
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
            UpdateStatus::Downloading {
                version,
                downloaded,
                total,
            } => {
                self.background_client_action = Some(action);
                let progress =
                    crate::update::format_download_progress_bar(crate::update::DownloadProgress {
                        downloaded,
                        total,
                    });
                self.set_status_notice(format!("↑ {} · {}", version, progress));
                self.remove_client_maintenance_message(action);
            }
            UpdateStatus::Installing { version } => {
                self.background_client_action = Some(action);
                self.set_status_notice(format!("↑ {} · preparing", version));
                self.remove_client_maintenance_message(action);
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
                if crate::update::summary_is_divergence(&error)
                    || crate::update::summary_is_divergence(
                        error.trim_start_matches("Update failed: "),
                    )
                {
                    self.offer_update_merge(action, &error);
                } else {
                    // One line only: the full error is already in the log.
                    let reason = crate::update::summarize_update_error(&error);
                    self.set_status_notice(format!("Update failed: {reason}"));
                    self.set_client_maintenance_message(
                        action,
                        Self::client_maintenance_card_message(
                            action,
                            format!("failed ({reason})"),
                            "",
                        ),
                    );
                }
            }
        }
    }

    pub(super) fn maybe_finish_background_client_reload(&mut self) -> bool {
        if self.is_processing {
            return false;
        }

        // Downloading and installation are entirely background work. Once the
        // replacement is ready, do not take the terminal away while the user is
        // typing. The tick loop retries as soon as this small quiet window ends.
        const RELOAD_TYPING_QUIET_WINDOW: std::time::Duration =
            std::time::Duration::from_millis(1200);
        if self
            .last_user_interaction
            .is_some_and(|activity| activity.elapsed() < RELOAD_TYPING_QUIET_WINDOW)
        {
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
        #[cfg(not(test))]
        if let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            // Survives exec and lets the replacement measure the complete period
            // from the old client's final interactive state to its first frame.
            crate::env::set_var("JCODE_RELOAD_GAP_STARTED_MS", now.as_millis().to_string());
        }
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
                        "{} ready - will reload after the current turn",
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
                if !self.maybe_finish_background_client_reload() {
                    self.set_status_notice(format!("↑ {} ready · reloads when idle", version));
                    self.remove_client_maintenance_message(action);
                }
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
                if crate::update::summary_is_divergence(&message)
                    || crate::update::summary_is_divergence(
                        message.trim_start_matches("Update failed: "),
                    )
                {
                    self.offer_update_merge(action, &message);
                    return;
                }
                // One line only: the full error is already in the log.
                let reason = crate::update::summarize_update_error(&message);
                self.set_status_notice(format!("{} failed: {reason}", action.title()));
                self.set_client_maintenance_message(
                    action,
                    Self::client_maintenance_card_message(action, format!("failed ({reason})"), ""),
                );
            }
        }
    }

    /// Render a friendly "diverged" update card and arm the merge offer so the
    /// user can hand the reconciliation to a fresh jcode agent with one key.
    ///
    /// This replaces the old generic "Status: failed / Continuing with the
    /// current version." card for the specific (and recoverable) case where the
    /// local checkout and upstream have diverged.
    pub(super) fn offer_update_merge(
        &mut self,
        action: crate::bus::ClientMaintenanceAction,
        detail: &str,
    ) {
        let repo_dir = crate::build::get_repo_dir();
        let key_label = crate::tui::keybind::fallback_switch_key_label();
        let detail = detail.trim().to_string();

        // A single, friendly line: no "Status: failed" header and no "Continuing
        // with the current version." footer. Put the recovery hotkey first so it
        // remains visible when the renderer truncates the notice to one row.
        // Bypass `client_maintenance_card_message` (which would prepend a
        // "Status:" line) and set the card content directly.
        let content = format!(
            "Update diverged. Press {} to let a jcode agent merge local and upstream (or run `git pull` / `git rebase` yourself).",
            key_label
        );
        self.set_client_maintenance_message(action, content);
        self.set_status_notice(format!(
            "Local and upstream diverged - press {} to let an agent merge",
            key_label
        ));
        self.pending_merge_offer = Some(super::PendingMergeOffer { repo_dir, detail });
    }

    pub(super) fn clear_update_merge_offer(&mut self) {
        self.pending_merge_offer = None;
    }

    /// Whether the configured fallback/accept key should be treated as accepting
    /// the armed merge offer (true only while an offer is pending).
    pub(super) fn merge_offer_key_matches(
        &self,
        code: crossterm::event::KeyCode,
        modifiers: crossterm::event::KeyModifiers,
    ) -> bool {
        self.pending_merge_offer.is_some() && self.fallback_switch_key_matches(code, modifiers)
    }

    /// Accept the armed merge offer: spawn a fresh jcode session pre-loaded with
    /// a prompt to reconcile the diverged branches. Returns true when an offer
    /// was present and consumed.
    pub(super) fn accept_update_merge_offer(&mut self) -> bool {
        let Some(offer) = self.pending_merge_offer.take() else {
            return false;
        };

        let repo_label = offer
            .repo_dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "the jcode repository".to_string());
        let prompt = format!(
            "A jcode self-update could not fast-forward because the local checkout and upstream have diverged.\n\n\
Repository: {repo}\n\
Update error: {detail}\n\n\
Please reconcile the local and upstream histories so the update can proceed:\n\
1. Inspect the divergence (`git status`, `git fetch`, `git log --oneline --graph HEAD @{{u}}`).\n\
2. Merge or rebase onto the upstream branch, resolving any conflicts.\n\
3. Verify the build still works.\n\
Do not force-push or discard local commits without confirming they are already upstream.",
            repo = repo_label,
            detail = if offer.detail.is_empty() {
                "(local and upstream have diverged)".to_string()
            } else {
                offer.detail.clone()
            },
        );

        match self.launch_update_merge_agent(prompt, offer.repo_dir.as_deref()) {
            Ok(true) => {
                self.push_display_message(DisplayMessage::system(
                    "↗ Spawned a jcode agent to merge the diverged update.",
                ));
                self.set_status_notice("Merge agent launched");
            }
            Ok(false) => {
                self.push_display_message(DisplayMessage::system(
                    "Could not open a new terminal for the merge agent. Run `git pull` / `git rebase` manually, or start `jcode` in the repo and ask it to merge.",
                ));
                self.set_status_notice("No terminal available for merge agent");
            }
            Err(error) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to spawn merge agent: {}",
                    error
                )));
                self.set_status_notice("Merge agent failed to start");
            }
        }
        true
    }

    /// Spawn a fresh jcode session, in the repo directory when known, with a
    /// startup prompt instructing it to merge the diverged update.
    fn launch_update_merge_agent(
        &self,
        prompt: String,
        repo_dir: Option<&std::path::Path>,
    ) -> anyhow::Result<bool> {
        let mut session = crate::session::Session::create(Some(self.session.id.clone()), None);
        if let Some(dir) = repo_dir {
            session.working_dir = Some(dir.display().to_string());
        } else {
            session.working_dir = self.session.working_dir.clone();
        }
        let session_id = session.id.clone();
        let _ = session.save();
        App::save_startup_submission_for_session(&session_id, prompt, Vec::new());
        self.spawn_merge_session_terminal(&session_id, repo_dir)
    }

    fn spawn_merge_session_terminal(
        &self,
        session_id: &str,
        repo_dir: Option<&std::path::Path>,
    ) -> anyhow::Result<bool> {
        let exe = super::launch_client_executable();
        let cwd = repo_dir
            .map(std::path::Path::to_path_buf)
            .filter(|path| path.is_dir())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let socket = std::env::var("JCODE_SOCKET").ok();
        super::spawn_in_new_terminal(&exe, session_id, &cwd, socket.as_deref())
    }
}
