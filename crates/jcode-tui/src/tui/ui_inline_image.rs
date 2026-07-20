//! Inline image transcript section.
//!
//! Images attached to the conversation (pasted screenshots, `read` of an image
//! file, generated images) render directly in the chat flow, sized to fit the
//! chat width with a capped height. This replaces the old "pinned image side
//! panel" surface.
//!
//! Design goals:
//! * **Lazy.** Prepare only needs each image's `(id, width, height)`, obtained
//!   from a cheap header parse (no full decode, no disk write, no retained
//!   bytes). The full decode + terminal transmit happens at draw time, and only
//!   for images currently on screen.
//! * **Single source of pixels.** The base64 payloads stay in their existing
//!   owner (`App::side_pane_images()`); this section keeps only ids and a small
//!   ingest-time payload registry so the draw step can materialize on demand.
//! * **Correct fit.** Images scale to fit width (preserving aspect) and cap at a
//!   fraction of the viewport so a tall screenshot never buries the transcript.

use crate::tui::mermaid;
use jcode_tui_messages::{ImageRegion, ImageRegionRender, PreparedMessages};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock, mpsc};

/// One image to render inline, resolved from a `RenderedImage`.
#[derive(Clone)]
pub(crate) struct InlineImageItem {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub label: String,
    pub uses_text_fallback: bool,
}

/// Cap an inline image at this fraction of the chat viewport height so a tall
/// image cannot push the rest of the transcript off-screen.
const MAX_VIEWPORT_FRACTION_PERCENT: u16 = 55;
/// Never shrink an inline image below this many rows.
const MIN_IMAGE_ROWS: u16 = 3;
/// Fixed row cap for images anchored inside the transcript body. The body is
/// prepared and cached independently of the viewport height, so anchored
/// placeholder geometry must not depend on it; a fixed cap keeps tall
/// screenshots from dominating the flow while staying resize-stable.
const ANCHORED_MAX_ROWS: u16 = 16;

/// Discrete per-image expand levels. Clicking the `expand` badge cycles an
/// image through these caps. The caps are *fixed row counts* (not a fraction of
/// the viewport) on purpose: anchored placeholder geometry feeds the body cache
/// which is keyed by width only, so the expand level must stay viewport
/// independent or it would break that invariant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ImageExpandLevel {
    /// Default fit-to-flow size (`ANCHORED_MAX_ROWS`).
    #[default]
    Fit,
    /// Roughly 2.5x taller, for a closer look without leaving the transcript.
    Large,
    /// Effectively uncapped height: tall enough that virtually every image is
    /// width-bound, i.e. rendered at the largest size the chat pane allows.
    Full,
}

impl ImageExpandLevel {
    /// Next level in the click cycle (Fit -> Large -> Full -> Fit).
    pub(crate) fn next(self) -> Self {
        match self {
            ImageExpandLevel::Fit => ImageExpandLevel::Large,
            ImageExpandLevel::Large => ImageExpandLevel::Full,
            ImageExpandLevel::Full => ImageExpandLevel::Fit,
        }
    }

    /// Anchored row cap for this level. Stays viewport independent so the
    /// width-keyed body cache remains valid across resizes. The `Full` cap is
    /// bounded by kitty's virtual-placement row limit (296 diacritic slots),
    /// with margin, so stable fit rendering keeps working at every level.
    fn anchored_cap_rows(self) -> u16 {
        match self {
            ImageExpandLevel::Fit => ANCHORED_MAX_ROWS,
            ImageExpandLevel::Large => 40,
            ImageExpandLevel::Full => 200,
        }
    }
}

/// Resolve the expand level for an image id. Implemented by `App` so the lookup
/// stays close to the persisted/live state, while this module owns the geometry.
pub(crate) trait ImageExpandLevels {
    fn expand_level(&self, id: u64) -> ImageExpandLevel;
}

/// A levels source that reports every image as `Fit`. Used by tests that
/// exercise section/line building without an `App` to resolve expand state.
#[cfg(test)]
pub(crate) struct AllFit;
#[cfg(test)]
impl ImageExpandLevels for AllFit {
    fn expand_level(&self, _id: u64) -> ImageExpandLevel {
        ImageExpandLevel::Fit
    }
}

/// Adapter so prepare code can resolve per-image expand levels straight from the
/// app state without copying the whole map into this module.
pub(crate) struct AppExpandLevels<'a>(pub &'a dyn crate::tui::TuiState);
impl ImageExpandLevels for AppExpandLevels<'_> {
    fn expand_level(&self, id: u64) -> ImageExpandLevel {
        self.0.image_expand_level(id)
    }
}

/// Ingest-time registry mapping image id -> (media_type, base64) so the draw
/// step can materialize bytes without threading payloads through the cached
/// prepared-frame model. Bounded; entries are cheap (two `String`s + id).
static PAYLOAD_REGISTRY: LazyLock<Mutex<PayloadRegistry>> =
    LazyLock::new(|| Mutex::new(PayloadRegistry::new()));

/// Payloads can be dropped when images are hidden or when the staging byte
/// budget is exceeded. Prepared frames intentionally retain only image ids, so
/// a later draw asks the next prepare pass to restage the missing source from the
/// App's canonical image list instead of leaving a cold image blank forever.
static PAYLOAD_RESTAGE_IDS: LazyLock<Mutex<HashSet<u64>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));
static PAYLOAD_RESTAGE_PENDING: AtomicBool = AtomicBool::new(false);
static PAYLOAD_RESTAGE_ALL: AtomicBool = AtomicBool::new(false);
const PAYLOAD_RESTAGE_MAX: usize = 512;

const PAYLOAD_REGISTRY_MAX: usize = 512;
/// Byte budget for the payload registry. Entries hold the *full base64
/// payload* (a 5 MB screenshot is ~6.7 MB of base64), so a pure entry-count
/// bound could still pin gigabytes of RAM across a screenshot-heavy session.
/// Evicted payloads are re-registered by the next prepare pass that resolves
/// the image, so the only cost of a tight budget is a string clone later.
const PAYLOAD_REGISTRY_MAX_BYTES: usize = 64 * 1024 * 1024;

struct PayloadRegistry {
    map: std::collections::HashMap<u64, (String, String)>,
    order: std::collections::VecDeque<u64>,
    total_bytes: usize,
}

impl PayloadRegistry {
    fn new() -> Self {
        Self {
            map: std::collections::HashMap::new(),
            order: std::collections::VecDeque::new(),
            total_bytes: 0,
        }
    }

    /// Insert a payload. Returns true when the id was newly inserted (false
    /// when it was already registered).
    fn insert(&mut self, id: u64, media_type: &str, data_b64: &str) -> bool {
        if self.map.contains_key(&id) {
            return false;
        }
        self.total_bytes = self
            .total_bytes
            .saturating_add(media_type.len() + data_b64.len());
        self.map
            .insert(id, (media_type.to_string(), data_b64.to_string()));
        self.order.push_back(id);
        // Evict oldest-first past either bound, but never the entry just
        // inserted: a single over-budget payload must stay resident or its
        // image could never materialize.
        while (self.order.len() > PAYLOAD_REGISTRY_MAX
            || self.total_bytes > PAYLOAD_REGISTRY_MAX_BYTES)
            && self.order.len() > 1
        {
            if let Some(old) = self.order.pop_front()
                && let Some((media_type, data_b64)) = self.map.remove(&old)
            {
                self.total_bytes = self
                    .total_bytes
                    .saturating_sub(media_type.len() + data_b64.len());
            }
        }
        true
    }

    fn get(&self, id: u64) -> Option<(String, String)> {
        self.map.get(&id).cloned()
    }

    fn remove(&mut self, id: u64) {
        if let Some((media_type, data_b64)) = self.map.remove(&id) {
            self.total_bytes = self
                .total_bytes
                .saturating_sub(media_type.len() + data_b64.len());
            if let Some(pos) = self.order.iter().position(|entry| *entry == id) {
                self.order.remove(pos);
            }
        }
    }

    fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
        self.total_bytes = 0;
    }
}

