//! Mermaid diagram rendering for terminal display
//!
//! Renders mermaid diagrams to PNG images, then displays them using
//! ratatui-image which supports Kitty, Sixel, iTerm2, and halfblock protocols.
//! The protocol is auto-detected based on terminal capabilities.
//!
//! ## Optimizations
//! - Adaptive PNG sizing based on terminal dimensions and diagram complexity
//! - Pre-loaded StatefulProtocol during content preparation
//! - Fit mode for small terminals (scales to fit instead of cropping)
//! - Blocking locks for consistent rendering (no frame skipping)
//! - Skip redundant renders when nothing changed
//! - Clear only on render failure, not before every render

use jcode_tui_workspace::color_support::rgb;
#[path = "mermaid_active.rs"]
mod active;
#[path = "mermaid_debug.rs"]
mod debug_support;
#[path = "mermaid_svg.rs"]
mod svg;
use base64::Engine as _;
use image::DynamicImage;
use image::GenericImageView;
#[cfg(all(
    feature = "renderer",
    not(all(feature = "mmdr-size-api", mmdr_size_api_available))
))]
use mermaid_rs_renderer::render::render_svg;
#[cfg(all(
    feature = "renderer",
    feature = "mmdr-size-api",
    mmdr_size_api_available
))]
use mermaid_rs_renderer::render::{
    measure_svg_dimensions as mmdr_measure_svg_dimensions,
    render_svg_with_dimensions as mmdr_render_svg_with_dimensions,
};
#[cfg(feature = "renderer")]
use mermaid_rs_renderer::{
    config::{LayoutConfig, RenderConfig},
    layout::{Layout, compute_layout},
    parser::parse_mermaid,
    theme::Theme,
};
use ratatui::prelude::*;
use ratatui::widgets::StatefulWidget;
use ratatui_image::{
    CropOptions, Resize, ResizeEncodeRender, StatefulImage,
    picker::{Picker, ProtocolType, cap_parser::Parser},
    protocol::StatefulProtocol,
};
use serde::Serialize;
use std::borrow::Cow;
use std::cell::Cell;
use std::collections::{HashMap, VecDeque, hash_map::Entry};
use std::fs;
use std::hash::{Hash as _, Hasher};
use std::panic;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock, mpsc};
use std::time::Instant;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) struct RenderProfile {
    preferred_aspect_per_mille: Option<u16>,
}

impl RenderProfile {
    fn from_preferred_aspect_ratio(ratio: Option<f32>) -> Self {
        let preferred_aspect_per_mille = ratio
            .filter(|ratio| ratio.is_finite() && *ratio > 0.0)
            .map(|ratio| (ratio * 1000.0).round().clamp(100.0, 10_000.0) as u16);
        Self {
            preferred_aspect_per_mille,
        }
    }

    fn preferred_aspect_ratio(self) -> Option<f32> {
        self.preferred_aspect_per_mille
            .map(|value| value as f32 / 1000.0)
    }

    #[cfg(feature = "renderer")]
    fn cache_suffix(self) -> Option<String> {
        self.preferred_aspect_per_mille
            .map(|value| format!("_a{value}"))
    }
}

thread_local! {
    static RENDER_PROFILE_CONTEXT: Cell<RenderProfile> = Cell::new(RenderProfile::default());
}

fn current_render_profile() -> RenderProfile {
    RENDER_PROFILE_CONTEXT.with(|profile| profile.get())
}

pub fn current_preferred_aspect_ratio_bucket() -> Option<u16> {
    current_render_profile().preferred_aspect_per_mille
}

pub fn preferred_aspect_ratio_bucket(ratio: Option<f32>) -> Option<u16> {
    RenderProfile::from_preferred_aspect_ratio(ratio).preferred_aspect_per_mille
}

struct RenderProfileGuard {
    previous: RenderProfile,
}

impl Drop for RenderProfileGuard {
    fn drop(&mut self) {
        RENDER_PROFILE_CONTEXT.with(|profile| profile.set(self.previous));
    }
}

fn push_render_profile(profile: RenderProfile) -> RenderProfileGuard {
    let previous = RENDER_PROFILE_CONTEXT.with(|current| {
        let previous = current.get();
        current.set(profile);
        previous
    });
    RenderProfileGuard { previous }
}

pub fn with_preferred_aspect_ratio<R>(ratio: Option<f32>, f: impl FnOnce() -> R) -> R {
    let _guard = push_render_profile(RenderProfile::from_preferred_aspect_ratio(ratio));
    f()
}

