//! Per-vendor price files: the `[pricing.providers.<vendor>].file` layer that
//! sits between the user's own inline cards and jcode's derived chain.
//!
//! A vendor file is a local JSON document stating rate rules for model ids:
//!
//! ```json
//! {"models": {"deepseek-v4-pro": {"cost": {"input": 2.0, "output": 8.0}}}}
//! ```
//!
//! There is no outer vendor key: the vendor is the `[pricing.providers.<vendor>]`
//! config key this file hangs under (see `jcode_config_types::ProviderPricingFile`).
//! Rules are matched by **model id regardless of route**, which is what lets a
//! `provider = OpenRouter, model = deepseek-flash` call be priced from DeepSeek's
//! official CNY numbers instead of silently falling back to models.dev's USD ones.
//!
//! Three properties are deliberate and are what its tests pin down:
//!
//! * **Lookups never block.** A vendor file is read inline, because a local read
//!   is not network I/O. There is no fetch path at all.
//! * **A file is fresh while it is unchanged.** The cached copy records the
//!   file's mtime and size; an edit makes the copy stale immediately, so "my
//!   price file" means "saving it changes the price". The check costs one `stat`
//!   per lookup (never a re-read), and a file that is missing or unreadable is
//!   left alone for a short failure backoff so a broken file costs one attempt
//!   per window, not one per lookup.
//! * **A file never fabricates a rate.** Missing, unreadable, unparseable,
//!   empty, or out of effect all mean "this file has nothing to say", so the
//!   next layer prices the model. Degradation is per lookup.

use crate::model_pricing::entry::ModelPricingEntry;
use crate::model_pricing::{catalog, normalize_model_id};
use jcode_config_types::ModelPricingRuleFile;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Where the cached vendor files live. One file for every vendor, so a vendor
/// whose `file` is removed from the config simply stops being read.
///
/// The name is historical (it held `[[pricing.sources]]` sheets before this
/// layer became per-vendor). It is kept so the config layer's bare-name guard
/// list stays stable and an old cache is simply read as empty and rewritten in
/// the new `vendors` shape.
pub(crate) const VENDOR_FILES_CACHE_FILE: &str = "pricing_sources.json";
const VENDOR_FILES_SCHEMA_VERSION: u32 = 1;

/// A price file that decides what money is spent does not get read without a
/// bound; this is the same ceiling models.dev's own catalog comfortably fits in.
const MAX_VENDOR_FILE_BYTES: usize = 32 * 1024 * 1024;
/// The bound above, exposed to the tests that build a body just past it.
#[cfg(test)]
pub(super) const MAX_VENDOR_FILE_BYTES_TEST: usize = MAX_VENDOR_FILE_BYTES;

/// How many times a vendor file was actually opened, for the test that pins
/// "an unchanged file is stat'ed but not re-read".
#[cfg(test)]
static LOCAL_FILE_READS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// One read vendor file, plus the file it was read from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct VendorFileCatalog {
    /// The path this copy came from. A config edit that points the same vendor
    /// somewhere else must not keep serving the old file, so a mismatch is
    /// treated as "no copy yet".
    pub(super) location: String,
    /// The local file this copy was read from, identified by mtime and size.
    ///
    /// A copy is fresh exactly while this matches the file on disk, which is why
    /// an edit takes effect at the next lookup. `None` is treated as stale once,
    /// re-read, and fingerprinted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) fingerprint: Option<FileFingerprint>,
    /// model id -> entry, exactly the shape a hand-written card resolves to.
    pub(super) models: BTreeMap<String, ModelPricingEntry>,
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
pub(super) struct VendorFilesCache {
    #[serde(default)]
    pub(super) schema_version: u32,
    #[serde(default)]
    pub(super) vendors: HashMap<String, VendorFileCatalog>,
}

impl Default for VendorFilesCache {
    fn default() -> Self {
        Self {
            schema_version: VENDOR_FILES_SCHEMA_VERSION,
            vendors: HashMap::new(),
        }
    }
}

