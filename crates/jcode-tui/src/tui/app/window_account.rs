//! Per-window accounts: which stored Claude/OpenAI account THIS window uses.
//!
//! The server reports it through `History.account_labels` and
//! `SessionAccountChanged`. Local (standalone) sessions derive it from the
//! provider's pin. Nothing here reads the global `active_account_label()` to
//! decide what this window uses, except as the "follows the default" fallback.

use super::auth::{AccountCommand, AccountFailoverMode};
use super::*;
use crate::protocol::SessionAccountInfo;
use jcode_provider_core::{AccountPin, AccountProviderKind};

/// Provider family key ("claude" | "openai") for a provider/login name.
pub(crate) fn account_family(provider: &str) -> Option<&'static str> {
    let normalized = provider.trim().to_ascii_lowercase();
    if normalized.starts_with("claude") || normalized.starts_with("anthropic") {
        Some("claude")
    } else if normalized.starts_with("openai")
        || matches!(normalized.as_str(), "codex" | "chatgpt")
    {
        Some("openai")
    } else {
        None
    }
}

/// Stored default account label for a family: the stored active account,
/// else the first account. Ignores any runtime override.
pub(crate) fn stored_default_label(family: &str) -> Option<String> {
    match family {
        "claude" => {
            let auth = crate::auth::claude::load_auth_file().ok()?;
            auth.active_anthropic_account
                .filter(|label| auth.anthropic_accounts.iter().any(|a| &a.label == label))
                .or_else(|| auth.anthropic_accounts.first().map(|a| a.label.clone()))
        }
        "openai" => {
            let auth = crate::auth::codex::load_auth_file().ok()?;
            auth.active_openai_account
                .filter(|label| auth.openai_accounts.iter().any(|a| &a.label == label))
                .or_else(|| auth.openai_accounts.first().map(|a| a.label.clone()))
        }
        _ => None,
    }
}

/// Identity captured at pin time, in the exact form credential loading
/// compares (`auth::*::account_identity`), so a pin survives positional
/// relabeling and is never mistaken for another login.
fn pin_identity(family: &str, label: &str) -> Option<String> {
    match family {
        "claude" => crate::auth::claude::pin_for_label(label).ok()?.identity,
        "openai" => crate::auth::codex::pin_for_label(label).ok()?.identity,
        _ => None,
    }
}

/// Current label of the account a pin names, by identity (see
/// `auth::*::resolve_pin`).
fn resolve_local_pin(kind: AccountProviderKind, pin: &AccountPin) -> Option<String> {
    match kind {
        AccountProviderKind::Claude => crate::auth::claude::resolve_pin(pin),
        AccountProviderKind::OpenAi => crate::auth::codex::resolve_pin(pin),
    }
}

fn label_exists(family: &str, label: &str) -> bool {
    match family {
        "claude" => crate::auth::claude::list_accounts()
            .unwrap_or_default()
            .iter()
            .any(|a| a.label == label),
        "openai" => crate::auth::codex::list_accounts()
            .unwrap_or_default()
            .iter()
            .any(|a| a.label == label),
        _ => false,
    }
}

fn stored_labels(family: &str) -> Vec<String> {
    match family {
        "claude" => crate::auth::claude::list_accounts()
            .unwrap_or_default()
            .into_iter()
            .map(|a| a.label)
            .collect(),
        "openai" => crate::auth::codex::list_accounts()
            .unwrap_or_default()
            .into_iter()
            .map(|a| a.label)
            .collect(),
        _ => Vec::new(),
    }
}

fn family_display(family: &str) -> &'static str {
    match family {
        "claude" => "Claude",
        "openai" => "OpenAI",
        _ => "provider",
    }
}