#[derive(Debug, Clone)]
pub struct DiagramInfo {
    /// Hash for mermaid cache lookup
    pub hash: u64,
    /// Original PNG width
    pub width: u32,
    /// Original PNG height
    pub height: u32,
    /// Optional label/title
    pub label: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessMemorySnapshot {
    pub rss_bytes: Option<u64>,
    pub peak_rss_bytes: Option<u64>,
    pub virtual_bytes: Option<u64>,
}

static LOG_INFO_HOOK: OnceLock<fn(&str)> = OnceLock::new();
static LOG_WARN_HOOK: OnceLock<fn(&str)> = OnceLock::new();
static RENDER_COMPLETED_HOOK: OnceLock<fn()> = OnceLock::new();
static MEMORY_SNAPSHOT_HOOK: OnceLock<fn() -> ProcessMemorySnapshot> = OnceLock::new();

pub fn set_log_hooks(info: fn(&str), warn: fn(&str)) {
    let _ = LOG_INFO_HOOK.set(info);
    let _ = LOG_WARN_HOOK.set(warn);
}

pub fn set_render_completed_hook(hook: fn()) {
    let _ = RENDER_COMPLETED_HOOK.set(hook);
}

pub fn set_memory_snapshot_hook(hook: fn() -> ProcessMemorySnapshot) {
    let _ = MEMORY_SNAPSHOT_HOOK.set(hook);
}

pub(crate) fn log_info(message: &str) {
    if let Some(hook) = LOG_INFO_HOOK.get() {
        hook(message);
    }
}

pub(crate) fn log_warn(message: &str) {
    if let Some(hook) = LOG_WARN_HOOK.get() {
        hook(message);
    }
}

pub(crate) fn notify_render_completed() {
    if let Some(hook) = RENDER_COMPLETED_HOOK.get() {
        hook();
    }
}

pub(crate) fn process_memory_snapshot() -> ProcessMemorySnapshot {
    MEMORY_SNAPSHOT_HOOK
        .get()
        .map(|hook| hook())
        .unwrap_or_default()
}

pub(crate) fn panic_payload_to_string(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

pub use active::{
    active_diagram_count, clear_active_diagrams, clear_streaming_preview_diagram,
    get_active_diagrams, register_active_diagram, restore_active_diagrams,
    set_streaming_preview_diagram, snapshot_active_diagrams,
};

#[path = "mermaid_model.rs"]
mod model;
pub use model::{
    DiagramBlock, DiagramCacheKey, DiagramId, DiagramOrigin, DiagramRenderProfile,
    DiagramRenderRequest, MermaidTheme, RenderArtifact, RenderError, RenderMode, RenderPriority,
    RenderStatus, RenderTarget, normalize_aspect_ratio,
};

#[path = "mermaid_cache_render.rs"]
mod cache_render;
#[path = "mermaid_content.rs"]
mod content_render;
#[path = "mermaid_inline.rs"]
mod inline_image;
#[path = "mermaid_runtime.rs"]
mod runtime;
#[path = "mermaid_viewport.rs"]
mod viewport_render;
#[path = "mermaid_widget.rs"]
mod widget_render;

pub use cache_render::{
    RenderResult, debug_bump_deferred_render_epoch_for_tests, deferred_render_epoch,
    evict_render_cache_for_content, get_cached_path, is_mermaid_lang, render_mermaid,
    render_mermaid_deferred, render_mermaid_deferred_with_registration,
    render_mermaid_deferred_with_stream_scope, render_mermaid_sized, render_mermaid_untracked,
};
#[cfg(feature = "renderer")]
pub use content_render::terminal_theme;
pub use content_render::{
    INLINE_DIAGRAM_MAX_ROWS, INLINE_FIT_MIN_ROWS, MermaidContent, TERMINAL_IMAGE_FALLBACK_NOTE,
    diagram_placeholder_lines, error_to_lines, estimate_image_height,
    image_widget_placeholder_markdown, inline_fit_geometry, inline_image_placeholder_lines,
    inline_transcript_aspect_goal, inline_transcript_aspect_goal_with_font,
    parse_image_placeholder, parse_inline_image_placeholder, result_to_content, result_to_lines,
    text_image_fallback_note_line, transcript_preferred_aspect_ratio,
    transcript_preferred_aspect_ratio_with_font, write_video_export_marker,
};
pub use inline_image::{
    inline_image_dims, inline_image_id, inline_image_is_materialized, materialize_inline_image,
    materialize_inline_image_by_id, rediscover_inline_image,
};
pub use runtime::force_test_kitty_picker;
pub use runtime::{
    error_lines_for, get_cached_png, get_font_size, image_protocol_available, init_picker,
    is_video_export_mode, protocol_type, register_external_image, register_inline_image,
    set_video_export_mode, uses_text_image_fallback, with_image_protocol_override,
};
pub use viewport_render::{
    InlineFitReadiness, inline_fit_readiness, invalidate_render_state, prewarm_inline_fit_state,
    render_image_widget_fit_stable, render_image_widget_viewport,
    render_image_widget_viewport_precise,
};
pub use widget_render::{render_image_widget, render_image_widget_fit, render_image_widget_scale};

use cache_render::LAYOUT_CACHE_MAX;
#[cfg(test)]
use cache_render::calculate_render_size;
use cache_render::{
    CachedDiagram, MermaidCache, RENDER_CACHE_MAX, RENDER_WIDTH_BUCKET_CELLS,
    bump_deferred_render_epoch, clear_layout_cache, get_cached_diagram,
    get_cached_diagram_in_memory, get_cached_diagram_prefer_width, layout_cache_usage,
};
use viewport_render::clear_image_area;
use widget_render::{BORDER_WIDTH, draw_left_border, render_stateful_image_safe};

#[cfg(feature = "renderer")]
#[derive(Debug, Clone, Copy)]
struct MeasuredSvgDimensions {
    width: f32,
    height: f32,
    viewbox_width: f32,
    viewbox_height: f32,
}

#[cfg(all(
    feature = "renderer",
    not(all(feature = "mmdr-size-api", mmdr_size_api_available))
))]
fn measure_svg_dimensions_from_svg(
    svg_source: &str,
    output_dimensions: Option<(f32, f32)>,
) -> MeasuredSvgDimensions {
    let root_tag = svg_source
        .find("<svg")
        .and_then(|start| {
            let end = svg_source[start..].find('>')? + start;
            Some(&svg_source[start..=end])
        })
        .unwrap_or("");

    let (viewbox_width, viewbox_height) = svg::parse_svg_viewbox_size(root_tag)
        .or_else(|| svg::parse_svg_explicit_size(root_tag))
        .unwrap_or((DEFAULT_RENDER_WIDTH as f32, DEFAULT_RENDER_HEIGHT as f32));

    let (width, height) = if let Some((target_width, target_height)) = output_dimensions {
        let target_width = target_width.max(1.0);
        let target_height = target_height.max(1.0);
        let scale = (target_width / viewbox_width.max(1.0))
            .min(target_height / viewbox_height.max(1.0))
            .max(0.0001);
        (
            (viewbox_width * scale).max(1.0),
            (viewbox_height * scale).max(1.0),
        )
    } else {
        svg::parse_svg_explicit_size(root_tag).unwrap_or((viewbox_width, viewbox_height))
    };

    MeasuredSvgDimensions {
        width,
        height,
        viewbox_width,
        viewbox_height,
    }
}

#[cfg(all(
    feature = "renderer",
    not(all(feature = "mmdr-size-api", mmdr_size_api_available))
))]
fn render_svg_for_png(
    layout: &Layout,
    theme: &Theme,
    layout_config: &LayoutConfig,
    output_dimensions: Option<(f32, f32)>,
) -> (String, MeasuredSvgDimensions) {
    let svg_source = render_svg(layout, theme, layout_config);
    let dimensions = measure_svg_dimensions_from_svg(&svg_source, output_dimensions);
    let svg = if let Some((target_width, target_height)) = output_dimensions {
        svg::retarget_svg_for_png(&svg_source, target_width as f64, target_height as f64)
    } else {
        svg_source
    };
    (svg, dimensions)
}

#[cfg(all(
    feature = "renderer",
    feature = "mmdr-size-api",
    mmdr_size_api_available
))]
fn render_svg_for_png(
    layout: &Layout,
    theme: &Theme,
    layout_config: &LayoutConfig,
    output_dimensions: Option<(f32, f32)>,
) -> (String, MeasuredSvgDimensions) {
    // Measure the natural (content-hugging) canvas first, then fit it into the
    // requested target box while preserving aspect ratio. Forcing the raw
    // target dimensions letterboxes wide diagrams inside the ~4:3 request box:
    // the PNG (and therefore the transcript placeholder) reserves huge
    // transparent bands above/below the ink. This mirrors the legacy
    // `retarget_svg_for_png` fit semantics.
    let natural = mmdr_measure_svg_dimensions(layout, layout_config, None);
    let fitted = output_dimensions.map(|(target_width, target_height)| {
        let target_width = target_width.max(1.0);
        let target_height = target_height.max(1.0);
        let natural_width = natural.width.max(1.0);
        let natural_height = natural.height.max(1.0);
        let scale = (target_width / natural_width)
            .min(target_height / natural_height)
            .max(0.0001);
        (
            (natural_width * scale).max(1.0),
            (natural_height * scale).max(1.0),
        )
    });
    let dimensions = mmdr_measure_svg_dimensions(layout, layout_config, fitted);
    let svg = mmdr_render_svg_with_dimensions(layout, theme, layout_config, fitted);
    (
        svg,
        MeasuredSvgDimensions {
            width: dimensions.width,
            height: dimensions.height,
            viewbox_width: dimensions.viewbox_width,
            viewbox_height: dimensions.viewbox_height,
        },
    )
}

