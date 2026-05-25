use crate::tui::app::App;
use crate::tui::{AccountPickerAction, InlineInteractiveState, PickerAction, PickerKind};

pub(super) enum InlinePickerPreviewRequest {
    Model {
        filter: String,
    },
    Login {
        filter: String,
    },
    Account {
        provider_filter: Option<String>,
        filter: String,
    },
}

impl InlinePickerPreviewRequest {
    fn kind(&self) -> PickerKind {
        match self {
            Self::Model { .. } => PickerKind::Model,
            Self::Login { .. } => PickerKind::Login,
            Self::Account { .. } => PickerKind::Account,
        }
    }

    pub(super) fn filter(&self) -> &str {
        match self {
            Self::Model { filter } | Self::Login { filter } | Self::Account { filter, .. } => {
                filter
            }
        }
    }

    fn account_provider_filter(&self) -> Option<&str> {
        match self {
            Self::Account {
                provider_filter: Some(provider_filter),
                ..
            } => Some(provider_filter.as_str()),
            _ => None,
        }
    }

    pub(super) fn open(&self, app: &mut App) {
        match self {
            Self::Model { .. } => app.open_model_picker(),
            Self::Login { .. } => app.open_login_picker_inline(),
            Self::Account {
                provider_filter, ..
            } => app.open_account_picker(provider_filter.as_deref()),
        }
    }

    pub(super) fn matches_picker(&self, app: &App, picker: &InlineInteractiveState) -> bool {
        if !picker.preview || picker.kind != self.kind() {
            return false;
        }

        if self.kind() != PickerKind::Account {
            return true;
        }

        let desired_provider =
            app.inline_account_picker_provider_id(self.account_provider_filter());
        desired_provider.as_deref() == picker_account_provider_scope(picker)
    }
}

pub(super) fn picker_account_provider_scope(picker: &InlineInteractiveState) -> Option<&str> {
    picker.entries.first().and_then(|entry| match entry.action {
        PickerAction::Account(
            AccountPickerAction::Switch {
                ref provider_id, ..
            }
            | AccountPickerAction::Add { ref provider_id }
            | AccountPickerAction::Replace {
                ref provider_id, ..
            },
        ) => Some(provider_id.as_str()),
        PickerAction::Account(AccountPickerAction::OpenCenter {
            provider_filter: Some(ref provider_id),
        }) => Some(provider_id.as_str()),
        PickerAction::Account(AccountPickerAction::OpenCenter {
            provider_filter: None,
        })
        | PickerAction::Model
        | PickerAction::Login(_)
        | PickerAction::Usage { .. }
        | PickerAction::AgentTarget(_)
        | PickerAction::AgentModelChoice { .. } => None,
    })
}