/// "This window now uses X. The default for new windows is still Y (...)."
pub(crate) fn use_in_window_message(label: &str, default: Option<&str>) -> String {
    match default {
        Some(default) if default != label => format!(
            "This window now uses {label}. The default for new windows is still {default} (/account default {label} to change it)."
        ),
        _ => format!("This window now uses {label}, which is also the default for new windows."),
    }
}

pub(crate) fn set_default_message(label: &str, window: Option<&SessionAccountInfo>) -> String {
    match window.and_then(|info| info.label.as_deref().map(|l| (l, info.pinned))) {
        Some((current, true)) if current != label => format!(
            "New windows now use {label}. This window stays pinned to {current} (/account switch {label} to move it)."
        ),
        _ => format!(
            "New windows and windows without a pin now use {label}, including this one."
        ),
    }
}

impl App {
    /// Scope badge for a saved account row in the inline picker, plus
    /// whether the row is this window's account.
    pub(crate) fn account_row_badge(&self, family: &str, label: &str) -> (&'static str, bool) {
        let window = self.window_account(family);
        let is_window = self.window_account_label(family).as_deref() == Some(label);
        let pinned = is_window && window.as_ref().is_some_and(|w| w.pinned);
        let is_default = stored_default_label(family).as_deref() == Some(label);
        let badge = match (is_window, pinned, is_default) {
            (true, true, _) => "pinned · this window",
            (true, false, true) => "this window · default",
            (true, false, false) => "this window",
            (false, _, true) => "default",
            _ => "saved",
        };
        (badge, is_window)
    }

    /// Subtitle suffix for account-center rows (read by the picker badges).
    pub(crate) fn account_row_scope_suffix(&self, family: &str, label: &str) -> String {
        let window = self.window_account(family);
        let mut parts = Vec::new();
        if self.window_account_label(family).as_deref() == Some(label) {
            parts.push("this window");
            if window.as_ref().is_some_and(|w| w.pinned) {
                parts.push("pinned");
            }
        }
        if stored_default_label(family).as_deref() == Some(label) {
            parts.push("default");
        }
        parts.iter().map(|p| format!(" - {p}")).collect()
    }

    /// The account this window uses for `family` ("claude" | "openai").
    pub(crate) fn window_account(&self, family: &str) -> Option<SessionAccountInfo> {
        if let Some(info) = self.window_accounts.iter().find(|i| i.provider == family) {
            return Some(info.clone());
        }
        if self.is_remote {
            return None;
        }
        // Local session: the provider's pin, else the stored default.
        let kind = AccountProviderKind::from_key(family)?;
        let default = stored_default_label(family);
        match self.provider.account_pin(kind) {
            Some(pin) => Some(SessionAccountInfo {
                provider: family.to_string(),
                is_default: default.as_deref() == Some(pin.label.as_str()),
                label: Some(pin.label),
                pinned: true,
            }),
            None => default.map(|label| SessionAccountInfo {
                provider: family.to_string(),
                label: Some(label),
                pinned: false,
                is_default: true,
            }),
        }
    }

    /// Label this window uses for `family`, falling back to the default.
    pub(crate) fn window_account_label(&self, family: &str) -> Option<String> {
        self.window_account(family)
            .and_then(|info| info.label)
            .or_else(|| stored_default_label(family))
    }

    pub(crate) fn set_window_account_from_info(&mut self, info: &SessionAccountInfo) {
        self.window_accounts.retain(|i| i.provider != info.provider);
        self.window_accounts.push(info.clone());
    }

