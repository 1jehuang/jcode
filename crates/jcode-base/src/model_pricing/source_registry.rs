//! Extra `[[pricing.sources]]` price sheets and the merge that puts them
//! between the user's own `[pricing.providers]` cards and models.dev.
//!
//! A source is a JSON document in models.dev's shape
//! (`{provider: {models: {id: {cost, tariffs, schedule, ...}}}}`) that lives at
//! an `https://` URL or in a local file, with its own scope, priority and TTL.
//! The user's cards stay authoritative (spec 4.4); a source only fills in what
//! those leave unsaid, and models.dev still prices anything no source covers.
//!
//! Three properties of this module are deliberate and are what its tests pin
//! down:
//!
//! * **Lookups never block on the network.** A remote sheet is read from the
//!   on-disk cache; a missing or stale copy is refreshed by the background task
//!   and the source is simply unused until that succeeds. A `file://` sheet is
//!   re-read inline, because a local read is not network I/O.
//! * **A source never fabricates a rate.** Unreachable, unparseable, empty, or
//!   stale all mean "this source has nothing to say", so the next layer prices
//!   the model. The previous good copy stays on disk: degradation is per
//!   lookup, not destruction of what was fetched.
//! * **The order is a property of the config.** Sources resolve by `priority`
//!   ascending and then by `id` lexicographically (`config::pricing` sorts once
//!   at validation time), never by map iteration order.

use crate::config::{PricingSource, SourceLocation};
use crate::model_pricing::entry::ModelPricingEntry;
use crate::model_pricing::sources;
use crate::model_pricing::{catalog, models_dev_provider_id, normalize_model_id};
use jcode_provider_core::Currency;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// Where the fetched sheets live. One file for every source, so a source that
/// is removed from the config simply stops being read.
const SOURCES_CACHE_FILE: &str = "pricing_sources.json";
const SOURCES_SCHEMA_VERSION: u32 = 1;

/// A price sheet that decides what money is spent does not get read without a
/// bound; this is the same ceiling models.dev's own catalog comfortably fits in.
const MAX_SHEET_BYTES: usize = 32 * 1024 * 1024;

/// One fetched sheet, plus what it was fetched from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct SourceCatalog {
    /// The location this copy came from. A config edit that points the same
    /// `id` somewhere else must not keep serving the old sheet until the TTL
    /// runs out, so a mismatch is treated as "no copy yet".
    pub(super) location: String,
    pub(super) fetched_at_unix_secs: u64,
    /// provider id -> model id -> entry, exactly like the models.dev cache.
    pub(super) providers: HashMap<String, HashMap<String, ModelPricingEntry>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct SourcesCache {
    #[serde(default)]
    pub(super) schema_version: u32,
    #[serde(default)]
    pub(super) sources: HashMap<String, SourceCatalog>,
}

impl Default for SourcesCache {
    fn default() -> Self {
        Self {
            schema_version: SOURCES_SCHEMA_VERSION,
            sources: HashMap::new(),
        }
    }
}

static SOURCES_CACHE: Mutex<Option<(PathBuf, Arc<SourcesCache>)>> = Mutex::new(None);
/// Ids whose background refresh is already running, so a burst of lookups
/// launches one fetch per source (the per-source shape of the single-flight
/// flag the models.dev refresh uses).
static SOURCES_REFRESHING: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// The source layer's answer for one `(provider, model)` pair at `at`.
///
/// `None` means no source covers the pair, or every source that does has
/// nothing usable to say (unreachable, unparseable, stale, or out of effect).
/// The caller then asks the next layer, which is the whole point: a source
/// never turns "I do not know" into a number.
pub(super) struct SourceHit {
    /// The `id` of the sheet that supplied the rates, for honest labelling.
    pub(super) source_id: String,
    pub(super) entry: ModelPricingEntry,
    pub(super) currency: Currency,
}