fn render_size_backend() -> &'static str {
    if cfg!(all(feature = "mmdr-size-api", mmdr_size_api_available)) {
        "mmdr-size-api"
    } else {
        "svg-retarget-fallback"
    }
}

/// Render Mermaid source images slightly denser than the immediate terminal-pixel
/// target so the terminal image protocol scales down from a sharper PNG without
/// making SVG-to-PNG rasterization dominate interactive frames.
const RENDER_SUPERSAMPLE: f64 = 1.1;
const DEFAULT_RENDER_WIDTH: u32 = 2400;
const DEFAULT_RENDER_HEIGHT: u32 = 1800;
const DEFAULT_PICKER_FONT_SIZE: (u16, u16) = (8, 16);

/// When true, mermaid placeholders include image hashes even without a
/// terminal image protocol (used by the video export pipeline so it can
/// embed cached PNGs into the SVG frames).
static VIDEO_EXPORT_MODE: AtomicBool = AtomicBool::new(false);

/// Global picker for terminal capability detection
/// Initialized once on first use
static PICKER: OnceLock<Option<Picker>> = OnceLock::new();

/// Track whether cache eviction has run
static CACHE_EVICTED: OnceLock<()> = OnceLock::new();

/// Cache for rendered mermaid diagrams
static RENDER_CACHE: LazyLock<Mutex<MermaidCache>> =
    LazyLock::new(|| Mutex::new(MermaidCache::new()));

/// Monotonic epoch bumped when a deferred background render completes.
/// UI markdown caches key off this so placeholder-only cached entries are
/// naturally refreshed on the next redraw.
static DEFERRED_RENDER_EPOCH: AtomicU64 = AtomicU64::new(1);

/// Count of `path.exists()`/`read_dir` filesystem stat syscalls performed by
/// the render-cache lookup paths. The inline-image scroll hot path used to pay
/// one of these per visible (and prefetched) image *per frame*, so this counter
/// makes that cost observable to the image-scroll benchmark and regression tests.
static CACHE_STAT_SYSCALLS: AtomicU64 = AtomicU64::new(0);

#[inline]
pub(crate) fn record_cache_stat_syscall() {
    CACHE_STAT_SYSCALLS.fetch_add(1, Ordering::Relaxed);
}

/// Total filesystem stat syscalls performed by the render-cache lookups so far.
pub fn cache_stat_syscalls() -> u64 {
    CACHE_STAT_SYSCALLS.load(Ordering::Relaxed)
}

type PendingRenderKey = (u64, u32, RenderProfile);
type PendingRenderMap = HashMap<PendingRenderKey, PendingDeferredRender>;

/// Background mermaid renders currently queued or in flight, keyed by
/// (content hash, target width, render profile).
static PENDING_RENDER_REQUESTS: LazyLock<Mutex<PendingRenderMap>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Sender for the shared deferred Mermaid render worker.
static DEFERRED_RENDER_TX: OnceLock<mpsc::Sender<DeferredRenderTask>> = OnceLock::new();
static SVG_FONT_DB_PREWARM_STARTED: OnceLock<()> = OnceLock::new();

/// Serialize the actual Mermaid parse/layout/png pipeline.
///
/// The render path temporarily swaps the panic hook around the renderer for
/// defense-in-depth, so we keep only one active render at a time. This also
/// prevents duplicate expensive work when a background streaming render and a
/// foreground final render race for the same diagram.
#[cfg(feature = "renderer")]
static RENDER_WORK_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Reuse a loaded system font database across Mermaid PNG renders.
/// Loading fonts dominates part of the cold PNG stage if done per render.
static SVG_FONT_DB: LazyLock<Arc<usvg::fontdb::Database>> = LazyLock::new(|| {
    let mut db = usvg::fontdb::Database::new();
    db.load_system_fonts();
    Arc::new(db)
});

/// Maximum number of StatefulProtocol entries to keep in IMAGE_STATE.
/// Each entry holds the full decoded+encoded image data and can consume
/// several MB of RAM (e.g. a 1440×1080 RGBA image ≈ 6 MB, plus protocol
/// encoding overhead).  Keeping this bounded prevents unbounded memory
/// growth over long sessions with many diagrams.
///
/// Sized to comfortably cover the viewport plus the look-ahead prefetch band
/// (see `ui_inline_image::prefetch`) so scrolling back through a transcript of
/// inline screenshots reuses warm protocol state instead of re-encoding.
const IMAGE_STATE_MAX: usize = 24;
/// Approximate source-pixel budget for `IMAGE_STATE`.
///
/// `ratatui-image::StatefulProtocol` retains the original decoded image in
/// addition to protocol-specific encoded data. A count-only cap therefore lets
/// a handful of 4K screenshots pin hundreds of MiB. The source-pixel budget is
/// intentionally conservative; encoded buffers are extra, so keeping decoded
/// sources below this line keeps the real cache working set bounded too.
const IMAGE_STATE_MAX_SOURCE_BYTES: usize = 48 * 1024 * 1024;

/// Maximum number of Kitty virtual-placement state entries to keep.
///
/// Unlike `IMAGE_STATE` (which holds full decoded+encoded `StatefulProtocol`
/// data), a steady-state `KittyViewportState` entry is tiny: once its one-shot
/// `pending_transmit` payload has been drawn it is just metadata (a path, a u32
/// id, and a few dimensions, ~100 bytes). The terminal itself retains the
/// transmitted pixels, so keeping the id->geometry mapping warm lets a scroll
/// back over a long transcript of screenshots re-address the existing image with
/// unicode placeholders instead of paying a synchronous decode + scale + base64
/// re-transmit. We therefore size this far larger than `IMAGE_STATE_MAX` so the
/// scroll working set for a screenshot-heavy session stays warm; the memory cost
/// of the extra metadata entries is negligible.
const KITTY_VIEWPORT_STATE_MAX: usize = 256;
/// Maximum encoded Kitty transmissions retained before their first draw.
///
/// A prewarmed state temporarily owns a base64 PNG escape payload. Count-only
/// eviction is not sufficient because a handful of high-resolution images can
/// otherwise retain hundreds of MiB while they are still off screen. Once a
/// state is drawn this drops to zero and only its tiny terminal id metadata
/// remains. As with the other byte-bounded caches, one oversized newest entry is
/// retained so a single large image can still make forward progress.
const KITTY_VIEWPORT_PENDING_MAX_BYTES: usize = 32 * 1024 * 1024;

