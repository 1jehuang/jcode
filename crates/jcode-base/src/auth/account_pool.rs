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

/// The `default_provider` route an account key selects. OAuth and API-key
/// logins of the same vendor map to distinct routes so the billing identity
/// the user ordered is the one new sessions use.
pub fn default_route_for_key(key: &str) -> Option<&'static str> {
    let provider = key.split_once(':').map_or(key, |(provider, _)| provider);
    match provider {
        "claude" => Some("claude-oauth"),
        "anthropic-api" => Some("anthropic-api"),
        "openai" => Some("openai-oauth"),
        "openai-api" => Some("openai-api"),
        other => crate::provider_catalog::resolve_login_provider(other).and_then(|login| {
            crate::provider::MultiProvider::config_default_provider_for_login_provider(login)
        }),
    }
}

/// Keep a configured model when it belongs to the same vendor as `route`,
/// re-prefixed for the new route. A model from another vendor cannot run on
/// the new route, so it is cleared and the provider's own default applies.
fn model_for_route(model: Option<&str>, route: &str) -> Option<String> {
    let model = model?.trim();
    let target = jcode_provider_core::AuthRoute::parse(route)?.active_provider();
    let (provider, bare) =
        match jcode_provider_core::selection::explicit_model_provider_prefix(model) {
            Some((provider, _, bare)) => (Some(provider), bare),
            None => (None, model),
        };
    let vendor = provider.or_else(|| {
        if bare.starts_with("claude") {
            Some(jcode_provider_core::ActiveProvider::Claude)
        } else if bare.starts_with("gpt") || bare.starts_with("o3") || bare.starts_with("o4") {
            Some(jcode_provider_core::ActiveProvider::OpenAI)
        } else {
            None
        }
    })?;
    (vendor == target).then(|| format!("{route}:{bare}"))
}

/// Whether two `default_provider` values pick the same route, so `claude`
/// and `claude-oauth` are not treated as a change.
pub fn same_route(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim(), b.trim());
    match (
        jcode_provider_core::AuthRoute::parse(a),
        jcode_provider_core::AuthRoute::parse(b),
    ) {
        (Some(a), Some(b)) => a == b,
        _ => a.eq_ignore_ascii_case(b),
    }
}

/// The account a configured default route points at, among `keys`. For
/// multi-account OAuth the active login wins, then the first listed one.
pub fn account_for_route<'a>(
    route: &str,
    keys: impl IntoIterator<Item = &'a str> + Clone,
    active: impl Fn(&str) -> Option<String>,
) -> Option<&'a str> {
    let matches = |key: &&str| default_route_for_key(key).is_some_and(|r| same_route(r, route));
    keys.clone()
        .into_iter()
        .filter(matches)
        .find(|key| {
            key.split_once(':')
                .is_some_and(|(provider, label)| active(provider).as_deref() == Some(label))
        })
        .or_else(|| keys.into_iter().find(matches))
}

fn active_label(provider: &str) -> Option<String> {
    match provider {
        "claude" => crate::auth::claude::active_account_label(),
        "openai" => crate::auth::codex::active_account_label(),
        _ => None,
    }
}

/// Which of `keys` is the default for new sessions, per the saved config.
/// This is the single source of truth: the account order follows it.
pub fn default_account_key<'a>(keys: impl IntoIterator<Item = &'a str> + Clone) -> Option<&'a str> {
    let cfg = crate::config::Config::load();
    let route = cfg.provider.default_provider.as_deref()?;
    account_for_route(route, keys, active_label)
}

impl AccountPool {
    /// Move the auto-switch account for `route` to the front. Accounts the
    /// user kept manual stay manual: choosing a default elsewhere must not
    /// silently enroll a metered key in failover. Returns whether it moved.
    fn promote_route(&mut self, route: &str, active: impl Fn(&str) -> Option<String>) -> bool {
        let members: Vec<&str> = self.members.iter().map(String::as_str).collect();
        let key = account_for_route(route, members.iter().copied(), &active)
            .map(str::to_owned)
            .or_else(|| {
                // A never-placed account that is auto-switch by default.
                let (key, api_key) = key_for_route(route, &active)?;
                (default_member(&key, api_key) && !self.excluded.contains(&key)).then_some(key)
            });
        let Some(key) = key else {
            return false;
        };
        if self.members.first() == Some(&key) {
            return false;
        }
        self.members.retain(|member| *member != key);
        self.members.insert(0, key);
        true
    }
}

/// The pool key a route selects when no placed member matches, and whether
/// it is a metered API key.
fn key_for_route(route: &str, active: impl Fn(&str) -> Option<String>) -> Option<(String, bool)> {
    for provider in ["claude", "openai"] {
        if default_route_for_key(provider).is_some_and(|r| same_route(r, route)) {
            return Some((account_key(provider, Some(&active(provider)?)), false));
        }
    }
    let login = crate::provider_catalog::login_providers()
        .iter()
        .copied()
        .find(|login| default_route_for_key(login.id).is_some_and(|r| same_route(r, route)))?;
    Some((
        login.id.to_string(),
        login.auth_kind == crate::provider_catalog::LoginProviderAuthKind::ApiKey,
    ))
}

/// Keep the auto-switch order in step after the default provider changed
/// through any path (model picker, `/account`, login, Desktop).
pub fn sync_order_with_default_route(route: &str) -> Result<()> {
    let mut pool = AccountPool::load();
    if pool.promote_route(route, active_label) {
        pool.save()?;
    }
    Ok(())
}

