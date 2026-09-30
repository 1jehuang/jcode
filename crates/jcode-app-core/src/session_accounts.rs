//! Per-session account helpers shared by the agent and the server.
//!
//! A session pins one stored account per provider family (see
//! `jcode_provider_core::account_pin`). These helpers resolve labels to pins,
//! read the stored default without the process-global runtime override, and
//! build the `SessionAccountInfo` snapshot clients render.
//!
//! The label/identity lookups mirror slice S1's `auth::{claude,codex}`
//! helpers (`pin_for_label`, `resolve_pin`, `default_account_label`). They
//! live here so this slice builds on its own; after the merge they can
//! delegate to the S1 functions.

use crate::protocol::SessionAccountInfo;
use crate::provider::{AccountPin, AccountProviderKind, Provider};
use crate::session::Session;
use anyhow::Result;
use std::collections::BTreeMap;

/// Account state a child session (swarm worker, split, transfer, overnight
/// coordinator) inherits from its parent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountInheritance {
    pub pins: BTreeMap<String, AccountPin>,
    pub failover: Option<bool>,
}

impl AccountInheritance {
    pub fn from_session(session: &Session) -> Self {
        Self {
            pins: session.account_pins.clone(),
            failover: session.account_failover,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.pins.is_empty() && self.failover.is_none()
    }

    /// Copy the pins and the failover toggle onto a child session. The
    /// failover "home" is not inherited: it belongs to the parent's history.
    pub fn apply_to_session(&self, session: &mut Session) {
        session.account_pins = self.pins.clone();
        session.account_failover = self.failover;
    }
}

/// Stored (label, identity) pairs for one provider family.
fn stored_accounts(kind: AccountProviderKind) -> Vec<(String, Option<String>)> {
    match kind {
        AccountProviderKind::Claude => crate::auth::claude::list_accounts()
            .unwrap_or_default()
            .into_iter()
            .map(|account| {
                let identity = claude_identity(&account);
                (account.label, identity)
            })
            .collect(),
        AccountProviderKind::OpenAi => crate::auth::codex::list_accounts()
            .unwrap_or_default()
            .into_iter()
            .map(|account| {
                let identity = openai_identity(&account);
                (account.label, identity)
            })
            .collect(),
    }
}

fn secret_fingerprint(secret: &str) -> Option<String> {
    use sha2::{Digest, Sha256};
    let secret = secret.trim();
    if secret.is_empty() {
        return None;
    }
    Some(format!("{:x}", Sha256::digest(secret.as_bytes()))[..16].to_string())
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn claude_identity(account: &crate::auth::claude::AnthropicAccount) -> Option<String> {
    non_empty(account.email.as_deref()).or_else(|| secret_fingerprint(&account.refresh))
}

fn openai_identity(account: &crate::auth::codex::OpenAiAccount) -> Option<String> {
    non_empty(account.email.as_deref())
        .or_else(|| non_empty(account.account_id.as_deref()))
        .or_else(|| secret_fingerprint(&account.refresh_token))
}

/// Build a pin for a stored account label. Fails when no such account exists.
pub fn pin_for_label(kind: AccountProviderKind, label: &str) -> Result<AccountPin> {
    let label = label.trim();
    stored_accounts(kind)
        .into_iter()
        .find(|(stored, _)| stored == label)
        .map(|(label, identity)| AccountPin::new(label, identity))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No {} account with label '{}' found",
                provider_display(kind),
                label
            )
        })
}

/// Current label of the account a pin names: identity first (labels are
/// positional and move when an earlier account is removed), then label.
pub fn resolve_pin(kind: AccountProviderKind, pin: &AccountPin) -> Option<String> {
    let accounts = stored_accounts(kind);
    if let Some(identity) = pin.identity.as_deref()
        && let Some((label, _)) = accounts
            .iter()
            .find(|(_, stored)| stored.as_deref() == Some(identity))
    {
        return Some(label.clone());
    }
    accounts
        .into_iter()
        .find(|(label, _)| *label == pin.label)
        .map(|(label, _)| label)
}

/// Stored default account (`active_*_account`, else the first account). Never
/// consults the process-global runtime override.
pub fn default_label(kind: AccountProviderKind) -> Option<String> {
    match kind {
        AccountProviderKind::Claude => {
            let auth = crate::auth::claude::load_auth_file().ok()?;
            auth.active_anthropic_account
                .filter(|label| auth.anthropic_accounts.iter().any(|a| &a.label == label))
                .or_else(|| auth.anthropic_accounts.first().map(|a| a.label.clone()))
        }
        AccountProviderKind::OpenAi => {
            let auth = crate::auth::codex::load_auth_file().ok()?;
            auth.active_openai_account
                .filter(|label| auth.openai_accounts.iter().any(|a| &a.label == label))
                .or_else(|| auth.openai_accounts.first().map(|a| a.label.clone()))
        }
    }
}

