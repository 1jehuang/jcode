use super::openai_helpers::{classify_openai_limits, usage_percent_to_ratio};
use super::{
    AccountUsageSnapshot, ModelScopedUsageWindow, OpenAIUsageData, ProviderUsage, UsageData,
    UsageLimit,
};
use std::collections::HashMap;
use std::time::Instant;

/// Shared Anthropic usage cache used by the info widget, `/usage`, and
/// multi-account fallback logic so they don't hammer the same endpoint through
/// separate code paths.
static ANTHROPIC_USAGE_CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, UsageData>>> =
    std::sync::OnceLock::new();

static OPENAI_USAGE_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A new usage limit was observed for an OpenAI account. Usage fetches that
/// started before it can no longer be trusted to say the account is open, so
/// advance the generation they are checked against.
pub fn note_openai_usage_limit_observed() {
    OPENAI_USAGE_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

pub(crate) fn openai_usage_generation() -> u64 {
    OPENAI_USAGE_GENERATION.load(std::sync::atomic::Ordering::SeqCst)
}

/// Called while holding the lock that publishes an OpenAI unavailability mark.
/// Its version lets a deferred snapshot clear only observations it includes.
pub(crate) fn note_openai_unavailability_observed() -> u64 {
    OPENAI_USAGE_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
}

/// Shared OpenAI usage cache keyed by account label/token prefix.
static OPENAI_ACCOUNT_USAGE_CACHE: std::sync::OnceLock<
    std::sync::Mutex<HashMap<String, OpenAIUsageData>>,
> = std::sync::OnceLock::new();

fn anthropic_usage_cache() -> &'static std::sync::Mutex<HashMap<String, UsageData>> {
    ANTHROPIC_USAGE_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn openai_usage_cache() -> &'static std::sync::Mutex<HashMap<String, OpenAIUsageData>> {
    OPENAI_ACCOUNT_USAGE_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// Read fresh usage attributed to a saved account without probing credentials,
/// reading files, waiting for a cache lock, or scheduling a network refresh.
/// Missing, stale, and failed reports are unknown, not zero usage.
pub fn cached_account_usage_snapshot(
    provider: super::MultiAccountProviderKind,
    label: &str,
) -> Option<AccountUsageSnapshot> {
    cached_account_usage_status_snapshot(provider, label).filter(|usage| usage.error.is_none())
}

/// Read fresh account-scoped usage, including failed probes for status indicators.
/// A failed probe must not be interpreted as zero usage or an exhausted quota.
/// Like the quota-only accessor, this never probes or blocks the render thread.
pub fn cached_account_usage_status_snapshot(
    provider: super::MultiAccountProviderKind,
    label: &str,
) -> Option<AccountUsageSnapshot> {
    if let Some(remote) = super::remote_accounts::cached(provider, label) {
        return remote;
    }
    let label = label.trim();
    if label.is_empty() {
        return None;
    }
    let key = format!("label:{label}");
    match provider {
        super::MultiAccountProviderKind::Anthropic => {
            let map = ANTHROPIC_USAGE_CACHE.get()?.try_lock().ok()?;
            let data = anthropic_usage_for_label(&map, label)?;
            if data.is_stale() {
                return None;
            }
            Some(anthropic_snapshot_from_usage(label.into(), None, data))
        }
        super::MultiAccountProviderKind::OpenAI => {
            let map = OPENAI_ACCOUNT_USAGE_CACHE.get()?.try_lock().ok()?;
            let data = map.get(&key)?;
            if data.is_stale() {
                return None;
            }
            Some(openai_snapshot_from_usage(label.into(), None, data))
        }
    }
}

/// Whether a saved account is known to be out of quota right now. Unlike
/// `cached_account_usage_snapshot`, an old sample still counts while the
/// exhausted window's reset time is in the future: a weekly limit observed an
/// hour ago is still binding, and routing a new session or a failover onto it
/// only wastes a request. Unknown usage is never treated as exhausted.
/// Never probes, reads credentials, or blocks on a contended cache lock.
pub fn account_known_exhausted(provider: super::MultiAccountProviderKind, label: &str) -> bool {
    if let Some(remote) = super::remote_accounts::cached(provider, label) {
        return remote.is_some_and(|snapshot| {
            snapshot.exhausted
                || (provider == super::MultiAccountProviderKind::Anthropic
                    && [snapshot.five_hour_ratio, snapshot.seven_day_ratio]
                        .into_iter()
                        .flatten()
                        .any(|ratio| ratio >= 0.99)
                    && reset_in_future(snapshot.resets_at.as_deref()))
        });
    }
    let label = label.trim();
    if label.is_empty() {
        return false;
    }
    let key = format!("label:{label}");
    match provider {
        super::MultiAccountProviderKind::Anthropic => ANTHROPIC_USAGE_CACHE
            .get()
            .and_then(|cache| cache.try_lock().ok())
            .and_then(|map| anthropic_usage_for_label(&map, label).map(anthropic_known_exhausted))
            .unwrap_or(false),
        super::MultiAccountProviderKind::OpenAI => OPENAI_ACCOUNT_USAGE_CACHE
            .get()
            .and_then(|cache| cache.try_lock().ok())
            .and_then(|map| map.get(&key).map(openai_known_exhausted))
            .unwrap_or(false),
    }
}

fn reset_in_future(reset: Option<&str>) -> bool {
    reset
        .and_then(|reset| chrono::DateTime::parse_from_rfc3339(reset).ok())
        .is_some_and(|reset| reset > chrono::Utc::now())
}

fn anthropic_known_exhausted(data: &UsageData) -> bool {
    // The snapshot already discards rolled-over windows and failed probes
    // without a real last-good sample.
    let snapshot = anthropic_snapshot_from_usage(String::new(), None, data);
    let dated = |ratio: Option<f32>, reset: Option<&str>| {
        ratio.is_some_and(|ratio| ratio >= 0.99) && reset_in_future(reset)
    };
    dated(
        snapshot.five_hour_ratio,
        data.five_hour_resets_at.as_deref(),
    ) || dated(
        snapshot.seven_day_ratio,
        data.seven_day_resets_at.as_deref(),
    ) || (!data.is_stale() && snapshot.exhausted)
}

fn openai_known_exhausted(data: &OpenAIUsageData) -> bool {
    if !data.is_stale() && data.last_error.is_none() && data.exhausted() {
        return true;
    }
    // OpenAI can round to 100% while still serving, so a stale sample only
    // counts when the provider also enforced the limit.
    data.hard_limit_reached
        && [data.five_hour.as_ref(), data.seven_day.as_ref()]
            .into_iter()
            .flatten()
            .any(|window| {
                window.usage_ratio >= 0.99 && reset_in_future(window.resets_at.as_deref())
            })
}

#[cfg(test)]
mod account_snapshot_tests {
    use super::*;
    use crate::usage::{MultiAccountProviderKind, OpenAIUsageWindow};

    #[test]
    fn stale_samples_stay_exhausted_until_their_window_resets() {
        let future = (chrono::Utc::now() + chrono::Duration::days(6)).to_rfc3339();
        let past = (chrono::Utc::now() - chrono::Duration::minutes(1)).to_rfc3339();
        let old = Instant::now()
            .checked_sub(std::time::Duration::from_secs(70 * 60))
            .unwrap();
        let window = |ratio: f32, reset: &str| OpenAIUsageWindow {
            name: "7d".into(),
            usage_ratio: ratio,
            resets_at: Some(reset.to_string()),
        };
        let openai = OpenAIUsageData {
            seven_day: Some(window(1.0, &future)),
            hard_limit_reached: true,
            fetched_at: Some(old),
            ..Default::default()
        };
        assert!(openai.is_stale());
        assert!(openai_known_exhausted(&openai));
        // Rolled over, rounded-but-not-enforced, and headroom are not exhausted.
        let rolled = OpenAIUsageData {
            seven_day: Some(window(1.0, &past)),
            ..openai.clone()
        };
        assert!(!openai_known_exhausted(&rolled));
        let soft = OpenAIUsageData {
            hard_limit_reached: false,
            ..openai.clone()
        };
        assert!(!openai_known_exhausted(&soft));
        let headroom = OpenAIUsageData {
            seven_day: Some(window(0.4, &future)),
            ..openai
        };
        assert!(!openai_known_exhausted(&headroom));

        let anthropic = UsageData {
            five_hour: 1.0,
            five_hour_resets_at: Some(
                (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
            ),
            seven_day: 0.2,
            fetched_at: Some(old),
            last_success_at: Some(old),
            ..Default::default()
        };
        assert!(anthropic_known_exhausted(&anthropic));
        // A later failed probe keeps the dated last-good exhaustion.
        let errored = UsageData {
            last_error: Some("HTTP 429".into()),
            ..anthropic.clone()
        };
        assert!(anthropic_known_exhausted(&errored));
        let reset = UsageData {
            five_hour_resets_at: Some(past),
            ..anthropic
        };
        assert!(!anthropic_known_exhausted(&reset));
        assert!(!anthropic_known_exhausted(&UsageData::default()));
    }

    #[test]
    fn anthropic_keys_share_saved_identity_without_exposing_token_prefixes() {
        let token = "shared-prefix-20chars-account-a";
        let accounts = [("saved-a", token)];
        let key =
            |label| anthropic_usage_cache_key_with_accounts(token, label, accounts.into_iter());
        assert!(key(Some("saved-a")).starts_with("label:saved-a:"));
        assert_eq!(anthropic_usage_key_label(&key(None)), Some("saved-a"));
        assert_eq!(key(None), key(Some("saved-a")));
        assert_eq!(key(Some("default")), key(None));
        let unknown = anthropic_usage_cache_key_with_accounts(token, None, std::iter::empty());
        let other = anthropic_usage_cache_key_with_accounts(
            "shared-prefix-20chars-account-b",
            None,
            std::iter::empty(),
        );
        assert_ne!(unknown, other);
        assert!(unknown.starts_with("token:"));
        assert!(!unknown.contains("shared-prefix"));
        assert_eq!(anthropic_usage_key_label(&unknown), None);
    }

    #[test]
    fn anthropic_snapshot_either_shared_window_exhausted_and_errors_unknown() {
        for (five_hour, seven_day, expected) in [
            (1.0, 0.2, true),
            (0.2, 1.0, true),
            (1.0, 1.0, true),
            (0.2, 0.3, false),
        ] {
            let mut usage = UsageData {
                five_hour,
                seven_day,
                fetched_at: Some(Instant::now()),
                ..Default::default()
            };
            let snapshot = anthropic_snapshot_from_usage("scope".into(), None, &usage);
            assert_eq!(snapshot.exhausted, expected);
            assert_eq!(snapshot.five_hour_ratio, Some(five_hour));
            assert_eq!(snapshot.seven_day_ratio, Some(seven_day));
            usage.last_error = Some("transport failed".into());
            let unknown = anthropic_snapshot_from_usage("scope".into(), None, &usage);
            assert!(!unknown.exhausted);
            assert!(unknown.five_hour_ratio.is_none());
            assert!(unknown.seven_day_ratio.is_none());
            assert!(unknown.error.is_some());
        }
    }

    #[test]
    fn cached_account_snapshot_is_scoped_fresh_and_nonblocking() {
        let label = "router-cache-only-test";
        let key = anthropic_usage_cache_key_with_accounts(
            "router-cache-only-token",
            Some(label),
            std::iter::empty(),
        );
        let provider = MultiAccountProviderKind::Anthropic;
        let data = UsageData {
            five_hour: 0.42,
            fetched_at: Some(Instant::now()),
            ..Default::default()
        };
        store_anthropic_usage(key.clone(), data.clone());
        let snapshot = cached_account_usage_snapshot(provider, label).unwrap();
        assert_eq!(snapshot.five_hour_ratio, Some(0.42));
        assert_eq!(snapshot.label, label);
        assert!(cached_account_usage_snapshot(provider, "router-other-account").is_none());
        assert!(cached_account_usage_status_snapshot(provider, "router-other-account").is_none());
        assert!(cached_account_usage_status_snapshot(provider, " ").is_none());
        assert!(cached_account_usage_snapshot(MultiAccountProviderKind::OpenAI, label).is_none());
        {
            let _guard = anthropic_usage_cache().lock().unwrap();
            assert!(cached_account_usage_snapshot(provider, label).is_none());
            assert!(cached_account_usage_status_snapshot(provider, label).is_none());
        }
        store_anthropic_usage(
            key.clone(),
            UsageData {
                fetched_at: Some(
                    Instant::now()
                        - super::super::ANTHROPIC_CACHE_DURATION
                        - std::time::Duration::from_secs(1),
                ),
                ..data.clone()
            },
        );
        assert!(cached_account_usage_snapshot(provider, label).is_none());
        assert!(cached_account_usage_status_snapshot(provider, label).is_none());
        store_anthropic_usage(
            key.clone(),
            UsageData {
                last_error: Some("unavailable".into()),
                last_success_at: Some(Instant::now()),
                five_hour_resets_at: Some(
                    (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
                ),
                ..data
            },
        );
        assert!(cached_account_usage_snapshot(provider, label).is_none());
        let stale = cached_account_usage_status_snapshot(provider, label).unwrap();
        assert_eq!(stale.error.as_deref(), Some("unavailable"));
        assert_eq!(stale.five_hour_ratio, Some(0.42));
        assert!(!stale.exhausted);
        anthropic_usage_cache().lock().unwrap().remove(&key);
    }
}

pub(super) fn invalidate_openai_usage_after_reset(access_token: &str, account_label: Option<&str>) {
    if let Ok(mut map) = openai_usage_cache().lock() {
        OPENAI_USAGE_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if account_label.is_none() && access_token.is_empty() {
            // A daemon cannot receive the TUI's bearer token over the protocol.
            // Legacy/external OAuth caches have token keys, not account labels.
            map.retain(|key, _| !key.starts_with("token:"));
        } else {
            map.remove(&openai_usage_cache_key(access_token, account_label));
            map.remove(&openai_usage_cache_key(access_token, None));
        }
    }
    if let Some(cache) = super::PROVIDER_USAGE_CACHE.get()
        && let Ok(mut map) = cache.lock()
    {
        // Removing just OpenAI can leave an all-fresh map of other providers,
        // causing fetch_all_provider_usage to skip fetching OpenAI entirely.
        // Their per-account caches remain intact, so clearing this aggregate is cheap.
        map.clear();
    }
}

/// Forget one Claude login's quota after a session-limit reset so the next
/// check fetches fresh limits. `None` clears token-keyed (unlabelled) entries.
pub(super) fn invalidate_anthropic_usage_after_reset(account_label: Option<&str>) {
    let label = account_label
        .map(str::trim)
        .filter(|label| !label.is_empty());
    // Keys are label plus credential fingerprint, so match every credential
    // stored under the label.
    let default_prefix = anthropic_label_key_prefix("default");
    let matches = |key: &str| match label {
        Some(label) => key.starts_with(&anthropic_label_key_prefix(label)),
        None => key.starts_with("token:") || key.starts_with(&default_prefix),
    };
    if let Ok(mut map) = anthropic_usage_cache().lock() {
        map.retain(|key, _| !matches(key));
    }
    // Never hold the memory mutex while waiting for the on-disk transaction.
    // Fetch completion writes disk first, then memory.
    super::disk_cache::invalidate(|key| matches(key));
    if let Some(cache) = super::PROVIDER_USAGE_CACHE.get()
        && let Ok(mut map) = cache.lock()
    {
        // As for OpenAI: a partially cleared aggregate would skip the refetch.
        map.clear();
    }
}

/// Key prefix shared by every credential stored under one Claude label.
fn anthropic_label_key_prefix(label: &str) -> String {
    format!("label:{label}:")
}

/// Short, non-reversible fingerprint of an access token.
fn anthropic_token_fingerprint(access_token: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(access_token.trim().as_bytes());
    format!("{:x}", digest)[..16].to_string()
}

/// Usage is keyed by the label *and* the credential. A relogin that reuses a
/// label (often "default") for a different Claude account must not inherit
/// the old account's exhausted limits or its shared 429 backoff, while the
/// same credential keeps one key so the cross-process backoff still applies.
pub(super) fn anthropic_usage_cache_key(access_token: &str, account_label: Option<&str>) -> String {
    let accounts = crate::auth::claude::list_accounts().unwrap_or_default();
    anthropic_usage_cache_key_with_accounts(
        access_token,
        account_label,
        accounts
            .iter()
            .map(|a| (a.label.as_str(), a.access.as_str())),
    )
}

fn anthropic_usage_cache_key_with_accounts<'a>(
    access_token: &str,
    account_label: Option<&str>,
    accounts: impl Iterator<Item = (&'a str, &'a str)>,
) -> String {
    let fingerprint = anthropic_token_fingerprint(access_token);
    // Routing passes a token while account polling passes a label. Resolve the
    // saved login holding the exact token so both paths share freshness and,
    // critically, backoff.
    let saved_label = (!access_token.is_empty())
        .then(|| {
            accounts
                .into_iter()
                .find(|(_, token)| *token == access_token)
                .map(|(label, _)| label)
        })
        .flatten();
    match saved_label
        .or(account_label)
        .map(str::trim)
        .filter(|label| !label.is_empty())
    {
        Some(label) => format!("{}{fingerprint}", anthropic_label_key_prefix(label)),
        None => format!("token:{fingerprint}"),
    }
}

/// Saved label encoded in a Claude usage cache key, if any.
pub(super) fn anthropic_usage_key_label(cache_key: &str) -> Option<&str> {
    cache_key
        .strip_prefix("label:")?
        .rsplit_once(':')
        .map(|(label, _)| label)
}

/// The freshest Claude usage cached under any credential of `label`.
fn anthropic_usage_for_label<'a>(
    map: &'a HashMap<String, UsageData>,
    label: &str,
) -> Option<&'a UsageData> {
    let prefix = anthropic_label_key_prefix(label);
    map.iter()
        .filter(|(key, _)| key.starts_with(&prefix))
        .map(|(_, data)| data)
        .max_by_key(|data| data.fetched_at)
}