    /// Status-line account for this window. Runs every frame, so it reads
    /// only in-memory state (server-reported accounts or the local pin),
    /// never auth files.
    pub(crate) fn widget_window_account(
        &self,
        auth_method: crate::tui::info_widget::AuthMethod,
    ) -> Option<crate::tui::info_widget::WindowAccount> {
        use crate::tui::info_widget::AuthMethod;
        let family = match auth_method {
            AuthMethod::AnthropicOAuth => "claude",
            AuthMethod::OpenAIOAuth => "openai",
            AuthMethod::Unknown => {
                let provider = self
                    .remote_provider_name
                    .clone()
                    .unwrap_or_else(|| self.provider.name().to_string());
                account_family(&provider)?
            }
            // API keys have no stored account.
            _ => return None,
        };
        let info = match self.window_accounts.iter().find(|i| i.provider == family) {
            Some(info) => info.clone(),
            None if !self.is_remote => {
                let kind = AccountProviderKind::from_key(family)?;
                let pin = self.provider.account_pin(kind)?;
                SessionAccountInfo {
                    provider: family.to_string(),
                    label: Some(pin.label),
                    pinned: true,
                    is_default: false,
                }
            }
            None => return None,
        };
        Some(crate::tui::info_widget::WindowAccount {
            label: info.label?,
            pinned: info.pinned,
            is_default: info.is_default,
        })
    }

    /// History is authoritative for the (possibly new) session. Old servers
    /// send nothing, which falls back to the stored default.
    pub(crate) fn replace_window_accounts(&mut self, accounts: Vec<SessionAccountInfo>) {
        self.window_accounts = accounts;
    }

    /// Server says this window's account changed. A `reason` means the server
    /// moved it (failover, return home, removed account), so tell the user.
    pub(crate) fn handle_session_account_changed(
        &mut self,
        provider: String,
        label: Option<String>,
        pinned: bool,
        is_default: bool,
        reason: Option<String>,
    ) {
        let previous = self
            .window_accounts
            .iter()
            .find(|i| i.provider == provider)
            .and_then(|i| i.label.clone());
        self.set_window_account_from_info(&SessionAccountInfo {
            provider: provider.clone(),
            label: label.clone(),
            pinned,
            is_default,
        });
        crate::auth::AuthStatus::invalidate_cache();
        if let Some(reason) = reason.filter(|r| !r.trim().is_empty()) {
            let to = label.as_deref().unwrap_or("the default account");
            let text = match previous.as_deref() {
                Some(from) if Some(from) != label.as_deref() => {
                    format!("⚡ This window moved from {from} to {to} ({reason})")
                }
                _ => format!("⚡ This window now uses {to} ({reason})"),
            };
            self.push_display_message(DisplayMessage::system(text));
        }
        if let Some(label) = &label {
            self.set_status_notice(format!("Account: {label}"));
        }
    }

    /// Credentials of `account_label` changed. Only relevant to this window
    /// when unscoped or when it is this window's own account.
    pub(super) fn credential_change_is_for_this_window(
        &self,
        provider: Option<&str>,
        account_label: Option<&str>,
    ) -> bool {
        let Some(changed) = account_label else {
            return true;
        };
        let family = provider
            .and_then(account_family)
            .or_else(|| AccountProviderKind::from_label(changed).map(AccountProviderKind::key));
        let Some(family) = family else {
            return true;
        };
        match self.window_account_label(family) {
            Some(mine) => mine == changed,
            None => true,
        }
    }

    fn apply_local_pin(&mut self, family: &str, label: Option<&str>) -> Result<(), String> {
        let kind = AccountProviderKind::from_key(family).ok_or("unknown provider")?;
        let pin = label.map(|label| AccountPin::new(label, pin_identity(family, label)));
        self.provider
            .set_account_pin(kind, pin.clone())
            .map_err(|e| e.to_string())?;
        match pin {
            Some(pin) => {
                self.session.account_pins.insert(family.to_string(), pin);
            }
            None => {
                self.session.account_pins.remove(family);
            }
        }
        // A manual choice replaces any automatic "return home" target.
        self.session.account_failover_home.remove(family);
        let _ = self.session.save();
        let provider = self.provider.clone();
        tokio::spawn(async move { provider.invalidate_credentials().await });
        let default = stored_default_label(family);
        let resolved = label.map(str::to_string).or_else(|| default.clone());
        self.set_window_account_from_info(&SessionAccountInfo {
            provider: family.to_string(),
            is_default: resolved.is_some() && resolved == default,
            label: resolved,
            pinned: label.is_some(),
        });
        crate::auth::AuthStatus::invalidate_cache();
        self.context_limit = self.provider.context_window() as u64;
        self.context_warning_shown = false;
        let held = if family == "claude" { "anthropic" } else { "openai" };
        self.release_rate_limit_hold_after_credentials_changed(Some(held));
        Ok(())
    }

