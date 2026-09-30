//! Same-provider account failover helpers.
//!
//! Account state here is per *account label*, never "the active account":
//! each session (provider instance) has its own pin, so one window running
//! out of usage must not move or block any other window.

use super::ActiveProvider;
use jcode_provider_core::{AccountPin, AccountProviderKind};
use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, Mutex, RwLock};

/// Per-session (per `MultiProvider` instance) failover state.
#[derive(Default)]
pub(super) struct SessionAccountFailover {
    /// Session toggle. `None` follows `[provider].same_provider_account_failover`.
    pub(super) enabled: RwLock<Option<bool>>,
    /// Preferred pin to return to after an automatic move, per provider.
    pub(super) home: RwLock<BTreeMap<AccountProviderKind, AccountPin>>,
}

impl SessionAccountFailover {
    /// Copy for a fork (a resumed session or helper keeps the same setting).
    pub(super) fn copy(&self) -> Self {
        Self {
            enabled: RwLock::new(self.enabled()),
            home: RwLock::new(
                self.home
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone(),
            ),
        }
    }

    pub(super) fn enabled(&self) -> Option<bool> {
        *self
            .enabled
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(super) fn set_enabled(&self, enabled: Option<bool>) {
        *self
            .enabled
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = enabled;
    }

    pub(super) fn home(&self, kind: AccountProviderKind) -> Option<AccountPin> {
        self.home
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&kind)
            .cloned()
    }

    pub(super) fn set_home(&self, kind: AccountProviderKind, home: Option<AccountPin>) {
        let mut map = self
            .home
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match home {
            Some(pin) => {
                map.insert(kind, pin);
            }
            None => {
                map.remove(&kind);
            }
        }
    }
}

pub(super) fn multi_account_provider_kind(
    provider: ActiveProvider,
) -> Option<crate::usage::MultiAccountProviderKind> {
    match provider {
        ActiveProvider::Claude => Some(crate::usage::MultiAccountProviderKind::Anthropic),
        ActiveProvider::OpenAI => Some(crate::usage::MultiAccountProviderKind::OpenAI),
        _ => None,
    }
}

pub(super) fn account_kind(provider: ActiveProvider) -> Option<AccountProviderKind> {
    match provider {
        ActiveProvider::Claude => Some(AccountProviderKind::Claude),
        ActiveProvider::OpenAI => Some(AccountProviderKind::OpenAi),
        _ => None,
    }
}

pub(super) fn account_label_prefix(kind: AccountProviderKind) -> &'static str {
    match kind {
        AccountProviderKind::Claude => "claude",
        AccountProviderKind::OpenAi => "openai",
    }
}

pub(super) fn account_kind_display(kind: AccountProviderKind) -> &'static str {
    match kind {
        AccountProviderKind::Claude => "Claude",
        AccountProviderKind::OpenAi => "OpenAI",
    }
}

pub(super) fn account_usage_probe(
    provider: ActiveProvider,
) -> Option<crate::usage::AccountUsageProbe> {
    let kind = multi_account_provider_kind(provider)?;
    crate::usage::account_usage_probe_sync(kind)
}

/// Config default for same-provider account failover. Sessions can override
/// it (`MultiProvider::set_account_failover`).
pub(super) fn same_provider_account_failover_config() -> bool {
    crate::config::Config::load()
        .provider
        .same_provider_account_failover
}

pub(super) fn account_failover_return_home_config() -> bool {
    crate::config::Config::load()
        .provider
        .account_failover_return_home
}

/// Stored account of `kind`: (label, identity) in stored order. The identity
/// is the one credential loading compares (`auth::*::account_identity`), so
/// a pin built here survives relabeling and is accepted by the provider.
pub(super) fn stored_accounts(kind: AccountProviderKind) -> Vec<(String, Option<String>)> {
    match kind {
        AccountProviderKind::Claude => crate::auth::claude::list_accounts()
            .unwrap_or_default()
            .into_iter()
            .map(|account| {
                let identity = crate::auth::claude::account_identity(&account);
                (account.label, identity)
            })
            .collect(),
        AccountProviderKind::OpenAi => crate::auth::codex::list_accounts()
            .unwrap_or_default()
            .into_iter()
            .map(|account| {
                let identity = crate::auth::codex::account_identity(&account);
                (account.label, identity)
            })
            .collect(),
    }
}

