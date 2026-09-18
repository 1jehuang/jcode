//! Extra `[[pricing.sources]]` price sheets and the merge that puts them
//! between the user's own `[pricing.providers]` cards and models.dev.
//!
//! A source is a JSON document in models.dev's shape
//! (`{provider: {models: {id: {cost, tariffs, schedule, ...}}}}`) that lives in
//! a **local file**, with its own scope and priority. jcode never fetches a
//! price sheet: a `file` value names a path on disk (a bare name resolves under
//! `~/.jcode/cache/`). The user's cards stay authoritative (spec 4.4); a source
//! only fills in what those leave unsaid, and models.dev still prices anything
//! no source covers.
//!
//! Three properties of this module are deliberate and are what its tests pin
//! down:
//!
//! * **Lookups never block.** A local sheet is read inline, because a local read
//!   is not network I/O. There is no fetch path at all.
//! * **A sheet is fresh while its file is unchanged.** The cached copy records
//!   the file's mtime and size; an edit makes the copy stale immediately, so "my
//!   price file" means "saving it changes the price". The check costs one `stat`
//!   per lookup (never a re-read), and a file that is missing or unreadable is
//!   left alone for a short failure backoff so a broken source costs one attempt
//!   per window, not one per lookup.
//! * **A source never fabricates a rate.** Missing, unreadable, unparseable,
//!   empty, or out of effect all mean "this source has nothing to say", so the
//!   next layer prices the model. Degradation is per lookup.
//! * **The order is a property of the config.** Sources resolve by `priority`
//!   ascending and then by `id` lexicographically (`config::pricing` sorts once
//!   at validation time), never by map iteration order.

use crate::config::PricingSource;
use crate::model_pricing::entry::{ModelPricingEntry, RuleOutOfEffect};
use crate::model_pricing::sources;
use crate::model_pricing::{catalog, models_dev_provider_id, normalize_model_id};
use jcode_provider_core::Currency;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// Where the read sheets live. One file for every source, so a source that is
/// removed from the config simply stops being read.
pub(crate) const SOURCES_CACHE_FILE: &str = "pricing_sources.json";
const SOURCES_SCHEMA_VERSION: u32 = 1;

/// A price sheet that decides what money is spent does not get read without a
/// bound; this is the same ceiling models.dev's own catalog comfortably fits in.
const MAX_SHEET_BYTES: usize = 32 * 1024 * 1024;
/// The bound above, exposed to the tests that build a body just past it.
#[cfg(test)]
pub(super) const MAX_SHEET_BYTES_TEST: usize = MAX_SHEET_BYTES;

/// How many times a local sheet file was actually opened, for the test that
/// pins "an unchanged file is stat'ed but not re-read".
#[cfg(test)]
static LOCAL_SHEET_READS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// One read sheet, plus the file it was read from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct SourceCatalog {
    /// The path this copy came from. A config edit that points the same `id`
    /// somewhere else must not keep serving the old sheet, so a mismatch is
    /// treated as "no copy yet".
    pub(super) location: String,
    /// The local file this copy was read from, identified by mtime and size.
    ///
    /// A sheet is fresh exactly while this matches the file on disk, which is
    /// why an edit takes effect at the next lookup. `None` is treated as stale
    /// once, re-read, and fingerprinted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) fingerprint: Option<FileFingerprint>,
    /// provider id -> model id -> entry, exactly like the models.dev cache.
    pub(super) providers: HashMap<String, HashMap<String, ModelPricingEntry>>,
}

/// What "the same local file" means for cache freshness: modification time plus
/// size.
///
/// mtime alone is enough to notice an edit whose byte length did not change (a
/// changed rate is often the same width), and size catches the rare edit the
/// filesystem's timestamp granularity hides. Nanoseconds since the epoch, `0`
/// when the platform will not report a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct FileFingerprint {
    pub(super) mtime_nanos: u64,
    pub(super) size: u64,
}

