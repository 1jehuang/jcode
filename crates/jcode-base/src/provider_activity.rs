//! Cross-provider activity ledger.
//!
//! Tracks two things per login/credential ("source key"):
//!   1. When jcode last successfully used it (for recency-sorted `/usage`).
//!   2. Locally accumulated API-key spend (day / month / all-time), mirroring
//!      the figures the TUI cost paths compute, since most providers do not
//!      expose per-key spend through their public APIs. Spend is kept **per
//!      currency**: an amount is never relabelled or converted here, because
//!      the pricing path knows the currency its rates are denominated in and
//!      this ledger only stores what it was handed.
//!
//! Data persists to `~/.jcode/provider_activity.json` and is shared across
//! processes (server records last-used, TUI records spend, `/usage` reads
//! both), so queries re-read the file with a short TTL instead of trusting a
//! process-local cache. The file is *also* shared with upstream jcode, which
//! is why the format stays readable by the older schema; see [`ProviderSpend`].
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
use jcode_provider_core::Currency;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Ledger format version written by this build.
///
/// 1 is the schema upstream jcode still writes: one `*_usd` figure per window.
/// 2 adds the per-currency buckets and keeps the `*_usd` figures as a mirror.
pub const PROVIDER_ACTIVITY_SCHEMA_VERSION: u32 = 2;

fn default_schema_version() -> u32 {
    PROVIDER_ACTIVITY_SCHEMA_VERSION
}

/// Re-reads of the ledger are throttled to this interval for query paths.
const QUERY_RELOAD_TTL: Duration = Duration::from_secs(2);

/// Skip persisting a new last-used timestamp when the stored one is within
/// this many seconds, so busy sessions do not rewrite the file on every call.
const LAST_USED_WRITE_THROTTLE_SECS: u64 = 30;

/// Spend accumulated for one credential: one bucket per window *and* currency.
///
/// Each window is a map from currency to amount, with the `day_date` / `month`
/// labels deciding when a window rolls over.
///
/// # Rollback support (F14)
///
/// Every window also carries a `*_usd` mirror: the naive sum of that window's
/// buckets, written on every update. The fork and upstream jcode share
/// `~/.jcode/provider_activity.json`, and an older binary only knows those
/// figures, so serde drops the maps it does not understand when it writes the
/// file back. Without the mirror that single write would silently zero the
/// whole ledger; with it the old reader still sees the totals, and the next
/// read by this build migrates the mirror back into a `USD` bucket (see
/// `ProviderSpend::migrate_legacy_usd_figures`).
///
/// A summed mirror can only be exact when a window holds one currency. With
/// mixed currencies it is an *approximation*: it is not a converted total, and
/// the per-currency split does not survive a rollback (the totals do). That is
/// inherent to keeping the file readable by a USD-only reader, and it is why
/// the buckets, not the mirror, are the written source of truth.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderSpend {
    /// `YYYY-MM-DD` the `day` buckets belong to.
    #[serde(default)]
    pub day_date: String,
    /// Per-currency spend for `day_date`.
    #[serde(default)]
    pub day: BTreeMap<Currency, f64>,
    /// Sum of `day`; see the rollback note on this type.
    #[serde(default)]
    pub day_usd: f64,
    /// `YYYY-MM` the month buckets belong to.
    #[serde(default)]
    pub month: String,
    /// Per-currency spend for `month`.
    ///
    /// Deliberately not called `month`: that key already holds the window
    /// label, and an older binary reads it to decide whether to roll the
    /// window. Overloading the key would break the very rollback this type
    /// exists to protect.
    #[serde(default)]
    pub month_spend: BTreeMap<Currency, f64>,
    /// Sum of `month_spend`; see the rollback note on this type.
    #[serde(default)]
    pub month_usd: f64,
    /// Per-currency spend since the ledger was created.
    #[serde(default)]
    pub all_time: BTreeMap<Currency, f64>,
    /// Sum of `all_time`; see the rollback note on this type.
    #[serde(default)]
    pub all_time_usd: f64,
}

