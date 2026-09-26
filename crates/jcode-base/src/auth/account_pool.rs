//! The auto-switch pool: which accounts Jcode may rotate through when the
//! active one runs out, and in what order.
//!
//! Entries are keyed `provider` for single-credential providers (API keys,
//! device logins) and `provider:label` for multi-account OAuth providers
//! (`openai:openai-otter`). Accounts the user never placed fall back to a
//! caller-supplied default: subscription logins join the pool, metered API
//! keys stay out, so nobody is silently billed per token after a quota runs out.
//!
//! The file stores only keys and order, never credentials.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const FILE_NAME: &str = "account-pool.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountPool {
    /// Auto-switch members in rotation order.
    #[serde(default)]
    pub members: Vec<String>,
    /// Accounts the user explicitly kept out of rotation.
    #[serde(default)]
    pub excluded: Vec<String>,
}

/// Stable pool key for an account.
pub fn account_key(provider: &str, label: Option<&str>) -> String {
    match label {
        Some(label) => format!("{provider}:{label}"),
        None => provider.to_string(),
    }
}

/// One stored OAuth login, without any token material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthLogin {
    /// Auth-status provider id: `openai` or `claude`.
    pub provider: &'static str,
    pub label: String,
    pub email: Option<String>,
    /// The login new requests use right now.
    pub active: bool,
}

/// Every multi-account OAuth login on this machine, in stored order.
pub fn oauth_logins() -> Vec<OAuthLogin> {
    let mut logins = Vec::new();
    let openai_active = crate::auth::codex::active_account_label();
    for account in crate::auth::codex::list_accounts().unwrap_or_default() {
        logins.push(OAuthLogin {
            provider: "openai",
            active: openai_active.as_deref() == Some(account.label.as_str()),
            label: account.label,
            email: account.email,
        });
    }
    let claude_active = crate::auth::claude::active_account_label();
    for account in crate::auth::claude::list_accounts().unwrap_or_default() {
        logins.push(OAuthLogin {
            provider: "claude",
            active: claude_active.as_deref() == Some(account.label.as_str()),
            label: account.label,
            email: account.email,
        });
    }
    logins
}

fn path() -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?.join(FILE_NAME))
}

impl AccountPool {
    /// Missing or unreadable files mean "use defaults", never an error.
    pub fn load() -> Self {
        path()
            .ok()
            .filter(|path| path.exists())
            .and_then(|path| crate::storage::read_json(&path).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        crate::storage::write_json(&path()?, self)
    }

    /// Whether `key` rotates automatically. `default` applies to accounts the
    /// user has not placed yet.
    pub fn is_member(&self, key: &str, default: bool) -> bool {
        if self.members.iter().any(|member| member == key) {
            return true;
        }
        if self.excluded.iter().any(|excluded| excluded == key) {
            return false;
        }
        default
    }

    /// Split `accounts` (key, default membership) into ordered pool members and
    /// the rest. Placed members keep the user's order, and new defaults follow
    /// in the caller's order.
    pub fn partition<'a>(&self, accounts: &'a [(String, bool)]) -> (Vec<&'a str>, Vec<&'a str>) {
        let mut members: Vec<&str> = self
            .members
            .iter()
            .filter_map(|member| {
                accounts
                    .iter()
                    .find(|(key, _)| key == member)
                    .map(|(key, _)| key.as_str())
            })
            .collect();
        let mut others = Vec::new();
        for (key, default) in accounts {
            if members.contains(&key.as_str()) {
                continue;
            }
            if self.is_member(key, *default) {
                members.push(key);
            } else {
                others.push(key.as_str());
            }
        }
        (members, others)
    }

    /// Move `key` into (or out of) the pool at `index` among the currently
    /// visible members. `visible` is the member order the user saw, so
    /// defaulted members are pinned in place when the first edit is made.
    pub fn place(&mut self, key: &str, pooled: bool, index: usize, visible: &[&str]) {
        let mut order: Vec<String> = visible
            .iter()
            .filter(|member| **member != key)
            .map(|member| member.to_string())
            .collect();
        // Keep placed members that are not visible (for example logged-out
        // accounts) so hiding an account never forgets its position.
        for member in &self.members {
            if member != key && !order.contains(member) {
                order.push(member.clone());
            }
        }
        self.excluded.retain(|excluded| excluded != key);
        if pooled {
            order.insert(index.min(visible.len()).min(order.len()), key.to_string());
        } else {
            self.excluded.push(key.to_string());
        }
        self.members = order;
    }