static VENDOR_FILES_CACHE: Mutex<Option<(PathBuf, Arc<VendorFilesCache>)>> = Mutex::new(None);
/// Serializes the read-modify-write of the on-disk/in-memory vendor files cache.
///
/// [`VENDOR_FILES_CACHE`]'s own lock only covers the memory assignment, so
/// without this two different vendors' reads could interleave and drop a file.
/// It is deliberately a separate lock from [`VENDOR_FILES_CACHE`] because
/// `save_catalog` takes both, in this order, and nothing takes them reversed.
static VENDOR_FILES_WRITE_LOCK: Mutex<()> = Mutex::new(());

/// The rule a vendor file states for `model`, if the file is readable and names
/// it.
///
/// `None` means the file has nothing usable to say about the model (missing,
/// unreadable, unparseable, oversized, or the model is simply absent). The
/// caller then asks the next layer, which is the whole point: a file never turns
/// "I do not know" into a number.
pub(super) fn vendor_rule(vendor: &str, path: &Path, model: &str) -> Option<ModelPricingEntry> {
    let cache = usable_catalog(vendor, path)?;
    let catalog = cache.vendors.get(vendor)?;
    let normalized = normalize_model_id(model);
    if let Some(entry) = catalog.models.get(normalized) {
        return Some(entry.clone());
    }
    // OpenRouter-style ids may still carry their `provider/` prefix.
    if let Some((_, bare)) = normalized.rsplit_once('/') {
        return catalog.models.get(bare).cloned();
    }
    None
}

/// The path string a cache entry records for `path`, and the form compared to
/// decide whether a cached copy still describes the vendor's file.
fn location_key(path: &Path) -> String {
    path.display().to_string()
}

/// The copy of the vendor's file a lookup may use right now.
///
/// Fresh means "the copy was read from the file this vendor still points at,
/// unchanged" (same path, mtime and size). A stale (or missing) copy is re-read
/// inline: a local read is cheap and is not network I/O, so a lookup never has
/// to wait for a background refresh. A file that cannot be read is skipped for
/// the failure backoff rather than retried on every lookup.
fn usable_catalog(vendor: &str, path: &Path) -> Option<Arc<VendorFilesCache>> {
    let cache = load_cache();
    if let Some(catalog) = cache.vendors.get(vendor)
        && catalog.location == location_key(path)
        && is_fresh(path, catalog)
    {
        return Some(cache);
    }

    if in_backoff(vendor) {
        return None;
    }

    match read_local_file_fingerprinted(path) {
        Ok((fingerprint, models)) => {
            clear_failure(vendor);
            save_catalog(vendor, path, Some(fingerprint), models);
            Some(load_cache())
        }
        Err(error) => {
            note_failure(vendor);
            crate::logging::warn(&format!(
                "pricing provider `{vendor}` file ({}) is unavailable: {error}; \
                 falling through to the next price source",
                path.display()
            ));
            None
        }
    }
}

/// Whether a cached copy still describes the file the vendor points at.
fn is_fresh(path: &Path, catalog: &VendorFileCatalog) -> bool {
    local_fingerprint(path).is_some_and(|current| catalog.fingerprint == Some(current))
}

/// The fingerprint of the file at `path`, or `None` when it cannot be stat'ed
/// (missing, unreadable, or gone).
fn local_fingerprint(path: &Path) -> Option<FileFingerprint> {
    std::fs::metadata(path)
        .ok()
        .map(|meta| FileFingerprint::of(&meta))
}

/// How long a vendor whose file just failed is left alone before being tried
/// again.
///
/// The route catalog asks for a price once per route, so "read the file again
/// on every lookup" turns one missing file into thousands of syscalls and log
/// lines. A minute is short enough that fixing the file is noticed quickly, and
/// long enough that a broken file costs one attempt per minute instead of one
/// per price.
///
/// Only *failures* are recorded: a file that succeeds is governed by its
/// fingerprint, so an edit is read at the next lookup rather than after the
/// backoff.
const FAILURE_BACKOFF_SECS: u64 = 60;