/// Resolve the extra-source layer for `(source_key, model)`.
///
/// Sources are consulted in resolution order. The first one that covers the
/// pair and can price it puts forward its card; a later source of the *same
/// currency* may fill in rate fields the winner left unset (spec 4.4's
/// field-level fallback, one layer down), but it never changes the currency and
/// never contributes a tariff, schedule, or long-context tier: those describe
/// the winning sheet's own rate card.
pub(super) fn source_card(source_key: &str, model: &str, at: SystemTime) -> Option<SourceHit> {
    let config = sources::pricing_config();
    if config.sources.is_empty() {
        return None;
    }

    let mut hit: Option<SourceHit> = None;
    for source in &config.sources {
        if !covers_provider(source, source_key) || !covers_model(source, model) {
            continue;
        }
        let Some(cache) = usable_catalog(source) else {
            continue;
        };
        let Some(entry) = entry_for(&cache, &source.id, source_key, model) else {
            continue;
        };
        // A sheet can carry the same `effective_from` / `effective_until` a
        // hand-written rule can. Out of effect means this source does not price
        // the call; the next source (or models.dev) does.
        if entry.out_of_effect_reason(at).is_some() {
            crate::logging::debug(&format!(
                "pricing source `{}` states a rule for {source_key}/{model} that is out of effect \
                 at this instant; skipping it",
                source.id
            ));
            continue;
        }
        match &mut hit {
            None => {
                hit = Some(SourceHit {
                    source_id: source.id.clone(),
                    entry,
                    currency: source.currency.clone(),
                });
            }
            Some(existing) => {
                if existing.currency == source.currency {
                    fill_missing_cost(&mut existing.entry, &entry);
                }
            }
        }
    }
    hit
}

/// Whether a source's `scope` lets it price `source_key`.
///
/// An empty scope means "every provider". Otherwise the entries use the exact
/// same three identity forms a `[pricing.providers]` key does (spec 4.2.1 /
/// F12), so `["deepseek"]` also covers `openai-compatible:deepseek` and
/// `["openai-compatible:my-gateway"]` is the same as `["my-gateway"]`.
fn covers_provider(source: &PricingSource, source_key: &str) -> bool {
    source.scope.is_empty()
        || source
            .scope
            .iter()
            .any(|entry| super::sources::provider_key_matches(entry, source_key))
}

/// Whether a source's `models` globs let it price `model`.
fn covers_model(source: &PricingSource, model: &str) -> bool {
    if source.models.is_empty() {
        return true;
    }
    let normalized = normalize_model_id(model);
    source.models.iter().any(|pattern| {
        super::glob_matches(pattern, model) || super::glob_matches(pattern, normalized)
    })
}

/// The copy of `source` a lookup may use right now.
///
/// Fresh means "fetched within this source's own TTL". A local file is re-read
/// inline when its copy is stale (or missing); a remote sheet is only ever
/// fetched by the background refresher, so this returns `None` in the meantime
/// and the lookup falls through to the next layer instead of blocking.
fn usable_catalog(source: &PricingSource) -> Option<Arc<SourcesCache>> {
    let cache = load_cache();
    if let Some(catalog) = cache.sources.get(&source.id)
        && catalog.location == source.location.describe()
        && is_fresh(source, catalog)
    {
        return Some(cache);
    }

    if in_backoff(&source.id) {
        return None;
    }

    match &source.location {
        SourceLocation::LocalFile(path) => match read_local_sheet(path) {
            Ok(providers) => {
                clear_failure(&source.id);
                save_catalog(&source.id, &source.location.describe(), providers);
                Some(load_cache())
            }
            Err(error) => {
                note_failure(&source.id);
                crate::logging::warn(&format!(
                    "pricing source `{}` ({}) is unavailable: {error}; \
                     falling through to the next price source",
                    source.id,
                    source.location.describe()
                ));
                None
            }
        },
        SourceLocation::Remote(_) => {
            schedule_source_refresh(source);
            None
        }
    }
}