/// Image state cache - holds StatefulProtocol for each rendered image
/// Keyed by content hash; source_path guards prevent stale reuse when
/// a higher-resolution PNG for the same hash replaces the old one.
static IMAGE_STATE: LazyLock<Mutex<ImageStateCache>> =
    LazyLock::new(|| Mutex::new(ImageStateCache::new()));

/// Cache decoded source images to avoid reloading from disk on every pan
static SOURCE_CACHE: LazyLock<Mutex<SourceImageCache>> =
    LazyLock::new(|| Mutex::new(SourceImageCache::new()));

/// Cache images pre-scaled to their inline placeholder geometry. Non-Kitty
/// protocols cannot re-address a terminal-retained image like Kitty can, but
/// keeping this bounded decoded source lets scroll-only updates crop the visible
/// rows without re-decoding or re-scaling the complete screenshot.
static FITTED_SOURCE_CACHE: LazyLock<Mutex<FittedSourceCache>> =
    LazyLock::new(|| Mutex::new(FittedSourceCache::new()));

/// Cache Kitty-specific viewport state so scroll-only updates can reuse the
/// same transmitted image data and adjust placeholders instead of rebuilding a
/// fresh cropped protocol payload on every tick.
static KITTY_VIEWPORT_STATE: LazyLock<Mutex<KittyViewportCache>> =
    LazyLock::new(|| Mutex::new(KittyViewportCache::new()));

/// Terminal image ids whose Kitty allocations should be deleted on the next
/// image draw. Eviction can happen while only cache locks are available, so the
/// actual escape sequence is deferred until a render buffer is being built.
static KITTY_PENDING_DELETE_IDS: LazyLock<Mutex<VecDeque<u32>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));

/// Monotonic process-local Kitty image id allocator. Folding a 64-bit content
/// hash into 32 bits allowed unrelated images to alias and overwrite each other.
static NEXT_KITTY_IMAGE_ID: AtomicU32 = AtomicU32::new(1);

fn queue_kitty_delete(unique_id: u32) {
    if unique_id == 0 {
        return;
    }
    if let Ok(mut pending) = KITTY_PENDING_DELETE_IDS.lock()
        && !pending.contains(&unique_id)
    {
        pending.push_back(unique_id);
    }
}

fn queue_kitty_delete_if_transmitted(state: &KittyViewportState) {
    // A pending transmit has never reached the terminal, so there is no terminal
    // allocation to reclaim. Skipping it also keeps the deferred-delete queue
    // bounded naturally during large offscreen prewarm bursts.
    if state.pending_transmit.is_none() {
        queue_kitty_delete(state.unique_id);
    }
}

fn take_kitty_delete_ids() -> Vec<u32> {
    KITTY_PENDING_DELETE_IDS
        .lock()
        .map(|mut pending| pending.drain(..).collect())
        .unwrap_or_default()
}

/// Last render state for skip-redundant-render optimization
static LAST_RENDER: LazyLock<Mutex<HashMap<u64, LastRenderState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Render errors for lazy mermaid diagrams (hash -> error message)
static RENDER_ERRORS: LazyLock<Mutex<HashMap<u64, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Cap for the LAST_RENDER / RENDER_ERRORS bookkeeping maps. Entries are tiny,
/// but both maps are keyed by content hash and previously grew without bound
/// over a long session. On overflow the map is simply cleared: LAST_RENDER only
/// powers a skipped-render stat, and a cleared RENDER_ERRORS entry just means
/// one failed diagram re-renders (and re-fails) once more.
pub(crate) const RENDER_BOOKKEEPING_MAX: usize = 1024;

/// Insert into a bounded bookkeeping map, clearing it if it would exceed
/// [`RENDER_BOOKKEEPING_MAX`] distinct keys.
pub(crate) fn bounded_bookkeeping_insert<V>(map: &mut HashMap<u64, V>, hash: u64, value: V) {
    if map.len() >= RENDER_BOOKKEEPING_MAX && !map.contains_key(&hash) {
        map.clear();
    }
    map.insert(hash, value);
}

/// Prevent unbounded growth when a long session contains many unique diagrams.
const ACTIVE_DIAGRAMS_MAX: usize = 128;

/// State for a rendered image
struct ImageState {
    protocol: StatefulProtocol,
    source_path: PathBuf,
    /// Exact bytes retained by the protocol's decoded source image before any
    /// protocol-specific encoding overhead.
    source_bytes: usize,
    /// The area this was last rendered to (for change detection)
    last_area: Option<Rect>,
    /// Resize mode locked at creation time (prevents flickering on scroll)
    resize_mode: ResizeMode,
    /// Whether the last render clipped from the top (to show bottom portion)
    last_crop_top: bool,
    /// Last viewport parameters (for pan/scroll)
    last_viewport: Option<ViewportState>,
}

/// LRU-bounded cache for ImageState entries.
struct ImageStateCache {
    entries: HashMap<u64, ImageState>,
    order: VecDeque<u64>,
    total_source_bytes: usize,
}

