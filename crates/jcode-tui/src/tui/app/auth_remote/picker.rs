//! Shared inline picker presentation, populated only from the SSH host's status.
use super::{App, Phase, command};
use crate::provider_catalog::{self, LoginProviderDescriptor, LoginProviderTarget};
use crate::tui::{InlineInteractiveState, PickerAction, PickerEntry, PickerKind, PickerOption};

fn remote_setup_detail(provider: LoginProviderDescriptor, host: &str) -> String {
    if super::PROVIDERS.contains(&provider.id) {
        let prerequisite = if provider.id == "google" {
            " Google OAuth client credentials must already be configured on the SSH host."
        } else {
            ""
        };
        format!(
            "Browser approval opens on this computer, credentials stay on the SSH host.{prerequisite} /login {}",
            provider.id
        )
    } else if matches!(provider.target, LoginProviderTarget::AutoImport) {
        format!(
            "Remote-host setup required: open Jcode directly on {host} and use /login auto-import to review that host's other-tool logins. This SSH picker cannot perform Auto Import. To copy this computer's Jcode-managed login instead, choose an explicit Import local entry. No local credentials are accessed here."
        )
    } else {
        format!(
            "Remote-host setup required: open Jcode directly on {host} and use /login {} to configure this provider there. This authentication method is not supported by the SSH login flow. No local authentication is started and no local credentials are accessed here.",
            provider.id
        )
    }
}

fn provider_entry(provider: LoginProviderDescriptor, host: &str) -> PickerEntry {
    entry(
        provider.display_name.into(),
        PickerOption {
            provider: provider.auth_kind.label().into(),
            api_method: "checking remote".into(),
            available: true,
            detail: format!(
                "Destination: {host}. Checking remote login status. {} · {}",
                provider.menu_detail,
                remote_setup_detail(provider, host)
            ),
            estimated_reference_cost_micros: None,
        },
        PickerAction::RemoteLogin {
            provider: provider.id,
            import: false,
        },
        provider.recommended,
    )
}

fn import_entry(provider: &'static str, label: &str, host: &str) -> PickerEntry {
    entry(
        format!("Import local {label} login"),
        PickerOption {
            provider: "Local → SSH".into(),
            api_method: "confirm copy".into(),
            available: true,
            detail: format!(
                "Destination: {host}. One-time copy of your selected Jcode-managed {provider} account. Confirmation required. No local credentials read yet. Existing remote logins are never overwritten."
            ),
            estimated_reference_cost_micros: None,
        },
        PickerAction::RemoteLogin {
            provider,
            import: true,
        },
        matches!(provider, "openai" | "claude"),
    )
}

fn entry(
    name: String,
    option: PickerOption,
    action: PickerAction,
    recommended: bool,
) -> PickerEntry {
    PickerEntry {
        name,
        options: vec![option],
        action,
        selected_option: 0,
        is_current: false,
        is_default: false,
        is_favorite: false,
        recommended,
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
        let mut entries = if imports_only {
            Vec::new()
        } else {
            provider_catalog::tui_login_providers()
                .into_iter()
                .map(|provider| provider_entry(provider, host))
                .collect()
        };
        entries.extend([
            import_entry("openai", "OpenAI", host),
            import_entry("claude", "Claude", host),
        ]);
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
            "Login on {host}: choose a provider or Import local login. Some methods require setup directly on the SSH host. Esc cancels."
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
            let Some(descriptor) = provider_catalog::login_providers()
                .iter()
                .find(|descriptor| descriptor.id == provider)
                .copied()
            else {
                continue;
            };
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
                "Destination: {host}. Remote status: {label} ({}). {} · {}",
                status
                    .map(|status| status.method_detail.as_str())
                    .unwrap_or("could not read remote status; reopen /login to retry"),
                descriptor.menu_detail,
                remote_setup_detail(descriptor, host),
            );
        }
        // Preserve the user's filter, cursor, and selection as async status arrives.
    }
}