/// When each vendor last failed, for [`FAILURE_BACKOFF_SECS`]. Process local:
/// it exists to bound work, not to remember anything.
static VENDOR_FAILURES: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

/// True while a vendor that recently failed is being left alone.
fn in_backoff(vendor: &str) -> bool {
    let Ok(failures) = VENDOR_FAILURES.lock() else {
        return false;
    };
    let Some(failures) = failures.as_ref() else {
        return false;
    };
    match failures.get(vendor) {
        Some(failed_at) => {
            catalog::now_unix_secs().saturating_sub(*failed_at) < FAILURE_BACKOFF_SECS
        }
        None => false,
    }
}

fn note_failure(vendor: &str) {
    let Ok(mut failures) = VENDOR_FAILURES.lock() else {
        return;
    };
    let failures = failures.get_or_insert_with(HashMap::new);
    failures.insert(vendor.to_string(), catalog::now_unix_secs());
}

fn clear_failure(vendor: &str) {
    let Ok(mut failures) = VENDOR_FAILURES.lock() else {
        return;
    };
    if let Some(failures) = failures.as_mut() {
        failures.remove(vendor);
    }
}

/// A vendor file's parsed models paired with the fingerprint of the bytes read.
type FingerprintedFile = (FileFingerprint, BTreeMap<String, ModelPricingEntry>);

/// Test-only thin wrapper over [`read_local_file_fingerprinted`] that drops the
/// fingerprint.
///
/// Production callers go through [`read_local_file_fingerprinted`] directly
/// because they need the fingerprint to decide whether a cached copy is still
/// fresh; the tests that only care about parse/size behaviour use this.
#[cfg(test)]
pub(super) fn read_local_file(path: &Path) -> anyhow::Result<BTreeMap<String, ModelPricingEntry>> {
    read_local_file_fingerprinted(path).map(|(_, models)| models)
}

/// Read one vendor file from a local path, enforcing the size ceiling, and also
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
fn read_local_file_fingerprinted(path: &Path) -> anyhow::Result<FingerprintedFile> {
    use std::io::Read as _;

    #[cfg(test)]
    LOCAL_FILE_READS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let metadata = std::fs::metadata(path)
        .map_err(|error| anyhow::anyhow!("cannot stat {}: {error}", path.display()))?;
    if !metadata.is_file() {
        anyhow::bail!("{} is not a regular file", path.display());
    }
    if metadata.len() > MAX_VENDOR_FILE_BYTES as u64 {
        anyhow::bail!("file is larger than {MAX_VENDOR_FILE_BYTES} bytes");
    }
    let fingerprint = FileFingerprint::of(&metadata);
    let file = std::fs::File::open(path)
        .map_err(|error| anyhow::anyhow!("cannot read {}: {error}", path.display()))?;
    let mut body = String::new();
    file.take((MAX_VENDOR_FILE_BYTES + 1) as u64)
        .read_to_string(&mut body)
        .map_err(|error| anyhow::anyhow!("cannot read {}: {error}", path.display()))?;
    Ok((fingerprint, parse_vendor_file(&body, path)?))
}