/// A token refresh rotates the access token of the same Claude login, which
/// changes its usage cache key. Carry the cached usage and the shared 429
/// backoff over to the new key so the refreshed login is not refetched (and
/// rejected) as if it were a new account. A relogin under the same label does
/// not go through here and still starts with a clean key.
pub fn carry_anthropic_usage_across_token_refresh(
    account_label: &str,
    old_access_token: &str,
    new_access_token: &str,
) {
    if old_access_token.trim().is_empty() || old_access_token == new_access_token {
        return;
    }
    let from = anthropic_usage_cache_key(old_access_token, Some(account_label));
    let to = anthropic_usage_cache_key(new_access_token, Some(account_label));
    if let Ok(mut map) = anthropic_usage_cache().lock()
        && !map.contains_key(&to)
        && let Some(entry) = map.remove(&from)
    {
        map.insert(to.clone(), entry);
    }
    super::disk_cache::rename(&from, &to);
    super::accessors::rename_active_usage_key(&from, &to);
}

pub(super) fn openai_usage_cache_key(access_token: &str, account_label: Option<&str>) -> String {
    if let Some(label) = account_label
        .map(str::trim)
        .filter(|label| !label.is_empty())
    {
        return format!("label:{}", label);
    }

    let prefix = access_token
        .get(..20)
        .unwrap_or(access_token)
        .trim()
        .to_string();
    format!("token:{}", prefix)
}

