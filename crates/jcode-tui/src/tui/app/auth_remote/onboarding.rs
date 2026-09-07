//! First-attach import suggestion. Probes only remote status, never local credentials.
use super::{
    App, DisplayMessage, Operation, PendingLogin, Phase, RemoteLogin, Reply, Target, Task,
};
use crate::tui::{InlineInteractiveState, PickerAction, PickerEntry, PickerKind, PickerOption};

#[derive(Default)]
pub(in crate::tui::app) struct Onboarding {
    checked: bool,
    task: Option<Task>,
}

#[cfg(test)]
mod tests {
    use super::super::command::ProviderStatus;
    use super::super::tests::with_app;
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    fn empty_status() -> Vec<ProviderStatus> {
        crate::provider_catalog::auth_status_login_providers()
            .into_iter()
            .map(|p| ProviderStatus {
                id: p.id.into(),
                state: crate::auth::AuthState::NotConfigured,
                method_detail: "not configured".into(),
            })
            .collect()
    }

    fn queue_empty_status(app: &mut App) {
        app.remote_login_onboarding = Onboarding {
            checked: false,
            task: Some(Task::ready(Ok(Reply::Status {
                providers: empty_status(),
            }))),
        };
    }

    #[test]
    fn ssh_onboarding_requires_complete_empty_status_including_api_keys() {
        assert!(remote_has_no_logins(&empty_status()));
        assert!(!remote_has_no_logins(&[]));
        let mut partial = empty_status();
        partial.pop();
        assert!(!remote_has_no_logins(&partial));
        for index in 0..empty_status().len() {
            for state in [
                crate::auth::AuthState::Available,
                crate::auth::AuthState::Expired,
            ] {
                let mut statuses = empty_status();
                statuses[index].state = state;
                assert!(!remote_has_no_logins(&statuses), "{}", statuses[index].id);
            }
        }
    }

    #[test]
    fn ssh_onboarding_offers_once_and_no_opens_normal_login_without_copying() {
        with_app(|app| {
            queue_empty_status(app);
            assert!(app.poll_ssh_login_onboarding());
            assert!(app.remote_login.as_ref().unwrap().phase == Phase::ImportOffer);
            assert!(app.remote_login.as_ref().unwrap().task.is_none());
            assert!(
                app.display_messages()
                    .last()
                    .unwrap()
                    .content
                    .contains("No logins are configured on test-remote")
            );
            let picker = app.inline_interactive_state.as_ref().unwrap();
            assert_eq!(picker.entries[picker.selected].name, "No");
            app.handle_ssh_login_key(KeyCode::Enter, KeyModifiers::NONE, None);
            assert!(app.remote_login.as_ref().unwrap().phase == Phase::Choosing);
            assert!(app.inline_interactive_state.as_ref().unwrap().entries.len() > 2);
            app.cancel_ssh_login();
            assert!(!app.poll_ssh_login_onboarding());
            assert!(app.remote_login.is_none());
            assert!(app.pasted_contents.is_empty());
            assert!(app.queued_messages.is_empty());
        });
    }

    #[test]
    fn ssh_onboarding_yes_chooses_provider_then_requires_separate_copy_consent() {
        with_app(|app| {
            queue_empty_status(app);
            assert!(app.poll_ssh_login_onboarding());
            app.handle_ssh_login_key(KeyCode::Up, KeyModifiers::NONE, None);
            app.handle_ssh_login_key(KeyCode::Enter, KeyModifiers::NONE, None);
            assert_eq!(
                app.inline_interactive_state.as_ref().unwrap().entries.len(),
                2
            );
            assert!(app.remote_login.as_ref().unwrap().task.is_none());
            app.handle_ssh_login_key(KeyCode::Enter, KeyModifiers::NONE, None);
            assert!(app.remote_login.as_ref().unwrap().phase == Phase::ImportConsent);
            assert_eq!(app.remote_login.as_ref().unwrap().provider, "openai");
            assert!(app.remote_login.as_ref().unwrap().task.is_none());
            // Default No is conservative even after Yes to the initial offer.
            app.handle_ssh_login_key(KeyCode::Enter, KeyModifiers::NONE, None);
            assert!(app.remote_login.is_none());
            assert!(
                app.display_messages()
                    .last()
                    .unwrap()
                    .content
                    .contains("No local credentials were read or copied")
            );
        });
    }