/// Parse a vendor file body into `model id -> entry`.
///
/// The only accepted shape is `{"models": {"<model-id>": <rule>}}`; the old
/// vendor-keyed shape (`{"deepseek": {"models": {...}}}`) is rejected with a
/// message that says what to remove. A rule is validated by the config layer's
/// own `convert_rule`, so a file rule and a hand-written card accept exactly the
/// same shape and produce the same errors.
fn parse_vendor_file(
    body: &str,
    path: &Path,
) -> anyhow::Result<BTreeMap<String, ModelPricingEntry>> {
    if body.len() > MAX_VENDOR_FILE_BYTES {
        anyhow::bail!("file is larger than {MAX_VENDOR_FILE_BYTES} bytes");
    }

    #[derive(Deserialize)]
    struct VendorFileDoc {
        models: BTreeMap<String, ModelPricingRuleFile>,
    }

    let doc: VendorFileDoc = match serde_json::from_str(body) {
        Ok(doc) => doc,
        Err(error) => {
            if let Some(vendor) = legacy_outer_vendor_key(body) {
                anyhow::bail!(
                    "expected {{\"models\": {{...}}}} with no outer provider key, but the file \
                     has an outer key `{vendor}`; the vendor is already the \
                     `[pricing.providers.{vendor}]` config key, so remove that key from the file"
                );
            }
            return Err(anyhow::anyhow!("{error}"));
        }
    };

    let mut models = BTreeMap::new();
    for (id, rule) in &doc.models {
        let rule_path = format!("{}.models.{id}", path.display());
        let converted = crate::config::pricing::convert_rule(rule, &rule_path)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        models.insert(id.clone(), ModelPricingEntry::from_rule(&converted));
    }
    Ok(models)
}

/// The outer key of a legacy vendor-keyed file (`{"deepseek": {"models": ...}}`),
/// if the body has that shape.
///
/// Exists only to make the migration error message name the key to remove; a
/// body that is not a JSON object, or that already has a top-level `models`, is
/// not the legacy shape.
fn legacy_outer_vendor_key(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let object = value.as_object()?;
    if object.contains_key("models") {
        return None;
    }
    object.iter().find_map(|(key, value)| {
        value
            .as_object()
            .and_then(|inner| inner.contains_key("models").then(|| key.clone()))
    })
}

fn cache_path() -> PathBuf {
    crate::storage::jcode_dir()
        .unwrap_or_else(|_| PathBuf::from(".").join(".jcode"))
        .join("cache")
        .join(VENDOR_FILES_CACHE_FILE)
}

fn load_cache() -> Arc<VendorFilesCache> {
    let path = cache_path();
    if let Ok(memory) = VENDOR_FILES_CACHE.lock()
        && let Some((cached_path, cache)) = memory.as_ref()
        && cached_path == &path
    {
        return Arc::clone(cache);
    }
    let cache: VendorFilesCache = crate::storage::read_json(&path).unwrap_or_default();
    let cache = Arc::new(cache);
    if let Ok(mut memory) = VENDOR_FILES_CACHE.lock() {
        *memory = Some((path, Arc::clone(&cache)));
    }
    cache
}

