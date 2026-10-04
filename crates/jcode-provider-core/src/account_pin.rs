//! Per-session account pins.
//!
//! A pin names one stored jcode account for one provider family. It lives on
//! the provider instance (one per session fork), so two windows can use two
//! different subscriptions of the same provider at the same time.

use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

/// Stable account pin: a label plus an identity so relabeling (labels are
/// positional) cannot silently move a session to another subscription.
///
/// `identity` is the account email, else the OpenAI account id, else a short
/// sha256 prefix of the refresh token captured when the pin was made.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccountPin {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
}

impl AccountPin {
    pub fn new(label: impl Into<String>, identity: Option<String>) -> Self {
        Self {
            label: label.into(),
            identity,
        }
    }
}

/// Which account a credential load should use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AccountScope<'a> {
    /// The default account (stored active account, external sources included).
    #[default]
    Default,
    /// Exactly this jcode-stored account.
    Pinned(&'a AccountPin),
}

impl<'a> AccountScope<'a> {
    pub fn from_pin(pin: Option<&'a AccountPin>) -> Self {
        match pin {
            Some(pin) => Self::Pinned(pin),
            None => Self::Default,
        }
    }

    pub fn pin(self) -> Option<&'a AccountPin> {
        match self {
            Self::Default => None,
            Self::Pinned(pin) => Some(pin),
        }
    }
}

/// Shared per-instance pin state for a provider runtime.
///
/// Cloning shares the slot (the spawned stream task clones it so it sees a pin
/// change mid-retry). [`AccountPinSlot::fork`] copies the *value* into a new
/// slot for a new session, which must not share mutable pin state.
#[derive(Clone, Debug, Default)]
pub struct AccountPinSlot {
    pin: Arc<RwLock<Option<AccountPin>>>,
    resolved_label: Arc<RwLock<Option<String>>>,
}

impl AccountPinSlot {
    /// Current pin (a snapshot; re-read per credential resolution).
    pub fn get(&self) -> Option<AccountPin> {
        self.pin
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Replace the pin. Returns true when the value changed.
    pub fn set(&self, pin: Option<AccountPin>) -> bool {
        let mut guard = self
            .pin
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *guard == pin {
            return false;
        }
        *guard = pin;
        drop(guard);
        self.set_resolved_label(None);
        true
    }

    /// Label the last credential resolution used (pin or default account).
    pub fn resolved_label(&self) -> Option<String> {
        self.resolved_label
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn set_resolved_label(&self, label: Option<String>) {
        *self
            .resolved_label
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = label;
    }

    /// Independent slot carrying the same pin value.
    pub fn fork(&self) -> Self {
        Self {
            pin: Arc::new(RwLock::new(self.get())),
            resolved_label: Arc::new(RwLock::new(self.resolved_label())),
        }
    }
}

/// Provider families that support multiple stored accounts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountProviderKind {
    Claude,
    #[serde(rename = "openai")]
    OpenAi,
    /// Claude Code CLI instances. Labels are instance ids from
    /// `[provider.claude_code]`, not stored jcode accounts.
    #[serde(rename = "claude-code")]
    ClaudeCode,
}

impl AccountProviderKind {
    pub const ALL: [AccountProviderKind; 3] = [Self::Claude, Self::OpenAi, Self::ClaudeCode];

    /// Stable key used for session persistence and the wire
    /// ("claude" | "openai" | "claude-code").
    pub fn key(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::OpenAi => "openai",
            Self::ClaudeCode => "claude-code",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key.trim().to_ascii_lowercase().as_str() {
            "claude" | "anthropic" => Some(Self::Claude),
            "openai" | "codex" => Some(Self::OpenAi),
            "claude-code" | "claude_code" | "claudecode" => Some(Self::ClaudeCode),
            _ => None,
        }
    }

    /// Infer the provider from an account label prefix (`claude-fox`, `openai-otter`).
    /// Claude Code instance ids are free-form, so they are never inferred.
    pub fn from_label(label: &str) -> Option<Self> {
        let label = label.trim();
        if label.starts_with("claude-") {
            Some(Self::Claude)
        } else if label.starts_with("openai-") {
            Some(Self::OpenAi)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_serde_roundtrip_and_identity_defaults() {
        let pin = AccountPin::new("claude-fox", Some("fox@example.com".into()));
        let json = serde_json::to_string(&pin).unwrap();
        assert_eq!(serde_json::from_str::<AccountPin>(&json).unwrap(), pin);
        let bare: AccountPin = serde_json::from_str(r#"{"label":"openai-otter"}"#).unwrap();
        assert_eq!(bare.identity, None);
    }

    #[test]
    fn provider_kind_keys() {
        for kind in AccountProviderKind::ALL {
            assert_eq!(AccountProviderKind::from_key(kind.key()), Some(kind));
        }
        assert_eq!(
            AccountProviderKind::from_label("openai-fox"),
            Some(AccountProviderKind::OpenAi)
        );
    }

    #[test]
    fn claude_code_kind_serde() {
        let kind = AccountProviderKind::ClaudeCode;
        assert_eq!(serde_json::to_string(&kind).unwrap(), "\"claude-code\"");
        assert_eq!(
            serde_json::from_str::<AccountProviderKind>("\"claude-code\"").unwrap(),
            kind
        );
        assert_eq!(kind.key(), "claude-code");
        assert_eq!(
            AccountProviderKind::from_label("claude-code-x"),
            Some(AccountProviderKind::Claude)
        );
        assert_eq!(
            AccountProviderKind::from_key("claude"),
            Some(AccountProviderKind::Claude)
        );
    }
}
