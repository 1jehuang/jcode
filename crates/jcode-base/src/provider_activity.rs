//! Cross-provider activity ledger.
//!
//! Tracks two things per login/credential ("source key"):
//!   1. When jcode last successfully used it (for recency-sorted `/usage`).
//!   2. Locally accumulated API-key spend in USD (day / month / all-time),
//!      mirroring the dollar figures the TUI cost paths compute, since most
//!      providers do not expose per-key spend through their public APIs.
//!
//! Data persists to `~/.jcode/provider_activity.json` and is shared across
//! processes (server records last-used, TUI records spend, `/usage` reads
//! both), so queries re-read the file with a short TTL instead of trusting a
//! process-local cache.
//!
//! ChatGPT OAuth token counts and API-equivalent estimates use a separate
//! `openai_oauth_usage.json` ledger, written only by provider completion hooks.
//! See [`openai_oauth_usage_summary`]. These estimates never enter API-key spend.
//!
//! Source key conventions:
//!   - `claude:oauth:<label>` / `claude:api-key`
//!   - `openai:oauth:<label>` / `openai:api-key`
//!   - `openai-compatible:<profile-id>` (DeepSeek, Moonshot, NVIDIA NIM, ...)
//!   - `openrouter`, `jcode`, `copilot`, `gemini`, `cursor`, `bedrock`,
//!     `antigravity`, `azure-openai`

#[path = "provider_activity_oauth.rs"]
mod oauth_usage;
pub use oauth_usage::{openai_oauth_usage_summary, record_openai_oauth_usage};

