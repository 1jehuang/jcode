use super::*;

impl App {
    pub(crate) fn prompt_new_account_label(
        &mut self,
        provider: crate::tui::account_picker::AccountProviderKind,
    ) {
        let (provider_id, display_name) = match provider {
            crate::tui::account_picker::AccountProviderKind::Anthropic => {
                ("claude", "Anthropic/Claude")
            }
            crate::tui::account_picker::AccountProviderKind::OpenAi => ("openai", "OpenAI"),
        };
        self.push_display_message(DisplayMessage::system(format!(
            "Enter a label for the new {} account, then press Enter. Use /cancel to abort.",
            display_name
        )));
        self.set_status_notice(format!("Account: new {} label...", display_name));
        self.pending_account_input = Some(PendingAccountInput::NewAccountLabel {
            provider_id: provider_id.to_string(),
            display_name: display_name.to_string(),
        });
    }

    pub(crate) fn prompt_account_value(
        &mut self,
        prompt: String,
        command_prefix: String,
        empty_value: Option<String>,
        status_notice: String,
    ) {
        self.push_display_message(DisplayMessage::system(format!(
            "{} Use /cancel to abort.",
            prompt
        )));
        self.set_status_notice(status_notice.clone());
        self.pending_account_input = Some(PendingAccountInput::CommandValue {
            prompt,
            command_prefix,
            empty_value,
            status_notice,
        });
    }

    pub(crate) fn handle_pending_account_input(
        &mut self,
        pending: PendingAccountInput,
        input: String,
    ) {
        let trimmed = input.trim();
        if trimmed == "/cancel" {
            self.push_display_message(DisplayMessage::system(
                "Account action cancelled.".to_string(),
            ));
            self.set_status_notice("Account: cancelled");
            return;
        }

        match pending {
            PendingAccountInput::Rename { provider_id, label } => {
                if trimmed.is_empty() {
                    self.push_display_message(DisplayMessage::error(
                        "Account name cannot be empty.".to_string(),
                    ));
                    self.pending_account_input =
                        Some(PendingAccountInput::Rename { provider_id, label });
                    return;
                }
                self.rename_saved_account(&provider_id, &label, trimmed);
            }
            PendingAccountInput::NewAccountLabel {
                provider_id,
                display_name,
            } => {
                if trimmed.is_empty() {
                    self.push_display_message(DisplayMessage::error(
                        "Account label cannot be empty.".to_string(),
                    ));
                    self.pending_account_input = Some(PendingAccountInput::NewAccountLabel {
                        provider_id,
                        display_name,
                    });
                    return;
                }
                if self.account_manager_scope().is_some() {
                    let exists = match provider_id.as_str() {
                        "claude" => crate::auth::claude::list_accounts().map(|accounts| {
                            accounts.iter().any(|account| account.label == trimmed)
                        }),
                        "openai" => crate::auth::codex::list_accounts().map(|accounts| {
                            accounts.iter().any(|account| account.label == trimmed)
                        }),
                        _ => Ok(false),
                    };
                    match exists {
                        Ok(false) => crate::tui::app::auth::execute_account_command_local(
                            self,
                            AccountCommand::Add {
                                provider_id,
                                label: Some(trimmed.to_owned()),
                            },
                        ),
                        result => {
                            let message = match result {
                                Ok(true) => "That account name is already saved. Choose a different name, or use Replace login on that account.".to_owned(),
                                Err(error) => format!("Could not read saved accounts: {error}"),
                                Ok(false) => unreachable!(),
                            };
                            self.push_display_message(DisplayMessage::error(message));
                            self.pending_account_input =
                                Some(PendingAccountInput::NewAccountLabel {
                                    provider_id,
                                    display_name,
                                });
                        }
                    }
                } else {
                    self.input = format!("/accounts {} add {}", provider_id, trimmed);
                    self.cursor_pos = self.input.len();
                    self.submit_input();
                }
            }
            PendingAccountInput::CommandValue {
                prompt,
                command_prefix,
                empty_value,
                status_notice,
            } => {
                let value = if trimmed.is_empty() {
                    if let Some(value) = empty_value {
                        value
                    } else {
                        self.push_display_message(DisplayMessage::error(
                            "A value is required for this setting.".to_string(),
                        ));
                        self.pending_account_input = Some(PendingAccountInput::CommandValue {
                            prompt,
                            command_prefix,
                            empty_value: None,
                            status_notice,
                        });
                        return;
                    }
                } else {
                    trimmed.to_string()
                };
                self.input = format!("{} {}", command_prefix, value);
                self.cursor_pos = self.input.len();
                self.submit_input();
            }
        }
    }

    pub(crate) fn next_account_picker_action(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> anyhow::Result<Option<crate::tui::account_picker::AccountPickerCommand>> {
        use crate::tui::account_picker::OverlayAction;

        let action = {
            let Some(picker_cell) = self.account_picker_overlay.as_ref() else {
                return Ok(None);
            };
            let mut picker = picker_cell.borrow_mut();
            picker.handle_overlay_key(code, modifiers)?
        };

        match action {
            OverlayAction::Continue => Ok(None),
            OverlayAction::Close => {
                let return_scope = self.account_picker_overlay.as_ref().and_then(|picker| {
                    let picker = picker.borrow();
                    picker
                        .is_manager_subpage()
                        .then(|| picker.manager_scope())
                        .flatten()
                });
                if let Some(scope) = return_scope {
                    self.open_account_center(scope.as_deref());
                } else {
                    self.account_picker_overlay = None;
                }
                Ok(None)
            }
            OverlayAction::Execute(command) => {
                if self.account_manager_scope().is_none() {
                    self.account_picker_overlay = None;
                }
                Ok(Some(command))
            }
            OverlayAction::SubmitPrompt(value) => {
                self.submit_account_manager_prompt(value);
                Ok(None)
            }
            OverlayAction::CancelPrompt => {
                self.cancel_account_manager_prompt();
                Ok(None)
            }
        }
    }
}