pub(super) fn cached_anthropic_usage(cache_key: &str) -> Option<UsageData> {
    let cache = anthropic_usage_cache();
    let map = cache.lock().ok()?;
    let cached = map.get(cache_key)?.clone();
    (!cached.is_stale()).then_some(cached)
}

/// Fresh cached usage for a Claude account label (any credential under it).
pub(super) fn cached_anthropic_usage_for_label(label: &str) -> Option<UsageData> {
    let prefix = anthropic_label_key_prefix(label);
    let map = anthropic_usage_cache().lock().ok()?;
    map.iter()
        .filter(|(key, data)| key.starts_with(&prefix) && !data.is_stale())
        .max_by_key(|(_, data)| data.fetched_at)
        .map(|(_, data)| data.clone())
}

/// Fresh cached usage for an OpenAI account label.
pub(super) fn cached_openai_usage_for_label(label: &str) -> Option<OpenAIUsageData> {
    cached_openai_usage(&openai_usage_cache_key("", Some(label)))
}

/// Forget one OpenAI label's cached usage so the next read refetches it.
pub(super) fn forget_openai_usage_for_label(label: &str) {
    if let Ok(mut map) = openai_usage_cache().lock() {
        map.remove(&openai_usage_cache_key("", Some(label)));
    }
}