    /// Local (standalone) resume: push the session's account pins, failover
    /// toggle, and failover home onto the provider, like the server agent's
    /// `restore_account_pins_from_session`. A pin whose identity is gone (or
    /// whose label now names another login) is dropped and the user is told.
    pub(crate) fn restore_local_window_accounts(&mut self) {
        let mut dropped = Vec::new();
        for kind in AccountProviderKind::ALL {
            let key = kind.key();
            match self.session.account_pins.get(key).cloned() {
                Some(pin) => match resolve_local_pin(kind, &pin) {
                    Some(label) => {
                        let pin = AccountPin::new(label, pin.identity.clone());
                        if self.provider.set_account_pin(kind, Some(pin.clone())).is_ok() {
                            self.session.account_pins.insert(key.to_string(), pin);
                        }
                    }
                    None => {
                        let _ = self.provider.set_account_pin(kind, None);
                        let default = stored_default_label(key);
                        dropped.push((key, pin.label.clone(), default));
                    }
                },
                None => {
                    if self.provider.account_pin(kind).is_some() {
                        let _ = self.provider.set_account_pin(kind, None);
                    }
                }
            }
            self.provider
                .set_account_failover_home(kind, self.session.account_failover_home.get(key).cloned());
        }
        for (key, label, default) in &dropped {
            self.session.account_pins.remove(*key);
            let reason = match default {
                Some(default) => format!("{label} is no longer available, this window uses the default {default}"),
                None => format!("{label} is no longer available, this window uses the default account"),
            };
            self.push_display_message(DisplayMessage::system(format!("⚡ {reason}")));
        }
        if !dropped.is_empty() {
            let _ = self.session.save();
        }
        self.window_account_failover = self.session.account_failover;
        self.provider.set_account_failover(self.session.account_failover);
    }

    fn apply_set_default(&mut self, family: &str, label: &str) -> Result<(), String> {
        let result = match family {
            "claude" => crate::auth::claude::set_active_account(label),
            "openai" => crate::auth::codex::set_active_account(label),
            _ => return Err(format!("{family} has no stored accounts")),
        };
        result.map_err(|e| e.to_string())?;
        // `set_active_account` also sets the process runtime override, which
        // must not act as a window pin. Unpinned windows read the stored default.
        match family {
            "claude" => crate::auth::claude::set_active_account_override(None),
            _ => crate::auth::codex::set_active_account_override(None),
        }
        let route = if family == "claude" {
            "claude-oauth"
        } else {
            "openai-oauth"
        };
        if let Err(err) = crate::auth::account_pool::sync_order_with_default_route(route) {
            crate::logging::warn(&format!("account pool order sync failed: {err}"));
        }
        crate::auth::AuthStatus::invalidate_cache();
        // An unpinned window follows the new default.
        if let Some(info) = self.window_accounts.iter_mut().find(|i| i.provider == family) {
            if info.pinned {
                info.is_default = info.label.as_deref() == Some(label);
            } else {
                info.label = Some(label.to_string());
                info.is_default = true;
            }
        }
        Ok(())
    }