impl FileFingerprint {
    /// The fingerprint of a stat'ed file.
    fn of(metadata: &std::fs::Metadata) -> Self {
        let mtime_nanos = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|since| since.as_nanos().min(u64::MAX as u128) as u64)
            .unwrap_or(0);
        Self {
            mtime_nanos,
            size: metadata.len(),
        }
    }
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
/// Serializes the read-modify-write of the on-disk/in-memory sources cache.
///
/// [`SOURCES_CACHE`]'s own lock only covers the memory assignment, so without
/// this two different sources' reads could interleave and drop a sheet. It is
/// deliberately a separate lock from [`SOURCES_CACHE`] because `save_catalog`
/// takes both, in this order, and nothing takes them reversed.
static SOURCES_WRITE_LOCK: Mutex<()> = Mutex::new(());

/// The source layer's answer for one `(provider, model)` pair at `at`.
///
/// `None` means no source covers the pair, or every source that does has
/// nothing usable to say (missing, unreadable, unparseable, or out of effect).
/// The caller then asks the next layer, which is the whole point: a source never
/// turns "I do not know" into a number.
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

/// The sheet rule that covers `(source_key, model)` and is out of effect at
/// `at`, if any, together with the `id` of the sheet that states it.
///
/// This is the sheet-layer half of the F8/F20 marker: the caller prices the
/// call from the next layer anyway (that is the fall-through), but a user who
/// wrote a sheet has to learn that their rule stopped applying instead of
/// silently reading a models.dev number.
///
/// Only the *first* sheet that states an entry for the pair can be that reason.
/// An earlier covering sheet that states nothing for this model would not have
/// priced the call even while in effect, and an earlier sheet that *is* in
/// effect wins outright, so neither is reported. That is why this mirrors
/// `source_card`'s iteration order rather than collecting every expired sheet.
pub(super) fn out_of_effect_sheet(
    source_key: &str,
    model: &str,
    at: SystemTime,
) -> Option<(String, RuleOutOfEffect)> {
    let config = sources::pricing_config();
    if config.sources.is_empty() {
        return None;
    }

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
        // The first sheet that states an entry for the pair decides: it either
        // prices the call, or its expiry is what sent the price below.
        return entry
            .out_of_effect_reason(at)
            .map(|reason| (source.id.clone(), reason));
    }
    None
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

/// The path string a cache entry records for `path`, and the form compared to
/// decide whether a cached copy still describes the source's file.
fn location_key(path: &Path) -> String {
    path.display().to_string()
}

/// The copy of `source` a lookup may use right now.
///
/// Fresh means "the copy was read from the file this source still points at,
/// unchanged" (same path, mtime and size). A stale (or missing) copy is re-read
/// inline: a local read is cheap and is not network I/O, so a lookup never has
/// to wait for a background refresh. A file that cannot be read is skipped for
/// the failure backoff rather than retried on every lookup.
fn usable_catalog(source: &PricingSource) -> Option<Arc<SourcesCache>> {
    let cache = load_cache();
    if let Some(catalog) = cache.sources.get(&source.id)
        && catalog.location == location_key(&source.path)
        && is_fresh(source, catalog)
    {
        return Some(cache);
    }

    if in_backoff(&source.id) {
        return None;
    }

    match read_local_sheet_fingerprinted(&source.path) {
        Ok((fingerprint, providers)) => {
            clear_failure(&source.id);
            save_catalog(&source.id, &source.path, Some(fingerprint), providers);
            Some(load_cache())
        }
        Err(error) => {
            note_failure(&source.id);
            crate::logging::warn(&format!(
                "pricing source `{}` ({}) is unavailable: {error}; \
                 falling through to the next price source",
                source.id,
                source.path.display()
            ));
            None
        }
    }
}

/// Whether a cached copy still describes the file the source points at.
fn is_fresh(source: &PricingSource, catalog: &SourceCatalog) -> bool {
    local_fingerprint(&source.path).is_some_and(|current| catalog.fingerprint == Some(current))
}

/// The fingerprint of the file at `path`, or `None` when it cannot be stat'ed
/// (missing, unreadable, or gone).
fn local_fingerprint(path: &std::path::Path) -> Option<FileFingerprint> {
    std::fs::metadata(path)
        .ok()
        .map(|meta| FileFingerprint::of(&meta))
}

/// How long a source that just failed is left alone before being tried again.
///
/// The route catalog asks for a price once per route, so "read the file again
/// on every lookup" turns one missing file into thousands of syscalls and log
/// lines. A minute is short enough that fixing the file is noticed quickly, and
/// long enough that a broken source costs one attempt per minute instead of one
/// per price.
///
/// Only *failures* are recorded: a source that succeeds is governed by its
/// file's fingerprint, so an edit is read at the next lookup rather than after
/// the backoff.
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