/// Record an image payload so [`materialize_visible`] can decode it on demand.
///
/// Skipped entirely for images that are already materialized: their decoded
/// bytes live in the render cache (memory) and cache dir (disk), so staging
/// the base64 copy again would just hold multi-megabyte payloads resident
/// twice. [`materialize_visible`] rediscovers evicted entries from disk.
pub(crate) fn register_payload(id: u64, media_type: &str, data_b64: &str) {
    if let Ok(mut requested) = PAYLOAD_RESTAGE_IDS.lock() {
        requested.remove(&id);
        PAYLOAD_RESTAGE_PENDING.store(!requested.is_empty(), Ordering::Release);
    }
    if mermaid::inline_image_is_materialized(id) {
        return;
    }
    let newly_inserted = match PAYLOAD_REGISTRY.lock() {
        Ok(mut reg) => reg.insert(id, media_type, data_b64),
        Err(_) => false,
    };
    // A fresh payload may succeed where a previously evicted/corrupt one
    // failed, so give the prewarm pipeline its retries back.
    if newly_inserted {
        // The id hashes the complete payload, so re-registering the same id is
        // not fresh content. Keep ID-wide decode failures capped across staging
        // eviction; geometry preparation may still be retried.
        if let Ok(mut failures) = PREWARM_FIT_FAILURES.lock() {
            failures.retain(|req, _| req.id != id);
        }
    }
}

/// Drop the staged base64 payload for an image whose decoded bytes are now
/// persisted in the render cache; see [`register_payload`].
fn release_payload(id: u64) {
    if let Ok(mut reg) = PAYLOAD_REGISTRY.lock() {
        reg.remove(id);
    }
}

fn clear_staged_payloads() {
    let mut cleared = false;
    if let Ok(mut reg) = PAYLOAD_REGISTRY.lock() {
        cleared |= !reg.map.is_empty();
        reg.clear();
    }
    // `pin_images == false` returns before computing a replacement cache key.
    // Without invalidating this cache, re-enabling images can hit an old
    // stage_payloads=true result even though its payload registry was cleared,
    // permanently leaving unmaterialized images with nothing to decode.
    if let Ok(mut cache) = ANCHORED_CACHE.lock() {
        cleared |= cache.take().is_some();
    }
    if cleared {
        PAYLOAD_RESTAGE_ALL.store(true, Ordering::Release);
    }
}

fn request_payload_restage(id: u64) {
    let mut newly_requested = false;
    if let Ok(mut requested) = PAYLOAD_RESTAGE_IDS.lock() {
        if requested.len() >= PAYLOAD_RESTAGE_MAX && !requested.contains(&id) {
            requested.clear();
            PAYLOAD_RESTAGE_ALL.store(true, Ordering::Release);
            PAYLOAD_RESTAGE_PENDING.store(false, Ordering::Release);
            newly_requested = true;
        } else {
            newly_requested = requested.insert(id);
            PAYLOAD_RESTAGE_PENDING.store(true, Ordering::Release);
        }
    }
    if newly_requested {
        // Materialization normally happens on the worker. Wake the UI so the
        // next prepare pass can recover the evicted source from App state.
        crate::bus::Bus::global().publish(crate::bus::BusEvent::MermaidRenderCompleted);
    }
}

/// Rare recovery path for payloads removed by the staging byte budget or an
/// image visibility toggle. The steady-state fast path is one relaxed atomic
/// load; only an actual miss clones/scans the App's image list.
pub(crate) fn restage_requested_payloads(app: &dyn crate::tui::TuiState) {
    if !app.pin_images() || !app.inline_images_visible() {
        return;
    }
    let restage_all = PAYLOAD_RESTAGE_ALL.swap(false, Ordering::AcqRel);
    if !restage_all && !PAYLOAD_RESTAGE_PENDING.load(Ordering::Acquire) {
        return;
    }
    let requested = if restage_all {
        HashSet::new()
    } else {
        PAYLOAD_RESTAGE_IDS
            .lock()
            .map(|ids| ids.clone())
            .unwrap_or_default()
    };
    if !restage_all && requested.is_empty() {
        PAYLOAD_RESTAGE_PENDING.store(false, Ordering::Release);
        return;
    }

    let mut restored = HashSet::new();
    for image in app.side_pane_images() {
        let id = mermaid::inline_image_id(&image.media_type, &image.data);
        if restage_all || requested.contains(&id) {
            register_payload(id, &image.media_type, &image.data);
            restored.insert(id);
        }
    }
    if let Ok(mut pending) = PAYLOAD_RESTAGE_IDS.lock() {
        pending.retain(|id| !restored.contains(id));
        PAYLOAD_RESTAGE_PENDING.store(!pending.is_empty(), Ordering::Release);
    }
}

/// Ensure the image with `id` is materialized (decoded + cached) so it can be
/// drawn. Returns true on success.
///
/// Steady-state frames hit a cheap in-memory presence probe (no payload clone,
/// no payload hash); only the first visible frame for an image pays the decode
/// + cache cost.
pub(crate) fn materialize_visible(id: u64) -> bool {
    if mermaid::inline_image_is_materialized(id) {
        return true;
    }
    if let Some((media_type, data_b64)) = PAYLOAD_REGISTRY.lock().ok().and_then(|reg| reg.get(id)) {
        let materialized = mermaid::materialize_inline_image_by_id(id, &media_type, &data_b64);
        if materialized.is_some() {
            // The decoded bytes now live in the render cache and cache dir;
            // holding the base64 copy too would double-count every image.
            release_payload(id);
            return true;
        }
        return false;
    }
    // No staged payload: either it was dropped after a successful
    // materialization and the render-cache entry has since been LRU-evicted
    // (restore it from the persisted cache file), or this is a mermaid
    // diagram whose PNG lives in the shared render cache/disk.
    if mermaid::rediscover_inline_image(id).is_some() {
        return true;
    }
    if mermaid::get_cached_path(id).is_some() {
        return true;
    }
    request_payload_restage(id);
    false
}

/// One pending prewarm request: build everything needed to draw image `id`
/// at the given placeholder geometry (decode payload, write cache file, scale
/// to the target box, escape-encode for Kitty).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct PrewarmRequest {
    id: u64,
    target_cols: u16,
    target_rows: u16,
}

const PREWARM_QUEUE_CAPACITY: usize = 32;
static PREWARM_TX: OnceLock<mpsc::SyncSender<PrewarmRequest>> = OnceLock::new();
/// Requests queued or in flight, so a 60fps scroll doesn't enqueue the same
/// image dozens of times before the worker finishes it. Dedup by image id, not
/// exact geometry: during a live resize only one request per image may consume
/// CPU; completion triggers a repaint that queues the newest geometry if needed.
static PREWARM_INFLIGHT: LazyLock<Mutex<HashSet<u64>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));
/// A full bounded channel must not drop the newest visible request. Keep one
/// coalesced overflow request; the worker drains it after every queued job.
static PREWARM_OVERFLOW: LazyLock<Mutex<Option<PrewarmRequest>>> =
    LazyLock::new(|| Mutex::new(None));
static PREWARM_RETRY_NEEDED: AtomicBool = AtomicBool::new(false);

/// Per-image count of failed materialize attempts. Without this memo an
/// undecodable payload (or one evicted from the registry) would loop forever:
/// draw schedules a prewarm, the worker fails and nudges a repaint, the
/// repaint reschedules the same prewarm. After
/// [`PREWARM_FAILURE_MAX_ATTEMPTS`] failures the id is skipped until
/// [`register_payload`] sees a fresh payload for it.
static PREWARM_MATERIALIZE_FAILURES: LazyLock<Mutex<HashMap<u64, u8>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static PREWARM_FIT_FAILURES: LazyLock<Mutex<HashMap<PrewarmRequest, u8>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
const PREWARM_FAILURE_MAX_ATTEMPTS: u8 = 3;
/// Bound the failure memo; it only holds pathological ids, so if it fills up
/// something is systemically wrong and starting over is harmless.
const PREWARM_FAILURES_MAX: usize = 512;

