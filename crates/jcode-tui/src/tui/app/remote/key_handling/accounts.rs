//! Account-picker command handling for the remote key handler.

use super::*;

impl App {
    pub(in crate::tui::app::remote) async fn handle_account_picker_command_remote(
        &mut self,
        remote: &mut RemoteConnection,
        command: crate::tui::account_picker::AccountPickerCommand,
    ) -> Result<()> {
        let navigation = matches!(
            &command,
            crate::tui::account_picker::AccountPickerCommand::OpenAccountCenter { .. }
                | crate::tui::account_picker::AccountPickerCommand::OpenAddReplaceFlow { .. }
        );
        let message_start = self.display_messages.len();
        let selected = Self::account_manager_action_target(&command);
        let managed = self.account_manager_scope().is_some();
        let result: Result<()> = async {
            match command {
                crate::tui::account_picker::AccountPickerCommand::OpenAccountCenter {
                    provider_filter,
                } => self.open_account_center(provider_filter.as_deref()),
                crate::tui::account_picker::AccountPickerCommand::OpenAddReplaceFlow {
                    provider_filter,
                } => self.open_account_add_replace_flow(provider_filter.as_deref()),
                crate::tui::account_picker::AccountPickerCommand::SubmitInput(input) => {
                    crate::tui::app::auth::handle_account_command_remote(self, &input, remote)
                        .await?;
                }
                crate::tui::account_picker::AccountPickerCommand::PromptValue {
                    prompt,
                    command_prefix,
                    empty_value,
                    status_notice,
                } => self.prompt_account_value(prompt, command_prefix, empty_value, status_notice),
                crate::tui::account_picker::AccountPickerCommand::PromptNew { provider } => {
                    self.prompt_new_account_label(provider)
                }
                other => {
                    if let Some(command) =
                        crate::tui::app::auth::account_command_from_picker(&other)
                    {
                        crate::tui::app::auth::execute_account_command_remote(
                            self, command, remote,
                        )
                        .await?;
                    }
                }
            }
            Ok(())
        }
        .await;
        if !navigation && managed {
            if let Err(error) = &result {
                self.push_display_message(DisplayMessage::error(format!(
                    "Account action failed: {error}"
                )));
            }
            self.finish_account_manager_action(message_start, selected);
            return Ok(());
        }
        result
    }
}