/// A sheet's parsed providers paired with the fingerprint of the bytes read.
type FingerprintedSheet = (
    FileFingerprint,
    HashMap<String, HashMap<String, ModelPricingEntry>>,
);

/// Test-only thin wrapper over [`read_local_sheet_fingerprinted`] that drops the
/// fingerprint.
///
/// Production callers go through [`read_local_sheet_fingerprinted`] directly
/// because they need the fingerprint to decide whether a cached copy is still
/// fresh; the tests that only care about parse/size behaviour use this.
#[cfg(test)]
pub(super) fn read_local_sheet(
    path: &std::path::Path,
) -> anyhow::Result<HashMap<String, HashMap<String, ModelPricingEntry>>> {
    read_local_sheet_fingerprinted(path).map(|(_, providers)| providers)
}

/// Read one sheet from a local path, enforcing the size ceiling, and also
/// return the fingerprint of the bytes it read.
///
/// The `stat` runs before the `open` on purpose: a path can name a FIFO, and
/// opening one with no writer blocks a price lookup forever. A non-regular file
/// is refused outright, and the size is checked from metadata and again while
/// reading (`take`) so an oversized file is never fully buffered.
///
/// The fingerprint comes from the same `stat` that gates the open, so a copy is
/// recorded as "this exact version of the file" without a second syscall. It is
/// taken *before* the read: an edit that lands mid-read leaves the recorded
/// fingerprint behind the file, so the next lookup re-reads rather than trusting
/// a half-old copy.
fn read_local_sheet_fingerprinted(path: &std::path::Path) -> anyhow::Result<FingerprintedSheet> {
    use std::io::Read as _;

    #[cfg(test)]
    LOCAL_SHEET_READS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let metadata = std::fs::metadata(path)
        .map_err(|error| anyhow::anyhow!("cannot stat {}: {error}", path.display()))?;
    if !metadata.is_file() {
        anyhow::bail!("{} is not a regular file", path.display());
    }
    if metadata.len() > MAX_SHEET_BYTES as u64 {
        anyhow::bail!("sheet is larger than {MAX_SHEET_BYTES} bytes");
    }
    let fingerprint = FileFingerprint::of(&metadata);
    let file = std::fs::File::open(path)
        .map_err(|error| anyhow::anyhow!("cannot read {}: {error}", path.display()))?;
    let mut body = String::new();
    file.take((MAX_SHEET_BYTES + 1) as u64)
        .read_to_string(&mut body)
        .map_err(|error| anyhow::anyhow!("cannot read {}: {error}", path.display()))?;
    Ok((fingerprint, parse_sheet(&body)?))
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
    //
    // Two keys can both match (`claude` and `claude-api` both map to models.dev
    // `anthropic`), so this picks the lexicographically smallest instead of
    // whichever `HashMap` iteration reaches first: the config layer's own
    // provider map is a `BTreeMap`, and "which section prices the model" must
    // not depend on hash order (the module doc's determinism promise).
    source
        .providers
        .keys()
        .filter(|key| super::sources::provider_key_matches(key, source_key))
        .min()
        .and_then(|key| source.providers.get(key))
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
/// Only ever called with a successfully parsed sheet, so a failed read leaves
/// the last good copy exactly as it was.
///
/// The whole read-modify-write runs under [`SOURCES_WRITE_LOCK`], and the
/// in-memory `Arc` is the base when it is already for this path. Without that,
/// two different sources' reads could interleave (`read{}`, `read{}`, `mem={A}`,
/// `mem={B}`, `write{A}`, `write{B}`) and drop one sheet from disk *and* memory;
/// the next lookup would re-read it, and in the meantime a call could be priced
/// from the wrong layer.
///
/// This is in-process only: a second process sharing the cache file can still
/// race this one and win, and the loser's sheet is simply re-read on its next
/// stale lookup, so the state converges rather than corrupts.
pub(super) fn save_catalog(
    id: &str,
    path: &Path,
    fingerprint: Option<FileFingerprint>,
    providers: HashMap<String, HashMap<String, ModelPricingEntry>>,
) {
    let cache_file = cache_path();
    let location = location_key(path);
    // Serialize the entire read-modify-write, not just the memory assignment.
    let _guard = SOURCES_WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut cache: SourcesCache = match SOURCES_CACHE.lock() {
        Ok(memory) => match memory.as_ref() {
            // The in-memory copy is the freshest in-process state for this path
            // (every writer holds the same lock), so base the update on it.
            Some((cached_path, cache)) if cached_path == &cache_file => (**cache).clone(),
            _ => crate::storage::read_json(&cache_file).unwrap_or_default(),
        },
        Err(_) => crate::storage::read_json(&cache_file).unwrap_or_default(),
    };
    cache.schema_version = SOURCES_SCHEMA_VERSION;
    let changed = cache
        .sources
        .get(id)
        .map(|existing| existing.providers != providers)
        .unwrap_or(true);
    cache.sources.insert(
        id.to_string(),
        SourceCatalog {
            location,
            fingerprint,
            providers,
        },
    );
    let cache = Arc::new(cache);
    if let Ok(mut memory) = SOURCES_CACHE.lock() {
        *memory = Some((cache_file.clone(), Arc::clone(&cache)));
    }
    if let Err(error) = crate::storage::write_json(&cache_file, cache.as_ref()) {
        crate::logging::warn(&format!(
            "could not persist pricing sources cache: {error:#}"
        ));
    }
    drop(_guard);
    if changed {
        // The rates a lookup would resolve just changed, and the memos
        // downstream (route catalog, TUI per-model price) decide freshness from
        // this counter, so a refresh takes effect on the next read instead of
        // whenever something else happens to invalidate them.
        super::generation::bump_pricing_generation();
    }
}

#[cfg(test)]
pub(crate) fn save_test_source(id: &str, path: &Path, entries: &[(&str, &str, ModelPricingEntry)]) {
    let mut providers: HashMap<String, HashMap<String, ModelPricingEntry>> = HashMap::new();
    for (provider, model, entry) in entries {
        providers
            .entry((*provider).to_string())
            .or_default()
            .insert((*model).to_string(), entry.clone());
    }
    // A primed source has to look like one this process just read, or the mtime
    // check would treat it as stale and re-read the (usually absent) file. The
    // fingerprint comes from the file the caller points at, so priming a source
    // means writing that file.
    let fingerprint = std::fs::metadata(path)
        .ok()
        .map(|metadata| FileFingerprint::of(&metadata));
    let cache_file = cache_path();
    let mut cache: SourcesCache = crate::storage::read_json(&cache_file).unwrap_or_default();
    cache.sources.insert(
        id.to_string(),
        SourceCatalog {
            location: location_key(path),
            fingerprint,
            providers,
        },
    );
    let cache = Arc::new(cache);
    if let Ok(mut memory) = SOURCES_CACHE.lock() {
        *memory = Some((cache_file.clone(), Arc::clone(&cache)));
    }
    if let Err(error) = crate::storage::write_json(&cache_file, cache.as_ref()) {
        panic!("could not persist test sources cache: {error:#}");
    }
}

#[cfg(test)]
pub(crate) fn cached_source_ids_for_tests() -> Vec<String> {
    let mut ids: Vec<String> = load_cache().sources.keys().cloned().collect();
    ids.sort();
    ids
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

/// How many local sheet files have been opened since the counter was reset, so
/// a test can prove an unchanged file is stat'ed rather than re-read.
#[cfg(test)]
pub(crate) fn local_sheet_reads_for_tests() -> usize {
    LOCAL_SHEET_READS.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
pub(crate) fn reset_local_sheet_reads_for_tests() {
    LOCAL_SHEET_READS.store(0, std::sync::atomic::Ordering::Relaxed);
}

/// What the cache recorded as the file fingerprint for `id`, for tests that
/// assert a copy is bound to the version of the file it was read from.
#[cfg(test)]
pub(crate) fn cached_fingerprint_for_tests(id: &str) -> Option<(u64, u64)> {
    load_cache()
        .sources
        .get(id)
        .and_then(|catalog| catalog.fingerprint)
        .map(|fingerprint| (fingerprint.mtime_nanos, fingerprint.size))
}