/// Pin for a stored label (with its identity when known).
pub(super) fn pin_for_label(kind: AccountProviderKind, label: &str) -> AccountPin {
    let identity = stored_accounts(kind)
        .into_iter()
        .find(|(stored, _)| stored == label)
        .and_then(|(_, identity)| identity);
    AccountPin::new(label, identity)
}

/// The stored default account label (what an unpinned session uses).
pub(super) fn default_account_label(kind: AccountProviderKind) -> Option<String> {
    match kind {
        AccountProviderKind::Claude => crate::auth::claude::active_account_label(),
        AccountProviderKind::OpenAi => crate::auth::codex::active_account_label(),
    }
}

/// Failover order after `current`: pool members only (upstream
/// `AccountPool::rotation`, which cycles from the account after `current` and
/// wraps), minus accounts whose recorded usage limit has not reset yet. A
/// known-exhausted account is a doomed request, so it is skipped rather than
/// tried last.
pub(super) fn account_rotation(
    kind: AccountProviderKind,
    current: Option<&str>,
    stored_labels: &[String],
) -> Vec<String> {
    crate::auth::account_pool::AccountPool::load()
        .rotation(account_label_prefix(kind), current, stored_labels)
        .into_iter()
        .filter(|label| account_exhausted(kind, label).is_none())
        .collect()
}

// ---------------------------------------------------------------------------
// Per-account exhaustion ledger
// ---------------------------------------------------------------------------

/// How long an account whose usage limit gave no reset time counts as
/// exhausted.
const UNKNOWN_RESET_EXHAUSTION_SECS: i64 = 30 * 60;

#[derive(Clone, Debug)]
struct AccountExhaustion {
    /// Reset time reported by the provider.
    resets_at: Option<i64>,
    /// When it was recorded (used when `resets_at` is unknown).
    recorded_at: i64,
    /// Credential fingerprint of the login that ran out (see
    /// [`account_fingerprint`]). A different login later stored under the
    /// same label (relogin, relabel) is another subscription and must not
    /// inherit this mark.
    fingerprint: Option<String>,
}

impl AccountExhaustion {
    fn until(&self) -> i64 {
        self.resets_at
            .unwrap_or(self.recorded_at + UNKNOWN_RESET_EXHAUSTION_SECS)
    }
}

/// Process-wide: exhaustion is a fact about the account (every window on it
/// is out), unlike the pin, which is per session.
static ACCOUNT_EXHAUSTION: LazyLock<
    Mutex<HashMap<(AccountProviderKind, String), AccountExhaustion>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Fingerprint of the login currently stored at `label`: the account
/// identity (email, else provider account id, else a refresh-token hash).
/// Usage limits belong to the subscription, so a token refresh keeps the
/// fingerprint while a login to another subscription changes it.
fn account_fingerprint(kind: AccountProviderKind, label: &str) -> Option<String> {
    match kind {
        AccountProviderKind::Claude => crate::auth::claude::list_accounts()
            .ok()?
            .iter()
            .find(|account| account.label == label)
            .and_then(crate::auth::claude::account_identity),
        AccountProviderKind::OpenAi => crate::auth::codex::list_accounts()
            .ok()?
            .iter()
            .find(|account| account.label == label)
            .and_then(crate::auth::codex::account_identity),
    }
}

pub(crate) fn record_account_exhausted(
    kind: AccountProviderKind,
    label: &str,
    resets_at: Option<i64>,
) {
    let fingerprint = account_fingerprint(kind, label);
    ACCOUNT_EXHAUSTION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(
            (kind, label.to_string()),
            AccountExhaustion {
                resets_at,
                recorded_at: now_unix(),
                fingerprint,
            },
        );
}

pub(crate) fn clear_account_exhausted(kind: AccountProviderKind, label: &str) {
    ACCOUNT_EXHAUSTION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&(kind, label.to_string()));
}

