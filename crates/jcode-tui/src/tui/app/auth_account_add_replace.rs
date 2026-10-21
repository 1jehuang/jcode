use super::*;

impl App {
    pub(crate) fn open_account_add_replace_flow(&mut self, provider_filter: Option<&str>) {
        use crate::tui::account_picker::{AccountPicker, AccountPickerCommand, AccountPickerItem};

        let mut items = vec![AccountPickerItem::action(
            "account-flow",
            "Accounts",
            "Back to accounts",
            "Return to your saved accounts",
            AccountPickerCommand::OpenAccountCenter {
                provider_filter: provider_filter.map(str::to_owned),
            },
        )];
        for (id, name) in [("claude", "Claude"), ("openai", "OpenAI")] {
            if provider_filter
                .is_some_and(|filter| filter != id && !(id == "claude" && filter == "anthropic"))
            {
                continue;
            }
            items.push(AccountPickerItem::action(
                id,
                name,
                "Add new account",
                format!("Sign in to another {name} account. Existing saved accounts are kept."),
                AccountPickerCommand::SubmitInput(format!("/accounts {id} add")),
            ));
        }

        self.account_picker_overlay = Some(std::cell::RefCell::new(
            AccountPicker::new(" Add account ", items)
                .with_manager_scope(provider_filter.map(str::to_owned)),
        ));
        self.inline_interactive_state = None;
        self.input.clear();
        self.cursor_pos = 0;
        self.set_status_notice("Accounts: choose a provider for the new account");
    }
}