    /// Order same-provider failover candidates: pool members only, cycling
    /// from the account after `current` so rotation spreads load instead of
    /// always retrying the first account.
    pub fn rotation(
        &self,
        provider: &str,
        current: Option<&str>,
        labels: &[String],
    ) -> Vec<String> {
        let keyed: Vec<(String, bool)> = labels
            .iter()
            .map(|label| (account_key(provider, Some(label)), true))
            .collect();
        let current_key = current.map(|label| account_key(provider, Some(label)));
        let mut members: Vec<String> = self
            .partition(&keyed)
            .0
            .into_iter()
            .map(str::to_string)
            .collect();
        if let Some(position) = current_key
            .as_ref()
            .and_then(|key| members.iter().position(|member| member == key))
        {
            members.rotate_left(position + 1);
        }
        let prefix = format!("{provider}:");
        members
            .into_iter()
            .filter(|key| Some(key) != current_key.as_ref())
            .filter_map(|key| key.strip_prefix(&prefix).map(str::to_string))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accounts(entries: &[(&str, bool)]) -> Vec<(String, bool)> {
        entries
            .iter()
            .map(|(key, default)| (key.to_string(), *default))
            .collect()
    }

    #[test]
    fn defaults_put_subscriptions_in_and_api_keys_out() {
        let pool = AccountPool::default();
        let list = accounts(&[
            ("openai:openai-otter", true),
            ("openai-api", false),
            ("claude:claude-otter", true),
        ]);
        let (members, others) = pool.partition(&list);
        assert_eq!(members, ["openai:openai-otter", "claude:claude-otter"]);
        assert_eq!(others, ["openai-api"]);
    }

    #[test]
    fn placing_moves_between_groups_and_preserves_order() {
        let mut pool = AccountPool::default();
        let list = accounts(&[("a", true), ("b", true), ("key", false)]);
        let (visible, _) = pool.partition(&list);
        let visible: Vec<&str> = visible.to_vec();
        pool.place("key", true, 0, &visible);
        assert_eq!(pool.partition(&list).0, ["key", "a", "b"]);
        let visible: Vec<String> = pool
            .partition(&list)
            .0
            .iter()
            .map(|s| s.to_string())
            .collect();
        let visible: Vec<&str> = visible.iter().map(String::as_str).collect();
        pool.place("a", false, 0, &visible);
        let (members, others) = pool.partition(&list);
        assert_eq!(members, ["key", "b"]);
        assert_eq!(others, ["a"]);
        // Reordering within the pool.
        let visible: Vec<String> = members.iter().map(|s| s.to_string()).collect();
        let visible: Vec<&str> = visible.iter().map(String::as_str).collect();
        pool.place("b", true, 0, &visible);
        assert_eq!(pool.partition(&list).0, ["b", "key"]);
    }

    #[test]
    fn hidden_members_keep_their_place() {
        let mut pool = AccountPool {
            members: vec!["gone".into(), "a".into()],
            excluded: vec![],
        };
        let list = accounts(&[("a", true), ("b", true)]);
        pool.place("b", true, 0, &["a"]);
        assert!(pool.members.contains(&"gone".to_string()));
        assert_eq!(pool.partition(&list).0, ["b", "a"]);
    }

    #[test]
    fn rotation_cycles_after_current_and_skips_excluded() {
        let pool = AccountPool {
            members: vec![],
            excluded: vec!["openai:openai-fox".into()],
        };
        let labels: Vec<String> = ["openai-otter", "openai-fox", "openai-panda", "openai-wolf"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(
            pool.rotation("openai", Some("openai-panda"), &labels),
            ["openai-wolf", "openai-otter"]
        );
        assert_eq!(
            pool.rotation("openai", None, &labels),
            ["openai-otter", "openai-panda", "openai-wolf"]
        );
    }
}