/// Live ledger entry for `label`: dropped once its reset passed, or when the
/// login stored at `label` is no longer the one that ran out.
fn live_exhaustion(kind: AccountProviderKind, label: &str) -> Option<AccountExhaustion> {
    let fingerprint = account_fingerprint(kind, label);
    let mut ledger = ACCOUNT_EXHAUSTION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let key = (kind, label.to_string());
    let entry = ledger.get(&key)?.clone();
    // Strict comparison. An email/account-id fingerprint is stable across
    // token refreshes. A weak refresh-hash one rotates on refresh, which only
    // drops the mark early (one request re-learns it); inheriting a mark
    // would block a fresh subscription until the old one's reset.
    if entry.until() <= now_unix() || entry.fingerprint != fingerprint {
        ledger.remove(&key);
        return None;
    }
    Some(entry)
}

/// `Some(until)` while `label` is known to be out of usage (unix seconds of
/// the reset, or of the end of the unknown-reset window). `None` once reset,
/// or once another login took over the label.
pub(crate) fn account_exhausted(kind: AccountProviderKind, label: &str) -> Option<i64> {
    live_exhaustion(kind, label).map(|entry| entry.until())
}

/// Reported reset time of `label`, if recorded and still in the future.
pub(crate) fn account_resets_at(kind: AccountProviderKind, label: &str) -> Option<i64> {
    live_exhaustion(kind, label).and_then(|entry| entry.resets_at)
}

#[cfg(test)]
pub(crate) fn reset_account_exhaustion_for_tests() {
    ACCOUNT_EXHAUSTION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
}

/// Local wall-clock `HH:MM` for a unix time.
pub(crate) fn format_reset_clock(unix: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(unix, 0)
        .map(|at| at.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_else(|| "?".to_string())
}

/// Compact "1h12m" / "5m" style duration.
pub(crate) fn format_reset_in(secs: i64) -> String {
    let secs = secs.max(0);
    let days = secs / 86_400;
    let hours = secs % 86_400 / 3_600;
    let minutes = secs % 3_600 / 60;
    if days > 0 {
        format!("{days}d{hours}h")
    } else if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else {
        format!("{}m", minutes.max(1))
    }
}

/// "All 3 Claude accounts are out of usage. First reset: claude-fox at 14:05
/// (in 1h12m)."
pub(super) fn all_accounts_exhausted_reason(
    kind: AccountProviderKind,
    stored_labels: &[String],
) -> String {
    let mut reason = format!(
        "All {} {} accounts are out of usage.",
        stored_labels.len(),
        account_kind_display(kind)
    );
    let earliest = stored_labels
        .iter()
        .filter_map(|label| account_resets_at(kind, label).map(|at| (at, label)))
        .min();
    if let Some((at, label)) = earliest {
        reason.push_str(&format!(
            " First reset: {} at {} (in {}).",
            label,
            format_reset_clock(at),
            format_reset_in(at - now_unix())
        ));
    }
    reason
}

// ---------------------------------------------------------------------------
// Guidance text
// ---------------------------------------------------------------------------

pub(super) fn account_switch_guidance(provider: ActiveProvider) -> Option<String> {
    let probe = account_usage_probe(provider)?;
    probe.switch_guidance().or_else(|| {
        (probe.current_exhausted() && probe.all_accounts_exhausted()).then(|| {
            format!(
                "All {} accounts appear exhausted. Use `/usage` to inspect reset times.",
                probe.provider.display_name()
            )
        })
    })
}

pub(super) fn usage_exhausted_reason(provider: ActiveProvider) -> String {
    let mut reason = "OAuth usage exhausted".to_string();
    if let Some(guidance) = account_switch_guidance(provider) {
        reason.push_str(". ");
        reason.push_str(&guidance);
    }
    reason
}

fn error_looks_like_usage_limit(summary: &str) -> bool {
    let lower = summary.to_ascii_lowercase();
    [
        "quota",
        "insufficient_quota",
        "rate limit",
        "rate_limit",
        "rate_limit_exceeded",
        "too many requests",
        "billing",
        "credit",
        "payment required",
        "usage exhausted",
        "limit reached",
        "429",
        "402",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

pub(super) fn maybe_annotate_limit_summary(provider: ActiveProvider, summary: String) -> String {
    if !error_looks_like_usage_limit(&summary) {
        return summary;
    }
    let Some(guidance) = account_switch_guidance(provider) else {
        return summary;
    };
    if summary.contains(&guidance) {
        return summary;
    }
    format!("{}. {}", summary, guidance)
}