    pub(super) fn execute_window_account_command_local(&mut self, command: AccountCommand) {
        match command {
            AccountCommand::UseInWindow { provider_id, label } => {
                let Some(family) = account_family(&provider_id) else {
                    self.push_display_message(DisplayMessage::error(format!(
                        "Provider {provider_id} does not support multiple accounts."
                    )));
                    return;
                };
                if !label_exists(family, &label) {
                    self.push_display_message(DisplayMessage::error(format!(
                        "No {} account named {label}.",
                        family_display(family)
                    )));
                    return;
                }
                match self.apply_local_pin(family, Some(&label)) {
                    Ok(()) => {
                        let default = stored_default_label(family);
                        self.push_display_message(DisplayMessage::system(
                            use_in_window_message(&label, default.as_deref()),
                        ));
                        self.set_status_notice(format!("Account: {label} (this window)"));
                    }
                    Err(e) => self.push_display_message(DisplayMessage::error(format!(
                        "Failed to switch account: {e}"
                    ))),
                }
            }
            AccountCommand::SetDefault { provider_id, label } => {
                let Some(family) = account_family(&provider_id) else {
                    return;
                };
                let window = self.window_account(family);
                match self.apply_set_default(family, &label) {
                    Ok(()) => {
                        if window.as_ref().is_none_or(|w| !w.pinned) {
                            let provider = self.provider.clone();
                            tokio::spawn(async move { provider.invalidate_credentials().await });
                        }
                        self.push_display_message(DisplayMessage::system(set_default_message(
                            &label,
                            window.as_ref(),
                        )));
                        self.set_status_notice(format!("Default account: {label}"));
                    }
                    Err(e) => self.push_display_message(DisplayMessage::error(format!(
                        "Failed to set the default account: {e}"
                    ))),
                }
            }
            AccountCommand::Unpin { provider_id } => {
                let families: Vec<&str> = match provider_id.as_deref().and_then(account_family) {
                    Some(family) => vec![family],
                    None => vec!["claude", "openai"],
                };
                let mut lines = Vec::new();
                for family in families {
                    if let Err(e) = self.apply_local_pin(family, None) {
                        self.push_display_message(DisplayMessage::error(format!(
                            "Failed to unpin {}: {e}",
                            family_display(family)
                        )));
                        continue;
                    }
                    if let Some(default) = stored_default_label(family) {
                        lines.push(format!(
                            "{} in this window follows the default again ({default}).",
                            family_display(family)
                        ));
                    }
                }
                if lines.is_empty() {
                    lines.push("This window now follows the default account.".to_string());
                }
                self.push_display_message(DisplayMessage::system(lines.join("\n")));
                self.set_status_notice("Account: following default");
            }
            AccountCommand::Failover(mode) => {
                if mode != AccountFailoverMode::Status {
                    let enabled = match mode {
                        AccountFailoverMode::On => Some(true),
                        AccountFailoverMode::Off => Some(false),
                        _ => None,
                    };
                    self.window_account_failover = enabled;
                    self.session.account_failover = enabled;
                    // The provider decides whether to fail over; the session
                    // field alone only changes what is displayed.
                    self.provider.set_account_failover(enabled);
                    let _ = self.session.save();
                    self.push_display_message(DisplayMessage::system(failover_set_message(
                        enabled,
                    )));
                    self.set_status_notice(format!(
                        "Account failover: {}",
                        failover_word(self.effective_account_failover())
                    ));
                } else {
                    self.push_display_message(DisplayMessage::system(
                        self.render_account_failover_status(),
                    ));
                }
            }
            _ => {}
        }
    }