/// Credential refresh callers must respect the shared backoff too, before
/// attempting any network operation with an expired credential.
pub(super) fn cached_or_shared_anthropic_usage(cache_key: &str) -> Option<UsageData> {
    cached_anthropic_usage(cache_key).or_else(|| {
        let data = super::disk_cache::fresh(cache_key)?;
        store_anthropic_usage(cache_key.to_string(), data.clone());
        Some(data)
    })
}

pub(super) fn store_anthropic_usage(cache_key: String, data: UsageData) {
    if let Ok(mut map) = anthropic_usage_cache().lock() {
        map.insert(cache_key, data);
    }
}

pub(super) fn cached_openai_usage(cache_key: &str) -> Option<OpenAIUsageData> {
    let cache = openai_usage_cache();
    let map = cache.lock().ok()?;
    let cached = map.get(cache_key)?.clone();
    (!cached.is_stale()).then_some(cached)
}

/// A recent successful snapshot that may stand in for a failed transport
/// probe. Rolled-over windows and old data are never reused.
pub(super) fn recent_good_openai_usage(cache_key: &str) -> Option<OpenAIUsageData> {
    let cache = openai_usage_cache();
    let map = cache.lock().ok()?;
    let cached = map.get(cache_key)?;
    let recent = cached
        .fetched_at
        .is_some_and(|at| at.elapsed() <= super::TRANSPORT_LAST_GOOD_MAX_AGE);
    let rolled_over = super::display::usage_reset_passed([
        cached
            .five_hour
            .as_ref()
            .and_then(|w| w.resets_at.as_deref()),
        cached
            .seven_day
            .as_ref()
            .and_then(|w| w.resets_at.as_deref()),
        cached.spark.as_ref().and_then(|w| w.resets_at.as_deref()),
    ]);
    (recent && !rolled_over && cached.last_error.is_none() && cached.has_limits())
        .then(|| cached.clone())
}

