use super::*;
use crate::tui::account_picker::{AccountPickerCommand, AccountProviderKind};

impl App {
    pub(crate) fn account_manager_scope(&self) -> Option<Option<String>> {
        self.account_picker_overlay
            .as_ref()
            .and_then(|picker| picker.borrow().manager_scope())
    }

    pub(crate) fn account_manager_action_target(
        command: &AccountPickerCommand,
    ) -> Option<(String, String)> {
        let (provider, label) = match command {
            AccountPickerCommand::Switch { provider, label }
            | AccountPickerCommand::SetDefault { provider, label }
            | AccountPickerCommand::Login { provider, label }
            | AccountPickerCommand::Replace { provider, label }
            | AccountPickerCommand::Rename { provider, label }
            | AccountPickerCommand::Remove { provider, label } => (provider, label),
            _ => return None,
        };
        let provider_id = match provider {
            AccountProviderKind::Anthropic => "claude",
            AccountProviderKind::OpenAi => "openai",
        };
        Some((provider_id.to_owned(), label.clone()))
    }

    /// Keep account actions in their own page, including validation failures.
    pub(crate) fn finish_account_manager_action(
        &mut self,
        message_start: usize,
        selected: Option<(String, String)>,
    ) {
        let Some(scope) = self.account_manager_scope() else {
            return;
        };
        let messages = &self.display_messages[message_start.min(self.display_messages.len())..];
        let error = messages.iter().any(|message| message.role == "error");
        let feedback = messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        if self.pending_account_input.is_some() || self.pending_login.is_some() {
            self.show_account_manager_prompt(&feedback);
            if !feedback.is_empty()
                && let Some(picker) = &self.account_picker_overlay
            {
                picker.borrow_mut().set_notice(feedback, error);
            }
            return;
        }
        self.open_account_center(scope.as_deref());
        if let Some(picker) = &self.account_picker_overlay {
            let mut picker = picker.borrow_mut();
            if let Some((provider, label)) = selected {
                picker.select_account(&provider, Some(&label));
            }
            if !feedback.is_empty() {
                picker.set_notice(feedback, error);
            }
        }
    }

    fn show_account_manager_prompt(&self, feedback: &str) {
        let Some(picker) = &self.account_picker_overlay else {
            return;
        };
        let mut picker = picker.borrow_mut();
        if picker.prompt_is_open() {
            return;
        }
        if let Some(pending) = &self.pending_account_input {
            match pending {
                PendingAccountInput::Rename { provider_id, label } => picker.show_prompt(
                    "Rename account",
                    &format!("New name for {provider_id} account `{label}`. Your other accounts are kept."),
                    label,
                    false,
                ),
                PendingAccountInput::NewAccountLabel { display_name, .. } => picker.show_prompt(
                    "Add account",
                    &format!("Enter a name for the new {display_name} login."),
                    "",
                    false,
                ),
                PendingAccountInput::CommandValue { prompt, .. } => {
                    picker.show_prompt("Account setting", prompt, "", false)
                }
            }
        } else if self.pending_login.is_some() {
            let body = if feedback.is_empty() {
                "Complete sign-in in your browser. You can also paste the callback URL or authorization code here. Escape cancels and returns to Accounts."
            } else {
                feedback
            };
            picker.show_prompt("Sign in to account", body, "", true);
        }
    }

    pub(crate) fn submit_account_manager_prompt(&mut self, value: String) {
        let message_start = self.display_messages.len();
        let mut selected = None;
        if let Some(pending) = self.pending_account_input.take() {
            if let PendingAccountInput::Rename { provider_id, .. } = &pending {
                selected = Some((provider_id.clone(), value.trim().to_owned()));
            }
            let retry = pending.clone();
            self.handle_pending_account_input(pending, value);
            if self.pending_login.is_some()
                && let Some(picker) = &self.account_picker_overlay
            {
                // The name form has become a sign-in form. Do not retain its text.
                picker.borrow_mut().finish_prompt();
            }
            let failed = self.display_messages[message_start..]
                .iter()
                .any(|message| message.role == "error");
            if failed && self.pending_account_input.is_none() && self.pending_login.is_none() {
                self.pending_account_input = Some(retry);
            }
        } else if let Some(pending) = self.pending_login.take() {
            self.handle_login_input(pending, value);
            if self
                .pending_login
                .as_ref()
                .is_some_and(PendingLogin::is_exchanging)
                && let Some(picker) = &self.account_picker_overlay
            {
                picker.borrow_mut().show_waiting(
                    "Completing sign-in",
                    "Checking the signed-in account and saving it. Escape cancels and returns to Accounts.",
                );
            }
        }
        self.finish_account_manager_action(message_start, selected);
    }

    pub(crate) fn cancel_account_manager_prompt(&mut self) {
        let message_start = self.display_messages.len();
        let was_login = self.pending_login.is_some();
        if let Some(pending) = self.pending_login.take() {
            self.handle_login_input(pending, "/cancel".to_owned());
        }
        if self.pending_login.is_some() {
            if let Some(picker) = &self.account_picker_overlay {
                picker.borrow_mut().show_waiting(
                    "Completing sign-in",
                    "The login is already being saved. Waiting for its result before returning to Accounts.",
                );
            }
            return;
        }
        self.pending_account_input = None;
        if !was_login {
            self.push_display_message(DisplayMessage::system(
                "Account action cancelled.".to_owned(),
            ));
        }
        if let Some(picker) = &self.account_picker_overlay {
            picker.borrow_mut().finish_prompt();
        }
        self.finish_account_manager_action(message_start, None);
    }

    /// Login results remain visible even when a browser callback arrives later.
    pub(crate) fn finish_account_manager_login(
        &mut self,
        _provider: &str,
        success: bool,
        message: &str,
    ) {
        let Some(scope) = self.account_manager_scope() else {
            return;
        };
        self.open_account_center(scope.as_deref());
        if let Some(picker) = &self.account_picker_overlay {
            picker.borrow_mut().set_notice(message.to_owned(), !success);
        }
    }

    pub(crate) fn paste_account_manager_prompt(&mut self, text: &str) -> bool {
        self.account_picker_overlay
            .as_ref()
            .is_some_and(|picker| picker.borrow_mut().paste_prompt(text))
    }
}
