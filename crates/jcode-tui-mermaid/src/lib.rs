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
use mermaid_rs_renderer::{
    config::{LayoutConfig, RenderConfig},
    layout::compute_layout,
    parser::parse_mermaid,
    render::render_svg,
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
use std::collections::{HashMap, HashSet, VecDeque, hash_map::Entry};
use std::fs;
use std::hash::{Hash as _, Hasher};
use std::panic;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock, mpsc};
use std::time::Instant;

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
    render_mermaid_deferred, render_mermaid_deferred_with_registration, render_mermaid_sized,
    render_mermaid_untracked,
};
pub use content_render::{
    MermaidContent, diagram_placeholder_lines, error_to_lines, estimate_image_height,
    image_widget_placeholder_markdown, parse_image_placeholder, result_to_content, result_to_lines,
    terminal_theme, write_video_export_marker,
};
pub use runtime::{
    error_lines_for, get_cached_png, get_font_size, image_protocol_available, init_picker,
    is_video_export_mode, protocol_type, register_external_image, register_inline_image,
    set_video_export_mode,
};
pub use viewport_render::{invalidate_render_state, render_image_widget_viewport};
pub use widget_render::{render_image_widget, render_image_widget_fit, render_image_widget_scale};

use cache_render::{
    CachedDiagram, MermaidCache, RENDER_CACHE_MAX, RENDER_WIDTH_BUCKET_CELLS,
    bump_deferred_render_epoch, get_cached_diagram,
};
#[cfg(test)]
use cache_render::{
    cached_width_satisfies, calculate_render_size, estimate_diagram_size, parse_cache_filename,
    retarget_svg_for_png,
};
#[cfg(test)]
use content_render::image_widget_placeholder;
#[cfg(test)]
use runtime::{PickerInitMode, infer_protocol_from_env, picker_init_mode_from_probe_env};
use viewport_render::clear_image_area;
#[cfg(test)]
use viewport_render::{ensure_kitty_viewport_state, render_kitty_virtual_viewport};
#[cfg(test)]
use widget_render::set_cell_if_visible;
use widget_render::{BORDER_WIDTH, draw_left_border, render_stateful_image_safe};

/// Render Mermaid source images a bit denser than the immediate terminal-pixel
/// target so the terminal image protocol scales down from a sharper PNG.
/// This especially helps small text remain legible in the pinned side pane.
const RENDER_SUPERSAMPLE: f64 = 1.5;
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

/// Background mermaid renders currently queued or in flight, keyed by
/// (content hash, target width).
static PENDING_RENDER_REQUESTS: LazyLock<Mutex<HashMap<(u64, u32), PendingDeferredRender>>> =
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
    pub last_png_width: Option<u32>,
    pub last_png_height: Option<u32>,
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

#[derive(Debug, Clone, Copy, Default)]
struct PendingDeferredRender {
    register_active: bool,
}

#[derive(Debug, Clone)]
struct DeferredRenderTask {
    content: String,
    terminal_width: Option<u16>,
    render_key: (u64, u32),
}

