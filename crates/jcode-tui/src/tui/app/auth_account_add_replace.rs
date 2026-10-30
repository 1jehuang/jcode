use super::*;

impl App {
    pub(crate) fn open_account_add_replace_flow(&mut self, provider_filter: Option<&str>) {
        if self.is_remote || crate::tui::is_ssh_remote() {
            self.remote_account_management_notice();
            return;
        }
        use crate::tui::account_picker::{AccountPicker, AccountPickerCommand, AccountPickerItem};

        let mut items = vec![AccountPickerItem::action(
            "account-flow",
            "Add / Replace",
            "Back to account center",
            "Return to the full provider/auth account center",
            AccountPickerCommand::OpenAccountCenter {
                provider_filter: provider_filter.map(|value| value.to_string()),
            },
        )];

        let include_claude = provider_filter.is_none()
            || matches!(provider_filter, Some("claude") | Some("anthropic"));
        let include_openai = provider_filter.is_none() || matches!(provider_filter, Some("openai"));

        if include_claude {
            items.push(AccountPickerItem::action(
                "claude",
                "Claude",
                "Add new account",
                "Create the next numbered Claude account",
                AccountPickerCommand::SubmitInput("/account claude add".to_string()),
            ));
            let accounts = match crate::auth::claude::list_accounts() {
                Ok(accounts) => accounts,
                Err(error) => {
                    self.push_display_message(DisplayMessage::error(format!(
                        "Could not load saved Claude accounts: {error}"
                    )));
                    return;
                }
            };
            for account in accounts {
                let label = account.label.clone();
                items.push(AccountPickerItem::action(
                    "claude",
                    "Claude",
                    format!("Replace account {label}"),
                    "Refresh this saved Claude account in place",
                    AccountPickerCommand::SubmitInput(format!("/account claude add {}", label)),
                ));
            }
        }

        if include_openai {
            items.push(AccountPickerItem::action(
                "openai",
                "OpenAI",
                "Add new account",
                "Create the next numbered OpenAI account",
                AccountPickerCommand::SubmitInput("/account openai add".to_string()),
            ));
            let accounts = match crate::auth::codex::list_accounts() {
                Ok(accounts) => accounts,
                Err(error) => {
                    self.push_display_message(DisplayMessage::error(format!(
                        "Could not load saved OpenAI accounts: {error}"
                    )));
                    return;
                }
            };
            for account in accounts {
                let label = account.label.clone();
                items.push(AccountPickerItem::action(
                    "openai",
                    "OpenAI",
                    format!("Replace account {label}"),
                    "Refresh this saved OpenAI account in place",
                    AccountPickerCommand::SubmitInput(format!("/account openai add {}", label)),
                ));
            }
        }

        self.account_picker_overlay = Some(std::cell::RefCell::new(AccountPicker::new(
            " Add / Replace Account ",
            items,
        )));
        self.inline_interactive_state = None;
        self.input.clear();
        self.cursor_pos = 0;
        self.set_status_notice("Account center: choose add/replace target");
    }
}