    pub(super) async fn execute_window_account_command_remote(
        &mut self,
        command: AccountCommand,
        remote: &mut crate::tui::backend::RemoteConnection,
    ) -> anyhow::Result<()> {
        match command {
            AccountCommand::UseInWindow { provider_id, label } => {
                let Some(family) = account_family(&provider_id) else {
                    self.push_display_message(DisplayMessage::error(format!(
                        "Provider {provider_id} does not support multiple accounts."
                    )));
                    return Ok(());
                };
                if !crate::tui::is_ssh_remote() && !label_exists(family, &label) {
                    self.push_display_message(DisplayMessage::error(format!(
                        "No {} account named {label}.",
                        family_display(family)
                    )));
                    return Ok(());
                }
                remote.set_session_account(family, Some(&label)).await?;
                let default = stored_default_label(family);
                self.set_window_account_from_info(&SessionAccountInfo {
                    provider: family.to_string(),
                    is_default: default.as_deref() == Some(label.as_str()),
                    label: Some(label.clone()),
                    pinned: true,
                });
                self.context_warning_shown = false;
                self.push_display_message(DisplayMessage::system(use_in_window_message(
                    &label,
                    default.as_deref(),
                )));
                self.set_status_notice(format!("Account: {label} (this window)"));
            }
            AccountCommand::SetDefault { provider_id, label } => {
                let Some(family) = account_family(&provider_id) else {
                    return Ok(());
                };
                let window = self.window_account(family);
                remote.set_default_account(family, &label).await?;
                if let Some(info) = self.window_accounts.iter_mut().find(|i| i.provider == family)
                {
                    if info.pinned {
                        info.is_default = info.label.as_deref() == Some(label.as_str());
                    } else {
                        info.label = Some(label.clone());
                        info.is_default = true;
                    }
                }
                self.push_display_message(DisplayMessage::system(set_default_message(
                    &label,
                    window.as_ref(),
                )));
                self.set_status_notice(format!("Default account: {label}"));
            }
            AccountCommand::Unpin { provider_id } => {
                let families: Vec<&str> = match provider_id.as_deref().and_then(account_family) {
                    Some(family) => vec![family],
                    None => vec!["claude", "openai"],
                };
                for family in &families {
                    remote.set_session_account(family, None).await?;
                    let default = stored_default_label(family);
                    self.set_window_account_from_info(&SessionAccountInfo {
                        provider: family.to_string(),
                        is_default: default.is_some(),
                        label: default,
                        pinned: false,
                    });
                }
                self.push_display_message(DisplayMessage::system(
                    "This window now follows the default account.".to_string(),
                ));
                self.set_status_notice("Account: following default");
            }
            AccountCommand::Failover(AccountFailoverMode::Status) => {
                self.push_display_message(DisplayMessage::system(
                    self.render_account_failover_status(),
                ));
            }
            AccountCommand::Failover(mode) => {
                let enabled = match mode {
                    AccountFailoverMode::On => Some(true),
                    AccountFailoverMode::Off => Some(false),
                    _ => None,
                };
                remote.set_account_failover(enabled).await?;
                self.window_account_failover = enabled;
                self.push_display_message(DisplayMessage::system(failover_set_message(enabled)));
                self.set_status_notice(format!(
                    "Account failover: {}",
                    failover_word(self.effective_account_failover())
                ));
            }
            other => super::auth::execute_account_command_local(self, other),
        }
        Ok(())
    }

    fn effective_account_failover(&self) -> bool {
        self.window_account_failover
            .or(if self.is_remote {
                None
            } else {
                self.session.account_failover
            })
            .unwrap_or(crate::config::config().provider.same_provider_account_failover)
    }