/// Store a freshly read file, replacing whatever was there.
///
/// Only ever called with a successfully parsed file, so a failed read leaves the
/// last good copy exactly as it was.
///
/// The whole read-modify-write runs under [`VENDOR_FILES_WRITE_LOCK`], and the
/// in-memory `Arc` is the base when it is already for this path. Without that,
/// two different vendors' reads could interleave (`read{}`, `read{}`, `mem={A}`,
/// `mem={B}`, `write{A}`, `write{B}`) and drop one file from disk *and* memory;
/// the next lookup would re-read it, and in the meantime a call could be priced
/// from the wrong layer.
///
/// This is in-process only: a second process sharing the cache file can still
/// race this one and win, and the loser's file is simply re-read on its next
/// stale lookup, so the state converges rather than corrupts.
pub(super) fn save_catalog(
    vendor: &str,
    path: &Path,
    fingerprint: Option<FileFingerprint>,
    models: BTreeMap<String, ModelPricingEntry>,
) {
    let cache_file = cache_path();
    let location = location_key(path);
    // Serialize the entire read-modify-write, not just the memory assignment.
    let _guard = VENDOR_FILES_WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut cache: VendorFilesCache = match VENDOR_FILES_CACHE.lock() {
        Ok(memory) => match memory.as_ref() {
            // The in-memory copy is the freshest in-process state for this path
            // (every writer holds the same lock), so base the update on it.
            Some((cached_path, cache)) if cached_path == &cache_file => (**cache).clone(),
            _ => crate::storage::read_json(&cache_file).unwrap_or_default(),
        },
        Err(_) => crate::storage::read_json(&cache_file).unwrap_or_default(),
    };
    cache.schema_version = VENDOR_FILES_SCHEMA_VERSION;
    let changed = cache
        .vendors
        .get(vendor)
        .map(|existing| existing.models != models)
        .unwrap_or(true);
    cache.vendors.insert(
        vendor.to_string(),
        VendorFileCatalog {
            location,
            fingerprint,
            models,
        },
    );
    let cache = Arc::new(cache);
    if let Ok(mut memory) = VENDOR_FILES_CACHE.lock() {
        *memory = Some((cache_file.clone(), Arc::clone(&cache)));
    }
    if let Err(error) = crate::storage::write_json(&cache_file, cache.as_ref()) {
        crate::logging::warn(&format!(
            "could not persist pricing provider files cache: {error:#}"
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
pub(crate) fn save_test_vendor(vendor: &str, path: &Path, entries: &[(&str, ModelPricingEntry)]) {
    let mut models: BTreeMap<String, ModelPricingEntry> = BTreeMap::new();
    for (model, entry) in entries {
        models.insert((*model).to_string(), entry.clone());
    }
    // A primed vendor has to look like one this process just read, or the mtime
    // check would treat it as stale and re-read the (usually absent) file. The
    // fingerprint comes from the file the caller points at, so priming a vendor
    // means writing that file.
    let fingerprint = std::fs::metadata(path)
        .ok()
        .map(|metadata| FileFingerprint::of(&metadata));
    let cache_file = cache_path();
    let mut cache: VendorFilesCache = crate::storage::read_json(&cache_file).unwrap_or_default();
    cache.vendors.insert(
        vendor.to_string(),
        VendorFileCatalog {
            location: location_key(path),
            fingerprint,
            models,
        },
    );
    let cache = Arc::new(cache);
    if let Ok(mut memory) = VENDOR_FILES_CACHE.lock() {
        *memory = Some((cache_file.clone(), Arc::clone(&cache)));
    }
    if let Err(error) = crate::storage::write_json(&cache_file, cache.as_ref()) {
        panic!("could not persist test vendor files cache: {error:#}");
    }
}

#[cfg(test)]
pub(crate) fn cached_vendor_ids_for_tests() -> Vec<String> {
    let mut ids: Vec<String> = load_cache().vendors.keys().cloned().collect();
    ids.sort();
    ids
}

#[cfg(test)]
pub(crate) fn clear_vendor_files_cache_for_tests() {
    if let Ok(mut memory) = VENDOR_FILES_CACHE.lock() {
        *memory = None;
    }
    if let Ok(mut failures) = VENDOR_FAILURES.lock() {
        *failures = None;
    }
    let _ = std::fs::remove_file(cache_path());
}

/// Whether `vendor` is currently in its failure backoff, for the backoff test.
#[cfg(test)]
pub(crate) fn in_backoff_for_tests(vendor: &str) -> bool {
    in_backoff(vendor)
}

/// Forget every recorded failure, i.e. pretend the backoff window elapsed.
#[cfg(test)]
pub(crate) fn forget_failures_for_tests() {
    if let Ok(mut failures) = VENDOR_FAILURES.lock() {
        *failures = None;
    }
}

/// How many vendor files have been opened since the counter was reset, so a test
/// can prove an unchanged file is stat'ed rather than re-read.
#[cfg(test)]
pub(crate) fn local_file_reads_for_tests() -> usize {
    LOCAL_FILE_READS.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
pub(crate) fn reset_local_file_reads_for_tests() {
    LOCAL_FILE_READS.store(0, std::sync::atomic::Ordering::Relaxed);
}

/// What the cache recorded as the file fingerprint for `vendor`, for tests that
/// assert a copy is bound to the version of the file it was read from.
#[cfg(test)]
pub(crate) fn cached_fingerprint_for_tests(vendor: &str) -> Option<(u64, u64)> {
    load_cache()
        .vendors
        .get(vendor)
        .and_then(|catalog| catalog.fingerprint)
        .map(|fingerprint| (fingerprint.mtime_nanos, fingerprint.size))
}