fn prewarm_failures_exhausted(req: PrewarmRequest) -> bool {
    let materialize_exhausted = PREWARM_MATERIALIZE_FAILURES
        .lock()
        .ok()
        .and_then(|failures| failures.get(&req.id).copied())
        .is_some_and(|count| count >= PREWARM_FAILURE_MAX_ATTEMPTS);
    materialize_exhausted
        || PREWARM_FIT_FAILURES
            .lock()
            .ok()
            .and_then(|failures| failures.get(&req).copied())
            .is_some_and(|count| count >= PREWARM_FAILURE_MAX_ATTEMPTS)
}

fn record_materialize_failure(id: u64) {
    if let Ok(mut failures) = PREWARM_MATERIALIZE_FAILURES.lock() {
        if failures.len() >= PREWARM_FAILURES_MAX && !failures.contains_key(&id) {
            failures.clear();
        }
        let count = failures.entry(id).or_insert(0);
        *count = count.saturating_add(1);
        if *count == PREWARM_FAILURE_MAX_ATTEMPTS {
            crate::logging::warn(&format!(
                "inline image {id:#018x} failed to decode/materialize {} times; \
                 suspending all geometries until its payload is re-registered",
                PREWARM_FAILURE_MAX_ATTEMPTS
            ));
        }
    }
}

fn record_fit_failure(req: PrewarmRequest) {
    if let Ok(mut failures) = PREWARM_FIT_FAILURES.lock() {
        if failures.len() >= PREWARM_FAILURES_MAX && !failures.contains_key(&req) {
            failures.clear();
        }
        let count = failures.entry(req).or_insert(0);
        *count = count.saturating_add(1);
        if *count == PREWARM_FAILURE_MAX_ATTEMPTS {
            crate::logging::warn(&format!(
                "inline image {:#018x} failed to fit at {}x{} {} times; \
                 suspending this geometry until its payload is re-registered",
                req.id, req.target_cols, req.target_rows, PREWARM_FAILURE_MAX_ATTEMPTS
            ));
        }
    }
}

fn prewarm_sender() -> &'static mpsc::SyncSender<PrewarmRequest> {
    PREWARM_TX.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<PrewarmRequest>(PREWARM_QUEUE_CAPACITY);
        if let Err(err) = std::thread::Builder::new()
            .name("jcode-inline-image-prewarm".to_string())
            .spawn(move || prewarm_worker(rx))
        {
            crate::logging::warn(&format!(
                "Failed to spawn inline-image prewarm worker; first view will decode on the UI thread: {}",
                err
            ));
        }
        tx
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrewarmOutcome {
    Prepared,
    SourceMissing,
    MaterializeFailed,
    FitFailed,
}

fn payload_restage_requested(id: u64) -> bool {
    PAYLOAD_RESTAGE_ALL.load(Ordering::Acquire)
        || PAYLOAD_RESTAGE_IDS
            .lock()
            .map(|requested| requested.contains(&id))
            .unwrap_or(false)
}

fn prepare_prewarm_request(req: PrewarmRequest) -> PrewarmOutcome {
    if !materialize_visible(req.id) {
        return if payload_restage_requested(req.id) {
            PrewarmOutcome::SourceMissing
        } else {
            PrewarmOutcome::MaterializeFailed
        };
    }
    match mermaid::inline_fit_readiness(req.id, req.target_cols, req.target_rows, true) {
        mermaid::InlineFitReadiness::Ready | mermaid::InlineFitReadiness::Unsupported => {
            PrewarmOutcome::Prepared
        }
        mermaid::InlineFitReadiness::NeedsPrewarm => {
            if mermaid::prewarm_inline_fit_state(req.id, req.target_cols, req.target_rows, true) {
                PrewarmOutcome::Prepared
            } else {
                PrewarmOutcome::FitFailed
            }
        }
    }
}

fn finish_prewarm_request(req: PrewarmRequest) {
    let outcome = prepare_prewarm_request(req);
    match outcome {
        PrewarmOutcome::Prepared => {
            if let Ok(mut failures) = PREWARM_MATERIALIZE_FAILURES.lock() {
                failures.remove(&req.id);
            }
            if let Ok(mut failures) = PREWARM_FIT_FAILURES.lock() {
                failures.remove(&req);
            }
        }
        PrewarmOutcome::SourceMissing => {}
        PrewarmOutcome::MaterializeFailed => record_materialize_failure(req.id),
        PrewarmOutcome::FitFailed => record_fit_failure(req),
    }
    if let Ok(mut inflight) = PREWARM_INFLIGHT.lock() {
        inflight.remove(&req.id);
    }
    if outcome == PrewarmOutcome::Prepared {
        crate::bus::Bus::global().publish(crate::bus::BusEvent::MermaidRenderCompleted);
    }
}

fn prewarm_worker(rx: mpsc::Receiver<PrewarmRequest>) {
    for req in rx {
        finish_prewarm_request(req);
        // A producer never blocks the UI on a full channel. Drain the newest
        // coalesced overflow request before waiting on the channel again.
        while let Some(overflow) = PREWARM_OVERFLOW
            .lock()
            .ok()
            .and_then(|mut pending| pending.take())
        {
            finish_prewarm_request(overflow);
        }
        if PREWARM_RETRY_NEEDED.swap(false, Ordering::AcqRel) {
            // At least one older coalesced request was displaced. Space has now
            // been made, so repaint once and let the current viewport resubmit
            // whichever geometry is still relevant.
            crate::bus::Bus::global().publish(crate::bus::BusEvent::MermaidRenderCompleted);
        }
    }
}

fn coalesce_overflow(req: PrewarmRequest) {
    let displaced = PREWARM_OVERFLOW
        .lock()
        .ok()
        .and_then(|mut pending| pending.replace(req));
    if let Some(displaced) = displaced
        && displaced.id != req.id
        && let Ok(mut inflight) = PREWARM_INFLIGHT.lock()
    {
        inflight.remove(&displaced.id);
        PREWARM_RETRY_NEEDED.store(true, Ordering::Release);
    }
}

/// Make sure image `id` can be drawn cheaply this frame.
///
/// Returns true when the draw path can run now without heavy work (image
/// decoded and, on Kitty, scale+transmit state matches the placeholder
/// geometry). Returns false after scheduling background preparation; the
/// caller should skip drawing this frame and rely on the completion nudge to
/// repaint.
pub(crate) fn ensure_drawable(id: u64, target_cols: u16, target_rows: u16) -> bool {
    let materialized = mermaid::inline_image_is_materialized(id);
    let readiness = if materialized {
        mermaid::inline_fit_readiness(id, target_cols, target_rows, true)
    } else {
        // Not decoded yet. On any protocol the first draw would block on a
        // full decode, so prewarm regardless of protocol support.
        mermaid::InlineFitReadiness::NeedsPrewarm
    };

    match readiness {
        mermaid::InlineFitReadiness::Ready => true,
        mermaid::InlineFitReadiness::Unsupported => {
            // No picker or video export. Materialization is still useful so a
            // picker becoming available later can draw without decoding here.
            if materialized {
                true
            } else {
                schedule_prewarm(id, target_cols, target_rows);
                false
            }
        }
        mermaid::InlineFitReadiness::NeedsPrewarm => {
            schedule_prewarm(id, target_cols, target_rows);
            false
        }
    }
}

fn schedule_prewarm(id: u64, target_cols: u16, target_rows: u16) {
    let req = PrewarmRequest {
        id,
        target_cols,
        target_rows,
    };
    if prewarm_failures_exhausted(req) {
        return;
    }
    if let Ok(mut inflight) = PREWARM_INFLIGHT.lock()
        && !inflight.insert(id)
    {
        return;
    }
    match prewarm_sender().try_send(req) {
        Ok(()) => {}
        Err(mpsc::TrySendError::Full(req)) => {
            // Keep the UI thread non-blocking without dropping the newest
            // request. The single worker drains this coalesced slot after every
            // channel job, including failed ones that do not publish a repaint.
            coalesce_overflow(req);
        }
        Err(mpsc::TrySendError::Disconnected(req)) => {
            if let Ok(mut inflight) = PREWARM_INFLIGHT.lock() {
                inflight.remove(&id);
            }
            // Thread creation failure is rare; preserve correctness by doing the
            // complete preparation synchronously instead of leaving Kitty images
            // permanently blank after materialization alone.
            match prepare_prewarm_request(req) {
                PrewarmOutcome::Prepared => {
                    crate::bus::Bus::global().publish(crate::bus::BusEvent::MermaidRenderCompleted);
                }
                PrewarmOutcome::SourceMissing => {}
                PrewarmOutcome::MaterializeFailed => record_materialize_failure(req.id),
                PrewarmOutcome::FitFailed => record_fit_failure(req),
            }
        }
    }
}