impl ImageStateCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            total_source_bytes: 0,
        }
    }

    fn touch(&mut self, hash: u64) {
        if let Some(pos) = self.order.iter().position(|h| *h == hash) {
            self.order.remove(pos);
        }
        self.order.push_back(hash);
    }

    fn get_mut(&mut self, hash: u64) -> Option<&mut ImageState> {
        if self.entries.contains_key(&hash) {
            self.touch(hash);
            self.entries.get_mut(&hash)
        } else {
            None
        }
    }

    fn get(&self, hash: &u64) -> Option<&ImageState> {
        self.entries.get(hash)
    }

    fn insert(&mut self, hash: u64, state: ImageState) {
        if let Some(old) = self.entries.remove(&hash) {
            self.total_source_bytes = self.total_source_bytes.saturating_sub(old.source_bytes);
            if let Some(pos) = self.order.iter().position(|h| *h == hash) {
                self.order.remove(pos);
            }
        }
        self.total_source_bytes = self.total_source_bytes.saturating_add(state.source_bytes);
        self.entries.insert(hash, state);
        self.order.push_back(hash);
        // Keep one oversized image rather than immediately evicting the state
        // that the caller is about to draw and entering a decode/rebuild loop.
        while (self.order.len() > IMAGE_STATE_MAX
            || self.total_source_bytes > IMAGE_STATE_MAX_SOURCE_BYTES)
            && self.order.len() > 1
        {
            if let Some(old) = self.order.pop_front()
                && let Some(old_state) = self.entries.remove(&old)
            {
                self.total_source_bytes = self
                    .total_source_bytes
                    .saturating_sub(old_state.source_bytes);
            }
        }
    }

    fn remove(&mut self, hash: &u64) {
        if let Some(old) = self.entries.remove(hash) {
            self.total_source_bytes = self.total_source_bytes.saturating_sub(old.source_bytes);
        }
        if let Some(pos) = self.order.iter().position(|h| h == hash) {
            self.order.remove(pos);
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.total_source_bytes = 0;
    }

    fn iter(&self) -> impl Iterator<Item = (&u64, &ImageState)> {
        self.entries.iter()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ViewportState {
    scroll_x_px: u32,
    scroll_y_px: u32,
    view_w_px: u32,
    view_h_px: u32,
}

/// Resize mode for images - locked at creation time
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResizeMode {
    Fit,
    Scale,
    Crop,
    Viewport,
    FitViewport,
}

/// Cache decoded source images for fast viewport cropping.
///
/// Sized to cover the viewport plus the inline-image look-ahead prefetch band
/// so scrolling back over recently seen screenshots reuses the decoded pixels
/// instead of re-opening and re-decoding the cached PNG from disk.
const SOURCE_CACHE_MAX: usize = 16;
/// Exact decoded-byte budget for source images used by viewport and fit paths.
/// Count-only bounding is unsafe for heterogeneous images: sixteen 4K RGBA
/// screenshots are already roughly 500 MiB before allocator overhead.
const SOURCE_CACHE_MAX_BYTES: usize = 48 * 1024 * 1024;

/// Pre-scaled sources are normally much smaller than their originals because
/// they are bounded by the inline placeholder. Keep a modest working set for
/// back-scrolling while preventing terminal resizes from accumulating variants.
const FITTED_SOURCE_CACHE_MAX: usize = 16;
const FITTED_SOURCE_CACHE_MAX_BYTES: usize = 32 * 1024 * 1024;

struct SourceImageEntry {
    path: PathBuf,
    image: Arc<DynamicImage>,
    decoded_bytes: usize,
}

struct SourceImageCache {
    order: VecDeque<u64>,
    entries: HashMap<u64, SourceImageEntry>,
    total_decoded_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct FittedSourceKey {
    hash: u64,
    target_cols: u16,
    target_rows: u16,
    font_size: (u16, u16),
}

struct FittedSourceEntry {
    source_path: PathBuf,
    image: Arc<DynamicImage>,
    decoded_bytes: usize,
}

struct FittedSourceCache {
    order: VecDeque<FittedSourceKey>,
    entries: HashMap<FittedSourceKey, FittedSourceEntry>,
    total_decoded_bytes: usize,
}

struct KittyViewportState {
    source_path: PathBuf,
    zoom_percent: u8,
    font_size: (u16, u16),
    unique_id: u32,
    full_cols: u16,
    full_rows: u16,
    pending_transmit: Option<String>,
    /// Exact bytes currently held by `pending_transmit`. Stored explicitly so
    /// cache accounting remains cheap and tests can exercise budget eviction
    /// without allocating multi-megabyte strings.
    pending_transmit_bytes: usize,
    /// `Some((cols, rows))` when this entry was built by the inline fit path
    /// (image pre-scaled to fit a placeholder region); `None` for the zoomable
    /// diagram viewport path. Keeps the two users of this cache from
    /// mistaking each other's transmitted pixels.
    fit_target: Option<(u16, u16)>,
}

struct KittyViewportCache {
    entries: HashMap<u64, KittyViewportState>,
    /// Monotonic recency stamps make the per-frame hit path O(1). Finding the
    /// oldest entry is O(n) only during insertion/eviction, where PNG scaling
    /// and encoding dominate and the cache is capped at 256 entries.
    recency: HashMap<u64, u64>,
    clock: u64,
    total_pending_transmit_bytes: usize,
}

impl KittyViewportCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            recency: HashMap::new(),
            clock: 0,
            total_pending_transmit_bytes: 0,
        }
    }

    fn touch(&mut self, hash: u64) {
        self.clock = self.clock.saturating_add(1);
        self.recency.insert(hash, self.clock);
    }

    fn oldest_hash(&self) -> Option<u64> {
        self.recency
            .iter()
            .min_by_key(|(_, stamp)| *stamp)
            .map(|(hash, _)| *hash)
    }

    fn get_mut(&mut self, hash: u64) -> Option<&mut KittyViewportState> {
        if self.entries.contains_key(&hash) {
            self.touch(hash);
            self.entries.get_mut(&hash)
        } else {
            None
        }
    }

    fn insert(&mut self, hash: u64, state: KittyViewportState) {
        let pending_bytes = state.pending_transmit_bytes;
        if let std::collections::hash_map::Entry::Occupied(mut entry) = self.entries.entry(hash) {
            let old = entry.insert(state);
            self.total_pending_transmit_bytes = self
                .total_pending_transmit_bytes
                .saturating_sub(old.pending_transmit_bytes)
                .saturating_add(pending_bytes);
            if entry.get().unique_id != old.unique_id {
                queue_kitty_delete_if_transmitted(&old);
            }
            self.touch(hash);
        } else {
            self.entries.insert(hash, state);
            self.total_pending_transmit_bytes = self
                .total_pending_transmit_bytes
                .saturating_add(pending_bytes);
            self.touch(hash);
        }
        while (self.entries.len() > KITTY_VIEWPORT_STATE_MAX
            || self.total_pending_transmit_bytes > KITTY_VIEWPORT_PENDING_MAX_BYTES)
            && self.entries.len() > 1
        {
            if let Some(old) = self.oldest_hash()
                && let Some(old_state) = self.entries.remove(&old)
            {
                self.recency.remove(&old);
                self.total_pending_transmit_bytes = self
                    .total_pending_transmit_bytes
                    .saturating_sub(old_state.pending_transmit_bytes);
                queue_kitty_delete_if_transmitted(&old_state);
            }
        }
    }

    fn take_pending_transmit(&mut self, hash: u64) -> Option<(u32, Option<String>)> {
        let state = self.get_mut(hash)?;
        let unique_id = state.unique_id;
        let pending = state.pending_transmit.take();
        let pending_bytes = std::mem::take(&mut state.pending_transmit_bytes);
        self.total_pending_transmit_bytes = self
            .total_pending_transmit_bytes
            .saturating_sub(pending_bytes);
        Some((unique_id, pending))
    }

    #[cfg(feature = "renderer")]
    fn remove(&mut self, hash: &u64) {
        if let Some(state) = self.entries.remove(hash) {
            self.total_pending_transmit_bytes = self
                .total_pending_transmit_bytes
                .saturating_sub(state.pending_transmit_bytes);
            queue_kitty_delete_if_transmitted(&state);
        }
        self.recency.remove(hash);
    }

    fn clear(&mut self) {
        for state in self.entries.values() {
            queue_kitty_delete_if_transmitted(state);
        }
        self.entries.clear();
        self.recency.clear();
        self.clock = 0;
        self.total_pending_transmit_bytes = 0;
    }
}