#[cfg(test)]
pub(super) fn store_openai_usage(cache_key: String, data: OpenAIUsageData) {
    store_openai_usage_for_generation(openai_usage_generation(), cache_key, data);
}

pub(super) fn store_openai_usage_for_generation(
    generation: u64,
    cache_key: String,
    data: OpenAIUsageData,
) {
    store_openai_usage_for_generation_then(generation, cache_key, data, || {});
}

pub(crate) fn store_openai_usage_for_generation_then(
    generation: u64,
    cache_key: String,
    data: OpenAIUsageData,
    before_clear: impl FnOnce(),
) {
    // Some(None) identifies an accepted token-key snapshot for the literal
    // default scope. None means no confirmed scope should be cleared.
    // Clear after the cache lock is released to keep lock order flat.
    let mut reset_scope: Option<Option<String>> = None;
    if let Ok(mut map) = openai_usage_cache().lock() {
        // A request begun before a reset must not reinstate the old exhausted quota.
        if generation != openai_usage_generation() {
            return;
        }
        if data.confirms_usage_available() {
            if let Some(label) = cache_key.strip_prefix("label:") {
                reset_scope = Some(Some(label.to_string()));
            } else if cache_key.starts_with("token:") {
                reset_scope = Some(None);
            }
        }
        let previous = map.get(&cache_key).cloned();
        let previous_exhausted = previous
            .as_ref()
            .map(OpenAIUsageData::exhausted)
            .unwrap_or(false);
        let current_exhausted = data.exhausted();
        let previous_hard_limit = previous
            .as_ref()
            .map(|usage| usage.hard_limit_reached)
            .unwrap_or(false);
        if previous.is_none()
            || previous_exhausted != current_exhausted
            || previous_hard_limit != data.hard_limit_reached
        {
            crate::logging::info(&format!(
                "OpenAI limit diag: usage cache update key={} prev_exhausted={} new_exhausted={} prev_hard_limit={} new_hard_limit={} snapshot=({})",
                cache_key,
                previous_exhausted,
                current_exhausted,
                previous_hard_limit,
                data.hard_limit_reached,
                data.diagnostic_fields()
            ));
        }
        map.insert(cache_key, data);
    }
    if let Some(scope) = reset_scope {
        before_clear();
        // Fresh usage says this exact scope is open again (an early or banked
        // reset, or the window rolled over). Only named scopes have a ledger.
        crate::provider::models::clear_openai_unavailability_through_generation(
            scope.as_deref(),
            generation,
        );
    }
}

