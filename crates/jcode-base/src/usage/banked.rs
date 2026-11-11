//! Per-account banked usage resets for compact status displays.
//!
//! OpenAI banked resets live in the OpenAI usage cache. Claude session-limit
//! reset offers are fetched alongside usage but are not part of `UsageData`,
//! so the last observed offer is remembered here per account label.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Banked resets known for one saved account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountBankedResets {
    /// Resets that can be claimed right now.
    pub available: u64,
    /// RFC3339 time the next reset becomes available when none are left.
    pub next_available_at: Option<String>,
    /// RFC3339 expiry of the soonest-expiring claimable reset, when reported.
    pub expires_at: Option<String>,
}

impl AccountBankedResets {
    /// Soonest known expiry among claimable resets.
    pub fn earliest_expiry(expirations: &[Option<String>]) -> Option<String> {
        expirations
            .iter()
            .flatten()
            .filter_map(|raw| match chrono::DateTime::parse_from_rfc3339(raw) {
                Ok(at) => Some((at, raw.clone())),
                Err(error) => {
                    crate::logging::info(&format!("Ignoring invalid banked reset expiry: {error}"));
                    None
                }
            })
            .min_by_key(|(at, _)| *at)
            .map(|(_, raw)| raw)
    }
}

fn anthropic_offers() -> &'static Mutex<HashMap<String, AccountBankedResets>> {
    static OFFERS: OnceLock<Mutex<HashMap<String, AccountBankedResets>>> = OnceLock::new();
    OFFERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Remember the latest Claude reset offer for a login. `None` means the login
/// is not in the reset program, which clears any earlier observation.
pub(super) fn record_anthropic_offer(
    label: &str,
    offer: Option<&jcode_usage_types::AnthropicLimitResetOffer>,
) {
    let label = label.trim();
    if label.is_empty() {
        return;
    }
    if let Ok(mut map) = anthropic_offers().lock() {
        match offer {
            Some(offer) => {
                map.insert(
                    label.to_string(),
                    AccountBankedResets {
                        available: u64::from(offer.available),
                        next_available_at: (!offer.available)
                            .then(|| offer.next_available_at.clone())
                            .flatten(),
                        expires_at: None,
                    },
                );
            }
            None => {
                map.remove(label);
            }
        }
    }
}

/// Cached banked resets for a saved account. Never probes or blocks.
pub fn cached_account_banked_resets(
    provider: super::MultiAccountProviderKind,
    label: &str,
) -> Option<AccountBankedResets> {
    let label = label.trim();
    if label.is_empty() {
        return None;
    }
    match provider {
        super::MultiAccountProviderKind::Anthropic => {
            try_cache_lock(anthropic_offers())?.get(label).cloned()
        }
        super::MultiAccountProviderKind::OpenAI => {
            let map = try_cache_lock(super::cache::openai_usage_cache())?;
            let data = map.get(&format!("label:{label}"))?;
            if data.last_error.is_some() {
                return None;
            }
            data.openai_reset_credits
                .as_ref()
                .map(|credits| AccountBankedResets {
                    available: credits.available_count,
                    next_available_at: None,
                    expires_at: AccountBankedResets::earliest_expiry(
                        &credits.available_expirations,
                    ),
                })
        }
    }
}

fn try_cache_lock<T>(mutex: &Mutex<T>) -> Option<std::sync::MutexGuard<'_, T>> {
    match mutex.try_lock() {
        Ok(guard) => Some(guard),
        Err(std::sync::TryLockError::WouldBlock) => None,
        Err(std::sync::TryLockError::Poisoned(_)) => {
            crate::logging::info("Banked reset cache poisoned, skipping cached offer");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_expirations_do_not_hide_valid_ones() {
        assert_eq!(
            AccountBankedResets::earliest_expiry(&[
                Some("invalid".into()),
                Some("2099-02-01T00:00:00Z".into()),
                Some("2099-01-01T00:00:00Z".into()),
            ])
            .as_deref(),
            Some("2099-01-01T00:00:00Z")
        );
    }

    #[test]
    fn busy_and_poisoned_caches_are_unavailable() {
        let cache = Mutex::new(());
        let guard = cache.lock().unwrap();
        assert!(try_cache_lock(&cache).is_none());
        drop(guard);
        let _ = std::panic::catch_unwind(|| {
            let _guard = cache.lock().unwrap();
            panic!("poison cache");
        });
        assert!(try_cache_lock(&cache).is_none());
    }

    #[test]
    fn anthropic_offer_is_recorded_and_cleared() {
        let label = "banked-test-account";
        let offer = jcode_usage_types::AnthropicLimitResetOffer {
            account_label: Some(label.into()),
            available: false,
            next_available_at: Some("2099-01-01T00:00:00Z".into()),
            resets_per_week: 1,
        };
        record_anthropic_offer(label, Some(&offer));
        let got =
            cached_account_banked_resets(super::super::MultiAccountProviderKind::Anthropic, label)
                .unwrap();
        assert_eq!(got.available, 0);
        assert_eq!(
            got.next_available_at.as_deref(),
            Some("2099-01-01T00:00:00Z")
        );
        record_anthropic_offer(label, None);
        assert!(
            cached_account_banked_resets(super::super::MultiAccountProviderKind::Anthropic, label)
                .is_none()
        );
    }
}