impl SourceImageCache {
    fn new() -> Self {
        Self {
            order: VecDeque::new(),
            entries: HashMap::new(),
            total_decoded_bytes: 0,
        }
    }

    fn touch(&mut self, hash: u64) {
        if let Some(pos) = self.order.iter().position(|h| *h == hash) {
            self.order.remove(pos);
        }
        self.order.push_back(hash);
    }

    fn get(&mut self, hash: u64, expected_path: &Path) -> Option<Arc<DynamicImage>> {
        let img = match self.entries.get(&hash) {
            Some(entry) if entry.path == expected_path => Some(entry.image.clone()),
            Some(_) => {
                self.remove(hash);
                None
            }
            None => None,
        };
        if img.is_some() {
            self.touch(hash);
        }
        img
    }

    fn insert(&mut self, hash: u64, path: PathBuf, image: DynamicImage) -> Arc<DynamicImage> {
        let decoded_bytes = image.as_bytes().len();
        self.insert_with_decoded_bytes(hash, path, image, decoded_bytes)
    }

    fn insert_with_decoded_bytes(
        &mut self,
        hash: u64,
        path: PathBuf,
        image: DynamicImage,
        decoded_bytes: usize,
    ) -> Arc<DynamicImage> {
        self.remove(hash);
        let arc = Arc::new(image);
        self.total_decoded_bytes = self.total_decoded_bytes.saturating_add(decoded_bytes);
        self.entries.insert(
            hash,
            SourceImageEntry {
                path,
                image: arc.clone(),
                decoded_bytes,
            },
        );
        self.touch(hash);
        // Preserve one oversized source so a single large image can still draw
        // without thrashing between decode and immediate eviction.
        while (self.order.len() > SOURCE_CACHE_MAX
            || self.total_decoded_bytes > SOURCE_CACHE_MAX_BYTES)
            && self.order.len() > 1
        {
            if let Some(old) = self.order.pop_front()
                && let Some(old_entry) = self.entries.remove(&old)
            {
                self.total_decoded_bytes = self
                    .total_decoded_bytes
                    .saturating_sub(old_entry.decoded_bytes);
            }
        }
        arc
    }

    fn remove(&mut self, hash: u64) {
        if let Some(old) = self.entries.remove(&hash) {
            self.total_decoded_bytes = self.total_decoded_bytes.saturating_sub(old.decoded_bytes);
        }
        if let Some(pos) = self.order.iter().position(|h| *h == hash) {
            self.order.remove(pos);
        }
    }

    fn remove_if_path(&mut self, hash: u64, expected_path: &Path) {
        if self
            .entries
            .get(&hash)
            .is_some_and(|entry| entry.path == expected_path)
        {
            self.remove(hash);
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.total_decoded_bytes = 0;
    }
}

impl FittedSourceCache {
    fn new() -> Self {
        Self {
            order: VecDeque::new(),
            entries: HashMap::new(),
            total_decoded_bytes: 0,
        }
    }

    fn touch(&mut self, key: FittedSourceKey) {
        if let Some(pos) = self.order.iter().position(|entry| *entry == key) {
            self.order.remove(pos);
        }
        self.order.push_back(key);
    }

    fn get(&mut self, key: FittedSourceKey, source_path: &Path) -> Option<Arc<DynamicImage>> {
        let image = match self.entries.get(&key) {
            Some(entry) if entry.source_path == source_path => Some(entry.image.clone()),
            Some(_) => {
                self.remove_key(key);
                None
            }
            None => None,
        };
        if image.is_some() {
            self.touch(key);
        }
        image
    }

    fn insert(
        &mut self,
        key: FittedSourceKey,
        source_path: PathBuf,
        image: DynamicImage,
    ) -> Arc<DynamicImage> {
        let decoded_bytes = image.as_bytes().len();
        self.insert_with_decoded_bytes(key, source_path, image, decoded_bytes)
    }

    fn insert_with_decoded_bytes(
        &mut self,
        key: FittedSourceKey,
        source_path: PathBuf,
        image: DynamicImage,
        decoded_bytes: usize,
    ) -> Arc<DynamicImage> {
        // Only one placeholder geometry per image is useful after a resize. Drop
        // older variants immediately rather than waiting for global LRU pressure.
        self.remove_hash(key.hash);
        let image = Arc::new(image);
        self.total_decoded_bytes = self.total_decoded_bytes.saturating_add(decoded_bytes);
        self.entries.insert(
            key,
            FittedSourceEntry {
                source_path,
                image: image.clone(),
                decoded_bytes,
            },
        );
        self.order.push_back(key);
        // Preserve one oversized fitted source so the image remains drawable
        // instead of cycling through scale -> immediate eviction on every frame.
        while (self.order.len() > FITTED_SOURCE_CACHE_MAX
            || self.total_decoded_bytes > FITTED_SOURCE_CACHE_MAX_BYTES)
            && self.order.len() > 1
        {
            if let Some(old) = self.order.pop_front()
                && let Some(entry) = self.entries.remove(&old)
            {
                self.total_decoded_bytes =
                    self.total_decoded_bytes.saturating_sub(entry.decoded_bytes);
            }
        }
        image
    }

    fn remove_key(&mut self, key: FittedSourceKey) {
        if let Some(entry) = self.entries.remove(&key) {
            self.total_decoded_bytes = self.total_decoded_bytes.saturating_sub(entry.decoded_bytes);
        }
        if let Some(pos) = self.order.iter().position(|entry| *entry == key) {
            self.order.remove(pos);
        }
    }