/// Change the stored default account used by new and unpinned sessions.
/// Unlike `auth::*::set_active_account`, this never sets the process-global
/// runtime override, so no pinned session moves.
pub fn set_default_label(kind: AccountProviderKind, label: &str) -> Result<()> {
    let label = label.trim();
    match kind {
        AccountProviderKind::Claude => {
            let mut auth = crate::auth::claude::load_auth_file()?;
            crate::auth::account_store::set_active_account(
                label,
                &auth.anthropic_accounts,
                &mut auth.active_anthropic_account,
                "No Claude account with label '{}' found",
                |account| account.label.as_str(),
            )?;
            crate::auth::claude::save_auth_file(&auth)?;
        }
        AccountProviderKind::OpenAi => {
            let mut auth = crate::auth::codex::load_auth_file()?;
            crate::auth::account_store::set_active_account(
                label,
                &auth.openai_accounts,
                &mut auth.active_openai_account,
                "No OpenAI account with label '{}' found",
                |account| account.label.as_str(),
            )?;
            crate::auth::codex::save_auth_file(&auth)?;
        }
    }
    // Keep the auto-switch order in step when this provider is the configured
    // default route. Another provider's default route keeps its place.
    if let Some(route) = crate::auth::account_pool::default_route_for_key(kind.key())
        && crate::config::Config::load()
            .provider
            .default_provider
            .as_deref()
            .is_some_and(|current| crate::auth::account_pool::same_route(current, route))
        && let Err(error) = crate::auth::account_pool::sync_order_with_default_route(route)
    {
        crate::logging::warn(&format!("Could not sync account order: {error}"));
    }
    crate::auth::AuthStatus::invalidate_cache();
    Ok(())
}

pub fn provider_display(kind: AccountProviderKind) -> &'static str {
    match kind {
        AccountProviderKind::Claude => "Claude",
        AccountProviderKind::OpenAi => "OpenAI",
    }
}

/// Legacy runtime name used by `CredentialsChanged` and the refresh path.
pub fn runtime_provider_name(kind: AccountProviderKind) -> &'static str {
    match kind {
        AccountProviderKind::Claude => "anthropic",
        AccountProviderKind::OpenAi => "openai",
    }
}

/// Provider family named by a wire `provider` value, else by the label prefix.
pub fn kind_for_request(provider: &str, label: Option<&str>) -> Option<AccountProviderKind> {
    AccountProviderKind::from_key(provider)
        .or_else(|| label.and_then(AccountProviderKind::from_label))
}

/// What one provider instance uses for `kind`.
pub fn account_info(provider: &dyn Provider, kind: AccountProviderKind) -> SessionAccountInfo {
    let pin = provider.account_pin(kind);
    let default = default_label(kind);
    let label = provider
        .resolved_account_label(kind)
        .or_else(|| {
            pin.as_ref()
                .map(|pin| resolve_pin(kind, pin).unwrap_or_else(|| pin.label.clone()))
        })
        .or_else(|| default.clone());
    SessionAccountInfo {
        provider: kind.key().to_string(),
        is_default: label.is_some() && label == default,
        pinned: pin.is_some(),
        label,
    }
}

/// Snapshot for every provider family that has a stored account or a pin.
pub fn account_infos(provider: &dyn Provider) -> Vec<SessionAccountInfo> {
    AccountProviderKind::ALL
        .into_iter()
        .filter(|kind| provider.account_pin(*kind).is_some() || !stored_accounts(*kind).is_empty())
        .map(|kind| account_info(provider, kind))
        .collect()
}

/// Snapshot for a persisted session whose live provider is not at hand (the
/// agent is busy). The label is the pin's current label, else the default.
pub fn account_infos_from_pins(pins: &BTreeMap<String, AccountPin>) -> Vec<SessionAccountInfo> {
    AccountProviderKind::ALL
        .into_iter()
        .filter(|kind| pins.contains_key(kind.key()) || !stored_accounts(*kind).is_empty())
        .map(|kind| {
            let pin = pins.get(kind.key());
            let default = default_label(kind);
            let label = pin
                .map(|pin| resolve_pin(kind, pin).unwrap_or_else(|| pin.label.clone()))
                .or_else(|| default.clone());
            SessionAccountInfo {
                provider: kind.key().to_string(),
                is_default: label.is_some() && label == default,
                pinned: pin.is_some(),
                label,
            }
        })
        .collect()
}

/// `SessionAccountChanged` event for one provider family.
pub fn account_changed_event(
    provider: &dyn Provider,
    kind: AccountProviderKind,
    reason: Option<String>,
) -> crate::protocol::ServerEvent {
    let info = account_info(provider, kind);
    crate::protocol::ServerEvent::SessionAccountChanged {
        provider: info.provider,
        label: info.label,
        pinned: info.pinned,
        is_default: info.is_default,
        reason,
    }
}