pub(super) fn provider_report_from_usage_data(
    display_name: String,
    data: &UsageData,
) -> ProviderUsage {
    if let Some(error) = &data.last_error {
        return ProviderUsage {
            provider_name: display_name,
            error: Some(error.clone()),
            ..Default::default()
        };
    }

    let mut limits = Vec::new();
    limits.push(UsageLimit {
        name: "5-hour window".to_string(),
        usage_percent: data.five_hour * 100.0,
        resets_at: data.five_hour_resets_at.clone(),
    });
    limits.push(UsageLimit {
        name: "7-day window".to_string(),
        usage_percent: data.seven_day * 100.0,
        resets_at: data.seven_day_resets_at.clone(),
    });
    if let Some(opus) = data.seven_day_opus {
        limits.push(UsageLimit {
            name: "7-day Opus window".to_string(),
            usage_percent: opus * 100.0,
            resets_at: data.seven_day_resets_at.clone(),
        });
    }
    for window in &data.model_scoped {
        limits.push(UsageLimit {
            name: format!("7-day {} window", window.model_name),
            usage_percent: window.utilization * 100.0,
            resets_at: window.resets_at.clone(),
        });
    }

    let mut extra_info = Vec::new();
    extra_info.push((
        "Extra usage (long context)".to_string(),
        if data.extra_usage_enabled {
            "enabled".to_string()
        } else {
            "disabled".to_string()
        },
    ));

    ProviderUsage {
        provider_name: display_name,
        limits,
        extra_info,
        hard_limit_reached: false,
        openai_reset_credits: None,
        anthropic_limit_reset: None,
        error: None,
        last_used_unix_secs: None,
    }
}