use chrono::{Datelike, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Re-reads of the ledger are throttled to this interval for query paths.
const QUERY_RELOAD_TTL: Duration = Duration::from_secs(2);

/// Skip persisting a new last-used timestamp when the stored one is within
/// this many seconds, so busy sessions do not rewrite the file on every call.
const LAST_USED_WRITE_THROTTLE_SECS: u64 = 30;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderSpend {
    /// `YYYY-MM-DD` the `day_usd` bucket belongs to.
    #[serde(default)]
    pub day_date: String,
    #[serde(default)]
    pub day_usd: f64,
    /// `YYYY-MM` the `month_usd` bucket belongs to.
    #[serde(default)]
    pub month: String,
    #[serde(default)]
    pub month_usd: f64,
    #[serde(default)]
    pub all_time_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderActivityEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_unix_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spend: Option<ProviderSpend>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderActivityStore {
    #[serde(default)]
    pub entries: HashMap<String, ProviderActivityEntry>,
}

struct CachedStore {
    loaded_at: Instant,
    store: ProviderActivityStore,
}

static LEDGER: Mutex<Option<CachedStore>> = Mutex::new(None);

fn ledger_path() -> PathBuf {
    crate::storage::jcode_dir()
        .unwrap_or_else(|_| PathBuf::from(".").join(".jcode"))
        .join("provider_activity.json")
}

fn load_store() -> ProviderActivityStore {
    crate::storage::read_json(&ledger_path()).unwrap_or_default()
}

fn save_store(store: &ProviderActivityStore) {
    let _ = crate::storage::write_json(&ledger_path(), store);
}

fn now_unix_secs() -> u64 {
    Utc::now().timestamp().max(0) as u64
}

fn roll_spend(spend: &mut ProviderSpend) {
    let now = Utc::now();
    let today = now.format("%Y-%m-%d").to_string();
    let month = format!("{}-{:02}", now.year(), now.month());
    if spend.day_date != today {
        spend.day_date = today;
        spend.day_usd = 0.0;
    }
    if spend.month != month {
        spend.month = month;
        spend.month_usd = 0.0;
    }
}

/// Run `mutate` against a freshly loaded copy of the ledger and persist it.
/// Returns without writing when `mutate` reports no change.
fn with_fresh_store(mutate: impl FnOnce(&mut ProviderActivityStore) -> bool) {
    let mut guard = match LEDGER.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    // Always merge against the on-disk state so concurrent writers (server
    // last-used vs TUI spend) do not clobber each other's entries.
    let mut store = load_store();
    if mutate(&mut store) {
        save_store(&store);
    }
    *guard = Some(CachedStore {
        loaded_at: Instant::now(),
        store,
    });
}

fn snapshot_entry(source_key: &str) -> Option<ProviderActivityEntry> {
    let mut guard = match LEDGER.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let needs_reload = guard
        .as_ref()
        .map(|cached| cached.loaded_at.elapsed() > QUERY_RELOAD_TTL)
        .unwrap_or(true);
    if needs_reload {
        *guard = Some(CachedStore {
            loaded_at: Instant::now(),
            store: load_store(),
        });
    }
    guard
        .as_ref()
        .and_then(|cached| cached.store.entries.get(source_key).cloned())
}

/// Record a successful use of a login/credential right now.
pub fn record_use(source_key: &str) {
    let source_key = source_key.trim();
    if source_key.is_empty() {
        return;
    }
    let now = now_unix_secs();
    let source_key = source_key.to_string();
    with_fresh_store(move |store| {
        let entry = store.entries.entry(source_key).or_default();
        let throttled = entry
            .last_used_unix_secs
            .map(|prev| now.saturating_sub(prev) < LAST_USED_WRITE_THROTTLE_SECS)
            .unwrap_or(false);
        if throttled {
            return false;
        }
        entry.last_used_unix_secs = Some(now);
        true
    });
}

/// Accumulate locally computed API-key spend (in USD) for a credential.
pub fn record_spend(source_key: &str, usd: f64) {
    let source_key = source_key.trim();
    if source_key.is_empty() || !usd.is_finite() || usd <= 0.0 {
        return;
    }
    let now = now_unix_secs();
    let source_key = source_key.to_string();
    with_fresh_store(move |store| {
        let entry = store.entries.entry(source_key).or_default();
        // Spend implies use; keep recency in the same write.
        entry.last_used_unix_secs = Some(now);
        let spend = entry.spend.get_or_insert_with(ProviderSpend::default);
        roll_spend(spend);
        spend.day_usd += usd;
        spend.month_usd += usd;
        spend.all_time_usd += usd;
        true
    });
}

pub fn last_used_unix_secs(source_key: &str) -> Option<u64> {
    snapshot_entry(source_key)?.last_used_unix_secs
}

/// Spend snapshot with day/month buckets rolled to the current date.
pub fn spend_snapshot(source_key: &str) -> Option<ProviderSpend> {
    let mut spend = snapshot_entry(source_key)?.spend?;
    roll_spend(&mut spend);
    Some(spend)
}

/// All ledger entries (source key -> activity), with spend buckets rolled.
/// Used by `/usage` to surface logins that have been used but have no
/// dedicated usage fetcher (Cursor, Bedrock, Azure, ...).
pub fn all_entries() -> Vec<(String, ProviderActivityEntry)> {
    let mut guard = match LEDGER.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let needs_reload = guard
        .as_ref()
        .map(|cached| cached.loaded_at.elapsed() > QUERY_RELOAD_TTL)
        .unwrap_or(true);
    if needs_reload {
        *guard = Some(CachedStore {
            loaded_at: Instant::now(),
            store: load_store(),
        });
    }
    let Some(cached) = guard.as_ref() else {
        return Vec::new();
    };
    let mut entries: Vec<(String, ProviderActivityEntry)> = cached
        .store
        .entries
        .iter()
        .map(|(key, entry)| {
            let mut entry = entry.clone();
            if let Some(spend) = entry.spend.as_mut() {
                roll_spend(spend);
            }
            (key.clone(), entry)
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

/// Human-facing display name for a ledger source key, e.g.
/// `openai-compatible:deepseek` -> `DeepSeek (API key)`,
/// `claude:oauth:claude-1` -> `Anthropic (Claude) [claude-1]`.
pub fn display_name_for_source_key(source_key: &str) -> String {
    if let Some(profile_id) = source_key.strip_prefix("openai-compatible:") {
        let name = crate::provider_catalog::openai_compatible_profile_by_id(profile_id)
            .map(|profile| profile.display_name.to_string())
            .unwrap_or_else(|| profile_id.to_string());
        return format!("{} (API key)", name);
    }
    if let Some(label) = source_key.strip_prefix("claude:oauth:") {
        return format!("Anthropic (Claude) [{}]", label);
    }
    if let Some(label) = source_key.strip_prefix("openai:oauth:") {
        return format!("OpenAI (ChatGPT) [{}]", label);
    }
    match source_key {
        "claude:api-key" => "Anthropic API key".to_string(),
        "openai:api-key" => "OpenAI API key".to_string(),
        "openrouter" => "OpenRouter".to_string(),
        "jcode" => "Jcode subscription".to_string(),
        "copilot" => "GitHub Copilot".to_string(),
        "gemini" => "Google Gemini".to_string(),
        "cursor" => "Cursor".to_string(),
        "bedrock" => "AWS Bedrock".to_string(),
        "antigravity" => "Antigravity".to_string(),
        "azure-openai" => "Azure OpenAI".to_string(),
        other => {
            // Slug -> Title Case fallback.
            other
                .split('-')
                .filter(|part| !part.is_empty())
                .map(|part| {
                    let mut chars = part.chars();
                    match chars.next() {
                        Some(first) => first.to_uppercase().to_string() + chars.as_str(),
                        None => String::new(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        }
    }
}

/// Human-readable relative age such as `just now`, `5m ago`, `3h ago`, `2d ago`.
pub fn format_relative_age(unix_secs: u64) -> String {
    let secs = now_unix_secs().saturating_sub(unix_secs);
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3_600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        let hours = secs / 3_600;
        let minutes = (secs % 3_600) / 60;
        if minutes > 0 {
            format!("{}h {}m ago", hours, minutes)
        } else {
            format!("{}h ago", hours)
        }
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

/// Map a human-facing provider label (e.g. `"DeepSeek"`, `"OpenRouter"`,
/// `"NVIDIA NIM"`) plus the optional `JCODE_RUNTIME_PROVIDER` key onto a
/// ledger source key. Used by spend recorders that only know display names.
pub fn source_key_for_provider_label(label: &str, runtime_provider: Option<&str>) -> String {
    let normalized = label.trim().to_ascii_lowercase();
    let runtime = runtime_provider
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty());

    // OpenRouter first: the catalog also carries an `openrouter` compatible
    // profile, but the ledger treats the public aggregator as its own bucket.
    if normalized.contains("openrouter") {
        // The OpenRouter slot multiplexes direct profiles; prefer the runtime
        // provider key when it names one.
        if let Some(runtime) = runtime.as_deref()
            && runtime != "openrouter"
            && crate::provider_catalog::openai_compatible_profile_by_id(runtime).is_some()
        {
            return format!("openai-compatible:{}", runtime);
        }
        return "openrouter".to_string();
    }

    // Direct OpenAI-compatible profiles, matched by id or display name.
    for profile in crate::provider_catalog::openai_compatible_profiles() {
        if normalized == profile.id || normalized == profile.display_name.to_ascii_lowercase() {
            return format!("openai-compatible:{}", profile.id);
        }
    }

    if normalized.contains("azure") {
        return "azure-openai".to_string();
    }
    if normalized.contains("bedrock") {
        return "bedrock".to_string();
    }
    if normalized.contains("anthropic") || normalized.contains("claude") {
        return "claude:api-key".to_string();
    }
    if normalized.contains("openai") {
        return "openai:api-key".to_string();
    }
    if normalized.contains("copilot") {
        return "copilot".to_string();
    }
    if normalized.contains("gemini") {
        return "gemini".to_string();
    }
    if normalized.contains("cursor") {
        return "cursor".to_string();
    }

    // Fallback: slug of the display name so unknown providers still bucket
    // consistently.
    let slug: String = normalized
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        "unknown".to_string()
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        crate::storage::lock_test_env()
    }

    struct EnvVarGuard {
        key: &'static str,
        prev: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let prev = std::env::var_os(key);
            crate::env::set_var(key, value);
            Self { key, prev }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(prev) = &self.prev {
                crate::env::set_var(self.key, prev);
            } else {
                crate::env::remove_var(self.key);
            }
        }
    }

    fn clear_ledger_cache() {
        if let Ok(mut guard) = LEDGER.lock() {
            *guard = None;
        }
    }

    #[test]
    fn record_use_and_spend_roundtrip_under_jcode_home() {
        let _env_lock = lock_env();
        clear_ledger_cache();
        let temp = tempfile::tempdir().expect("tempdir");
        let _home = EnvVarGuard::set("JCODE_HOME", temp.path().as_os_str());

        record_use("claude:oauth:claude-1");
        record_spend("claude:api-key", 0.25);
        record_spend("claude:api-key", 0.50);

        let used = last_used_unix_secs("claude:oauth:claude-1").expect("last used recorded");
        assert!(now_unix_secs().saturating_sub(used) < 5);

        let spend = spend_snapshot("claude:api-key").expect("spend recorded");
        assert!((spend.day_usd - 0.75).abs() < 1e-9);
        assert!((spend.all_time_usd - 0.75).abs() < 1e-9);
        // Spend also bumps recency.
        assert!(last_used_unix_secs("claude:api-key").is_some());

        // Persisted to disk, not just memory.
        clear_ledger_cache();
        let spend = spend_snapshot("claude:api-key").expect("spend reloaded from disk");
        assert!((spend.all_time_usd - 0.75).abs() < 1e-9);
    }

    #[test]
    fn record_spend_ignores_invalid_amounts() {
        let _env_lock = lock_env();
        clear_ledger_cache();
        let temp = tempfile::tempdir().expect("tempdir");
        let _home = EnvVarGuard::set("JCODE_HOME", temp.path().as_os_str());

        record_spend("openai:api-key", 0.0);
        record_spend("openai:api-key", -1.0);
        record_spend("openai:api-key", f64::NAN);
        assert!(spend_snapshot("openai:api-key").is_none());
    }

    #[test]
    fn source_key_mapping_covers_known_providers() {
        assert_eq!(
            source_key_for_provider_label("DeepSeek", None),
            "openai-compatible:deepseek"
        );
        assert_eq!(
            source_key_for_provider_label("Moonshot AI", None),
            "openai-compatible:moonshotai"
        );
        assert_eq!(
            source_key_for_provider_label("OpenRouter", None),
            "openrouter"
        );
        assert_eq!(
            source_key_for_provider_label("OpenRouter", Some("deepseek")),
            "openai-compatible:deepseek"
        );
        assert_eq!(
            source_key_for_provider_label("Anthropic", None),
            "claude:api-key"
        );
        assert_eq!(
            source_key_for_provider_label("OpenAI", None),
            "openai:api-key"
        );
        assert_eq!(
            source_key_for_provider_label("Some Custom Endpoint", None),
            "some-custom-endpoint"
        );
    }

    #[test]
    fn relative_age_formatting() {
        let now = now_unix_secs();
        assert_eq!(format_relative_age(now), "just now");
        assert_eq!(format_relative_age(now - 120), "2m ago");
        assert_eq!(format_relative_age(now - 3_600), "1h ago");
        assert_eq!(format_relative_age(now - 2 * 86_400), "2d ago");
    }
}
