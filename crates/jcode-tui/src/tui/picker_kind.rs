use super::*;

impl PickerKind {
    pub fn schema(&self) -> InlineInteractiveSchema {
        match self {
            Self::Panels => InlineInteractiveSchema {
                layout: InlineInteractiveLayout::Compact,
                primary_label: "PANEL",
                secondary_label: "STATE",
                secondary_preview_label: "STATE",
                tertiary_label: "",
                preview_submit_hint: "  ↵ reopen",
                active_submit_hint: "  ↑↓ ↵ Esc",
                shows_default_shortcut_hint: false,
                preview_activation_column: 0,
            },
            Self::Model => InlineInteractiveSchema {
                layout: InlineInteractiveLayout::ThreeColumn,
                primary_label: "MODEL",
                secondary_label: "PROVIDER",
                secondary_preview_label: "PROVIDER",
                tertiary_label: "METHOD",
                preview_submit_hint: "  ↵ open",
                active_submit_hint: "  ↑↓ ←→ ↵ Esc",
                shows_default_shortcut_hint: true,
                preview_activation_column: 2,
            },
            Self::Account => InlineInteractiveSchema {
                layout: InlineInteractiveLayout::Compact,
                primary_label: "ACCOUNT",
                secondary_label: "STATE",
                secondary_preview_label: "STATE",
                tertiary_label: "",
                preview_submit_hint: "  ↵ select",
                active_submit_hint: "  ↑↓/jk ↵ Esc",
                shows_default_shortcut_hint: false,
                preview_activation_column: 0,
            },
            Self::Login => InlineInteractiveSchema {
                layout: InlineInteractiveLayout::ThreeColumn,
                primary_label: "ITEM",
                secondary_label: "PROVIDER",
                secondary_preview_label: "PROVIDER",
                tertiary_label: "ACTION",
                preview_submit_hint: "  ↵ open",
                active_submit_hint: "  ↑↓ ←→ ↵ Esc",
                shows_default_shortcut_hint: true,
                preview_activation_column: 2,
            },
            Self::Usage => InlineInteractiveSchema {
                layout: InlineInteractiveLayout::ThreeColumn,
                primary_label: "ITEM",
                secondary_label: "STATUS",
                secondary_preview_label: "ITEM",
                tertiary_label: "WINDOW",
                preview_submit_hint: "  ↵ inspect",
                active_submit_hint: "  ↑↓ ←→ ↵ Esc",
                shows_default_shortcut_hint: false,
                preview_activation_column: 2,
            },
        }
    }

    pub fn uses_compact_navigation(&self) -> bool {
        self.schema().layout == InlineInteractiveLayout::Compact
    }

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
            Self::Usage | Self::Panels => {
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