    fn remove_hash(&mut self, hash: u64) {
        let keys: Vec<_> = self
            .entries
            .keys()
            .filter(|key| key.hash == hash)
            .copied()
            .collect();
        for key in keys {
            self.remove_key(key);
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.total_decoded_bytes = 0;
    }
}

/// Track what was rendered last frame for skip-redundant optimization
#[derive(Debug, Clone, PartialEq, Eq)]
struct LastRenderState {
    area: Rect,
    crop_top: bool,
    resize_mode: ResizeMode,
}

/// Debug stats for mermaid rendering
#[derive(Debug, Clone, Default, Serialize)]
pub struct MermaidDebugStats {
    pub total_requests: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    /// Layout-tier cache hits: the PNG cache missed but the computed layout
    /// was reused, so only SVG+PNG rasterization ran (no parse/compute_layout).
    pub layout_cache_hits: u64,
    /// Layout-tier cache misses: full parse + compute_layout executed.
    pub layout_cache_misses: u64,
    pub deferred_enqueued: u64,
    pub deferred_deduped: u64,
    pub deferred_superseded: u64,
    pub deferred_worker_renders: u64,
    pub deferred_worker_skips: u64,
    pub deferred_epoch_bumps: u64,
    pub render_success: u64,
    pub render_errors: u64,
    pub last_render_ms: Option<f32>,
    pub last_parse_ms: Option<f32>,
    pub last_layout_ms: Option<f32>,
    pub last_svg_ms: Option<f32>,
    pub last_png_ms: Option<f32>,
    pub last_error: Option<String>,
    pub last_hash: Option<String>,
    pub last_nodes: Option<usize>,
    pub last_edges: Option<usize>,
    pub last_content_len: Option<usize>,
    pub image_state_hits: u64,
    pub image_state_misses: u64,
    pub skipped_renders: u64,
    pub fit_state_reuse_hits: u64,
    pub fit_protocol_rebuilds: u64,
    pub viewport_state_reuse_hits: u64,
    pub viewport_protocol_rebuilds: u64,
    pub clear_operations: u64,
    pub last_image_render_ms: Option<f32>,
    pub cache_entries: usize,
    pub cache_dir: Option<String>,
    pub protocol: Option<String>,
    pub render_size_backend: &'static str,
    pub last_png_width: Option<u32>,
    pub last_png_height: Option<u32>,
    pub last_measured_width: Option<u32>,
    pub last_measured_height: Option<u32>,
    pub last_viewbox_width: Option<u32>,
    pub last_viewbox_height: Option<u32>,
    pub last_target_width: Option<u32>,
    pub last_target_height: Option<u32>,
    pub deferred_pending: usize,
    pub deferred_epoch: u64,
    /// Layout-tier cache resident entries (see `layout_cache_hits`).
    pub layout_cache_entries: usize,
    pub layout_cache_limit: usize,
    /// Approximate resident bytes held by cached layouts.
    pub layout_cache_approx_bytes: u64,
}

#[derive(Debug, Clone, Default)]
struct MermaidDebugState {
    stats: MermaidDebugStats,
}

static MERMAID_DEBUG: LazyLock<Mutex<MermaidDebugState>> =
    LazyLock::new(|| Mutex::new(MermaidDebugState::default()));

#[derive(Debug, Clone, Default)]
struct PendingDeferredRender {
    register_active: bool,
    terminal_width: Option<u16>,
    content: String,
    stream_scope: Option<u64>,
}

#[derive(Debug, Clone)]
struct DeferredRenderTask {
    content: String,
    terminal_width: Option<u16>,
    render_key: (u64, u32, RenderProfile),
}

#[cfg(feature = "renderer")]
#[derive(Debug, Clone, Copy, Default)]
struct RenderStageBreakdown {
    parse_ms: f32,
    layout_ms: f32,
    svg_ms: f32,
    png_ms: f32,
    measured_width: u32,
    measured_height: u32,
    viewbox_width: u32,
    viewbox_height: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct MermaidCacheEntry {
    pub hash: String,
    pub path: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct MermaidMemoryProfile {
    /// Resident set size for the current process (if available from OS).
    pub process_rss_bytes: Option<u64>,
    /// Peak resident set size for the current process (if available from OS).
    pub process_peak_rss_bytes: Option<u64>,
    /// Virtual memory size for the current process (if available from OS).
    pub process_virtual_bytes: Option<u64>,
    /// Number of render-cache entries currently resident in memory.
    pub render_cache_entries: usize,
    pub render_cache_limit: usize,
    /// Rough in-memory size of render-cache metadata (paths + structs), not image bytes.
    pub render_cache_metadata_estimate_bytes: u64,
    /// Number of image protocol states currently cached.
    pub image_state_entries: usize,
    pub image_state_limit: usize,
    /// Maximum decoded source bytes retained by image protocol states.
    pub image_state_source_limit_bytes: usize,
    /// Lower-bound estimate for image protocol buffers (derived from source PNG dimensions).
    pub image_state_protocol_min_estimate_bytes: u64,
    /// Number of decoded source images cached for viewport panning.
    pub source_cache_entries: usize,
    pub source_cache_limit: usize,
    /// Maximum exact decoded bytes retained by the source-image cache.
    pub source_cache_limit_bytes: usize,
    /// Estimated decoded source image bytes (RGBA estimate).
    pub source_cache_decoded_estimate_bytes: u64,
    /// Number of non-Kitty sources pre-scaled to inline placeholder geometry.
    pub fitted_source_cache_entries: usize,
    pub fitted_source_cache_limit: usize,
    pub fitted_source_cache_limit_bytes: usize,
    /// Exact decoded bytes held by pre-scaled non-Kitty sources.
    pub fitted_source_cache_decoded_bytes: u64,
    /// Kitty virtual-placement states and exact not-yet-drawn transmit bytes.
    pub kitty_viewport_state_entries: usize,
    pub kitty_viewport_state_limit: usize,
    pub kitty_pending_transmit_bytes: u64,
    pub kitty_pending_transmit_limit_bytes: usize,
    /// Number of active diagrams in the pinned-diagram list.
    pub active_diagrams: usize,
    pub active_diagrams_limit: usize,
    /// On-disk cache size under the mermaid cache directory.
    pub cache_disk_png_files: usize,
    pub cache_disk_png_bytes: u64,
    pub cache_disk_limit_bytes: u64,
    pub cache_disk_max_age_secs: u64,
    /// Mermaid-specific working set estimate (cache metadata + protocol floor + decoded source).
    pub mermaid_working_set_estimate_bytes: u64,
    /// Number of computed layouts cached in the layout tier.
    pub layout_cache_entries: usize,
    pub layout_cache_limit: usize,
    /// Approximate resident bytes held by cached layouts (nodes+edges+labels walk).
    pub layout_cache_approx_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MermaidMemoryBenchmark {
    pub iterations: usize,
    pub errors: usize,
    pub before: MermaidMemoryProfile,
    pub after: MermaidMemoryProfile,
    pub rss_delta_bytes: Option<i64>,
    pub working_set_delta_bytes: i64,
    pub peak_rss_bytes: Option<u64>,
    pub peak_working_set_estimate_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MermaidTimingSummary {
    pub avg_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MermaidFlickerBenchmark {
    pub protocol_supported: bool,
    pub protocol: Option<String>,
    pub steps: usize,
    pub changed_viewports: usize,
    pub fit_frames: usize,
    pub viewport_frames: usize,
    pub fit_timing: MermaidTimingSummary,
    pub viewport_timing: MermaidTimingSummary,
    pub deltas: MermaidDebugStatsDelta,
    pub viewport_protocol_rebuild_rate: f64,
    pub fit_protocol_rebuild_rate: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImageScrollBenchmark {
    /// Protocol the benchmark ran against (e.g. "Kitty").
    pub protocol: Option<String>,
    /// Number of distinct inline images in the simulated transcript.
    pub images: usize,
    /// Number of simulated scroll frames.
    pub frames: usize,
    /// Images visible per frame (drives the per-frame draw cost).
    pub visible_per_frame: usize,
    /// Per-frame UI-thread wall time across the scroll (ms).
    pub frame_timing: MermaidTimingSummary,
    /// Filesystem stat syscalls performed by render-cache lookups during the
    /// scroll (the cost this benchmark was built to catch). Steady-state
    /// scrolling should approach zero.
    pub cache_stat_syscalls: u64,
    /// Stat syscalls per rendered frame (cache_stat_syscalls / frames).
    pub cache_stat_syscalls_per_frame: f64,
    /// Frames where a visible image was not yet warm, so the UI thread skipped
    /// the draw and scheduled an off-thread prewarm (the "blank then pop" hitch
    /// the look-ahead prefetch is meant to eliminate).
    pub visible_draw_skips: u64,
    /// Kitty fit-state rebuilds during the scroll (decode + scale + transmit).
    /// Steady-state re-scrolling within the cache working set should be zero.
    pub fit_protocol_rebuilds: u64,
    /// Cheap fit-state reuse hits during the scroll.
    pub fit_state_reuse_hits: u64,
    /// Exact decoded source bytes retained by cached protocol states.
    pub retained_image_state_source_bytes: u64,
    /// Exact decoded bytes retained by full source images after the benchmark.
    pub retained_source_cache_decoded_bytes: u64,
    /// Exact decoded bytes retained by pre-scaled non-Kitty sources.
    pub retained_fitted_source_decoded_bytes: u64,
    /// Lower-bound total for Mermaid/image-owned memory after the benchmark.
    pub retained_working_set_estimate_bytes: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct MermaidDebugStatsDelta {
    pub image_state_hits: u64,
    pub image_state_misses: u64,
    pub skipped_renders: u64,
    pub fit_state_reuse_hits: u64,
    pub fit_protocol_rebuilds: u64,
    pub viewport_state_reuse_hits: u64,
    pub viewport_protocol_rebuilds: u64,
    pub clear_operations: u64,
}

mod debug;

pub use debug::{
    ImageStateInfo, ScrollFrameInfo, ScrollTestResult, TestRenderResult, clear_cache, debug_cache,
    debug_flicker_benchmark, debug_image_scroll_benchmark, debug_image_state,
    debug_memory_benchmark, debug_memory_profile, debug_render, debug_stats, debug_stats_json,
    debug_test_render, debug_test_resize_stability, debug_test_scroll, reset_debug_stats,
};

fn hash_content(content: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;

    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

/// Get PNG dimensions from file
fn get_png_dimensions(path: &Path) -> Option<(u32, u32)> {
    use std::io::Read as _;

    let mut header = [0u8; 24];
    let mut file = fs::File::open(path).ok()?;
    file.read_exact(&mut header).ok()?;
    if &header[0..8] == b"\x89PNG\r\n\x1a\n" {
        let width = u32::from_be_bytes([header[16], header[17], header[18], header[19]]);
        let height = u32::from_be_bytes([header[20], header[21], header[22], header[23]]);
        return Some((width, height));
    }
    None
}

/// Maximum age for cached files (3 days)
const CACHE_MAX_AGE_SECS: u64 = 3 * 24 * 60 * 60;

/// Maximum total cache size (50 MB)
const CACHE_MAX_SIZE_BYTES: u64 = 50 * 1024 * 1024;

/// File extensions the render/materialize paths write into the cache dir.
/// `evict_old_cache` must recognize every one of them: inline images keep
/// their source container format (`{hash}_inline.jpg` etc.), so an extension
/// missing here would never be evicted and leak on disk forever.
const CACHE_FILE_EXTENSIONS: [&str; 7] = ["png", "jpg", "gif", "webp", "bmp", "ico", "img"];

/// Evict old cache files on startup.
pub fn evict_old_cache() {
    let cache_dir = match RENDER_CACHE.lock() {
        Ok(cache) => cache.cache_dir.clone(),
        Err(_) => return,
    };

    let Ok(entries) = fs::read_dir(&cache_dir) else {
        return;
    };

    let now = std::time::SystemTime::now();
    let mut files: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    let mut total_size: u64 = 0;

    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| CACHE_FILE_EXTENSIONS.contains(&e))
            && let Ok(meta) = entry.metadata()
        {
            let size = meta.len();
            let modified = meta.modified().unwrap_or(now);
            files.push((path, size, modified));
            total_size += size;
        }
    }

    // Sort by modification time (oldest first)
    files.sort_by_key(|(_, _, modified)| *modified);

    let mut deleted_bytes: u64 = 0;

    for (path, size, modified) in &files {
        let age = now.duration_since(*modified).unwrap_or_default();
        let should_delete = age.as_secs() > CACHE_MAX_AGE_SECS
            || (total_size - deleted_bytes) > CACHE_MAX_SIZE_BYTES;

        if should_delete && fs::remove_file(path).is_ok() {
            deleted_bytes += size;
        }
    }
}

/// Clear image state (call on app exit to free memory)
pub fn clear_image_state() {
    if let Ok(mut state) = IMAGE_STATE.lock() {
        state.clear();
    }
    if let Ok(mut source) = SOURCE_CACHE.lock() {
        source.clear();
    }
    if let Ok(mut fitted) = FITTED_SOURCE_CACHE.lock() {
        fitted.clear();
    }
    if let Ok(mut kitty) = KITTY_VIEWPORT_STATE.lock() {
        kitty.clear();
    }
    if let Ok(mut last) = LAST_RENDER.lock() {
        last.clear();
    }
}

/// Take terminal control sequences that delete Kitty image allocations evicted
/// from the in-process caches. Exposed for TUI teardown, where there may be no
/// later frame in which an image widget can carry the deferred cleanup.
pub fn take_terminal_image_cleanup_payload() -> String {
    viewport_render::take_kitty_delete_payloads()
}

/// Attach pending Kitty deletion commands to the first cell of an ordinary TUI
/// frame. Escape sequences are zero-width, so preserving the original symbol
/// keeps the rendered frame visually unchanged even when no image remains.
pub fn render_pending_terminal_image_cleanup(buf: &mut Buffer) -> bool {
    let area = *buf.area();
    if area.width == 0 || area.height == 0 {
        return false;
    }
    let payload = take_terminal_image_cleanup_payload();
    if payload.is_empty() {
        return false;
    }
    let Some(cell) = buf.cell_mut((area.left(), area.top())) else {
        return false;
    };
    let existing = cell.symbol().to_string();
    let mut symbol = String::with_capacity(payload.len() + existing.len());
    symbol.push_str(&payload);
    symbol.push_str(&existing);
    cell.set_symbol(&symbol);
    true
}

#[cfg(test)]
static IMAGE_TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
#[path = "mermaid_tests.rs"]
mod tests;