/// Make `key` the default for new sessions: its route becomes the configured
/// default provider and, for multi-account OAuth, its login becomes active.
/// Returns whether anything changed.
pub fn apply_default_account(key: &str) -> Result<bool> {
    let Some(route) = default_route_for_key(key) else {
        return Ok(false);
    };
    let mut changed = false;
    if let Some((provider, label)) = key.split_once(':') {
        match provider {
            "claude" if crate::auth::claude::active_account_label().as_deref() != Some(label) => {
                crate::auth::claude::set_active_account(label)?;
                changed = true;
            }
            "openai" if crate::auth::codex::active_account_label().as_deref() != Some(label) => {
                crate::auth::codex::set_active_account(label)?;
                changed = true;
            }
            _ => {}
        }
    }
    let cfg = crate::config::Config::load();
    if !cfg
        .provider
        .default_provider
        .as_deref()
        .is_some_and(|current| same_route(current, route))
    {
        // Only prefixed routes can carry the model across. Other providers
        // keep a bare model id only when it is not pinned to another vendor.
        let model = if jcode_provider_core::AuthRoute::parse(route).is_some() {
            model_for_route(cfg.provider.default_model.as_deref(), route)
        } else {
            None
        };
        crate::config::Config::set_default_model(model.as_deref(), Some(route))?;
        changed = true;
    }
    Ok(changed)
}

/// Pool key of the Jcode subscription. It joins auto-switch by default but
/// always comes last among defaulted members: it is the fallback once the
/// user's own subscriptions run out.
pub const JCODE_SUBSCRIPTION_KEY: &str = "jcode";

/// Default auto-switch membership for an account the user never placed:
/// subscription logins (OAuth, device sign-in, the Jcode subscription) join,
/// metered API keys stay manual so nobody is silently billed per token.
pub fn default_member(key: &str, api_key: bool) -> bool {
    key == JCODE_SUBSCRIPTION_KEY || !api_key
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
    /// in the caller's order, except the Jcode subscription, which defaults to
    /// the end of the pool.
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
        let mut last = None;
        for (key, default) in accounts {
            if members.contains(&key.as_str()) {
                continue;
            }
            if self.is_member(key, *default) {
                if key == JCODE_SUBSCRIPTION_KEY {
                    last = Some(key.as_str());
                } else {
                    members.push(key);
                }
            } else {
                others.push(key.as_str());
            }
        }
        members.extend(last);
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
    fn account_keys_map_to_distinct_default_routes() {
        assert_eq!(
            default_route_for_key("claude:claude-otter"),
            Some("claude-oauth")
        );
        assert_eq!(
            default_route_for_key("anthropic-api"),
            Some("anthropic-api")
        );
        assert_eq!(
            default_route_for_key("openai:openai-otter"),
            Some("openai-oauth")
        );
        assert_eq!(default_route_for_key("openai-api"), Some("openai-api"));
        assert_eq!(default_route_for_key("openrouter"), Some("openrouter"));
        assert_eq!(default_route_for_key("no-such-provider"), None);
    }

    #[test]
    fn default_model_follows_same_vendor_routes_only() {
        assert_eq!(
            model_for_route(Some("anthropic-api:claude-opus-5-5"), "claude-oauth").as_deref(),
            Some("claude-oauth:claude-opus-5-5")
        );
        assert_eq!(
            model_for_route(Some("claude-opus-5-5"), "anthropic-api").as_deref(),
            Some("anthropic-api:claude-opus-5-5")
        );
        assert_eq!(
            model_for_route(Some("anthropic-api:claude-opus-5-5"), "openai-oauth"),
            None
        );
        assert_eq!(
            model_for_route(Some("openai-api:gpt-5.5"), "openai-oauth").as_deref(),
            Some("openai-oauth:gpt-5.5")
        );
        assert_eq!(model_for_route(None, "openai-api"), None);
    }

    #[test]
    fn jcode_subscription_joins_by_default_but_comes_last() {
        assert!(default_member("jcode", true));
        assert!(default_member("claude:claude-otter", false));
        assert!(!default_member("openai-api", true));
        let pool = AccountPool::default();
        let list = accounts(&[
            ("jcode", default_member("jcode", true)),
            ("claude:claude-otter", true),
            ("openai-api", default_member("openai-api", true)),
            ("copilot", true),
        ]);
        let (members, others) = pool.partition(&list);
        assert_eq!(members, ["claude:claude-otter", "copilot", "jcode"]);
        assert_eq!(others, ["openai-api"]);
        // Once the user places it, their order wins.
        let pool = AccountPool {
            members: vec!["jcode".into(), "claude:claude-otter".into()],
            excluded: vec![],
        };
        assert_eq!(pool.partition(&list).0[0], "jcode");
    }

    #[test]
    fn default_route_changes_elsewhere_reorder_the_pool() {
        let active = |provider: &str| match provider {
            "claude" => Some("claude-fox".to_string()),
            "openai" => Some("openai-otter".to_string()),
            _ => None,
        };
        let mut pool = AccountPool {
            members: vec![
                "claude:claude-otter".into(),
                "claude:claude-fox".into(),
                "openai:openai-otter".into(),
                "anthropic-api".into(),
            ],
            excluded: vec!["openai-api".into()],
        };
        // The model picker's `anthropic-api` route moves the API key to #1.
        assert!(pool.promote_route("anthropic-api", active));
        assert_eq!(pool.members[0], "anthropic-api");
        // `claude` and `claude-oauth` are the same route: the active login wins.
        assert!(pool.promote_route("claude", active));
        assert_eq!(pool.members[0], "claude:claude-fox");
        assert!(!pool.promote_route("claude-oauth", active));
        // A manual API key stays manual even when chosen as the default.
        assert!(!pool.promote_route("openai-api", active));
        assert!(!pool.members.contains(&"openai-api".to_string()));
        assert!(same_route("claude", "claude-oauth"));
        assert!(!same_route("claude-oauth", "anthropic-api"));
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