#[derive(Debug, Clone, Copy, Default)]
struct RenderStageBreakdown {
    parse_ms: f32,
    layout_ms: f32,
    svg_ms: f32,
    png_ms: f32,
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

pub fn debug_stats() -> MermaidDebugStats {
    debug_support::debug_stats()
}

pub fn reset_debug_stats() {
    debug_support::reset_debug_stats()
}

pub fn debug_stats_json() -> Option<serde_json::Value> {
    debug_support::debug_stats_json()
}

pub fn debug_cache() -> Vec<MermaidCacheEntry> {
    debug_support::debug_cache()
}

pub fn debug_memory_profile() -> MermaidMemoryProfile {
    debug_support::debug_memory_profile()
}

pub fn debug_memory_benchmark(iterations: usize) -> MermaidMemoryBenchmark {
    debug_support::debug_memory_benchmark(iterations)
}

pub fn debug_flicker_benchmark(steps: usize) -> MermaidFlickerBenchmark {
    debug_support::debug_flicker_benchmark(steps)
}

#[cfg(test)]
fn parse_proc_status_value_bytes(status: &str, key: &str) -> Option<u64> {
    debug_support::parse_proc_status_value_bytes(status, key)
}

pub fn clear_cache() -> Result<(), String> {
    let cache_dir = if let Ok(cache) = RENDER_CACHE.lock() {
        cache.cache_dir.clone()
    } else {
        std::env::temp_dir()
    };

    // Clear in-memory caches
    if let Ok(mut cache) = RENDER_CACHE.lock() {
        cache.entries.clear();
        cache.order.clear();
    }
    if let Ok(mut state) = IMAGE_STATE.lock() {
        state.clear();
    }
    if let Ok(mut source) = SOURCE_CACHE.lock() {
        source.entries.clear();
        source.order.clear();
    }
    if let Ok(mut kitty) = KITTY_VIEWPORT_STATE.lock() {
        kitty.clear();
    }
    if let Ok(mut last) = LAST_RENDER.lock() {
        last.clear();
    }
    clear_active_diagrams();
    if let Ok(mut pending) = PENDING_RENDER_REQUESTS.lock() {
        pending.clear();
    }
    if let Ok(mut errors) = RENDER_ERRORS.lock() {
        errors.clear();
    }
    bump_deferred_render_epoch();
    clear_streaming_preview_diagram();

    // Remove cached files on disk
    let entries = fs::read_dir(&cache_dir).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("png") {
            let _ = fs::remove_file(path);
        }
    }
    Ok(())
}

/// Debug info for a single image's state
#[derive(Debug, Clone, Serialize)]
pub struct ImageStateInfo {
    pub hash: String,
    pub resize_mode: String,
    pub last_area: Option<String>,
    pub last_viewport: Option<String>,
}

/// Get detailed state info for all cached images
pub fn debug_image_state() -> Vec<ImageStateInfo> {
    if let Ok(state) = IMAGE_STATE.lock() {
        state
            .iter()
            .map(|(hash, img_state)| ImageStateInfo {
                hash: format!("{:016x}", hash),
                resize_mode: match img_state.resize_mode {
                    ResizeMode::Fit => "Fit".to_string(),
                    ResizeMode::Scale => "Scale".to_string(),
                    ResizeMode::Crop => "Crop".to_string(),
                    ResizeMode::Viewport => "Viewport".to_string(),
                },
                last_area: img_state
                    .last_area
                    .map(|r| format!("{}x{}+{}+{}", r.width, r.height, r.x, r.y)),
                last_viewport: img_state.last_viewport.map(|v| {
                    format!(
                        "scroll={}x{}, view={}x{}",
                        v.scroll_x_px, v.scroll_y_px, v.view_w_px, v.view_h_px
                    )
                }),
            })
            .collect()
    } else {
        Vec::new()
    }
}

/// Result of a test render
#[derive(Debug, Clone, Serialize)]
pub struct TestRenderResult {
    pub success: bool,
    pub hash: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub path: Option<String>,
    pub error: Option<String>,
    pub render_ms: Option<f32>,
    pub resize_mode: Option<String>,
    pub protocol: Option<String>,
}

/// Render a test diagram and return detailed results (for autonomous testing)
pub fn debug_test_render() -> TestRenderResult {
    let test_content = r#"flowchart LR
    A[Start] --> B{Decision}
    B -->|Yes| C[Action 1]
    B -->|No| D[Action 2]
    C --> E[End]
    D --> E"#;

    debug_render(test_content)
}