    /// `/account failover status`: this window's account, the default, the
    /// rotation order, and each account's exhaustion as far as this client knows.
    pub(crate) fn render_account_failover_status(&self) -> String {
        let enabled = self.effective_account_failover();
        let source = match self.window_account_failover.or(if self.is_remote {
            None
        } else {
            self.session.account_failover
        }) {
            Some(_) => "set for this window",
            None => "config default",
        };
        let mut lines = vec![format!(
            "Account failover: {} ({source})",
            failover_word(enabled)
        )];
        let pool = crate::auth::account_pool::AccountPool::load();
        for family in ["claude", "openai"] {
            let labels = stored_labels(family);
            if labels.is_empty() {
                continue;
            }
            let window = self.window_account(family);
            let current = window.as_ref().and_then(|w| w.label.clone());
            let default = stored_default_label(family);
            lines.push(String::new());
            lines.push(format!("{}:", family_display(family)));
            lines.push(format!(
                "  This window: {}{}",
                current.as_deref().unwrap_or("none"),
                if window.as_ref().is_some_and(|w| w.pinned) {
                    " (pinned)"
                } else {
                    " (follows default)"
                }
            ));
            lines.push(format!(
                "  Default for new windows: {}",
                default.as_deref().unwrap_or("none")
            ));
            let rotation = pool.rotation(family, current.as_deref(), &labels);
            lines.push(format!(
                "  Rotation after {}: {}",
                current.as_deref().unwrap_or("current"),
                if rotation.is_empty() {
                    "no other accounts in the pool".to_string()
                } else {
                    rotation.join(" → ")
                }
            ));
            for label in &labels {
                lines.push(format!("  - {label}: {}", account_limit_state(family, label)));
            }
        }
        if lines.len() == 1 {
            lines.push("No saved Claude or OpenAI accounts.".to_string());
        }
        lines.push(String::new());
        lines.push("Change it: /account failover on|off|default".to_string());
        lines.join("\n")
    }
}

fn failover_word(enabled: bool) -> &'static str {
    if enabled { "on" } else { "off" }
}

fn failover_set_message(enabled: Option<bool>) -> String {
    match enabled {
        Some(true) => "Account failover is on for this window. When its account runs out of usage it moves to the next saved account of the same provider.".to_string(),
        Some(false) => "Account failover is off for this window. It stays on its account when that account runs out of usage.".to_string(),
        None => format!(
            "This window uses the configured account failover default ({}).",
            failover_word(crate::config::config().provider.same_provider_account_failover)
        ),
    }
}

/// Client-side knowledge of one account's limits: provider unavailability
/// marks and cached usage. Never fetches.
fn account_limit_state(family: &str, label: &str) -> String {
    if family == "openai" {
        let usage = crate::usage::get_openai_usage_sync();
        if usage.fetched_at.is_some()
            && super::window_account::stored_default_label("openai").as_deref() == Some(label)
            && usage.exhausted()
        {
            let reset = usage
                .five_hour
                .as_ref()
                .and_then(|w| w.resets_at.as_deref())
                .map(crate::usage::format_reset_time);
            return match reset {
                Some(reset) => format!("exhausted · resets {reset}"),
                None => "exhausted".to_string(),
            };
        }
    } else if super::window_account::stored_default_label("claude").as_deref() == Some(label) {
        let usage = crate::usage::get_sync();
        if usage.fetched_at.is_some() && usage.five_hour >= 0.99 {
            let reset = usage
                .five_hour_resets_at
                .as_deref()
                .map(crate::usage::format_reset_time);
            return match reset {
                Some(reset) => format!("exhausted · resets {reset}"),
                None => "exhausted".to_string(),
            };
        }
    }
    "available (as far as this window knows)".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_messages_name_the_window_and_the_default() {
        assert_eq!(
            use_in_window_message("claude-fox", Some("claude-otter")),
            "This window now uses claude-fox. The default for new windows is still claude-otter (/account default claude-fox to change it)."
        );
        assert!(use_in_window_message("claude-fox", Some("claude-fox")).contains("also the default"));
        let pinned = SessionAccountInfo {
            provider: "claude".into(),
            label: Some("claude-otter".into()),
            pinned: true,
            is_default: false,
        };
        assert!(set_default_message("claude-fox", Some(&pinned)).contains("stays pinned to claude-otter"));
    }

    #[test]
    fn account_family_maps_provider_names() {
        assert_eq!(account_family("Anthropic"), Some("claude"));
        assert_eq!(account_family("claude-oauth"), Some("claude"));
        assert_eq!(account_family("openai"), Some("openai"));
        assert_eq!(account_family("OpenRouter"), None);
    }
}