pub(super) fn usage_data_from_provider_report(report: &ProviderUsage) -> UsageData {
    if let Some(error) = &report.error {
        return UsageData {
            fetched_at: Some(Instant::now()),
            last_error: Some(error.clone()),
            ..Default::default()
        };
    }

    let five_hour = report
        .limits
        .iter()
        .find(|limit| limit.name == "5-hour window");
    let seven_day = report
        .limits
        .iter()
        .find(|limit| limit.name == "7-day window");
    let seven_day_opus = report
        .limits
        .iter()
        .find(|limit| limit.name == "7-day Opus window");
    let model_scoped = report
        .limits
        .iter()
        .filter_map(|limit| {
            let model_name = limit.name.strip_prefix("7-day ")?.strip_suffix(" window")?;
            if model_name == "Opus" {
                return None;
            }
            Some(ModelScopedUsageWindow {
                model_name: model_name.to_string(),
                utilization: usage_percent_to_ratio(limit.usage_percent),
                resets_at: limit.resets_at.clone(),
            })
        })
        .collect();
    let extra_usage_enabled = report.extra_info.iter().find_map(|(key, value)| {
        if key == "Extra usage (long context)" {
            Some(value == "enabled")
        } else {
            None
        }
    });

    UsageData {
        five_hour: five_hour
            .map(|limit| usage_percent_to_ratio(limit.usage_percent))
            .unwrap_or(0.0),
        five_hour_resets_at: five_hour.and_then(|limit| limit.resets_at.clone()),
        seven_day: seven_day
            .map(|limit| usage_percent_to_ratio(limit.usage_percent))
            .unwrap_or(0.0),
        seven_day_resets_at: seven_day.and_then(|limit| limit.resets_at.clone()),
        seven_day_opus: seven_day_opus.map(|limit| usage_percent_to_ratio(limit.usage_percent)),
        model_scoped,
        extra_usage_enabled: extra_usage_enabled.unwrap_or(false),
        fetched_at: Some(Instant::now()),
        // Reports do not carry the original probe timestamp or retry deadline.
        // Only a successful report with actual limits proves last-good quota.
        last_success_at: (report.error.is_none() && !report.limits.is_empty()).then(Instant::now),
        refresh_after: None,
        last_error: report.error.clone(),
    }
}

pub(super) fn openai_usage_data_from_provider_report(report: &ProviderUsage) -> OpenAIUsageData {
    let mut data = classify_openai_limits(&report.limits);
    data.hard_limit_reached = report.hard_limit_reached;
    data.openai_reset_credits = if report.error.is_none() {
        report.openai_reset_credits.clone()
    } else {
        None
    };
    data.fetched_at = Some(Instant::now());
    data.last_error = report.error.clone();
    data
}

pub(super) fn provider_report_from_openai_usage_data(
    display_name: String,
    data: &OpenAIUsageData,
) -> ProviderUsage {
    if let Some(error) = &data.last_error {
        return ProviderUsage {
            provider_name: display_name,
            error: Some(error.clone()),
            ..Default::default()
        };
    }

    let mut limits = Vec::new();
    if let Some(window) = &data.five_hour {
        limits.push(UsageLimit {
            name: window.name.clone(),
            usage_percent: window.usage_ratio * 100.0,
            resets_at: window.resets_at.clone(),
        });
    }
    if let Some(window) = &data.seven_day {
        limits.push(UsageLimit {
            name: window.name.clone(),
            usage_percent: window.usage_ratio * 100.0,
            resets_at: window.resets_at.clone(),
        });
    }
    if let Some(window) = &data.spark {
        limits.push(UsageLimit {
            name: window.name.clone(),
            usage_percent: window.usage_ratio * 100.0,
            resets_at: window.resets_at.clone(),
        });
    }

    ProviderUsage {
        provider_name: display_name,
        limits,
        extra_info: Vec::new(),
        hard_limit_reached: data.hard_limit_reached,
        openai_reset_credits: data.openai_reset_credits.clone(),
        anthropic_limit_reset: None,
        error: None,
        last_used_unix_secs: None,
    }
}

pub(super) fn openai_snapshot_from_usage(
    label: String,
    email: Option<String>,
    usage: &OpenAIUsageData,
) -> AccountUsageSnapshot {
    let five_hour_ratio = usage.five_hour.as_ref().map(|window| window.usage_ratio);
    let seven_day_ratio = usage.seven_day.as_ref().map(|window| window.usage_ratio);
    let exhausted = usage.exhausted();

    AccountUsageSnapshot {
        label,
        email,
        exhausted,
        primary_label: usage
            .five_hour
            .as_ref()
            .map(|window| window.name.trim_end_matches(" window").to_string()),
        five_hour_ratio,
        secondary_label: usage
            .seven_day
            .as_ref()
            .map(|window| window.name.trim_end_matches(" window").to_string()),
        seven_day_ratio,
        resets_at: usage
            .five_hour
            .as_ref()
            .and_then(|window| window.resets_at.clone())
            .or_else(|| {
                usage
                    .seven_day
                    .as_ref()
                    .and_then(|window| window.resets_at.clone())
            }),
        error: usage.last_error.clone(),
    }
}

