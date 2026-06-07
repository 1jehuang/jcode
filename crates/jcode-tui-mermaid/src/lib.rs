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
use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque, hash_map::Entry};
use std::fs;
use std::hash::{Hash as _, Hasher};
use std::panic;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
#[path = "mermaid_runtime.rs"]
mod runtime;
#[path = "mermaid_viewport.rs"]
mod viewport_render;
#[path = "mermaid_widget.rs"]
mod widget_render;

pub use cache_render::{
    RenderResult, deferred_render_epoch, get_cached_path, is_mermaid_lang, render_mermaid,
    render_mermaid_deferred, render_mermaid_deferred_with_registration,
    render_mermaid_deferred_with_stream_scope, render_mermaid_sized, render_mermaid_untracked,
};
#[cfg(feature = "renderer")]
pub use content_render::terminal_theme;
pub use content_render::{
    MermaidContent, diagram_placeholder_lines, error_to_lines, estimate_image_height,
    image_widget_placeholder_markdown, parse_image_placeholder, result_to_content, result_to_lines,
    write_video_export_marker,
};
pub use runtime::{
    error_lines_for, get_cached_png, get_font_size, image_protocol_available, init_picker,
    is_video_export_mode, protocol_type, register_external_image, register_inline_image,
    set_video_export_mode,
};
pub use viewport_render::{
    invalidate_render_state, render_image_widget_viewport, render_image_widget_viewport_precise,
};
pub use widget_render::{render_image_widget, render_image_widget_fit, render_image_widget_scale};

