//! Terminal text filtering.

use super::*;

impl PickerKind {
    pub fn filter_text(&self, entry: &PickerEntry) -> String {
        match self {
            Self::Account => {
                let provider = entry
                    .active_option()
                    .map(|option| option.provider.as_str())
                    .unwrap_or("");
                let state = entry.account_state_label().unwrap_or("");
                format!("{} {} {}", entry.name, provider, state)
            }
            Self::Login => {
                let auth_kind = entry
                    .active_option()
                    .map(|option| option.provider.as_str())
                    .unwrap_or("");
                let state = entry
                    .active_option()
                    .map(|option| option.api_method.as_str())
                    .unwrap_or("");
                let detail = entry
                    .active_option()
                    .map(|option| option.detail.as_str())
                    .unwrap_or("");
                format!("{} {} {} {}", entry.name, auth_kind, state, detail)
            }
            Self::Usage => {
                let status = entry
                    .active_option()
                    .map(|option| option.provider.as_str())
                    .unwrap_or("");
                let window = entry
                    .active_option()
                    .map(|option| option.api_method.as_str())
                    .unwrap_or("");
                let detail = entry
                    .active_option()
                    .map(|option| option.detail.as_str())
                    .unwrap_or("");
                format!("{} {} {} {}", entry.name, status, window, detail)
            }
            Self::Model => {
                let route = entry.active_option();
                let provider = route.map(|option| option.provider.as_str()).unwrap_or("");
                let method = route.map(|option| option.api_method.as_str()).unwrap_or("");
                let detail = route.map(|option| option.detail.as_str()).unwrap_or("");
                // Include the pretty name so a query like "opus 4.8" matches
                // the row even though the underlying id is `claude-opus-4-8`.
                let pretty =
                    crate::tui::app::helpers::model_names::pretty_known_model_family(&entry.name)
                        .unwrap_or_default();
                format!(
                    "{} {} {} {} {}",
                    entry.name, pretty, provider, method, detail
                )
            }
        }
    }
}