    #[test]
    fn ssh_onboarding_pasted_yes_and_no_stay_private() {
        with_app(|app| {
            queue_empty_status(app);
            assert!(app.poll_ssh_login_onboarding());
            app.handle_paste("yes".into());
            assert_eq!(app.input, "[hidden login input]");
            assert!(app.pasted_contents.is_empty());
            app.handle_ssh_login_key(KeyCode::Enter, KeyModifiers::NONE, None);
            assert_eq!(
                app.inline_interactive_state.as_ref().unwrap().entries.len(),
                2
            );
            app.handle_ssh_login_key(KeyCode::Enter, KeyModifiers::NONE, None);
            app.handle_paste("no".into());
            app.handle_ssh_login_key(KeyCode::Enter, KeyModifiers::NONE, None);
            assert!(app.remote_login.is_none());
            assert!(app.input.is_empty());
        });
    }

    #[test]
    fn ssh_onboarding_never_replaces_drafts_or_explicit_login() {
        with_app(|app| {
            queue_empty_status(app);
            app.input = "unfinished draft".into();
            assert!(!app.poll_ssh_login_onboarding());
            assert_eq!(app.input, "unfinished draft");
            assert!(app.remote_login.is_none());
            app.input.clear();
            app.pending_turn = true;
            assert!(!app.poll_ssh_login_onboarding());
            app.pending_turn = false;
            app.handle_ssh_login_command("/login");
            assert!(app.remote_login_onboarding.task.is_none());
            app.cancel_ssh_login();
            assert!(!app.poll_ssh_login_onboarding());
        });
    }

    #[test]
    fn ssh_onboarding_unknown_status_never_claims_signed_out_or_retries() {
        with_app(|app| {
            for reply in [
                Err("status failed"),
                Ok(Reply::Status { providers: vec![] }),
            ] {
                app.remote_login_onboarding = Onboarding {
                    checked: false,
                    task: Some(Task::ready(reply)),
                };
                assert!(!app.poll_ssh_login_onboarding());
                assert!(app.remote_login_onboarding.checked);
                assert!(app.remote_login.is_none());
                assert!(!app.poll_ssh_login_onboarding());
            }
        });
    }
}

impl Onboarding {
    pub(super) fn dismiss(&mut self) {
        self.checked = true;
        self.task = None;
    }
}

fn remote_has_no_logins(providers: &[super::command::ProviderStatus]) -> bool {
    // Missing/unknown rows, expired logins, and credentials for non-OAuth routes
    // must never be mistaken for an empty host. Auto-import is not a status row.
    let expected = crate::provider_catalog::auth_status_login_providers();
    !expected.is_empty()
        && providers
            .iter()
            .all(|p| p.state == crate::auth::AuthState::NotConfigured)
        && expected.iter().all(|entry| {
            providers
                .iter()
                .any(|p| p.id == entry.id && p.state == crate::auth::AuthState::NotConfigured)
        })
}

impl App {
    pub(in crate::tui::app) fn poll_ssh_login_onboarding(&mut self) -> bool {
        if !crate::tui::is_ssh_remote() || self.remote_login_onboarding.checked {
            return false;
        }
        // Never steal a draft, interrupt a running turn, or replace another UI.
        if self.remote_login.is_some() || self.pending_login.is_some() {
            self.remote_login_onboarding.dismiss();
            return false;
        }
        if self.should_quit
            || self.is_processing()
            || self.pending_turn
            || !self.input.is_empty()
            || self.inline_interactive_state.is_some()
            || self.inline_view_state.is_some()
            || self.login_picker_overlay.is_some()
            || self.remote_history_wait_started.is_some()
            || self.pending_prompt_before_history.is_some()
            || self.pending_prompt_after_model_switch.is_some()
            || self.pending_startup_prompt_echo.is_some()
        {
            return false;
        }
        if self.remote_login_onboarding.task.is_none() {
            let Ok(target) = Target::from_env() else {
                self.remote_login_onboarding.dismiss();
                return false;
            };
            if tokio::runtime::Handle::try_current().is_err() {
                return false;
            }
            self.remote_login_onboarding.task = Some(Task::spawn(
                target,
                String::new(),
                String::new(),
                Operation::Status,
                None,
            ));
            return false;
        }
        let task = self.remote_login_onboarding.task.as_mut().unwrap();
        let reply = match task.reply.try_recv() {
            Ok(reply) => reply,
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => return false,
            Err(_) => Err("Remote status check stopped"),
        };
        self.remote_login_onboarding.dismiss();
        if let Ok(Reply::Status { providers }) = reply
            && remote_has_no_logins(&providers)
        {
            return self.open_ssh_import_offer();
        }
        false
    }