#[cfg(test)]
use cache_render::calculate_render_size;
use cache_render::{
    CachedDiagram, MermaidCache, RENDER_CACHE_MAX, RENDER_WIDTH_BUCKET_CELLS,
    bump_deferred_render_epoch, get_cached_diagram,
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
    let dimensions = mmdr_measure_svg_dimensions(layout, layout_config, output_dimensions);
    let svg = mmdr_render_svg_with_dimensions(layout, theme, layout_config, output_dimensions);
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
const IMAGE_STATE_MAX: usize = 12;

/// Image state cache - holds StatefulProtocol for each rendered image
/// Keyed by content hash; source_path guards prevent stale reuse when
/// a higher-resolution PNG for the same hash replaces the old one.
static IMAGE_STATE: LazyLock<Mutex<ImageStateCache>> =
    LazyLock::new(|| Mutex::new(ImageStateCache::new()));

/// Cache decoded source images to avoid reloading from disk on every pan
static SOURCE_CACHE: LazyLock<Mutex<SourceImageCache>> =
    LazyLock::new(|| Mutex::new(SourceImageCache::new()));

/// Cache Kitty-specific viewport state so scroll-only updates can reuse the
/// same transmitted image data and adjust placeholders instead of rebuilding a
/// fresh cropped protocol payload on every tick.
static KITTY_VIEWPORT_STATE: LazyLock<Mutex<KittyViewportCache>> =
    LazyLock::new(|| Mutex::new(KittyViewportCache::new()));

/// Last render state for skip-redundant-render optimization
static LAST_RENDER: LazyLock<Mutex<HashMap<u64, LastRenderState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Render errors for lazy mermaid diagrams (hash -> error message)
static RENDER_ERRORS: LazyLock<Mutex<HashMap<u64, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Prevent unbounded growth when a long session contains many unique diagrams.
const ACTIVE_DIAGRAMS_MAX: usize = 128;

/// State for a rendered image
struct ImageState {
    protocol: StatefulProtocol,
    source_path: PathBuf,
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
}

impl ImageStateCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
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
        if let std::collections::hash_map::Entry::Occupied(mut entry) = self.entries.entry(hash) {
            entry.insert(state);
            self.touch(hash);
        } else {
            self.entries.insert(hash, state);
            self.order.push_back(hash);
            while self.order.len() > IMAGE_STATE_MAX {
                if let Some(old) = self.order.pop_front() {
                    self.entries.remove(&old);
                }
            }
        }
    }

    fn remove(&mut self, hash: &u64) {
        self.entries.remove(hash);
        if let Some(pos) = self.order.iter().position(|h| h == hash) {
            self.order.remove(pos);
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }

    fn iter(&self) -> impl Iterator<Item = (&u64, &ImageState)> {
        self.entries.iter()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
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
}

/// Cache decoded source images for fast viewport cropping
const SOURCE_CACHE_MAX: usize = 8;

struct SourceImageEntry {
    path: PathBuf,
    image: Arc<DynamicImage>,
}

struct SourceImageCache {
    order: VecDeque<u64>,
    entries: HashMap<u64, SourceImageEntry>,
}

struct KittyViewportState {
    source_path: PathBuf,
    zoom_percent: u8,
    font_size: (u16, u16),
    unique_id: u32,
    full_cols: u16,
    full_rows: u16,
    pending_transmit: Option<String>,
}

struct KittyViewportCache {
    entries: HashMap<u64, KittyViewportState>,
    order: VecDeque<u64>,
}

impl KittyViewportCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn touch(&mut self, hash: u64) {
        if let Some(pos) = self.order.iter().position(|h| *h == hash) {
            self.order.remove(pos);
        }
        self.order.push_back(hash);
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
        if let std::collections::hash_map::Entry::Occupied(mut entry) = self.entries.entry(hash) {
            entry.insert(state);
            self.touch(hash);
        } else {
            self.entries.insert(hash, state);
            self.order.push_back(hash);
            while self.order.len() > IMAGE_STATE_MAX {
                if let Some(old) = self.order.pop_front() {
                    self.entries.remove(&old);
                }
            }
        }
    }

    #[cfg(feature = "renderer")]
    fn remove(&mut self, hash: &u64) {
        self.entries.remove(hash);
        if let Some(pos) = self.order.iter().position(|h| h == hash) {
            self.order.remove(pos);
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }
}

impl SourceImageCache {
    fn new() -> Self {
        Self {
            order: VecDeque::new(),
            entries: HashMap::new(),
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
        let arc = Arc::new(image);
        self.entries.insert(
            hash,
            SourceImageEntry {
                path,
                image: arc.clone(),
            },
        );
        self.touch(hash);
        while self.order.len() > SOURCE_CACHE_MAX {
            if let Some(old) = self.order.pop_front() {
                self.entries.remove(&old);
            }
        }
        arc
    }

    fn remove(&mut self, hash: u64) {
        self.entries.remove(&hash);
        if let Some(pos) = self.order.iter().position(|h| *h == hash) {
            self.order.remove(pos);
        }
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
    /// Lower-bound estimate for image protocol buffers (derived from source PNG dimensions).
    pub image_state_protocol_min_estimate_bytes: u64,
    /// Number of decoded source images cached for viewport panning.
    pub source_cache_entries: usize,
    pub source_cache_limit: usize,
    /// Estimated decoded source image bytes (RGBA estimate).
    pub source_cache_decoded_estimate_bytes: u64,
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
    debug_flicker_benchmark, debug_image_state, debug_memory_benchmark, debug_memory_profile,
    debug_render, debug_stats, debug_stats_json, debug_test_render, debug_test_resize_stability,
    debug_test_scroll, reset_debug_stats,
};

fn hash_content(content: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;

    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

/// Get PNG dimensions from file
fn get_png_dimensions(path: &Path) -> Option<(u32, u32)> {
    let data = fs::read(path).ok()?;
    if data.len() > 24 && &data[0..8] == b"\x89PNG\r\n\x1a\n" {
        let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
        let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
        return Some((width, height));
    }
    None
}

/// Maximum age for cached files (3 days)
const CACHE_MAX_AGE_SECS: u64 = 3 * 24 * 60 * 60;

/// Maximum total cache size (50 MB)
const CACHE_MAX_SIZE_BYTES: u64 = 50 * 1024 * 1024;

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
        if path.extension().is_some_and(|e| e == "png")
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
        source.entries.clear();
        source.order.clear();
    }
    if let Ok(mut last) = LAST_RENDER.lock() {
        last.clear();
    }
}

#[cfg(test)]
#[path = "mermaid_tests.rs"]
mod tests;