/// Warm an inline image that is *not* on screen yet so it is ready to draw the
/// instant it scrolls into view. Unlike [`ensure_drawable`], this never blocks
/// and never draws: if the image still needs decode/scale/transmit work it is
/// scheduled on the background prewarm worker (deduped against in-flight and
/// already-warm state), otherwise it is a cheap no-op.
///
/// Callers pass the same `(target_cols, target_rows)` placeholder geometry the
/// draw path will use, so the prewarmed Kitty fit-state matches exactly and the
/// first on-screen frame hits the `Ready` fast path with no rescale.
pub(crate) fn prefetch(id: u64, target_cols: u16, target_rows: u16) {
    let readiness = if mermaid::inline_image_is_materialized(id) {
        mermaid::inline_fit_readiness(id, target_cols, target_rows, true)
    } else {
        mermaid::InlineFitReadiness::NeedsPrewarm
    };
    match readiness {
        // Already drawable, or no terminal protocol is currently available.
        mermaid::InlineFitReadiness::Ready | mermaid::InlineFitReadiness::Unsupported => {}
        mermaid::InlineFitReadiness::NeedsPrewarm => {
            schedule_prewarm(id, target_cols, target_rows);
        }
    }
}

fn resolve_item(
    image: &crate::session::RenderedImage,
    stage_payload: bool,
) -> Option<InlineImageItem> {
    let (id, width, height) = mermaid::inline_image_dims(&image.media_type, &image.data)?;
    if stage_payload {
        register_payload(id, &image.media_type, &image.data);
    }
    let label = image
        .label
        .clone()
        .unwrap_or_else(|| image.media_type.clone());
    Some(InlineImageItem {
        id,
        width,
        height,
        label,
        uses_text_fallback: mermaid::uses_text_image_fallback(),
    })
}

/// Inline images split by their transcript anchor so the body renderer can
/// place each one at the message that produced it.
#[derive(Default)]
pub(crate) struct AnchoredInlineImages {
    /// Images anchored to a tool result, keyed by tool call id.
    pub by_tool: HashMap<String, Vec<InlineImageItem>>,
    /// Images anchored to the nth (0-based) rendered user prompt.
    pub by_prompt: HashMap<usize, Vec<InlineImageItem>>,
    /// Images with no usable anchor; rendered at the end of the transcript.
    pub unanchored: Vec<InlineImageItem>,
}

impl AnchoredInlineImages {
    #[cfg(test)]
    pub(crate) fn has_anchored(&self) -> bool {
        !self.by_tool.is_empty() || !self.by_prompt.is_empty()
    }

    /// Items that will NOT be placed inside the transcript body: unanchored
    /// images plus anchored images whose anchor target does not exist among
    /// `messages` (e.g. live images for a tool whose transcript entry was
    /// replaced). These fall back to the bottom inline-images section so no
    /// image ever silently disappears.
    pub(crate) fn unplaced_items(
        &self,
        messages: &[jcode_tui_messages::DisplayMessage],
    ) -> Vec<InlineImageItem> {
        let mut items: Vec<InlineImageItem> = self.unanchored.clone();
        if self.by_tool.is_empty() && self.by_prompt.is_empty() {
            return items;
        }

        let mut tool_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut prompt_count = 0usize;
        for msg in messages {
            use crate::tui::DisplayMessageRoleExt as _;
            match msg.effective_role() {
                "tool" => {
                    if let Some(tool) = &msg.tool_data {
                        tool_ids.insert(tool.id.as_str());
                    }
                }
                "user" => {
                    if !crate::session::is_attached_image_label_text(&msg.content) {
                        prompt_count += 1;
                    }
                }
                _ => {}
            }
        }

        for (id, bucket) in &self.by_tool {
            if !tool_ids.contains(id.as_str()) {
                items.extend(bucket.iter().cloned());
            }
        }
        for (ordinal, bucket) in &self.by_prompt {
            if *ordinal >= prompt_count {
                items.extend(bucket.iter().cloned());
            }
        }
        items
    }
}

/// Resolve rendered images into anchored buckets (tool call / user prompt /
/// unanchored). Same lazy header-only cost profile as [`resolve_item`].
#[cfg(test)]
pub(crate) fn resolve_anchored_items(
    images: &[crate::session::RenderedImage],
) -> AnchoredInlineImages {
    resolve_anchored_items_inner(images, true)
}

fn resolve_anchored_items_inner(
    images: &[crate::session::RenderedImage],
    stage_payloads: bool,
) -> AnchoredInlineImages {
    let mut result = AnchoredInlineImages::default();
    for image in images {
        let Some(item) = resolve_item(image, stage_payloads) else {
            continue;
        };
        match &image.anchor {
            Some(crate::session::RenderedImageAnchor::ToolCall { id }) => {
                result.by_tool.entry(id.clone()).or_default().push(item);
            }
            Some(crate::session::RenderedImageAnchor::UserPrompt { ordinal }) => {
                result.by_prompt.entry(*ordinal).or_default().push(item);
            }
            None => result.unanchored.push(item),
        }
    }
    result
}

/// One-slot cache for [`resolve_anchored_items`], keyed by the image-set
/// signature. Resolving hashes every image payload (for ids), so body
/// preparation must not redo it per rebuild; the signature is already cached
/// per transcript version on the app side.
type AnchoredCacheKey = ((usize, u64), bool);
type AnchoredCache = Mutex<Option<(AnchoredCacheKey, std::sync::Arc<AnchoredInlineImages>)>>;
static ANCHORED_CACHE: LazyLock<AnchoredCache> = LazyLock::new(|| Mutex::new(None));

/// Resolve the app's images into anchored buckets, cached by the image-set
/// signature. Returns an empty result without touching payloads when the app
/// has no images or inline images are hidden.
pub(crate) fn resolve_anchored_items_cached(
    app: &dyn crate::tui::TuiState,
) -> std::sync::Arc<AnchoredInlineImages> {
    if !app.pin_images() {
        clear_staged_payloads();
        return std::sync::Arc::new(AnchoredInlineImages::default());
    }
    let signature = app.side_pane_images_signature();
    if signature.0 == 0 {
        clear_staged_payloads();
        return std::sync::Arc::new(AnchoredInlineImages::default());
    }
    let stage_payloads = app.inline_images_visible();
    if !stage_payloads {
        clear_staged_payloads();
    }
    let key = (signature, stage_payloads);
    if let Ok(cache) = ANCHORED_CACHE.lock()
        && let Some((cached_sig, cached)) = cache.as_ref()
        && *cached_sig == key
    {
        return cached.clone();
    }
    let resolved = std::sync::Arc::new(resolve_anchored_items_inner(
        &app.side_pane_images(),
        stage_payloads,
    ));
    if let Ok(mut cache) = ANCHORED_CACHE.lock() {
        *cache = Some((key, resolved.clone()));
    }
    resolved
}

/// Compute how many `(rows, cols)` an inline image occupies at `chat_width`,
/// capped at `cap_rows`. `cols` includes the 2-cell left border, matching what
/// the draw step actually paints, so layout (e.g. info widget placement) can
/// know the real horizontal extent.
fn fit_geometry_with_cap(width: u32, height: u32, chat_width: u16, cap_rows: u16) -> (u16, u16) {
    // Single source of truth for inline-fit placeholder geometry, shared with
    // the mermaid crate so diagrams and raster images stay in lockstep with
    // the draw-time fit math.
    mermaid::inline_fit_geometry(width, height, chat_width, cap_rows)
}

