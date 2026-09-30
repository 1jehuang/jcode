//! Per-session account pins.
//!
//! A pin names one stored jcode account for one provider family. It lives on
//! the provider instance (one per session fork), so two windows can use two
//! different subscriptions of the same provider at the same time.

use serde::{Deserialize, Serialize};

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

/// Provider families that support multiple stored accounts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountProviderKind {
    Claude,
    #[serde(rename = "openai")]
    OpenAi,
}

impl AccountProviderKind {
    pub const ALL: [AccountProviderKind; 2] = [Self::Claude, Self::OpenAi];

    /// Stable key used for session persistence and the wire ("claude" | "openai").
    pub fn key(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::OpenAi => "openai",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key.trim().to_ascii_lowercase().as_str() {
            "claude" | "anthropic" => Some(Self::Claude),
            "openai" | "codex" => Some(Self::OpenAi),
            _ => None,
        }
    }

    /// Infer the provider from an account label prefix (`claude-fox`, `openai-otter`).
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
}