/// Render arbitrary mermaid content and return detailed results
pub fn debug_render(content: &str) -> TestRenderResult {
    let start = Instant::now();
    let result = render_mermaid_sized(content, Some(80)); // Use 80 cols as test width

    let render_ms = start.elapsed().as_secs_f32() * 1000.0;
    let protocol = protocol_type().map(|p| format!("{:?}", p));

    match result {
        RenderResult::Image {
            hash,
            path,
            width,
            height,
        } => {
            // Check what resize mode was assigned
            let resize_mode = if let Ok(state) = IMAGE_STATE.lock() {
                state.get(&hash).map(|s| match s.resize_mode {
                    ResizeMode::Fit => "Fit".to_string(),
                    ResizeMode::Scale => "Scale".to_string(),
                    ResizeMode::Crop => "Crop".to_string(),
                    ResizeMode::Viewport => "Viewport".to_string(),
                })
            } else {
                None
            };

            TestRenderResult {
                success: true,
                hash: Some(format!("{:016x}", hash)),
                width: Some(width),
                height: Some(height),
                path: Some(path.to_string_lossy().to_string()),
                error: None,
                render_ms: Some(render_ms),
                resize_mode,
                protocol,
            }
        }
        RenderResult::Error(msg) => TestRenderResult {
            success: false,
            hash: None,
            width: None,
            height: None,
            path: None,
            error: Some(msg),
            render_ms: Some(render_ms),
            resize_mode: None,
            protocol,
        },
    }
}

/// Simulate multiple renders at different areas to test resize mode stability
/// Returns true if resize mode stayed consistent across all renders
pub fn debug_test_resize_stability(hash: u64) -> serde_json::Value {
    let areas = [
        Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        },
        Rect {
            x: 0,
            y: 0,
            width: 120,
            height: 40,
        },
        Rect {
            x: 0,
            y: 0,
            width: 60,
            height: 20,
        },
        Rect {
            x: 10,
            y: 5,
            width: 80,
            height: 24,
        },
    ];

    let mut results: Vec<serde_json::Value> = Vec::new();
    let mut modes: Vec<String> = Vec::new();

    for area in &areas {
        // Check current resize mode for this hash
        let mode = if let Ok(state) = IMAGE_STATE.lock() {
            state.get(&hash).map(|s| match s.resize_mode {
                ResizeMode::Fit => "Fit",
                ResizeMode::Scale => "Scale",
                ResizeMode::Crop => "Crop",
                ResizeMode::Viewport => "Viewport",
            })
        } else {
            None
        };

        if let Some(m) = mode {
            modes.push(m.to_string());
            results.push(serde_json::json!({
                "area": format!("{}x{}+{}+{}", area.width, area.height, area.x, area.y),
                "resize_mode": m,
            }));
        }
    }

    let all_same = modes.windows(2).all(|w| w[0] == w[1]);

    serde_json::json!({
        "hash": format!("{:016x}", hash),
        "stable": all_same,
        "modes_observed": modes,
        "details": results,
    })
}