fn is_fresh(source: &PricingSource, catalog: &SourceCatalog) -> bool {
    catalog::now_unix_secs().saturating_sub(catalog.fetched_at_unix_secs) < source.refresh_secs
}

/// How long a source that just failed is left alone before being tried again.
///
/// The route catalog asks for a price once per route, so "read the file again
/// on every lookup" turns one missing file into thousands of syscalls and log
/// lines. A minute is short enough that fixing the file is noticed quickly, and
/// long enough that a broken source costs one attempt per minute instead of one
/// per price.
///
/// Only *failures* are recorded: a source that succeeds is governed by its own
/// TTL, so a config edit that repoints it (or a file that changes) is read at
/// the next lookup rather than after the backoff.
const FAILURE_BACKOFF_SECS: u64 = 60;

/// When each source last failed, for [`FAILURE_BACKOFF_SECS`]. Process local:
/// it exists to bound work, not to remember anything.
static SOURCE_FAILURES: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

/// True while a source that recently failed is being left alone.
fn in_backoff(id: &str) -> bool {
    let Ok(failures) = SOURCE_FAILURES.lock() else {
        return false;
    };
    let Some(failures) = failures.as_ref() else {
        return false;
    };
    match failures.get(id) {
        Some(failed_at) => {
            catalog::now_unix_secs().saturating_sub(*failed_at) < FAILURE_BACKOFF_SECS
        }
        None => false,
    }
}

fn note_failure(id: &str) {
    let Ok(mut failures) = SOURCE_FAILURES.lock() else {
        return;
    };
    let failures = failures.get_or_insert_with(HashMap::new);
    failures.insert(id.to_string(), catalog::now_unix_secs());
}

fn clear_failure(id: &str) {
    let Ok(mut failures) = SOURCE_FAILURES.lock() else {
        return;
    };
    if let Some(failures) = failures.as_mut() {
        failures.remove(id);
    }
}

/// Read one sheet from a local path, enforcing the size ceiling.
fn read_local_sheet(
    path: &std::path::Path,
) -> anyhow::Result<HashMap<String, HashMap<String, ModelPricingEntry>>> {
    let body = std::fs::read_to_string(path)
        .map_err(|error| anyhow::anyhow!("cannot read {}: {error}", path.display()))?;
    parse_sheet(&body)
}

/// Parse a sheet body through the same parser the models.dev catalog uses, so
/// the two sources cannot disagree about the shape.
fn parse_sheet(body: &str) -> anyhow::Result<HashMap<String, HashMap<String, ModelPricingEntry>>> {
    if body.len() > MAX_SHEET_BYTES {
        anyhow::bail!("sheet is larger than {MAX_SHEET_BYTES} bytes");
    }
    Ok(catalog::parse_api_response(body)?.providers)
}

/// The entry a sheet states for `(source_key, model)`.
///
/// A sheet addresses providers by their models.dev id (`deepseek`,
/// `anthropic`), exactly like the upstream catalog. The caller's identity is
/// only sometimes that id (`openai-compatible:deepseek`, `claude:api-key`), so
/// the sheet's provider keys are matched with the same three identity forms
/// `scope` uses (spec 4.2.1 / F12): a compatible profile a sheet keys as
/// `my-gateway` is reached by `openai-compatible:my-gateway` and vice versa.
fn entry_for(
    cache: &SourcesCache,
    source_id: &str,
    source_key: &str,
    model: &str,
) -> Option<ModelPricingEntry> {
    let source = cache.sources.get(source_id)?;
    let models = sheet_provider(source, source_key)?;
    let normalized = normalize_model_id(model);
    if let Some(entry) = models.get(normalized) {
        return Some(entry.clone());
    }
    // OpenRouter-style ids may still carry their `provider/` prefix.
    if let Some((_, bare)) = normalized.rsplit_once('/') {
        return models.get(bare).cloned();
    }
    None
}