/// Compute `(rows, cols)` for an inline image at `chat_width`, given a viewport
/// height to cap against.
fn fit_geometry(width: u32, height: u32, chat_width: u16, viewport_height: u16) -> (u16, u16) {
    let cap_rows = ((viewport_height as u32 * MAX_VIEWPORT_FRACTION_PERCENT as u32) / 100)
        .clamp(MIN_IMAGE_ROWS as u32, u16::MAX as u32) as u16;
    fit_geometry_with_cap(width, height, chat_width, cap_rows)
}

/// Compute `(rows, cols)` for an image anchored inside the transcript body at a
/// given expand level. Uses a fixed (viewport-independent) row cap so the body's
/// prepared lines stay valid across resizes (the body cache is keyed by width
/// only); the expand level only swaps which fixed cap applies.
pub(crate) fn fit_geometry_anchored(
    width: u32,
    height: u32,
    chat_width: u16,
    level: ImageExpandLevel,
) -> (u16, u16) {
    fit_geometry_with_cap(width, height, chat_width, level.anchored_cap_rows())
}

/// Compute how many rows an inline image should occupy at `chat_width`, given a
/// viewport height to cap against.
#[cfg(test)]
fn fit_rows(width: u32, height: u32, chat_width: u16, viewport_height: u16) -> u16 {
    fit_geometry(width, height, chat_width, viewport_height).0
}

/// Build the dim label line shown above an inline image, e.g.
/// `  screenshot.png  1920×1080`, with a trailing show/hide badge
/// (`[Alt] [⇧] [I] hide` / `[Alt] [⇧] [I] show image`) so the toggle is
/// discoverable right where the image renders. The line is kept deliberately
/// short so it fits on one row; there is no visible expand badge, but clicking
/// the rendered image body still cycles the per-image size
/// (see `inline_image_body_target_from_screen`).
pub(crate) fn image_label_line(
    item: &InlineImageItem,
    width: u16,
    images_visible: bool,
    _level: ImageExpandLevel,
) -> Line<'static> {
    let dims = format!("{}×{}", item.width, item.height);
    let label = if item.label.is_empty() {
        dims
    } else {
        format!("{}  {}", item.label, dims)
    };
    let dim = Style::default().add_modifier(Modifier::DIM);
    let prefix = Line::from(vec![Span::styled("  ", dim), Span::styled(label, dim)]);
    let mut suffix = vec![
        Span::raw("  "),
        Span::styled(super::viewport::copy_badge_alt_badge(), dim),
        Span::styled(" [⇧] [I] ", dim),
    ];
    if images_visible {
        suffix.push(Span::styled("hide", dim));
    } else {
        suffix.push(Span::styled(
            "show image",
            Style::default().add_modifier(Modifier::DIM | Modifier::ITALIC),
        ));
    }
    jcode_tui_render::truncate_line_preserving_suffix_to_width(
        &prefix,
        &Line::from(suffix),
        width as usize,
    )
}

fn image_fallback_note_line(width: u16) -> Line<'static> {
    jcode_tui_render::truncate_line_with_ellipsis_to_width(
        &mermaid::text_image_fallback_note_line(),
        width as usize,
    )
}

/// Lines for images anchored at a transcript message: per image, a leading
/// blank, a dim label, a geometry-encoding marker line plus blank placeholder
/// rows (recognized by the image-region scan), and a trailing blank. When
/// `images_visible` is false the image collapses to just its label stub (with
/// a `show image` badge) and no placeholder rows are emitted, so nothing is
/// painted.
pub(crate) fn anchored_image_lines(
    items: &[InlineImageItem],
    width: u16,
    images_visible: bool,
    levels: &dyn ImageExpandLevels,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for item in items {
        let level = levels.expand_level(item.id);
        lines.push(Line::from(""));
        lines.push(image_label_line(item, width, images_visible, level));
        if images_visible {
            if item.uses_text_fallback {
                lines.push(image_fallback_note_line(width));
            }
            let (rows, cols) = fit_geometry_anchored(item.width, item.height, width, level);
            lines.extend(mermaid::inline_image_placeholder_lines(item.id, rows, cols));
        }
        lines.push(Line::from(""));
    }
    lines
}

/// Build the inline-images prepared section: a heading + correctly-sized
/// placeholder per image, with explicit `image_regions` (render = Fit) that the
/// viewport draws lazily. When `images_visible` is false each image collapses
/// to its label stub and no regions are emitted.
pub(crate) fn build_section(
    items: &[InlineImageItem],
    width: u16,
    viewport_height: u16,
    prefix_blank: bool,
    images_visible: bool,
    levels: &dyn ImageExpandLevels,
) -> PreparedMessages {
    use std::sync::Arc;

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut image_regions: Vec<ImageRegion> = Vec::new();

    if items.is_empty() {
        return empty();
    }

    if prefix_blank {
        lines.push(Line::from(""));
    }

    for item in items {
        let level = levels.expand_level(item.id);
        lines.push(image_label_line(item, width, images_visible, level));

        if images_visible {
            if item.uses_text_fallback {
                lines.push(image_fallback_note_line(width));
            }
            // The bottom (unanchored) section is rebuilt every frame, not body
            // cached, so a viewport-relative default fit is fine here. Expanded
            // levels use the discrete fixed caps so they grow predictably.
            let (rows, cols) = match level {
                ImageExpandLevel::Fit => {
                    fit_geometry(item.width, item.height, width, viewport_height)
                }
                _ => {
                    fit_geometry_with_cap(item.width, item.height, width, level.anchored_cap_rows())
                }
            };
            let region_start = lines.len();
            for _ in 0..rows {
                lines.push(Line::from(""));
            }
            image_regions.push(ImageRegion {
                abs_line_idx: region_start,
                end_line: region_start + rows as usize,
                hash: item.id,
                height: rows,
                width: cols,
                render: ImageRegionRender::Fit,
            });
        }
        // Trailing spacer between images.
        lines.push(Line::from(""));
    }

    let line_count = lines.len();
    let plain: Vec<String> = lines
        .iter()
        .map(jcode_tui_render::line_plain_text)
        .collect();

    PreparedMessages {
        wrapped_lines: lines,
        wrapped_plain_lines: Arc::new(plain),
        wrapped_copy_offsets: Arc::new(vec![0; line_count]),
        raw_plain_lines: Arc::new(Vec::new()),
        wrapped_line_map: Arc::new(Vec::new()),
        wrapped_user_indices: Vec::new(),
        wrapped_user_prompt_starts: Vec::new(),
        wrapped_user_prompt_ends: Vec::new(),
        user_prompt_texts: Vec::new(),
        image_regions,
        edit_tool_ranges: Vec::new(),
        copy_targets: Vec::new(),
        message_boundaries: Vec::new(),
        mermaid_pending_epoch: None,
    }
}