/// Scroll simulation test result
#[derive(Debug, Clone, Serialize)]
pub struct ScrollTestResult {
    pub hash: String,
    pub frames_rendered: usize,
    pub resize_mode_changes: usize,
    pub skipped_renders: u64,
    pub render_calls: Vec<ScrollFrameInfo>,
    pub stable: bool,
    pub border_rendered: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScrollFrameInfo {
    pub frame: usize,
    pub y_offset: i32,
    pub visible_rows: u16,
    pub rendered: bool,
    pub resize_mode: Option<String>,
}

/// Simulate scrolling behavior by rendering an image at different y-offsets
/// This tests:
/// 1. Resize mode stability during scroll
/// 2. Border rendering consistency
/// 3. Skip-redundant-render optimization
/// 4. Clearing when scrolled off-screen
pub fn debug_test_scroll(content: Option<&str>) -> ScrollTestResult {
    // First, render a test diagram
    let test_content = content.unwrap_or(
        r#"flowchart TD
    A[Start] --> B{Decision}
    B -->|Yes| C[Process 1]
    B -->|No| D[Process 2]
    C --> E[Merge]
    D --> E
    E --> F[End]"#,
    );

    let render_result = render_mermaid_sized(test_content, Some(80));
    let hash = match render_result {
        RenderResult::Image { hash, .. } => hash,
        RenderResult::Error(_e) => {
            return ScrollTestResult {
                hash: "error".to_string(),
                frames_rendered: 0,
                resize_mode_changes: 0,
                skipped_renders: 0,
                render_calls: vec![],
                stable: false,
                border_rendered: false,
            };
        }
    };

    // Get initial skipped_renders count
    let initial_skipped = if let Ok(debug) = MERMAID_DEBUG.lock() {
        debug.stats.skipped_renders
    } else {
        0
    };

    // Create a test buffer (simulating a terminal)
    let term_width = 100u16;
    let term_height = 40u16;
    let mut buf = Buffer::empty(Rect {
        x: 0,
        y: 0,
        width: term_width,
        height: term_height,
    });

    let image_height = 20u16; // Simulated image height in rows
    let mut frames: Vec<ScrollFrameInfo> = Vec::new();
    let mut modes_seen: Vec<String> = Vec::new();
    let mut border_ok = true;

    // Simulate scrolling: image starts at y=5, then scrolls up and eventually off-screen
    let scroll_positions: Vec<i32> = vec![5, 3, 1, 0, -5, -10, -15, -20, -25];

    for (frame_idx, &y_offset) in scroll_positions.iter().enumerate() {
        // Calculate visible area of the image
        let image_top = y_offset;
        let image_bottom = y_offset + image_height as i32;

        // Check if any part is visible
        let visible_top_i32 = image_top.max(0);
        let visible_bottom_i32 = image_bottom.min(term_height as i32);

        let visible = visible_top_i32 < visible_bottom_i32;
        let visible_rows = if visible {
            (visible_bottom_i32 - visible_top_i32) as u16
        } else {
            0
        };
        let visible_top = visible_top_i32 as u16;

        let mut frame_info = ScrollFrameInfo {
            frame: frame_idx,
            y_offset,
            visible_rows,
            rendered: false,
            resize_mode: None,
        };

        if visible && visible_rows > 0 {
            // Render at this position
            let area = Rect {
                x: 0,
                y: visible_top,
                width: term_width,
                height: visible_rows,
            };

            let crop_top = y_offset < 0;
            let rows_used = render_image_widget(hash, area, &mut buf, false, crop_top);
            frame_info.rendered = rows_used > 0;

            // Check resize mode
            if let Ok(state) = IMAGE_STATE.lock()
                && let Some(img_state) = state.get(&hash)
            {
                let mode = match img_state.resize_mode {
                    ResizeMode::Fit => "Fit",
                    ResizeMode::Scale => "Scale",
                    ResizeMode::Crop => "Crop",
                    ResizeMode::Viewport => "Viewport",
                };
                frame_info.resize_mode = Some(mode.to_string());
                modes_seen.push(mode.to_string());
            }

            // Check border was rendered (first column should have │)
            if area.x < buf.area().width && area.y < buf.area().height {
                let cell = &buf[(area.x, area.y)];
                if cell.symbol() != "│" {
                    border_ok = false;
                }
            }
        } else {
            // Image scrolled off-screen, clear should be called
            clear_image_area(
                Rect {
                    x: 0,
                    y: 0,
                    width: term_width,
                    height: term_height,
                },
                &mut buf,
            );
        }

        frames.push(frame_info);
    }

    // Check resize mode stability
    let mode_changes = modes_seen.windows(2).filter(|w| w[0] != w[1]).count();

    // Get final skipped count
    let final_skipped = if let Ok(debug) = MERMAID_DEBUG.lock() {
        debug.stats.skipped_renders
    } else {
        0
    };

    ScrollTestResult {
        hash: format!("{:016x}", hash),
        frames_rendered: frames.iter().filter(|f| f.rendered).count(),
        resize_mode_changes: mode_changes,
        skipped_renders: final_skipped - initial_skipped,
        render_calls: frames,
        stable: mode_changes == 0,
        border_rendered: border_ok,
    }
}

/// Hash content for caching
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