/// The provider section of `source` that belongs to `source_key`.
fn sheet_provider<'a>(
    source: &'a SourceCatalog,
    source_key: &str,
) -> Option<&'a HashMap<String, ModelPricingEntry>> {
    if let Some(models) = source.providers.get(source_key) {
        return Some(models);
    }
    if let Some(models) =
        models_dev_provider_id(source_key).and_then(|provider_id| source.providers.get(provider_id))
    {
        return Some(models);
    }
    // Last resort, and the only way a sheet keyed by an `openai-compatible:`
    // profile id the models.dev map does not know can be reached at all.
    source
        .providers
        .iter()
        .find(|(key, _)| super::sources::provider_key_matches(key, source_key))
        .map(|(_, models)| models)
}

/// Fill rate fields the winning sheet left unset from a lower-priority sheet of
/// the same currency.
fn fill_missing_cost(winner: &mut ModelPricingEntry, fallback: &ModelPricingEntry) {
    winner.cost.input = winner.cost.input.or(fallback.cost.input);
    winner.cost.output = winner.cost.output.or(fallback.cost.output);
    winner.cost.cache_read = winner.cost.cache_read.or(fallback.cost.cache_read);
    winner.cost.cache_write = winner.cost.cache_write.or(fallback.cost.cache_write);
}

fn cache_path() -> PathBuf {
    crate::storage::jcode_dir()
        .unwrap_or_else(|_| PathBuf::from(".").join(".jcode"))
        .join("cache")
        .join(SOURCES_CACHE_FILE)
}

fn load_cache() -> Arc<SourcesCache> {
    let path = cache_path();
    if let Ok(memory) = SOURCES_CACHE.lock()
        && let Some((cached_path, cache)) = memory.as_ref()
        && cached_path == &path
    {
        return Arc::clone(cache);
    }
    let cache: SourcesCache = crate::storage::read_json(&path).unwrap_or_default();
    let cache = Arc::new(cache);
    if let Ok(mut memory) = SOURCES_CACHE.lock() {
        *memory = Some((path, Arc::clone(&cache)));
    }
    cache
}

/// Store a freshly read sheet, replacing whatever was there.
///
/// Only ever called with a successfully parsed sheet, so a failed refresh
/// leaves the last good copy (and its timestamp) exactly as it was.
fn save_catalog(
    id: &str,
    location: &str,
    providers: HashMap<String, HashMap<String, ModelPricingEntry>>,
) {
    let path = cache_path();
    let mut cache: SourcesCache = crate::storage::read_json(&path).unwrap_or_default();
    cache.schema_version = SOURCES_SCHEMA_VERSION;
    let changed = cache
        .sources
        .get(id)
        .map(|existing| existing.providers != providers)
        .unwrap_or(true);
    cache.sources.insert(
        id.to_string(),
        SourceCatalog {
            location: location.to_string(),
            fetched_at_unix_secs: catalog::now_unix_secs(),
            providers,
        },
    );
    let cache = Arc::new(cache);
    if let Ok(mut memory) = SOURCES_CACHE.lock() {
        *memory = Some((path.clone(), Arc::clone(&cache)));
    }
    if let Err(error) = crate::storage::write_json(&path, cache.as_ref()) {
        crate::logging::warn(&format!(
            "could not persist pricing sources cache: {error:#}"
        ));
    }
    if changed {
        // The rates a lookup would resolve just changed, and the memos
        // downstream (route catalog, TUI per-model price) decide freshness from
        // this counter, so a refresh takes effect on the next read instead of
        // whenever something else happens to invalidate them.
        super::generation::bump_pricing_generation();
    }
}