fn empty() -> PreparedMessages {
    use std::sync::Arc;
    PreparedMessages {
        wrapped_lines: Vec::new(),
        wrapped_plain_lines: Arc::new(Vec::new()),
        wrapped_copy_offsets: Arc::new(Vec::new()),
        raw_plain_lines: Arc::new(Vec::new()),
        wrapped_line_map: Arc::new(Vec::new()),
        wrapped_user_indices: Vec::new(),
        wrapped_user_prompt_starts: Vec::new(),
        wrapped_user_prompt_ends: Vec::new(),
        user_prompt_texts: Vec::new(),
        image_regions: Vec::new(),
        edit_tool_ranges: Vec::new(),
        copy_targets: Vec::new(),
        message_boundaries: Vec::new(),
        mermaid_pending_epoch: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(width: u32, height: u32) -> InlineImageItem {
        InlineImageItem {
            id: 0xABCD,
            width,
            height,
            label: "test.png".to_string(),
            uses_text_fallback: false,
        }
    }

    /// 1x1 transparent PNG used by the materialize tests below.
    const MATERIALIZE_PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

    /// Materialize `id`, retrying against unrelated parallel tests that wipe
    /// the process-global image state (`mermaid::clear_image_state()` runs on
    /// diagram geometry changes, zoom, and session-picker paths). Each retry
    /// re-stages the payload because `register_payload` is skipped while an
    /// image is materialized and a wipe can clear both stores at once.
    fn materialize_with_retry<F: Fn() -> bool>(id: u64, check: F) {
        for _ in 0..100 {
            register_payload(id, "image/png", MATERIALIZE_PNG_B64);
            if materialize_visible(id) && check() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("image {id:#x} never reached the expected state despite retries");
    }

    #[test]
    fn materialize_visible_probe_is_cheap_after_first_call() {
        let id = mermaid::inline_image_id("image/png", MATERIALIZE_PNG_B64);
        // Steady state: the in-memory probe alone must report ready, without
        // needing the payload registry at all.
        materialize_with_retry(id, || {
            mermaid::inline_image_is_materialized(id) && materialize_visible(id)
        });
    }

    #[test]
    fn prefetch_is_noop_for_materialized_image_without_kitty() {
        // Without a Kitty picker the fit-state path is Unsupported, so a
        // materialized image has nothing to prewarm: prefetch must be a cheap
        // no-op (no panic, no scheduling) and the image stays materialized.
        let id = mermaid::inline_image_id("image/png", MATERIALIZE_PNG_B64);
        materialize_with_retry(id, || {
            prefetch(id, 80, 10);
            mermaid::inline_image_is_materialized(id)
        });
    }

    #[test]
    fn ensure_drawable_true_for_materialized_image_without_kitty() {
        // In tests no picker is initialized, so the stable-fit path reports
        // Unsupported; a materialized image must still be drawable so the
        // fallback renderers can run.
        let id = mermaid::inline_image_id("image/png", MATERIALIZE_PNG_B64);
        materialize_with_retry(id, || ensure_drawable(id, 80, 10));
    }

    #[test]
    fn fit_rows_caps_tall_image_to_viewport_fraction() {
        // A very tall image must be capped so it cannot bury the transcript.
        let rows = fit_rows(100, 100_000, 80, 40);
        let cap = ((40u32 * MAX_VIEWPORT_FRACTION_PERCENT as u32) / 100) as u16;
        assert!(rows <= cap, "rows {rows} should be <= cap {cap}");
        assert!(rows >= MIN_IMAGE_ROWS);
    }

    #[test]
    fn fit_rows_never_below_minimum() {
        let rows = fit_rows(10, 10, 80, 40);
        assert!(rows >= MIN_IMAGE_ROWS);
    }

    #[test]
    fn fit_geometry_height_bound_image_narrows_proportionally() {
        // Tall image hits the viewport cap; the recorded cols must shrink with
        // it so the border/label hug the actual rendered picture.
        let (rows, cols) = fit_geometry(1000, 4000, 100, 40);
        let cap = ((40u32 * MAX_VIEWPORT_FRACTION_PERCENT as u32) / 100) as u16;
        assert!(rows <= cap);
        // Width-bound it would be ~100 cols; height-bound it must be far less.
        assert!(cols < 50, "height-bound image should be narrow, got {cols}");
        assert!(cols > 2, "image must occupy some columns, got {cols}");
    }

    #[test]
    fn fit_geometry_small_window_never_exceeds_chat_width() {
        for chat_width in [1u16, 2, 3, 5, 10] {
            for viewport_height in [1u16, 2, 5, 10] {
                let (rows, cols) = fit_geometry(1920, 1080, chat_width, viewport_height);
                assert!(
                    cols <= chat_width.max(2),
                    "cols {cols} > width {chat_width}"
                );
                assert!(rows >= MIN_IMAGE_ROWS);
            }
        }
    }

    #[test]
    fn fit_geometry_zero_dims_safe() {
        let (rows, cols) = fit_geometry(0, 0, 80, 40);
        assert!(rows >= MIN_IMAGE_ROWS);
        assert!(cols <= 80);
    }

    #[test]
    fn build_section_records_region_width() {
        let items = vec![item(600, 400)];
        let section = build_section(&items, 80, 40, false, true, &AllFit);
        let region = &section.image_regions[0];
        assert!(
            region.width > 2,
            "region width should include the image, got {}",
            region.width
        );
        assert!(region.width <= 80);
    }

    #[test]
    fn build_section_emits_one_fit_region_per_image_with_label() {
        let items = vec![item(600, 400), item(800, 600)];
        let section = build_section(&items, 80, 40, true, true, &AllFit);
        assert_eq!(section.image_regions.len(), 2);
        for region in &section.image_regions {
            assert_eq!(region.render, ImageRegionRender::Fit);
            assert_eq!(region.hash, 0xABCD);
            // The region must point at blank placeholder lines, never the label.
            let first = &section.wrapped_lines[region.abs_line_idx];
            assert!(
                jcode_tui_render::line_plain_text(first).trim().is_empty(),
                "region should start on a blank placeholder line"
            );
            // Region height must match its line span.
            assert_eq!(
                region.end_line - region.abs_line_idx,
                region.height as usize
            );
        }
        // A dim label line precedes the first region.
        let label_line = jcode_tui_render::line_plain_text(&section.wrapped_lines[1]);
        assert!(
            label_line.contains("test.png"),
            "label missing: {label_line:?}"
        );
    }

    #[test]
    fn build_section_is_empty_for_no_items() {
        let section = build_section(&[], 80, 40, false, true, &AllFit);
        assert!(section.wrapped_lines.is_empty());
        assert!(section.image_regions.is_empty());
    }

    #[test]
    fn build_section_hidden_collapses_to_label_stub_with_show_badge() {
        let items = vec![item(600, 400)];
        let section = build_section(&items, 80, 40, false, false, &AllFit);
        assert!(
            section.image_regions.is_empty(),
            "hidden images must not emit drawable regions"
        );
        let text: String = section
            .wrapped_lines
            .iter()
            .map(jcode_tui_render::line_plain_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("test.png"), "label should remain: {text:?}");
        assert!(
            text.contains("show image"),
            "show badge should render: {text:?}"
        );
    }

    #[test]
    fn visible_label_line_advertises_hide_badge() {
        let line = image_label_line(&item(600, 400), 80, true, ImageExpandLevel::Fit);
        let text = jcode_tui_render::line_plain_text(&line);
        assert!(text.contains("[⇧] [I]"), "badge keys missing: {text:?}");
        assert!(text.contains("hide"), "hide hint missing: {text:?}");
    }

    #[test]
    fn fallback_image_has_an_attached_terminal_capability_note() {
        let mut fallback = item(600, 400);
        fallback.uses_text_fallback = true;
        let section = build_section(&[fallback], 80, 40, false, true, &AllFit);
        let text = section
            .wrapped_lines
            .iter()
            .map(jcode_tui_render::line_plain_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains(mermaid::TERMINAL_IMAGE_FALLBACK_NOTE));
        assert_eq!(section.image_regions[0].abs_line_idx, 2);
    }

    #[test]
    fn native_image_render_does_not_show_the_fallback_note() {
        let section = build_section(&[item(600, 400)], 80, 40, false, true, &AllFit);
        let text = section
            .wrapped_lines
            .iter()
            .map(jcode_tui_render::line_plain_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!text.contains(mermaid::TERMINAL_IMAGE_FALLBACK_NOTE));
        assert_eq!(section.image_regions[0].abs_line_idx, 1);
    }

    #[test]
    fn generated_image_label_is_truncated_to_one_terminal_row() {
        let mut generated = item(1536, 864);
        generated.label = "/home/jeremy/jcode/.jcode/generated-images/1783707839145-ig_0c5e377de871aba5016a51388779588196944d98eff2c7aa30.png".to_string();

        let line = image_label_line(&generated, 52, true, ImageExpandLevel::Fit);
        let text = jcode_tui_render::line_plain_text(&line);

        assert!(line.width() <= 52, "label exceeded chat width: {text:?}");
        assert!(
            text.contains('…'),
            "truncated label needs an ellipsis: {text:?}"
        );
        assert!(
            text.ends_with("hide"),
            "toggle suffix must remain visible: {text:?}"
        );
        assert!(
            !text.contains('\n'),
            "label must remain one logical line: {text:?}"
        );
    }

    #[test]
    fn expand_level_cycle_visits_every_level_and_wraps() {
        assert_eq!(ImageExpandLevel::Fit.next(), ImageExpandLevel::Large);
        assert_eq!(ImageExpandLevel::Large.next(), ImageExpandLevel::Full);
        assert_eq!(ImageExpandLevel::Full.next(), ImageExpandLevel::Fit);
    }

    #[test]
    fn expand_level_caps_grow_monotonically() {
        assert!(
            ImageExpandLevel::Fit.anchored_cap_rows() < ImageExpandLevel::Large.anchored_cap_rows()
        );
        assert!(
            ImageExpandLevel::Large.anchored_cap_rows()
                < ImageExpandLevel::Full.anchored_cap_rows()
        );
        // Full must stay under kitty's virtual-placement row limit (296) so
        // stable fit rendering keeps working at every level.
        assert!(ImageExpandLevel::Full.anchored_cap_rows() < 296);
    }

    #[test]
    fn visible_label_line_stays_single_purpose_without_expand_badge() {
        // The label must stay a short single line: no expand badge, no dots.
        let line = image_label_line(&item(600, 400), 80, true, ImageExpandLevel::Fit);
        let text = jcode_tui_render::line_plain_text(&line);
        assert!(
            !text.contains("expand") && !text.contains('○') && !text.contains('●'),
            "label line must not carry an expand badge: {text:?}"
        );
    }

    #[test]
    fn hidden_label_line_omits_expand_badge() {
        let line = image_label_line(&item(600, 400), 80, false, ImageExpandLevel::Fit);
        let text = jcode_tui_render::line_plain_text(&line);
        assert!(text.contains("show image"), "show badge missing: {text:?}");
        assert!(
            !text.contains("expand"),
            "hidden image must hide expand badge: {text:?}"
        );
    }

    #[test]
    fn expanded_level_makes_anchored_image_taller() {
        let fit = fit_geometry_anchored(1000, 4000, 100, ImageExpandLevel::Fit).0;
        let large = fit_geometry_anchored(1000, 4000, 100, ImageExpandLevel::Large).0;
        let full = fit_geometry_anchored(1000, 4000, 100, ImageExpandLevel::Full).0;
        assert!(large > fit, "Large ({large}) should exceed Fit ({fit})");
        assert!(full > large, "Full ({full}) should exceed Large ({large})");
    }

    #[test]
    fn anchored_image_lines_hidden_emit_no_placeholder_markers() {
        let items = vec![item(600, 400)];
        let lines = anchored_image_lines(&items, 80, false, &AllFit);
        assert!(
            lines
                .iter()
                .filter_map(mermaid::parse_inline_image_placeholder)
                .next()
                .is_none(),
            "hidden images must not emit geometry markers"
        );
        let text: String = lines
            .iter()
            .map(jcode_tui_render::line_plain_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("show image"), "show badge missing: {text:?}");
    }

    #[test]
    fn payload_registry_roundtrips() {
        register_payload(0xDEAD_BEEF, "image/png", "AAAA");
        let got = PAYLOAD_REGISTRY.lock().unwrap().get(0xDEAD_BEEF);
        assert_eq!(got, Some(("image/png".to_string(), "AAAA".to_string())));
    }

    /// The registry holds full base64 payloads, so it must be bounded by bytes
    /// as well as entry count, and eviction must keep the byte accounting and
    /// the map/order queue in sync. A single over-budget payload must survive
    /// (or its image could never materialize).
    #[test]
    fn payload_registry_evicts_by_byte_budget() {
        let mut reg = PayloadRegistry::new();
        // Payloads sized so ~5 of them exceed the byte budget long before the
        // 512-entry count bound.
        let payload = "x".repeat(PAYLOAD_REGISTRY_MAX_BYTES / 4);
        for id in 0..8u64 {
            reg.insert(id, "image/png", &payload);
            assert!(
                reg.total_bytes <= PAYLOAD_REGISTRY_MAX_BYTES || reg.order.len() == 1,
                "byte budget exceeded with {} entries / {} bytes",
                reg.order.len(),
                reg.total_bytes
            );
            assert_eq!(reg.map.len(), reg.order.len(), "map/order desynced");
        }
        // Newest payload always survives.
        assert!(reg.get(7).is_some(), "newest payload must not be evicted");
        // Oldest payloads were evicted to make room.
        assert!(reg.get(0).is_none(), "oldest payload should be evicted");
        // A single payload larger than the whole budget still gets stored.
        let mut solo = PayloadRegistry::new();
        let huge = "y".repeat(PAYLOAD_REGISTRY_MAX_BYTES + 1);
        solo.insert(99, "image/png", &huge);
        assert!(
            solo.get(99).is_some(),
            "an over-budget payload must stay resident so its image can draw"
        );
    }

    /// Re-registering a staged source may retry geometry preparation, but its
    /// stable content id keeps corrupt-payload decode failures capped.
    #[test]
    fn reregistering_payload_resets_fit_failures() {
        const ID: u64 = 0xFA11_ED01;
        let req = PrewarmRequest {
            id: ID,
            target_cols: 80,
            target_rows: 16,
        };
        for _ in 0..PREWARM_FAILURE_MAX_ATTEMPTS {
            record_fit_failure(req);
        }
        assert!(
            prewarm_failures_exhausted(req),
            "failure memo should suspend prewarm after max attempts"
        );
        register_payload(ID, "image/png", "BBBB");
        assert!(
            !prewarm_failures_exhausted(req),
            "restaging may retry geometry preparation"
        );
        PAYLOAD_REGISTRY.lock().unwrap().remove(ID);
    }

    #[test]
    fn prewarm_failures_are_scoped_to_geometry() {
        let failed = PrewarmRequest {
            id: 0xFA11_ED02,
            target_cols: 80,
            target_rows: 16,
        };
        for _ in 0..PREWARM_FAILURE_MAX_ATTEMPTS {
            record_fit_failure(failed);
        }
        let resized = PrewarmRequest {
            target_cols: 100,
            ..failed
        };
        assert!(prewarm_failures_exhausted(failed));
        assert!(
            !prewarm_failures_exhausted(resized),
            "a failed stale resize must not block preparation at the new geometry"
        );
        PREWARM_FIT_FAILURES.lock().unwrap().remove(&failed);
    }

    #[test]
    fn materialize_failures_apply_across_resize_geometries() {
        let failed = PrewarmRequest {
            id: 0xFA11_ED03,
            target_cols: 80,
            target_rows: 16,
        };
        for _ in 0..PREWARM_FAILURE_MAX_ATTEMPTS {
            record_materialize_failure(failed.id);
        }
        let resized = PrewarmRequest {
            target_cols: 120,
            target_rows: 24,
            ..failed
        };
        assert!(prewarm_failures_exhausted(failed));
        assert!(
            prewarm_failures_exhausted(resized),
            "a corrupt payload must not get three more full decodes after every resize"
        );
        register_payload(failed.id, "image/png", "BBBB");
        assert!(
            prewarm_failures_exhausted(resized),
            "restaging identical content must not reset its ID-wide decode cap"
        );
        PAYLOAD_REGISTRY.lock().unwrap().remove(failed.id);
        PREWARM_MATERIALIZE_FAILURES
            .lock()
            .unwrap()
            .remove(&failed.id);
    }

    #[test]
    fn overflow_slot_keeps_the_newest_request() {
        let first = PrewarmRequest {
            id: 1,
            target_cols: 80,
            target_rows: 16,
        };
        let newest = PrewarmRequest {
            id: 2,
            target_cols: 100,
            target_rows: 20,
        };
        let mut pending = None;
        assert!(pending.replace(first).is_none());
        assert_eq!(pending.replace(newest), Some(first));
        assert_eq!(pending, Some(newest));
    }

    /// Materialization must release the staged base64 payload (the decoded
    /// bytes are persisted in the render cache + cache dir), and later
    /// re-registrations for a materialized image must be no-ops so the payload
    /// is never staged twice. Draws must keep working afterwards.
    #[test]
    fn materialize_releases_payload_and_blocks_restaging() {
        // Distinct payload so this test's id cannot collide with others.
        let id = mermaid::inline_image_id("image/png", MATERIALIZE_PNG_B64_RELEASE);
        register_payload(id, "image/png", MATERIALIZE_PNG_B64_RELEASE);
        assert!(
            PAYLOAD_REGISTRY.lock().unwrap().get(id).is_some(),
            "payload staged before materialization"
        );
        assert!(materialize_visible(id), "materialization succeeds");
        assert!(
            PAYLOAD_REGISTRY.lock().unwrap().get(id).is_none(),
            "payload must be released after materialization"
        );
        // Prepare passes keep calling register_payload; it must stay empty.
        register_payload(id, "image/png", MATERIALIZE_PNG_B64_RELEASE);
        assert!(
            PAYLOAD_REGISTRY.lock().unwrap().get(id).is_none(),
            "re-registering a materialized image must not restage its payload"
        );
        // And the image must still be drawable without the payload.
        assert!(
            materialize_visible(id),
            "materialized image stays visible after payload release"
        );
    }

    /// 1x1 red PNG distinct from `MATERIALIZE_PNG_B64` so payload-release
    /// tests own their image id.
    const MATERIALIZE_PNG_B64_RELEASE: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==";

    /// PayloadRegistry::remove must keep byte accounting and the eviction
    /// queue in sync.
    #[test]
    fn payload_registry_remove_keeps_accounting_consistent() {
        let mut reg = PayloadRegistry::new();
        reg.insert(1, "image/png", "AAAA");
        reg.insert(2, "image/png", "BBBBBBBB");
        let bytes_with_both = reg.total_bytes;
        reg.remove(1);
        assert!(reg.get(1).is_none());
        assert_eq!(reg.map.len(), reg.order.len(), "map/order desynced");
        assert_eq!(
            reg.total_bytes,
            bytes_with_both - ("image/png".len() + "AAAA".len()),
            "byte accounting must shrink by exactly the removed entry"
        );
        // Removing an absent id is a no-op.
        reg.remove(42);
        assert_eq!(reg.map.len(), 1);
    }

    /// 1x1 transparent PNG, used to exercise the real header parse.
    const TINY_PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

    fn rendered_image(
        anchor: Option<crate::session::RenderedImageAnchor>,
    ) -> crate::session::RenderedImage {
        crate::session::RenderedImage {
            media_type: "image/png".to_string(),
            data: TINY_PNG_B64.to_string(),
            label: Some("tiny.png".to_string()),
            source: crate::session::RenderedImageSource::ToolResult {
                tool_name: "read".to_string(),
            },
            anchor,
        }
    }

    #[test]
    fn resolve_anchored_items_buckets_by_anchor() {
        let images = vec![
            rendered_image(Some(crate::session::RenderedImageAnchor::ToolCall {
                id: "tool-1".to_string(),
            })),
            rendered_image(Some(crate::session::RenderedImageAnchor::UserPrompt {
                ordinal: 2,
            })),
            rendered_image(None),
        ];
        let anchored = resolve_anchored_items(&images);
        assert!(anchored.has_anchored());
        assert_eq!(anchored.by_tool.get("tool-1").map(Vec::len), Some(1));
        assert_eq!(anchored.by_prompt.get(&2).map(Vec::len), Some(1));
        assert_eq!(anchored.unanchored.len(), 1);
    }

    #[test]
    fn resolving_hidden_images_does_not_stage_payload_bytes() {
        use base64::Engine as _;
        use image::ImageEncoder as _;

        clear_staged_payloads();
        let pixels = image::RgbaImage::from_pixel(3, 2, image::Rgba([91, 37, 211, 255]));
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(pixels.as_raw(), 3, 2, image::ExtendedColorType::Rgba8)
            .expect("encode fixture");
        let data = base64::engine::general_purpose::STANDARD.encode(png);
        let image = crate::session::RenderedImage {
            media_type: "image/png".to_string(),
            data,
            label: Some("hidden-fixture.png".to_string()),
            source: crate::session::RenderedImageSource::ToolResult {
                tool_name: "read".to_string(),
            },
            anchor: None,
        };
        let id = mermaid::inline_image_id(&image.media_type, &image.data);

        let resolved = resolve_anchored_items_inner(std::slice::from_ref(&image), false);
        assert_eq!(resolved.unanchored.len(), 1, "metadata still resolves");
        assert!(
            PAYLOAD_REGISTRY.lock().unwrap().get(id).is_none(),
            "hidden image payload must not be copied into the staging registry"
        );

        let visible = resolve_anchored_items_inner(std::slice::from_ref(&image), true);
        assert_eq!(visible.unanchored.len(), 1);
        assert!(
            PAYLOAD_REGISTRY.lock().unwrap().get(id).is_some(),
            "visible image payload should be staged for lazy materialization"
        );
        PAYLOAD_REGISTRY.lock().unwrap().remove(id);
    }

    #[test]
    fn clearing_staged_payloads_invalidates_resolved_image_cache() {
        PAYLOAD_RESTAGE_ALL.store(false, Ordering::Release);
        *ANCHORED_CACHE.lock().unwrap() = Some((
            ((1, 99), true),
            std::sync::Arc::new(AnchoredInlineImages::default()),
        ));
        clear_staged_payloads();
        assert!(
            ANCHORED_CACHE.lock().unwrap().is_none(),
            "showing images again must resolve and restage their payloads"
        );
        assert!(
            PAYLOAD_RESTAGE_ALL.load(Ordering::Acquire),
            "a prepared-frame cache hit must know that every payload needs restaging"
        );
    }

    #[test]
    fn unplaced_items_falls_back_for_missing_anchor_targets() {
        use jcode_tui_messages::DisplayMessage;

        let images = vec![
            rendered_image(Some(crate::session::RenderedImageAnchor::ToolCall {
                id: "tool-present".to_string(),
            })),
            rendered_image(Some(crate::session::RenderedImageAnchor::ToolCall {
                id: "tool-missing".to_string(),
            })),
            rendered_image(Some(crate::session::RenderedImageAnchor::UserPrompt {
                ordinal: 0,
            })),
            rendered_image(Some(crate::session::RenderedImageAnchor::UserPrompt {
                ordinal: 5,
            })),
            rendered_image(None),
        ];
        let anchored = resolve_anchored_items(&images);

        let tool_call = crate::message::ToolCall {
            id: "tool-present".to_string(),
            name: "read".to_string(),
            input: serde_json::Value::Null,
            intent: None,
            thought_signature: None,
        };
        let messages = vec![
            DisplayMessage::user("show me"),
            DisplayMessage::tool("output", tool_call),
        ];

        let unplaced = anchored.unplaced_items(&messages);
        // tool-missing (1) + prompt ordinal 5 (1) + unanchored (1) = 3.
        // tool-present and prompt 0 are placed in the body, not here.
        assert_eq!(unplaced.len(), 3);
    }

    #[test]
    fn anchored_image_lines_round_trip_through_region_scan() {
        let items = vec![item(600, 400)];
        let lines = anchored_image_lines(&items, 80, true, &AllFit);
        // Find the marker line and verify its geometry parse.
        let parsed: Vec<(u64, u16, u16)> = lines
            .iter()
            .filter_map(mermaid::parse_inline_image_placeholder)
            .collect();
        assert_eq!(parsed.len(), 1);
        let (hash, rows, cols) = parsed[0];
        assert_eq!(hash, 0xABCD);
        let (expected_rows, expected_cols) =
            fit_geometry_anchored(600, 400, 80, ImageExpandLevel::Fit);
        assert_eq!(rows, expected_rows);
        assert_eq!(cols, expected_cols);
        // Marker line is followed by rows-1 blank placeholder lines.
        let marker_idx = lines
            .iter()
            .position(|line| mermaid::parse_inline_image_placeholder(line).is_some())
            .unwrap();
        for offset in 1..rows as usize {
            let line = &lines[marker_idx + offset];
            assert!(
                jcode_tui_render::line_plain_text(line).trim().is_empty(),
                "placeholder row {offset} should be blank"
            );
        }
    }

    #[test]
    fn anchored_geometry_is_viewport_independent() {
        // The anchored fit must not depend on any viewport height so the body
        // cache (keyed by width only) stays valid across resizes.
        let (rows, cols) = fit_geometry_anchored(1920, 1080, 100, ImageExpandLevel::Fit);
        assert!(rows >= MIN_IMAGE_ROWS);
        assert!(rows <= ANCHORED_MAX_ROWS);
        assert!(cols <= 100);
    }
}