impl ProviderSpend {
    /// Add `amount` of `currency` to every window and refresh the mirrors.
    fn accrue(&mut self, currency: &Currency, amount: f64) {
        *self.day.entry(currency.clone()).or_insert(0.0) += amount;
        *self.month_spend.entry(currency.clone()).or_insert(0.0) += amount;
        *self.all_time.entry(currency.clone()).or_insert(0.0) += amount;
        self.refresh_usd_mirrors();
    }

    /// Rewrite each `*_usd` mirror as the sum of its buckets.
    fn refresh_usd_mirrors(&mut self) {
        self.day_usd = self.day.values().sum();
        self.month_usd = self.month_spend.values().sum();
        self.all_time_usd = self.all_time.values().sum();
    }

    /// Seed the `USD` buckets from the legacy `*_usd` figures.
    ///
    /// A file written before multi-currency — or written back by an older
    /// binary, which drops the bucket maps — carries money only in the mirrors.
    /// A v2 writer never leaves a window with a non-zero mirror and an empty
    /// bucket map: both are cleared together when the window rolls, and the
    /// mirror is rewritten whenever a bucket changes. An empty map next to a
    /// figure therefore means "legacy USD amount".
    fn migrate_legacy_usd_figures(&mut self) {
        if self.day.is_empty() && self.day_usd != 0.0 {
            self.day.insert(Currency::usd(), self.day_usd);
        }
        if self.month_spend.is_empty() && self.month_usd != 0.0 {
            self.month_spend.insert(Currency::usd(), self.month_usd);
        }
        if self.all_time.is_empty() && self.all_time_usd != 0.0 {
            self.all_time.insert(Currency::usd(), self.all_time_usd);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderActivityEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_unix_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spend: Option<ProviderSpend>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderActivityStore {
    /// Format version of this file; see [`PROVIDER_ACTIVITY_SCHEMA_VERSION`].
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub entries: HashMap<String, ProviderActivityEntry>,
}

impl Default for ProviderActivityStore {
    fn default() -> Self {
        Self {
            schema_version: PROVIDER_ACTIVITY_SCHEMA_VERSION,
            entries: HashMap::new(),
        }
    }
}

impl ProviderActivityStore {
    /// Bring a store read from disk up to the current shape.
    fn migrate_legacy_usd_figures(&mut self) {
        self.schema_version = PROVIDER_ACTIVITY_SCHEMA_VERSION;
        for entry in self.entries.values_mut() {
            if let Some(spend) = entry.spend.as_mut() {
                spend.migrate_legacy_usd_figures();
            }
        }
    }
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
    let mut store: ProviderActivityStore =
        crate::storage::read_json(&ledger_path()).unwrap_or_default();
    // Migration runs on every load, not just when `schema_version` says the
    // file is old: an older binary writing the file back drops both the bucket
    // maps and the version field, so the version alone cannot tell us the
    // shape of what is on disk.
    store.migrate_legacy_usd_figures();
    store
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
        spend.day.clear();
        spend.day_usd = 0.0;
    }
    if spend.month != month {
        spend.month = month;
        spend.month_spend.clear();
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

/// Accumulate locally computed API-key spend for a credential.
///
/// `amount` is in `currency`, and it stays in that currency: the ledger keeps
/// one bucket per currency and never converts, so a non-USD call must not be
/// written into — or approximated by — a USD figure. The `*_usd` mirrors are
/// the one exception, and they are explicitly an approximation (see
/// [`ProviderSpend`]).
pub fn record_spend(source_key: &str, amount: f64, currency: &Currency) {
    let source_key = source_key.trim();
    if source_key.is_empty() || !amount.is_finite() || amount <= 0.0 {
        return;
    }
    let now = now_unix_secs();
    let source_key = source_key.to_string();
    let currency = currency.clone();
    with_fresh_store(move |store| {
        let entry = store.entries.entry(source_key).or_default();
        // Spend implies use; keep recency in the same write.
        entry.last_used_unix_secs = Some(now);
        let spend = entry.spend.get_or_insert_with(ProviderSpend::default);
        roll_spend(spend);
        spend.accrue(&currency, amount);
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

    // A label that is already a ledger source key names its own profile:
    // `openai-compatible:<id>` is the identity the ledger keys on, and the
    // display-name lookups below cannot recover it (the profile's display name
    // is e.g. `DeepSeek`, and the multiplexing OpenRouter slot reports the
    // transport name for every direct profile). Without this, such a label
    // falls through to the `contains("openai")` branch and is billed to
    // `openai:api-key`.
    if let Some(profile_id) = normalized.strip_prefix("openai-compatible:") {
        let profile_id = profile_id.trim();
        if !profile_id.is_empty() {
            return format!("openai-compatible:{profile_id}");
        }
    }

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

    /// The pre-multi-currency shape of the ledger, verbatim from the previous
    /// build (and from upstream jcode): one USD figure per window, no
    /// `Currency` buckets and no `schema_version`.
    ///
    /// Used to simulate an *older reader*, whose serde derive drops every field
    /// it does not know about when it writes the file back.
    #[derive(Debug, Clone, Serialize, Deserialize, Default)]
    struct LegacyProviderSpend {
        #[serde(default)]
        day_date: String,
        #[serde(default)]
        day_usd: f64,
        #[serde(default)]
        month: String,
        #[serde(default)]
        month_usd: f64,
        #[serde(default)]
        all_time_usd: f64,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, Default)]
    struct LegacyActivityEntry {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_used_unix_secs: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spend: Option<LegacyProviderSpend>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, Default)]
    struct LegacyStore {
        #[serde(default)]
        entries: HashMap<String, LegacyActivityEntry>,
    }

    /// Read the ledger exactly as the current build would (raw JSON on disk).
    fn read_ledger_json() -> serde_json::Value {
        let raw = std::fs::read_to_string(ledger_path()).expect("ledger file exists");
        serde_json::from_str(&raw).expect("ledger is JSON")
    }

    #[test]
    fn legacy_fields_migrate_to_usd_bucket() {
        // F14, read direction: a file written before multi-currency carries its
        // money in `*_usd` only. Those figures are USD amounts, so they must
        // come back as a `USD` bucket instead of vanishing.
        let _env_lock = lock_env();
        clear_ledger_cache();
        let temp = tempfile::tempdir().expect("tempdir");
        let _home = EnvVarGuard::set("JCODE_HOME", temp.path().as_os_str());

        let now = Utc::now();
        let legacy = serde_json::json!({
            "entries": {
                "claude:api-key": {
                    "last_used_unix_secs": 1,
                    "spend": {
                        "day_date": now.format("%Y-%m-%d").to_string(),
                        "day_usd": 12.0,
                        "month": format!("{}-{:02}", now.year(), now.month()),
                        "month_usd": 12.0,
                        "all_time_usd": 12.0,
                    }
                }
            }
        });
        std::fs::write(
            ledger_path(),
            serde_json::to_string(&legacy).expect("serialize legacy ledger"),
        )
        .expect("write legacy ledger");
        clear_ledger_cache();

        let spend = spend_snapshot("claude:api-key").expect("legacy entry loads");
        let usd_bucket = std::collections::BTreeMap::from([(Currency::usd(), 12.0)]);
        assert_eq!(spend.day, usd_bucket, "legacy day_usd becomes a USD bucket");
        assert_eq!(spend.month_spend, usd_bucket);
        assert_eq!(spend.all_time, usd_bucket);

        // ...and the migrated shape is what the next write persists, together
        // with the format version.
        record_spend("claude:api-key", 3.0, &Currency::new("cny"));
        clear_ledger_cache();
        let written = read_ledger_json();
        let entry = &written["entries"]["claude:api-key"]["spend"];
        assert_eq!(written["schema_version"], 2);
        assert_eq!(entry["day"], serde_json::json!({"CNY": 3.0, "USD": 12.0}));
        assert_eq!(entry["day_usd"], 15.0);

        // Re-reading the migrated file must not seed the bucket again: the
        // legacy figure is already inside `day`.
        clear_ledger_cache();
        let spend = spend_snapshot("claude:api-key").expect("reload");
        assert_eq!(
            spend.day,
            std::collections::BTreeMap::from([
                (Currency::new("CNY"), 3.0),
                (Currency::usd(), 12.0),
            ])
        );
        assert!((spend.day_usd - 15.0).abs() < 1e-9);
    }

    #[test]
    fn usd_mirror_tracks_bucket_sum() {
        // F14, write direction: the mirror is rewritten from the buckets on
        // every write, so an older binary still reads a total instead of 0.
        let _env_lock = lock_env();
        clear_ledger_cache();
        let temp = tempfile::tempdir().expect("tempdir");
        let _home = EnvVarGuard::set("JCODE_HOME", temp.path().as_os_str());

        record_spend("openai:api-key", 30.0, &Currency::new("cny"));
        record_spend("openai:api-key", 5.0, &Currency::usd());

        let spend = spend_snapshot("openai:api-key").expect("spend recorded");
        assert_eq!(
            spend.day,
            std::collections::BTreeMap::from([
                (Currency::new("CNY"), 30.0),
                (Currency::usd(), 5.0),
            ]),
            "each currency keeps its own bucket, normalized"
        );
        // Mixed-currency windows make the mirror an approximation: it is the
        // naive sum of the buckets, never a converted total.
        assert!((spend.day_usd - 35.0).abs() < 1e-9);
        assert!((spend.month_usd - 35.0).abs() < 1e-9);
        assert!((spend.all_time_usd - 35.0).abs() < 1e-9);

        // The mirror is written, not just computed in memory.
        clear_ledger_cache();
        let entry = read_ledger_json()["entries"]["openai:api-key"]["spend"].clone();
        assert_eq!(entry["day_usd"], 35.0);
        assert_eq!(entry["month_usd"], 35.0);
        assert_eq!(entry["all_time_usd"], 35.0);
        assert_eq!(entry["day"], serde_json::json!({"CNY": 30.0, "USD": 5.0}));
    }

    #[test]
    fn multi_currency_buckets_accumulate() {
        let _env_lock = lock_env();
        clear_ledger_cache();
        let temp = tempfile::tempdir().expect("tempdir");
        let _home = EnvVarGuard::set("JCODE_HOME", temp.path().as_os_str());

        record_spend("openai-compatible:deepseek", 30.0, &Currency::new("CNY"));
        record_spend("openai-compatible:deepseek", 5.0, &Currency::new("CNY"));
        record_spend("openai-compatible:deepseek", 2.0, &Currency::usd());

        let spend = spend_snapshot("openai-compatible:deepseek").expect("spend recorded");
        let expected = std::collections::BTreeMap::from([
            (Currency::new("CNY"), 35.0),
            (Currency::usd(), 2.0),
        ]);
        assert_eq!(spend.day, expected, "same-currency calls add up");
        assert_eq!(spend.month_spend, expected);
        assert_eq!(spend.all_time, expected);
        assert!((spend.day_usd - 37.0).abs() < 1e-9);
    }

    #[test]
    fn rollback_roundtrip_keeps_buckets() {
        // F14 acceptance core. The fork and upstream jcode share
        // `~/.jcode/provider_activity.json`, so a user who falls back to an
        // older binary must not lose their spend history.
        let _env_lock = lock_env();
        clear_ledger_cache();
        let temp = tempfile::tempdir().expect("tempdir");
        let _home = EnvVarGuard::set("JCODE_HOME", temp.path().as_os_str());

        let key = "openai-compatible:deepseek";
        record_spend(key, 30.0, &Currency::new("CNY"));
        record_spend(key, 5.0, &Currency::usd());
        clear_ledger_cache();
        assert!(
            spend_snapshot(key)
                .expect("written")
                .day
                .contains_key(&Currency::new("CNY"))
        );

        // Older binary: deserialize the new file with the OLD struct shape and
        // write it straight back, the way `with_fresh_store` does on every
        // `record_use`/`record_spend`. Serde drops `day`/`month_spend`/
        // `all_time` and `schema_version` here, exactly as the old build does.
        let path = ledger_path();
        let raw = std::fs::read_to_string(&path).expect("new ledger on disk");
        let legacy: LegacyStore = serde_json::from_str(&raw).expect("old reader parses new file");
        let legacy_spend = legacy
            .entries
            .get(key)
            .and_then(|entry| entry.spend.as_ref())
            .expect("old reader sees the entry");
        // Without the mirror the old reader would see 0 here and zero the
        // ledger on write-back.
        assert!(
            (legacy_spend.day_usd - 35.0).abs() < 1e-9,
            "old reader must see the day total, saw {}",
            legacy_spend.day_usd
        );
        assert!((legacy_spend.month_usd - 35.0).abs() < 1e-9);
        assert!((legacy_spend.all_time_usd - 35.0).abs() < 1e-9);
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&legacy).expect("old reader serializes"),
        )
        .expect("old reader writes back");
        clear_ledger_cache();

        // New binary again: no window is lost. The mirror is a plain sum, so
        // what survives a rollback is the totals (as USD buckets), not the
        // per-currency split.
        let spend = spend_snapshot(key).expect("entry survives the rollback roundtrip");
        assert_eq!(
            spend.day,
            std::collections::BTreeMap::from([(Currency::usd(), 35.0)]),
            "day bucket survives"
        );
        assert_eq!(
            spend.month_spend,
            std::collections::BTreeMap::from([(Currency::usd(), 35.0)]),
            "month bucket survives"
        );
        assert_eq!(
            spend.all_time,
            std::collections::BTreeMap::from([(Currency::usd(), 35.0)]),
            "all_time bucket survives"
        );

        // ...and the ledger is writable again after the rollback: new spend
        // lands next to the migrated USD bucket.
        record_spend(key, 4.0, &Currency::new("CNY"));
        let spend = spend_snapshot(key).expect("still records after rollback");
        assert_eq!(
            spend.all_time,
            std::collections::BTreeMap::from([
                (Currency::new("CNY"), 4.0),
                (Currency::usd(), 35.0),
            ])
        );
    }

    #[test]
    fn record_use_and_spend_roundtrip_under_jcode_home() {
        let _env_lock = lock_env();
        clear_ledger_cache();
        let temp = tempfile::tempdir().expect("tempdir");
        let _home = EnvVarGuard::set("JCODE_HOME", temp.path().as_os_str());

        record_use("claude:oauth:claude-1");
        record_spend("claude:api-key", 0.25, &Currency::usd());
        record_spend("claude:api-key", 0.50, &Currency::usd());

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

        record_spend("openai:api-key", 0.0, &Currency::usd());
        record_spend("openai:api-key", -1.0, &Currency::usd());
        record_spend("openai:api-key", f64::NAN, &Currency::usd());
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

    /// A label can already *be* a ledger source key: the `History` payload
    /// reports the session's provider key, which for a direct profile is
    /// `openai-compatible:<id>`. That key must survive the mapping; otherwise
    /// it falls through to `contains("openai")` and the spend is attributed to
    /// `openai:api-key` instead of the profile that served the request (#1286).
    #[test]
    fn source_key_mapping_keeps_an_already_prefixed_profile_key() {
        assert_eq!(
            source_key_for_provider_label("openai-compatible:deepseek", None),
            "openai-compatible:deepseek"
        );
        assert_eq!(
            source_key_for_provider_label("OpenAI-Compatible:NVIDIA-NIM", None),
            "openai-compatible:nvidia-nim"
        );
        // An empty profile id is not a profile key; it keeps the old fallback.
        assert_eq!(
            source_key_for_provider_label("openai-compatible:", None),
            "openai:api-key"
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
