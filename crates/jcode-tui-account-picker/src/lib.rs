#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum AccountProviderKind {
    Anthropic,
    OpenAi,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum AccountPickerCommand {
    SubmitInput(String),
    OpenAccountCenter {
        provider_filter: Option<String>,
    },
    OpenAddReplaceFlow {
        provider_filter: Option<String>,
    },
    PromptValue {
        prompt: String,
        command_prefix: String,
        empty_value: Option<String>,
        status_notice: String,
    },
    Switch {
        provider: AccountProviderKind,
        label: String,
    },
    Login {
        provider: AccountProviderKind,
        label: String,
    },
    Remove {
        provider: AccountProviderKind,
        label: String,
    },
    PromptNew {
        provider: AccountProviderKind,
    },
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AccountPickerItem {
    pub provider_id: String,
    pub provider_label: String,
    pub title: String,
    pub subtitle: String,
    pub command: AccountPickerCommand,
}

impl AccountPickerItem {
    pub fn action(
        provider_id: impl Into<String>,
        provider_label: impl Into<String>,
        title: impl Into<String>,
        subtitle: impl Into<String>,
        command: AccountPickerCommand,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            provider_label: provider_label.into(),
            title: title.into(),
            subtitle: subtitle.into(),
            command,
        }
    }
}

#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AccountPickerSummary {
    pub ready_count: usize,
    pub attention_count: usize,
    pub setup_count: usize,
    pub provider_count: usize,
    pub named_account_count: usize,
    pub default_provider: Option<String>,
    pub default_model: Option<String>,
}

pub fn action_kind_label(command: &AccountPickerCommand) -> &'static str {
    match command {
        AccountPickerCommand::OpenAccountCenter { .. } => "overview",
        AccountPickerCommand::OpenAddReplaceFlow { .. } => "account",
        AccountPickerCommand::SubmitInput(input) if input.ends_with(" settings") => "overview",
        AccountPickerCommand::SubmitInput(input) if input.contains(" remove ") => "danger",
        AccountPickerCommand::SubmitInput(input) if input.contains(" login") => "login",
        AccountPickerCommand::SubmitInput(input) if input.contains(" add") => "account",
        AccountPickerCommand::SubmitInput(input) if input.contains(" switch ") => "account",
        AccountPickerCommand::PromptValue { .. } => "setting",
        AccountPickerCommand::Switch { .. } => "account",
        AccountPickerCommand::Login { .. } => "login",
        AccountPickerCommand::Remove { .. } => "danger",
        AccountPickerCommand::PromptNew { .. } => "account",
        AccountPickerCommand::SubmitInput(_) => "action",
    }
}

pub fn item_matches_filter(item: &AccountPickerItem, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let haystack = format!(
        "{} {} {} {} {}",
        item.provider_id,
        item.provider_label,
        item.title,
        item.subtitle,
        action_kind_label(&item.command)
    )
    .to_lowercase();
    filter
        .split_whitespace()
        .all(|needle| haystack.contains(&needle.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_filter_matches_provider_title_and_action_kind() {
        let item = AccountPickerItem::action(
            "openai",
            "OpenAI",
            "Remove account `work`",
            "active",
            AccountPickerCommand::Remove {
                provider: AccountProviderKind::OpenAi,
                label: "work".into(),
            },
        );
        assert!(item_matches_filter(&item, "openai danger"));
        assert!(item_matches_filter(&item, "work active"));
        assert!(!item_matches_filter(&item, "claude"));
    }
}

mod overlay;
pub use overlay::{AccountPicker, OverlayAction};