pub(super) fn anthropic_snapshot_from_usage(
    label: String,
    email: Option<String>,
    usage: &UsageData,
) -> AccountUsageSnapshot {
    // Error timestamps alone are not evidence of known quota: failed first
    // probes contain default zeroes. Only a real last-good sample with a
    // still-open, explicitly dated window is safe to show as stale.
    let ratio = |value: f32, reset: Option<&str>| {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return None;
        }
        let future_reset = reset
            .and_then(|reset| chrono::DateTime::parse_from_rfc3339(reset).ok())
            .is_some_and(|reset| reset > chrono::Utc::now());
        if usage.last_error.is_none() {
            // A success can remain cached across a reset while polling backs
            // off. Never route or display the rolled-over window as current.
            return (reset.is_none() || future_reset).then_some(value);
        }
        (usage.last_success_at.is_some() && usage.fetched_at.is_some() && future_reset)
            .then_some(value)
    };
    let five_hour_ratio = ratio(usage.five_hour, usage.five_hour_resets_at.as_deref());
    let seven_day_ratio = ratio(usage.seven_day, usage.seven_day_resets_at.as_deref());
    AccountUsageSnapshot {
        label,
        email,
        exhausted: usage.last_error.is_none()
            && [five_hour_ratio, seven_day_ratio]
                .into_iter()
                .flatten()
                .any(|ratio| ratio >= 0.99),
        primary_label: Some("5h".to_string()),
        five_hour_ratio,
        secondary_label: Some("7d".to_string()),
        seven_day_ratio,
        resets_at: five_hour_ratio
            .and(usage.five_hour_resets_at.clone())
            .or_else(|| seven_day_ratio.and(usage.seven_day_resets_at.clone())),
        error: usage.last_error.clone(),
    }
}

#[cfg(test)]
mod stale_account_snapshot_tests {
    use super::*;

    fn last_good() -> UsageData {
        UsageData {
            five_hour: 1.0,
            seven_day: 0.42,
            five_hour_resets_at: Some(
                (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
            ),
            seven_day_resets_at: Some(
                (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339(),
            ),
            fetched_at: Some(Instant::now()),
            last_success_at: Some(Instant::now()),
            last_error: Some("HTTP 429".into()),
            ..Default::default()
        }
    }

    #[test]
    fn stale_limits_preserve_error_but_never_exhaust_or_route() {
        let data = last_good();
        let snapshot = anthropic_snapshot_from_usage("stale-test".into(), None, &data);
        assert_eq!(snapshot.five_hour_ratio, Some(1.0));
        assert_eq!(snapshot.seven_day_ratio, Some(0.42));
        assert_eq!(snapshot.error.as_deref(), Some("HTTP 429"));
        assert!(!snapshot.exhausted);
    }

    #[test]
    fn stale_limits_require_provenance_and_each_unexpired_valid_window() {
        let snapshot = |data: &UsageData| anthropic_snapshot_from_usage("test".into(), None, data);
        let mut data = last_good();
        data.last_success_at = None;
        assert!(snapshot(&data).five_hour_ratio.is_none());
        assert!(snapshot(&data).seven_day_ratio.is_none());
        data = last_good();
        data.fetched_at = None;
        assert!(snapshot(&data).five_hour_ratio.is_none());
        data = last_good();
        for reset in [
            None,
            Some("invalid".into()),
            Some((chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339()),
        ] {
            data.five_hour_resets_at = reset;
            assert!(snapshot(&data).five_hour_ratio.is_none());
            assert_eq!(snapshot(&data).seven_day_ratio, Some(0.42));
            assert_eq!(snapshot(&data).resets_at, data.seven_day_resets_at);
        }
        data = last_good();
        for ratio in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
            data.five_hour = ratio;
            assert!(snapshot(&data).five_hour_ratio.is_none());
        }
    }

    #[test]
    fn successful_but_rolled_over_window_does_not_exhaust() {
        let mut data = last_good();
        data.last_error = None;
        data.five_hour_resets_at =
            Some((chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339());
        let snapshot = anthropic_snapshot_from_usage("test".into(), None, &data);
        assert!(snapshot.five_hour_ratio.is_none());
        assert_eq!(snapshot.seven_day_ratio, Some(0.42));
        assert_eq!(snapshot.resets_at, data.seven_day_resets_at);
        assert!(!snapshot.exhausted);
        assert!(snapshot.error.is_none());
    }
}