/// Spawn one background fetch for `source` if none is running.
///
/// Never called from a test build (see [`background_refresh_allowed`]): tests
/// exercise the cache and the fallback paths directly, and must not reach the
/// network.
fn schedule_source_refresh(source: &PricingSource) {
    if !super::background_refresh_allowed() {
        return;
    }
    let SourceLocation::Remote(url) = source.location.clone() else {
        return;
    };
    {
        let Ok(mut in_flight) = SOURCES_REFRESHING.lock() else {
            return;
        };
        let in_flight = in_flight.get_or_insert_with(HashSet::new);
        if !in_flight.insert(source.id.clone()) {
            return;
        }
    }

    let id = source.id.clone();
    let location = source.location.describe();
    let work = move || async move {
        let result = fetch_remote(&url).await;
        match result {
            Ok(providers) => {
                clear_failure(&id);
                save_catalog(&id, &location, providers);
            }
            Err(error) => {
                note_failure(&id);
                crate::logging::warn(&format!(
                    "pricing source `{id}` ({url}) could not be refreshed: {error:#}; \
                     keeping the last successful copy"
                ));
            }
        }
        if let Ok(mut in_flight) = SOURCES_REFRESHING.lock()
            && let Some(set) = in_flight.as_mut()
        {
            set.remove(&id);
        }
    };

    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(work());
    } else {
        std::thread::spawn(move || {
            if let Ok(runtime) = tokio::runtime::Runtime::new() {
                runtime.block_on(work());
            }
        });
    }
}

async fn fetch_remote(
    url: &str,
) -> anyhow::Result<HashMap<String, HashMap<String, ModelPricingEntry>>> {
    let client = crate::provider::shared_http_client();
    let response = client
        .get(url)
        .header("Accept", "application/json")
        .timeout(catalog::HTTP_TIMEOUT)
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!("HTTP {}", response.status());
    }
    let body = response.text().await?;
    parse_sheet(&body)
}

#[cfg(test)]
pub(crate) fn save_test_source(
    id: &str,
    location: &str,
    fetched_at_unix_secs: u64,
    entries: &[(&str, &str, ModelPricingEntry)],
) {
    let mut providers: HashMap<String, HashMap<String, ModelPricingEntry>> = HashMap::new();
    for (provider, model, entry) in entries {
        providers
            .entry((*provider).to_string())
            .or_default()
            .insert((*model).to_string(), entry.clone());
    }
    let path = cache_path();
    let mut cache: SourcesCache = crate::storage::read_json(&path).unwrap_or_default();
    cache.sources.insert(
        id.to_string(),
        SourceCatalog {
            location: location.to_string(),
            fetched_at_unix_secs,
            providers,
        },
    );
    let cache = Arc::new(cache);
    if let Ok(mut memory) = SOURCES_CACHE.lock() {
        *memory = Some((path.clone(), Arc::clone(&cache)));
    }
    if let Err(error) = crate::storage::write_json(&path, cache.as_ref()) {
        panic!("could not persist test sources cache: {error:#}");
    }
}

#[cfg(test)]
pub(crate) fn clear_sources_cache_for_tests() {
    if let Ok(mut memory) = SOURCES_CACHE.lock() {
        *memory = None;
    }
    if let Ok(mut failures) = SOURCE_FAILURES.lock() {
        *failures = None;
    }
    let _ = std::fs::remove_file(cache_path());
}

/// Whether `id` is currently in its failure backoff, for the backoff test.
#[cfg(test)]
pub(crate) fn in_backoff_for_tests(id: &str) -> bool {
    in_backoff(id)
}

/// Forget every recorded failure, i.e. pretend the backoff window elapsed.
#[cfg(test)]
pub(crate) fn forget_failures_for_tests() {
    if let Ok(mut failures) = SOURCE_FAILURES.lock() {
        *failures = None;
    }
}

/// What the cache holds for `id`, for tests that assert a failed refresh did
/// not destroy the last good copy.
#[cfg(test)]
pub(crate) fn cached_fetched_at_for_tests(id: &str) -> Option<u64> {
    load_cache()
        .sources
        .get(id)
        .map(|catalog| catalog.fetched_at_unix_secs)
}