    fn open_ssh_import_offer(&mut self) -> bool {
        let Ok(target) = Target::from_env() else {
            return false;
        };
        let host = target.host().to_string();
        self.pending_login = Some(PendingLogin::Remote);
        self.remote_login = Some(RemoteLogin {
            target,
            provider: String::new(),
            flow: hex::encode(rand::random::<[u8; 16]>()),
            phase: Phase::ImportOffer,
            input_kind: String::new(),
            input: String::new(),
            task: None,
            operation: Some(Operation::Status),
            quit_after_cancel: false,
        });
        self.push_display_message(DisplayMessage::system(format!(
            "No logins are configured on {host}. Import a local login first?\n\nYes lets you choose a local OpenAI or Claude login to copy to this host. No opens the normal login options. Nothing is read or copied until you choose an account and approve its destination warning."
        )));
        self.open_ssh_import_decision(true);
        true
    }

    pub(super) fn open_ssh_import_decision(&mut self, offer: bool) {
        let Some(login) = self.remote_login.as_ref() else {
            return;
        };
        let host = login.target.host();
        let entries = [true, false].into_iter().map(|accept| PickerEntry {
            name: if accept { "Yes" } else { "No" }.into(),
            options: vec![PickerOption {
                provider: host.into(),
                api_method: if offer { "import local login?" } else { "copy this login?" }.into(),
                available: true,
                detail: match (offer, accept) {
                    (true, true) => "Choose a local OpenAI or Claude login. No credentials read or copied yet.",
                    (true, false) => "Use the normal login options on the remote host instead.",
                    (false, true) => "Copy this selected login automatically to the trusted host above. No tokens to paste.",
                    (false, false) => "Cancel without reading or copying local credentials.",
                }.into(),
                estimated_reference_cost_micros: None,
            }],
            action: PickerAction::RemoteImportDecision { accept },
            selected_option: 0,
            is_current: false,
            is_default: false,
            is_favorite: false,
            recommended: false,
            recommendation_rank: usize::MAX,
            usage_score: 0,
            old: false,
            created_date: None,
            effort: None,
        }).collect();
        self.inline_view_state = None;
        self.inline_interactive_state = Some(InlineInteractiveState {
            kind: PickerKind::Login,
            entries,
            filtered: vec![0, 1],
            selected: 1, // Enter alone never consents to reading or copying credentials.
            column: 0,
            filter: String::new(),
            preview: false,
        });
        self.set_status_notice("Choose Yes or No with arrows (or Y/N), then Enter. Esc cancels.");
    }

    pub(in crate::tui::app) fn select_ssh_import_decision(&mut self, accept: bool) {
        if !crate::tui::is_ssh_remote() {
            return;
        }
        let Some(login) = self.remote_login.as_mut() else {
            return;
        };
        match login.phase {
            Phase::ImportOffer => {
                login.phase = Phase::Choosing;
                login.input.clear();
                self.input.clear();
                self.cursor_pos = 0;
                self.open_ssh_login_picker(accept);
            }
            Phase::ImportConsent => {
                if !accept {
                    self.cancel_ssh_login();
                } else {
                    login.input = "yes".into();
                    self.submit_ssh_login_input();
                }
            }
            _ => {}
        }
    }
}
