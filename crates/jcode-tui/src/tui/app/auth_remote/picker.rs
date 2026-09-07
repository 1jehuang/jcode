//! Shared inline picker presentation, populated only from the SSH host's status.
use super::{App, Phase, command};
use crate::tui::{InlineInteractiveState, PickerAction, PickerEntry, PickerKind, PickerOption};

const LABELS: [(&str, &str); 6] = [
    ("openai", "OpenAI"),
    ("claude", "Claude"),
    ("gemini", "Gemini"),
    ("antigravity", "Antigravity"),
    ("google", "Google"),
    ("copilot", "GitHub Copilot"),
];

fn entry(provider: &'static str, label: &str, import: bool, host: &str) -> PickerEntry {
    PickerEntry {
        name: if import {
            format!("Import local {label} login")
        } else {
            label.into()
        },
        options: vec![PickerOption {
            provider: if import {
                "Local → SSH".into()
            } else {
                "OAuth".into()
            },
            api_method: if import {
                "confirm copy".into()
            } else {
                "checking remote".into()
            },
            available: true,
            detail: if import {
                format!(
                    "Destination: {host}. One-time copy of your selected Jcode-managed {provider} account. Confirmation required. No local credentials read yet. Existing remote logins are never overwritten."
                )
            } else {
                format!(
                    "Destination: {host}. Checking remote login status. Browser approval opens on this computer, credentials stay on the SSH host. /login {provider}"
                )
            },
            estimated_reference_cost_micros: None,
        }],
        action: PickerAction::RemoteLogin { provider, import },
        selected_option: 0,
        is_current: false,
        is_default: false,
        is_favorite: false,
        recommended: matches!(provider, "openai" | "claude"),
        recommendation_rank: usize::MAX,
        usage_score: 0,
        old: false,
        created_date: None,
        effort: None,
    }
}

impl App {
    pub(super) fn open_ssh_login_picker(&mut self, imports_only: bool) {
        let Some(login) = self.remote_login.as_ref() else {
            return;
        };
        let host = login.target.host();
        let mut entries = vec![
            entry("openai", "OpenAI", true, host),
            entry("claude", "Claude", true, host),
        ];
        if !imports_only {
            entries.extend(
                LABELS
                    .into_iter()
                    .map(|(provider, label)| entry(provider, label, false, host)),
            );
        }
        self.inline_view_state = None;
        self.inline_interactive_state = Some(InlineInteractiveState {
            kind: PickerKind::Login,
            filtered: (0..entries.len()).collect(),
            entries,
            selected: 0,
            column: 0,
            filter: String::new(),
            preview: false,
        });
        let host = host.to_string();
        self.set_status_notice(format!(
            "Login on {host}: choose a provider or Import local login. Esc cancels."
        ));
        if tokio::runtime::Handle::try_current().is_ok() {
            self.remote_login
                .as_mut()
                .unwrap()
                .run(command::Operation::Status, None);
        } else {
            self.update_ssh_login_picker_status(None);
        }
    }

    pub(super) fn update_ssh_login_picker_status(
        &mut self,
        statuses: Option<&[command::ProviderStatus]>,
    ) {
        let Some(login) = self
            .remote_login
            .as_ref()
            .filter(|login| login.phase == Phase::Choosing)
        else {
            return;
        };
        let host = login.target.host();
        let Some(picker) = self.inline_interactive_state.as_mut() else {
            return;
        };
        for entry in &mut picker.entries {
            let PickerAction::RemoteLogin { provider, import } = entry.action else {
                continue;
            };
            if import {
                continue;
            }
            let status =
                statuses.and_then(|statuses| statuses.iter().find(|status| status.id == provider));
            let label = match status.map(|status| status.state) {
                Some(crate::auth::AuthState::Available) => "configured",
                Some(crate::auth::AuthState::Expired) => "attention",
                Some(crate::auth::AuthState::NotConfigured) => "setup",
                None => "status unknown",
            };
            entry.is_current = matches!(
                status.map(|status| status.state),
                Some(crate::auth::AuthState::Available)
            );
            entry.options[0].api_method = label.into();
            entry.options[0].detail = format!(
                "Destination: {host}. Remote status: {label} ({}). Browser approval opens on this computer, credentials stay on the SSH host. /login {provider}",
                status
                    .map(|status| status.method_detail.as_str())
                    .unwrap_or("could not read remote status; reopen /login to retry")
            );
        }
        // Preserve the user's filter, cursor, and selection as async status arrives.
    }
}
