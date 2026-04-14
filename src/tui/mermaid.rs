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

use super::color_support::rgb;
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

pub use active::{
    active_diagram_count, clear_active_diagrams, clear_streaming_preview_diagram,
    get_active_diagrams, register_active_diagram, restore_active_diagrams,
    set_streaming_preview_diagram, snapshot_active_diagrams,
};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PickerInitMode {
    Fast,
    Probe,
}

fn parse_env_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn picker_init_mode_from_probe_env(raw: Option<&str>) -> PickerInitMode {
    if let Some(raw) = raw
        && parse_env_bool(raw) == Some(true)
    {
        return PickerInitMode::Probe;
    }
    PickerInitMode::Fast
}

fn picker_init_mode_from_env() -> PickerInitMode {
    picker_init_mode_from_probe_env(std::env::var("JCODE_MERMAID_PICKER_PROBE").ok().as_deref())
}

fn infer_protocol_from_env(
    term: Option<&str>,
    term_program: Option<&str>,
    lc_terminal: Option<&str>,
    kitty_window_id: Option<&str>,
) -> Option<ProtocolType> {
    if kitty_window_id.is_some() {
        return Some(ProtocolType::Kitty);
    }

    let term = term.unwrap_or("").to_ascii_lowercase();
    let term_program = term_program.unwrap_or("").to_ascii_lowercase();
    let lc_terminal = lc_terminal.unwrap_or("").to_ascii_lowercase();

    if term.contains("kitty")
        || term_program.contains("kitty")
        || term_program.contains("wezterm")
        || term_program.contains("ghostty")
    {
        return Some(ProtocolType::Kitty);
    }

    if term_program.contains("iterm")
        || term.contains("iterm")
        || lc_terminal.contains("iterm")
        || lc_terminal.contains("wezterm")
    {
        return Some(ProtocolType::Iterm2);
    }

    if term.contains("sixel") {
        return Some(ProtocolType::Sixel);
    }

    None
}

fn query_font_size() -> (u16, u16) {
    match crossterm::terminal::window_size() {
        Ok(ws) if ws.columns > 0 && ws.rows > 0 && ws.width > 0 && ws.height > 0 => {
            let fw = ws.width / ws.columns;
            let fh = ws.height / ws.rows;
            if fw > 0 && fh > 0 {
                crate::logging::info(&format!(
                    "Detected terminal font size: {}x{} pixels/cell (window {}x{} px, {}x{} cells)",
                    fw, fh, ws.width, ws.height, ws.columns, ws.rows
                ));
                (fw, fh)
            } else {
                DEFAULT_PICKER_FONT_SIZE
            }
        }
        _ => DEFAULT_PICKER_FONT_SIZE,
    }
}

fn fast_picker() -> Picker {
    let _font_size = query_font_size();
    let mut picker = Picker::halfblocks();
    if let Some(protocol) = infer_protocol_from_env(
        std::env::var("TERM").ok().as_deref(),
        std::env::var("TERM_PROGRAM").ok().as_deref(),
        std::env::var("LC_TERMINAL").ok().as_deref(),
        std::env::var("KITTY_WINDOW_ID").ok().as_deref(),
    ) {
        picker.set_protocol_type(protocol);
    }
    picker
}

fn prewarm_svg_font_db_async() {
    SVG_FONT_DB_PREWARM_STARTED.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("jcode-mermaid-fontdb-prewarm".to_string())
            .spawn(|| {
                let _ = &*SVG_FONT_DB;
            });
    });
}

/// Initialize the global picker.
/// By default this uses a fast non-blocking path and avoids terminal probing.
/// Set JCODE_MERMAID_PICKER_PROBE=1 to force full stdio capability probing.
/// Also triggers cache eviction on first call.
pub fn init_picker() {
    PICKER.get_or_init(|| match picker_init_mode_from_env() {
        PickerInitMode::Fast => Some(fast_picker()),
        PickerInitMode::Probe => match Picker::from_query_stdio() {
            Ok(picker) => Some(picker),
            Err(err) => {
                crate::logging::warn(&format!(
                    "Mermaid picker probe failed ({}); using fast picker fallback",
                    err
                ));
                Some(fast_picker())
            }
        },
    });
    prewarm_svg_font_db_async();
    // Evict old cache files once per process
    CACHE_EVICTED.get_or_init(|| {
        evict_old_cache();
    });
}

/// Get the current protocol type (for debugging/display)
pub fn protocol_type() -> Option<ProtocolType> {
    let real = PICKER
        .get()
        .and_then(|p| p.as_ref().map(|picker| picker.protocol_type()));
    if real.is_some() {
        return real;
    }
    if VIDEO_EXPORT_MODE.load(Ordering::Relaxed) {
        Some(ProtocolType::Halfblocks)
    } else {
        None
    }
}

pub fn image_protocol_available() -> bool {
    PICKER.get().and_then(|p| p.as_ref()).is_some() || VIDEO_EXPORT_MODE.load(Ordering::Relaxed)
}

/// Enable video-export mode: mermaid images produce hash-placeholder lines
/// even without a real terminal image protocol.
pub fn set_video_export_mode(enabled: bool) {
    VIDEO_EXPORT_MODE.store(enabled, Ordering::Relaxed);
}

/// Check if video export mode is active.
pub fn is_video_export_mode() -> bool {
    VIDEO_EXPORT_MODE.load(Ordering::Relaxed)
}

/// Look up a cached PNG for the given mermaid content hash.
/// Returns (path, width, height) if a cached render exists on disk.
pub fn get_cached_png(hash: u64) -> Option<(PathBuf, u32, u32)> {
    let mut cache = RENDER_CACHE.lock().ok()?;
    let diagram = cache.get(hash, None)?;
    Some((diagram.path, diagram.width, diagram.height))
}

/// Register an external image file (e.g. from file_read) in the render cache
/// so it can be displayed with render_image_widget_fit/render_image_widget.
/// Returns the hash used for rendering.
pub fn register_external_image(path: &Path, width: u32, height: u32) -> u64 {
    use std::hash::{Hash as _, Hasher};
    let mut hasher = std::hash::DefaultHasher::new();
    path.hash(&mut hasher);
    let hash = hasher.finish();

    if let Ok(mut cache) = RENDER_CACHE.lock() {
        cache.insert(
            hash,
            CachedDiagram {
                path: path.to_path_buf(),
                width,
                height,
            },
        );
    }
    hash
}

pub fn register_inline_image(media_type: &str, data_b64: &str) -> Option<(u64, u32, u32)> {
    use std::hash::{Hash as _, Hasher};

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_b64)
        .ok()?;

    let mut hasher = std::hash::DefaultHasher::new();
    media_type.hash(&mut hasher);
    bytes.hash(&mut hasher);
    let hash = hasher.finish();

    if let Ok(mut cache) = RENDER_CACHE.lock() {
        if let Some(existing) = cache.get(hash, None) {
            return Some((hash, existing.width, existing.height));
        }

        let image = image::load_from_memory(&bytes).ok()?;
        let (width, height) = image.dimensions();
        let ext = inline_image_extension(media_type);
        let path = cache
            .cache_dir
            .join(format!("{:016x}_inline.{}", hash, ext));
        if !path.exists() {
            fs::write(&path, &bytes).ok()?;
        }
        cache.insert(
            hash,
            CachedDiagram {
                path,
                width,
                height,
            },
        );
        return Some((hash, width, height));
    }

    None
}

fn inline_image_extension(media_type: &str) -> &'static str {
    match media_type {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/x-icon" | "image/vnd.microsoft.icon" => "ico",
        _ => "img",
    }
}

pub fn error_lines_for(hash: u64) -> Option<Vec<Line<'static>>> {
    let message = RENDER_ERRORS
        .lock()
        .ok()
        .and_then(|errors| errors.get(&hash).cloned());
    message.map(|msg| error_to_lines(&msg))
}

/// Get terminal font size for adaptive sizing
pub fn get_font_size() -> Option<(u16, u16)> {
    PICKER
        .get()
        .and_then(|p| p.as_ref().map(|picker| picker.font_size()))
}

/// Maximum in-memory RENDER_CACHE entries (metadata only, not images).
const RENDER_CACHE_MAX: usize = 64;
/// Reuse a cached PNG only if it's at least this fraction of requested width.
/// This avoids visibly blurry upscaling after terminal/pane resizes.
const CACHE_WIDTH_MATCH_PERCENT: u32 = 85;
/// Quantize requested Mermaid render widths so tiny pane-width changes, like a
/// 1-cell scrollbar reservation, reuse the same cold render/cache entry.
const RENDER_WIDTH_BUCKET_CELLS: u32 = 4;

/// Mermaid rendering cache
struct MermaidCache {
    /// Map from content hash to rendered PNG info
    entries: HashMap<u64, CachedDiagram>,
    /// Insertion order for LRU eviction
    order: VecDeque<u64>,
    /// Cache directory
    cache_dir: PathBuf,
}

#[derive(Clone)]
struct CachedDiagram {
    path: PathBuf,
    width: u32,
    height: u32,
}

impl MermaidCache {
    fn new() -> Self {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("jcode")
            .join("mermaid");

        let _ = fs::create_dir_all(&cache_dir);

        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            cache_dir,
        }
    }

    fn touch(&mut self, hash: u64) {
        if let Some(pos) = self.order.iter().position(|h| *h == hash) {
            self.order.remove(pos);
        }
        self.order.push_back(hash);
    }

    fn get(&mut self, hash: u64, min_width: Option<u32>) -> Option<CachedDiagram> {
        if let Some(existing) = self.entries.get(&hash).cloned() {
            if existing.path.exists() && cached_width_satisfies(existing.width, min_width) {
                self.touch(hash);
                return Some(existing);
            }
            self.entries.remove(&hash);
            if let Some(pos) = self.order.iter().position(|h| *h == hash) {
                self.order.remove(pos);
            }
        }

        if let Some(found) = self.discover_on_disk(hash, min_width) {
            self.insert(hash, found.clone());
            return Some(found);
        }

        None
    }

    fn insert(&mut self, hash: u64, diagram: CachedDiagram) {
        if let std::collections::hash_map::Entry::Occupied(mut entry) = self.entries.entry(hash) {
            entry.insert(diagram);
            self.touch(hash);
        } else {
            self.entries.insert(hash, diagram);
            self.order.push_back(hash);
            while self.order.len() > RENDER_CACHE_MAX {
                if let Some(old) = self.order.pop_front() {
                    self.entries.remove(&old);
                }
            }
        }
    }

    fn cache_path(&self, hash: u64, target_width: u32) -> PathBuf {
        // Include target width in filename for size-specific caching
        self.cache_dir
            .join(format!("{:016x}_w{}.png", hash, target_width))
    }

    fn discover_on_disk(&self, hash: u64, min_width: Option<u32>) -> Option<CachedDiagram> {
        let mut candidates: Vec<(PathBuf, u32)> = Vec::new();
        let entries = fs::read_dir(&self.cache_dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("png") {
                continue;
            }
            let Some((file_hash, width_hint)) = parse_cache_filename(&path) else {
                continue;
            };
            if file_hash == hash {
                candidates.push((path, width_hint));
            }
        }
        if candidates.is_empty() {
            return None;
        }

        let selected = if let Some(min_w) = min_width {
            if let Some(candidate) = candidates
                .iter()
                .filter(|(_, w)| cached_width_satisfies(*w, Some(min_w)))
                .min_by_key(|(_, w)| *w)
            {
                candidate.clone()
            } else {
                return None;
            }
        } else {
            candidates
                .iter()
                .max_by_key(|(_, w)| *w)
                .cloned()
                .unwrap_or_else(|| candidates[0].clone())
        };

        let (path, width_hint) = selected;
        let (width, height) = get_png_dimensions(&path).unwrap_or((width_hint, width_hint));
        Some(CachedDiagram {
            path,
            width,
            height,
        })
    }
}

fn cached_width_satisfies(width: u32, min_width: Option<u32>) -> bool {
    let Some(min_width) = min_width else {
        return true;
    };
    if min_width == 0 {
        return true;
    }
    width.saturating_mul(100) >= min_width.saturating_mul(CACHE_WIDTH_MATCH_PERCENT)
}

fn parse_cache_filename(path: &Path) -> Option<(u64, u32)> {
    let stem = path.file_stem()?.to_str()?;
    let (hash_hex, width_part) = stem.split_once("_w")?;
    let hash = u64::from_str_radix(hash_hex, 16).ok()?;
    let width = width_part.parse::<u32>().ok()?;
    Some((hash, width))
}

fn get_cached_diagram(hash: u64, min_width: Option<u32>) -> Option<CachedDiagram> {
    let mut cache = RENDER_CACHE.lock().ok()?;
    cache.get(hash, min_width)
}

pub fn get_cached_path(hash: u64) -> Option<PathBuf> {
    get_cached_diagram(hash, None).map(|c| c.path)
}

fn invalidate_cached_image(hash: u64) {
    if let Ok(mut state) = IMAGE_STATE.lock() {
        state.remove(&hash);
    }
    if let Ok(mut kitty) = KITTY_VIEWPORT_STATE.lock() {
        kitty.remove(&hash);
    }
    if let Ok(mut source) = SOURCE_CACHE.lock() {
        source.remove(hash);
    }
}

/// Result of attempting to render a mermaid diagram
pub enum RenderResult {
    /// Successfully rendered to image - includes content hash for state lookup
    Image {
        hash: u64,
        path: PathBuf,
        width: u32,
        height: u32,
    },
    /// Error during rendering
    Error(String),
}

/// Check if a code block language is mermaid
pub fn is_mermaid_lang(lang: &str) -> bool {
    let lang_lower = lang.to_lowercase();
    lang_lower == "mermaid" || lang_lower.starts_with("mermaid")
}

/// Maximum allowed nodes in a diagram (prevents OOM on complex diagrams)
const MAX_NODES: usize = 100;
/// Maximum allowed edges in a diagram
const MAX_EDGES: usize = 200;

/// Count nodes and edges in mermaid content (rough estimate)
fn estimate_diagram_size(content: &str) -> (usize, usize) {
    svg::estimate_diagram_size(content)
}

/// Calculate optimal PNG dimensions based on terminal and diagram complexity
fn calculate_render_size(
    node_count: usize,
    edge_count: usize,
    terminal_width: Option<u16>,
) -> (f64, f64) {
    svg::calculate_render_size(node_count, edge_count, terminal_width)
}

fn retarget_svg_for_png(svg: &str, target_width: f64, target_height: f64) -> String {
    svg::retarget_svg_for_png(svg, target_width, target_height)
}

fn write_output_png_cached_fonts(
    svg: &str,
    output: &Path,
    render_cfg: &RenderConfig,
    theme: &Theme,
) -> anyhow::Result<()> {
    svg::write_output_png_cached_fonts(svg, output, render_cfg, theme)
}

/// Render a mermaid code block to PNG (cached)
/// Now accepts optional terminal_width for adaptive sizing
pub fn render_mermaid(content: &str) -> RenderResult {
    render_mermaid_sized(content, None)
}

/// Render with explicit terminal width for adaptive sizing
pub fn render_mermaid_sized(content: &str, terminal_width: Option<u16>) -> RenderResult {
    render_mermaid_sized_internal(content, terminal_width, true)
}

/// Render without registering the diagram in ACTIVE_DIAGRAMS.
/// Useful for internal widget visuals that should not appear in the
/// user-visible diagram pane.
pub fn render_mermaid_untracked(content: &str, terminal_width: Option<u16>) -> RenderResult {
    render_mermaid_sized_internal(content, terminal_width, false)
}

fn bump_deferred_render_epoch() {
    DEFERRED_RENDER_EPOCH.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut state) = MERMAID_DEBUG.lock() {
        state.stats.deferred_epoch_bumps += 1;
    }
}

pub fn deferred_render_epoch() -> u64 {
    DEFERRED_RENDER_EPOCH.load(Ordering::Relaxed)
}

fn deferred_render_sender() -> &'static mpsc::Sender<DeferredRenderTask> {
    DEFERRED_RENDER_TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<DeferredRenderTask>();
        if let Err(err) = std::thread::Builder::new()
            .name("jcode-mermaid-deferred".to_string())
            .spawn(move || deferred_render_worker(rx))
        {
            crate::logging::warn(&format!(
                "Failed to spawn mermaid deferred worker, falling back to synchronous rendering: {}",
                err
            ));
        }
        tx
    })
}

fn deferred_render_worker(rx: mpsc::Receiver<DeferredRenderTask>) {
    for task in rx {
        let register_active = match PENDING_RENDER_REQUESTS.lock() {
            Ok(pending) => pending
                .get(&task.render_key)
                .map(|request| request.register_active),
            Err(poisoned) => poisoned
                .into_inner()
                .get(&task.render_key)
                .map(|request| request.register_active),
        };

        let Some(register_active) = register_active else {
            if let Ok(mut state) = MERMAID_DEBUG.lock() {
                state.stats.deferred_worker_skips += 1;
            }
            continue;
        };

        if let Ok(mut state) = MERMAID_DEBUG.lock() {
            state.stats.deferred_worker_renders += 1;
        }

        let _ = render_mermaid_sized_internal(&task.content, task.terminal_width, register_active);

        if let Ok(mut pending) = PENDING_RENDER_REQUESTS.lock() {
            pending.remove(&task.render_key);
        }
        bump_deferred_render_epoch();
    }
}

/// Streaming-friendly Mermaid rendering.
///
/// If the diagram is already cached, returns it immediately. Otherwise this
/// queues the heavy render work onto a background thread and returns `None`
/// so the caller can keep the UI responsive with a lightweight placeholder.
pub fn render_mermaid_deferred(content: &str, terminal_width: Option<u16>) -> Option<RenderResult> {
    render_mermaid_deferred_with_registration(content, terminal_width, false)
}

pub fn render_mermaid_deferred_with_registration(
    content: &str,
    terminal_width: Option<u16>,
    register_active: bool,
) -> Option<RenderResult> {
    let hash = hash_content(content);
    let (node_count, edge_count) = estimate_diagram_size(content);

    if node_count > MAX_NODES || edge_count > MAX_EDGES {
        return Some(RenderResult::Error(format!(
            "Diagram too complex ({} nodes, {} edges). Max: {} nodes, {} edges.",
            node_count, edge_count, MAX_NODES, MAX_EDGES
        )));
    }

    let (target_width, _) = calculate_render_size(node_count, edge_count, terminal_width);
    let target_width_u32 = target_width as u32;

    if let Some(cached) = get_cached_diagram(hash, Some(target_width_u32)) {
        if register_active {
            register_active_diagram(hash, cached.width, cached.height, None);
        }
        return Some(RenderResult::Image {
            hash,
            path: cached.path,
            width: cached.width,
            height: cached.height,
        });
    }

    if let Some(err) = RENDER_ERRORS
        .lock()
        .ok()
        .and_then(|errors| errors.get(&hash).cloned())
    {
        return Some(RenderResult::Error(err));
    }

    let render_key = (hash, target_width_u32);
    let should_enqueue = match PENDING_RENDER_REQUESTS.lock() {
        Ok(mut pending) => {
            if let Some((_, existing_request)) =
                pending
                    .iter_mut()
                    .find(|((pending_hash, pending_width), _)| {
                        *pending_hash == hash
                            && cached_width_satisfies(*pending_width, Some(target_width_u32))
                    })
            {
                if register_active {
                    existing_request.register_active = true;
                }
                if let Ok(mut state) = MERMAID_DEBUG.lock() {
                    state.stats.deferred_deduped += 1;
                }
                false
            } else {
                match pending.entry(render_key) {
                    Entry::Occupied(mut occupied) => {
                        if register_active {
                            occupied.get_mut().register_active = true;
                        }
                        if let Ok(mut state) = MERMAID_DEBUG.lock() {
                            state.stats.deferred_deduped += 1;
                        }
                        false
                    }
                    Entry::Vacant(vacant) => {
                        vacant.insert(PendingDeferredRender { register_active });
                        if let Ok(mut state) = MERMAID_DEBUG.lock() {
                            state.stats.deferred_enqueued += 1;
                        }
                        true
                    }
                }
            }
        }
        Err(_) => {
            return Some(render_mermaid_sized_internal(
                content,
                terminal_width,
                register_active,
            ));
        }
    };

    if should_enqueue {
        let task = DeferredRenderTask {
            content: content.to_string(),
            terminal_width,
            render_key,
        };
        if deferred_render_sender().send(task).is_err() {
            if let Ok(mut pending) = PENDING_RENDER_REQUESTS.lock() {
                pending.remove(&render_key);
            }
            return Some(render_mermaid_sized_internal(
                content,
                terminal_width,
                register_active,
            ));
        }
    }

    None
}

fn render_mermaid_sized_internal(
    content: &str,
    terminal_width: Option<u16>,
    register_active: bool,
) -> RenderResult {
    if let Ok(mut state) = MERMAID_DEBUG.lock() {
        state.stats.total_requests += 1;
        state.stats.last_content_len = Some(content.len());
        state.stats.last_error = None;
        state.stats.last_parse_ms = None;
        state.stats.last_layout_ms = None;
        state.stats.last_svg_ms = None;
        state.stats.last_png_ms = None;
    }

    // Calculate content hash for caching
    let hash = hash_content(content);

    // Estimate complexity for sizing
    let (node_count, edge_count) = estimate_diagram_size(content);
    let complexity = node_count + edge_count;

    if let Ok(mut state) = MERMAID_DEBUG.lock() {
        state.stats.last_nodes = Some(node_count);
        state.stats.last_edges = Some(edge_count);
    }

    // Check complexity limits
    if node_count > MAX_NODES || edge_count > MAX_EDGES {
        let msg = format!(
            "Diagram too complex ({} nodes, {} edges). Max: {} nodes, {} edges.",
            node_count, edge_count, MAX_NODES, MAX_EDGES
        );
        if let Ok(mut state) = MERMAID_DEBUG.lock() {
            state.stats.render_errors += 1;
            state.stats.last_error = Some(msg.clone());
        }
        return RenderResult::Error(msg);
    }

    // Calculate target size
    let (target_width, target_height) =
        calculate_render_size(node_count, edge_count, terminal_width);
    let target_width_u32 = target_width as u32;
    let target_height_u32 = target_height as u32;

    if let Ok(mut state) = MERMAID_DEBUG.lock() {
        state.stats.last_target_width = Some(target_width_u32);
        state.stats.last_target_height = Some(target_height_u32);
    }

    // Check cache (memory + on-disk fallback, width-aware).
    if let Some(cached) = get_cached_diagram(hash, Some(target_width_u32)) {
        if let Ok(mut state) = MERMAID_DEBUG.lock() {
            state.stats.cache_hits += 1;
            state.stats.last_hash = Some(format!("{:016x}", hash));
        }
        if register_active {
            // Register as active diagram (for pinned widget display)
            register_active_diagram(hash, cached.width, cached.height, None);
        }
        return RenderResult::Image {
            hash,
            path: cached.path,
            width: cached.width,
            height: cached.height,
        };
    }

    if let Ok(mut state) = MERMAID_DEBUG.lock() {
        state.stats.cache_misses += 1;
        state.stats.last_hash = Some(format!("{:016x}", hash));
    }

    // Get cache path
    let png_path = {
        let cache = RENDER_CACHE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.cache_path(hash, target_width_u32)
    };
    let png_path_clone = png_path.clone();

    let _render_guard = RENDER_WORK_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    // Re-check cache after taking the render lock so a background worker that
    // just finished can satisfy this request without doing duplicate work.
    if let Some(cached) = get_cached_diagram(hash, Some(target_width_u32)) {
        if let Ok(mut errors) = RENDER_ERRORS.lock() {
            errors.remove(&hash);
        }
        if let Ok(mut state) = MERMAID_DEBUG.lock() {
            state.stats.cache_hits += 1;
            state.stats.last_hash = Some(format!("{:016x}", hash));
        }
        if register_active {
            register_active_diagram(hash, cached.width, cached.height, None);
        }
        return RenderResult::Image {
            hash,
            path: cached.path,
            width: cached.width,
            height: cached.height,
        };
    }

    // Wrap mermaid library calls in catch_unwind for defense-in-depth
    let content_owned = content.to_string();

    let prev_hook = panic::take_hook();
    panic::set_hook(Box::new(|_| {
        // Silently ignore panics from mermaid renderer
    }));

    let render_start = Instant::now();
    let render_result = panic::catch_unwind(move || -> Result<RenderStageBreakdown, String> {
        let parse_start = Instant::now();
        // Parse mermaid
        let parsed = parse_mermaid(&content_owned).map_err(|e| format!("Parse error: {}", e))?;
        let parse_ms = parse_start.elapsed().as_secs_f32() * 1000.0;

        // Configure theme for terminal (dark background friendly)
        let theme = terminal_theme();

        // Adaptive spacing based on complexity
        let spacing_factor = if complexity > 30 { 1.2 } else { 1.0 };
        let layout_config = LayoutConfig {
            node_spacing: 80.0 * spacing_factor,
            rank_spacing: 80.0 * spacing_factor,
            node_padding_x: 40.0,
            node_padding_y: 20.0,
            ..Default::default()
        };

        let layout_start = Instant::now();
        // Compute layout
        let layout = compute_layout(&parsed.graph, &theme, &layout_config);
        let layout_ms = layout_start.elapsed().as_secs_f32() * 1000.0;

        let svg_start = Instant::now();
        // Render to SVG
        let svg = render_svg(&layout, &theme, &layout_config);
        let svg = retarget_svg_for_png(&svg, target_width, target_height);
        let svg_ms = svg_start.elapsed().as_secs_f32() * 1000.0;

        // Convert SVG to PNG with adaptive dimensions
        let render_config = RenderConfig {
            width: target_width as f32,
            height: target_height as f32,
            background: theme.background.clone(),
        };

        // Ensure parent directory exists
        if let Some(parent) = png_path_clone.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create cache directory: {}", e))?;
        }

        let png_start = Instant::now();
        write_output_png_cached_fonts(&svg, &png_path_clone, &render_config, &theme)
            .map_err(|e| format!("Render error: {}", e))?;
        let png_ms = png_start.elapsed().as_secs_f32() * 1000.0;

        Ok(RenderStageBreakdown {
            parse_ms,
            layout_ms,
            svg_ms,
            png_ms,
        })
    });

    // Restore the original panic hook
    panic::set_hook(prev_hook);

    // Handle the result
    let render_ms = render_start.elapsed().as_secs_f32() * 1000.0;
    match render_result {
        Ok(Ok(stage_breakdown)) => {
            if let Ok(mut errors) = RENDER_ERRORS.lock() {
                errors.remove(&hash);
            }
            if let Ok(mut state) = MERMAID_DEBUG.lock() {
                state.stats.render_success += 1;
                state.stats.last_render_ms = Some(render_ms);
                state.stats.last_parse_ms = Some(stage_breakdown.parse_ms);
                state.stats.last_layout_ms = Some(stage_breakdown.layout_ms);
                state.stats.last_svg_ms = Some(stage_breakdown.svg_ms);
                state.stats.last_png_ms = Some(stage_breakdown.png_ms);
            }
        }
        Ok(Err(e)) => {
            if let Ok(mut errors) = RENDER_ERRORS.lock() {
                errors.insert(hash, e.clone());
            }
            if let Ok(mut state) = MERMAID_DEBUG.lock() {
                state.stats.render_errors += 1;
                state.stats.last_render_ms = Some(render_ms);
                state.stats.last_error = Some(e.clone());
            }
            return RenderResult::Error(e);
        }
        Err(panic_info) => {
            let msg = if let Some(s) = panic_info.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = panic_info.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic in mermaid renderer".to_string()
            };
            if let Ok(mut errors) = RENDER_ERRORS.lock() {
                errors.insert(hash, format!("Renderer panic: {}", msg));
            }
            if let Ok(mut state) = MERMAID_DEBUG.lock() {
                state.stats.render_errors += 1;
                state.stats.last_render_ms = Some(render_ms);
                state.stats.last_error = Some(format!("Renderer panic: {}", msg));
            }
            return RenderResult::Error(format!("Renderer panic: {}", msg));
        }
    }

    // Get actual dimensions from rendered PNG
    let (width, height) =
        get_png_dimensions(&png_path).unwrap_or((target_width_u32, target_height as u32));

    if let Ok(mut state) = MERMAID_DEBUG.lock() {
        state.stats.last_png_width = Some(width);
        state.stats.last_png_height = Some(height);
    }

    // Cache the result
    {
        let mut cache = RENDER_CACHE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.insert(
            hash,
            CachedDiagram {
                path: png_path.clone(),
                width,
                height,
            },
        );
    }
    // If we re-rendered at a new size/path, force widget state to reload.
    invalidate_cached_image(hash);

    if register_active {
        // Register this diagram as active for info widget display
        register_active_diagram(hash, width, height, None);
    }

    RenderResult::Image {
        hash,
        path: png_path,
        width,
        height,
    }
}

/// Border width for mermaid diagrams (left bar + space)
const BORDER_WIDTH: u16 = 2;

fn rect_contains_point(rect: Rect, x: u16, y: u16) -> bool {
    let right = rect.x.saturating_add(rect.width);
    let bottom = rect.y.saturating_add(rect.height);
    x >= rect.x && x < right && y >= rect.y && y < bottom
}

fn set_cell_if_visible(buf: &mut Buffer, x: u16, y: u16, ch: char, style: Option<Style>) {
    let bounds = *buf.area();
    if !rect_contains_point(bounds, x, y) {
        return;
    }
    let cell = &mut buf[(x, y)];
    cell.set_char(ch);
    if let Some(style) = style {
        cell.set_style(style);
    }
}

fn draw_left_border(buf: &mut Buffer, area: Rect) {
    let clamped = area.intersection(*buf.area());
    if clamped.width == 0 || clamped.height == 0 {
        return;
    }
    let border_style = Style::default().fg(rgb(100, 100, 100)); // DIM_COLOR
    let y_end = clamped.y.saturating_add(clamped.height);
    for row in clamped.y..y_end {
        set_cell_if_visible(buf, clamped.x, row, '│', Some(border_style));
        if clamped.width > 1 {
            let spacer_x = clamped.x.saturating_add(1);
            set_cell_if_visible(buf, spacer_x, row, ' ', None);
        }
    }
}

fn panic_payload_to_string(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

fn render_stateful_image_safe(
    hash: u64,
    area: Rect,
    buf: &mut Buffer,
    protocol: &mut StatefulProtocol,
    resize: Resize,
) -> bool {
    let widget = StatefulImage::default().resize(resize);
    match panic::catch_unwind(panic::AssertUnwindSafe(|| {
        widget.render(area, buf, protocol);
    })) {
        Ok(()) => true,
        Err(payload) => {
            crate::logging::warn(&format!(
                "Recovered image render panic for diagram {:016x}: {}",
                hash,
                panic_payload_to_string(payload.as_ref())
            ));
            clear_image_area(area, buf);
            false
        }
    }
}

/// Render an image at the given area using ratatui-image
/// If centered is true, the image will be horizontally centered within the area
/// If crop_top is true, clip from the top to show the bottom portion when partially visible
/// Returns the number of rows used
///
/// ## Optimizations
/// - Uses blocking locks for consistent rendering (no frame skipping)
/// - Skips render if area and settings unchanged from last frame
/// - Uses Fit mode for small terminals to scale instead of crop
/// - Only clears area if render fails
/// - Draws a left border (like code blocks) for visual consistency
pub fn render_image_widget(
    hash: u64,
    area: Rect,
    buf: &mut Buffer,
    centered: bool,
    crop_top: bool,
) -> u16 {
    // In video export mode, skip terminal image protocol rendering.
    // The placeholder marker stays in the buffer so the SVG pipeline
    // can detect it and embed the cached PNG directly.
    if VIDEO_EXPORT_MODE.load(Ordering::Relaxed) {
        return area.height;
    }

    let buf_area = *buf.area();
    let area = area.intersection(buf_area);

    if area.width == 0 || area.height == 0 {
        return 0;
    }

    // Skip if area is too small (need room for border + image)
    if area.width <= BORDER_WIDTH {
        return 0;
    }

    // Draw left border (vertical bar like code blocks)
    draw_left_border(buf, area);

    // Adjust area for image (after border)
    let image_area = Rect {
        x: area.x + BORDER_WIDTH,
        y: area.y,
        width: area.width - BORDER_WIDTH,
        height: area.height,
    };

    // Skip if image area is too small
    if image_area.width == 0 {
        return area.height;
    }

    let min_cached_width = PICKER
        .get()
        .and_then(|p| p.as_ref())
        .map(|picker| image_area.width as u32 * picker.font_size().0 as u32);
    let cached = get_cached_diagram(hash, min_cached_width);
    let (img_width, path) = if let Some(cached) = cached {
        (cached.width, Some(cached.path))
    } else {
        (0, None)
    };

    // Calculate the actual render area (potentially centered within image_area)
    let render_area = if centered && img_width > 0 {
        // Calculate actual rendered width in terminal cells
        let rendered_width = if let Some(Some(picker)) = PICKER.get() {
            let font_size = picker.font_size();
            let img_width_cells = (img_width as f32 / font_size.0 as f32).ceil() as u16;
            img_width_cells.min(image_area.width)
        } else {
            image_area.width
        };

        // Center horizontally within image_area
        let x_offset = (image_area.width.saturating_sub(rendered_width)) / 2;
        Rect {
            x: image_area.x + x_offset,
            y: image_area.y,
            width: rendered_width,
            height: image_area.height,
        }
    } else {
        image_area
    };

    // Try to render from existing state - single lock for the whole operation
    {
        let mut state = IMAGE_STATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let needs_reset = state
            .get(&hash)
            .map(|s| {
                s.resize_mode != ResizeMode::Crop
                    || path
                        .as_ref()
                        .map(|p| s.source_path.as_path() != p.as_path())
                        .unwrap_or(false)
            })
            .unwrap_or(false);
        if needs_reset {
            state.remove(&hash);
        }
        if let Some(img_state) = state.get_mut(hash) {
            img_state.resize_mode = ResizeMode::Crop;
            img_state.last_viewport = None;
            // Always use Crop mode - no rescaling during scroll
            let crop_opts = CropOptions {
                clip_top: crop_top,
                clip_left: false,
            };

            // If crop direction changed, force a re-encode so we don't reuse stale data
            if img_state.last_crop_top != crop_top {
                img_state
                    .protocol
                    .resize_encode(&Resize::Crop(Some(crop_opts)), render_area);
                img_state.last_crop_top = crop_top;
            }

            // Track whether this is a geometry-identical frame (for skipped_renders stat).
            let same_area = img_state.last_area == Some(render_area);
            let state_key = LastRenderState {
                area: render_area,
                crop_top,
                resize_mode: ResizeMode::Crop,
            };
            {
                let last_same = LAST_RENDER
                    .lock()
                    .ok()
                    .and_then(|mut map| {
                        let prev = map.get(&hash).cloned();
                        map.insert(hash, state_key.clone());
                        prev
                    })
                    .map(|prev| prev == state_key)
                    .unwrap_or(false);
                if last_same
                    && same_area
                    && let Ok(mut dbg) = MERMAID_DEBUG.lock()
                {
                    dbg.stats.skipped_renders += 1;
                }
            }
            if let Ok(mut dbg) = MERMAID_DEBUG.lock() {
                dbg.stats.image_state_hits += 1;
            }
            if !render_stateful_image_safe(
                hash,
                render_area,
                buf,
                &mut img_state.protocol,
                Resize::Crop(Some(crop_opts)),
            ) {
                return 0;
            }
            img_state.last_area = Some(render_area);
            return area.height;
        }
    }

    // State miss - need to load image from cache
    if let Some(path) = path
        && let Some(Some(picker)) = PICKER.get()
        && let Ok(img) = image::open(&path)
    {
        if let Ok(mut dbg) = MERMAID_DEBUG.lock() {
            dbg.stats.image_state_misses += 1;
        }
        let protocol = picker.new_resize_protocol(img);

        let mut state = IMAGE_STATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.insert(
            hash,
            ImageState {
                protocol,
                source_path: path.clone(),
                last_area: Some(render_area),
                resize_mode: ResizeMode::Crop,
                last_crop_top: false,
                last_viewport: None,
            },
        );

        if let Some(img_state) = state.get_mut(hash) {
            let crop_opts = CropOptions {
                clip_top: crop_top,
                clip_left: false,
            };
            img_state.last_crop_top = crop_top;
            if !render_stateful_image_safe(
                hash,
                render_area,
                buf,
                &mut img_state.protocol,
                Resize::Crop(Some(crop_opts)),
            ) {
                return 0;
            }
            return area.height;
        }
    }

    // Render failed - clear the area to avoid showing stale content
    let clr_area = area.intersection(buf_area);
    if clr_area.width > 0 && clr_area.height > 0 {
        super::color_support::clear_buf(clr_area, buf);
    }

    0
}

/// Render an image using Fit mode (scales to fit the available area).
/// draw_border controls whether a left border is drawn like code blocks.
pub fn render_image_widget_fit(
    hash: u64,
    area: Rect,
    buf: &mut Buffer,
    centered: bool,
    draw_border: bool,
) -> u16 {
    render_image_widget_fit_inner(hash, area, buf, centered, draw_border, false)
}

pub fn render_image_widget_scale(
    hash: u64,
    area: Rect,
    buf: &mut Buffer,
    draw_border: bool,
) -> u16 {
    render_image_widget_fit_inner(hash, area, buf, false, draw_border, true)
}

fn render_image_widget_fit_inner(
    hash: u64,
    area: Rect,
    buf: &mut Buffer,
    centered: bool,
    draw_border: bool,
    scale_up: bool,
) -> u16 {
    if VIDEO_EXPORT_MODE.load(Ordering::Relaxed) {
        return area.height;
    }

    let buf_area = *buf.area();
    let area = area.intersection(buf_area);

    if area.width == 0 || area.height == 0 {
        return 0;
    }

    let border_width = if draw_border { BORDER_WIDTH } else { 0 };
    if area.width <= border_width {
        return 0;
    }

    if draw_border {
        draw_left_border(buf, area);
    }

    let image_area = Rect {
        x: area.x + border_width,
        y: area.y,
        width: area.width - border_width,
        height: area.height,
    };

    if image_area.width == 0 {
        return area.height;
    }

    let min_cached_width = if scale_up {
        None
    } else {
        PICKER
            .get()
            .and_then(|p| p.as_ref())
            .map(|picker| image_area.width as u32 * picker.font_size().0 as u32)
    };
    let cached = get_cached_diagram(hash, min_cached_width);
    let (img_width, path) = if let Some(cached) = cached {
        (cached.width, Some(cached.path))
    } else {
        (0, None)
    };

    let render_area = if centered && img_width > 0 {
        let rendered_width = if let Some(Some(picker)) = PICKER.get() {
            let font_size = picker.font_size();
            let img_width_cells = (img_width as f32 / font_size.0 as f32).ceil() as u16;
            img_width_cells.min(image_area.width)
        } else {
            image_area.width
        };
        let x_offset = (image_area.width.saturating_sub(rendered_width)) / 2;
        Rect {
            x: image_area.x + x_offset,
            y: image_area.y,
            width: rendered_width,
            height: image_area.height,
        }
    } else {
        image_area
    };

    {
        let mut state = IMAGE_STATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let target_mode = if scale_up {
            ResizeMode::Scale
        } else {
            ResizeMode::Fit
        };
        let resize = if scale_up {
            Resize::Scale(None)
        } else {
            Resize::Fit(None)
        };
        let needs_reset = state
            .get(&hash)
            .map(|s| {
                s.resize_mode != target_mode
                    || path
                        .as_ref()
                        .map(|p| s.source_path.as_path() != p.as_path())
                        .unwrap_or(false)
            })
            .unwrap_or(false);
        if needs_reset {
            state.remove(&hash);
        }
        if let Some(img_state) = state.get_mut(hash) {
            img_state.resize_mode = target_mode;
            img_state.last_viewport = None;
            // Track identical-geometry frames for skipped_renders stat.
            let same_area = img_state.last_area == Some(render_area);
            let state_key = LastRenderState {
                area: render_area,
                crop_top: false,
                resize_mode: target_mode,
            };
            {
                let last_same = LAST_RENDER
                    .lock()
                    .ok()
                    .and_then(|mut map| {
                        let prev = map.get(&hash).cloned();
                        map.insert(hash, state_key.clone());
                        prev
                    })
                    .map(|prev| prev == state_key)
                    .unwrap_or(false);
                if last_same
                    && same_area
                    && let Ok(mut dbg) = MERMAID_DEBUG.lock()
                {
                    dbg.stats.skipped_renders += 1;
                }
            }
            if let Ok(mut dbg) = MERMAID_DEBUG.lock() {
                dbg.stats.image_state_hits += 1;
                dbg.stats.fit_state_reuse_hits += 1;
            }
            if !render_stateful_image_safe(hash, render_area, buf, &mut img_state.protocol, resize)
            {
                return 0;
            }
            img_state.last_area = Some(render_area);
            return area.height;
        }
    }

    if let Some(path) = path
        && let Some(Some(picker)) = PICKER.get()
        && let Ok(img) = image::open(&path)
    {
        if let Ok(mut dbg) = MERMAID_DEBUG.lock() {
            dbg.stats.image_state_misses += 1;
            dbg.stats.fit_protocol_rebuilds += 1;
        }
        let target_mode = if scale_up {
            ResizeMode::Scale
        } else {
            ResizeMode::Fit
        };
        let resize = if scale_up {
            Resize::Scale(None)
        } else {
            Resize::Fit(None)
        };
        let protocol = picker.new_resize_protocol(img);

        let mut state = IMAGE_STATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.insert(
            hash,
            ImageState {
                protocol,
                source_path: path.clone(),
                last_area: Some(render_area),
                resize_mode: target_mode,
                last_crop_top: false,
                last_viewport: None,
            },
        );

        if let Some(img_state) = state.get_mut(hash) {
            if !render_stateful_image_safe(hash, render_area, buf, &mut img_state.protocol, resize)
            {
                return 0;
            }
            return area.height;
        }
    }

    let clr_area = area.intersection(buf_area);
    if clr_area.width > 0 && clr_area.height > 0 {
        super::color_support::clear_buf(clr_area, buf);
    }

    0
}

fn load_source_image(hash: u64, path: &Path) -> Option<Arc<DynamicImage>> {
    if let Ok(mut cache) = SOURCE_CACHE.lock()
        && let Some(img) = cache.get(hash, path)
    {
        return Some(img);
    }

    let img = image::open(path).ok()?;
    if let Ok(mut cache) = SOURCE_CACHE.lock() {
        return Some(cache.insert(hash, path.to_path_buf(), img));
    }
    Some(Arc::new(img))
}

fn kitty_viewport_unique_id(hash: u64) -> u32 {
    let mixed = (hash as u32) ^ ((hash >> 32) as u32) ^ 0x4B49_5459;
    mixed.max(1)
}

fn kitty_is_tmux() -> bool {
    std::env::var("TERM").is_ok_and(|term| term.starts_with("tmux"))
        || std::env::var("TERM_PROGRAM").is_ok_and(|term_program| term_program == "tmux")
}

fn kitty_transmit_virtual(img: &DynamicImage, id: u32) -> String {
    let (w, h) = (img.width(), img.height());
    let img_rgba8 = img.to_rgba8();
    let bytes = img_rgba8.as_raw();

    let (start, escape, end) = Parser::escape_tmux(kitty_is_tmux());
    let mut data = String::from(start);

    let chunks = bytes.chunks(4096 / 4 * 3);
    let chunk_count = chunks.len();
    for (i, chunk) in chunks.enumerate() {
        let payload = base64::engine::general_purpose::STANDARD.encode(chunk);
        data.push_str(escape);

        match i {
            0 => {
                let more = if chunk_count > 1 { 1 } else { 0 };
                data.push_str(&format!(
                    "_Gq=2,i={id},a=T,U=1,f=32,t=d,s={w},v={h},m={more};{payload}"
                ));
            }
            n if n + 1 == chunk_count => {
                data.push_str(&format!("_Gq=2,m=0;{payload}"));
            }
            _ => {
                data.push_str(&format!("_Gq=2,m=1;{payload}"));
            }
        }
        data.push_str(escape);
        data.push('\\');
    }
    data.push_str(end);

    data
}

fn kitty_scaled_image_for_zoom(source: &DynamicImage, zoom_percent: u8) -> DynamicImage {
    use image::imageops::FilterType;

    let zoom = zoom_percent.clamp(50, 200) as u32;
    if zoom == 100 {
        return source.clone();
    }

    let scaled_w = ((source.width() as u64).saturating_mul(zoom as u64) / 100)
        .max(1)
        .min(u32::MAX as u64) as u32;
    let scaled_h = ((source.height() as u64).saturating_mul(zoom as u64) / 100)
        .max(1)
        .min(u32::MAX as u64) as u32;
    source.resize_exact(scaled_w, scaled_h, FilterType::Nearest)
}

fn div_ceil_u32_local(value: u32, divisor: u32) -> u32 {
    if divisor == 0 {
        value
    } else {
        value.saturating_add(divisor - 1) / divisor
    }
}

fn kitty_full_rect_for_image(img: &DynamicImage, font_size: (u16, u16)) -> (u16, u16) {
    (
        div_ceil_u32_local(img.width().max(1), font_size.0.max(1) as u32).min(u16::MAX as u32)
            as u16,
        div_ceil_u32_local(img.height().max(1), font_size.1.max(1) as u32).min(u16::MAX as u32)
            as u16,
    )
}

fn ensure_kitty_viewport_state(
    hash: u64,
    source_path: &Path,
    source: &DynamicImage,
    zoom_percent: u8,
    font_size: (u16, u16),
) -> Option<(u32, u16, u16)> {
    let zoom_percent = zoom_percent.clamp(50, 200);
    let mut cache = KITTY_VIEWPORT_STATE.lock().ok()?;
    if let Some(state) = cache.get_mut(hash)
        && state.source_path == source_path
        && state.zoom_percent == zoom_percent
    {
        return Some((state.unique_id, state.full_cols, state.full_rows));
    }

    let scaled = kitty_scaled_image_for_zoom(source, zoom_percent);
    let (full_cols, full_rows) = kitty_full_rect_for_image(&scaled, font_size);
    if full_cols == 0 || full_rows == 0 {
        return None;
    }

    let unique_id = cache
        .get_mut(hash)
        .map(|state| state.unique_id)
        .unwrap_or_else(|| kitty_viewport_unique_id(hash));

    cache.insert(
        hash,
        KittyViewportState {
            source_path: source_path.to_path_buf(),
            zoom_percent,
            unique_id,
            full_cols,
            full_rows,
            pending_transmit: Some(kitty_transmit_virtual(&scaled, unique_id)),
        },
    );

    if let Ok(mut dbg) = MERMAID_DEBUG.lock() {
        dbg.stats.viewport_protocol_rebuilds += 1;
    }

    cache
        .get_mut(hash)
        .map(|state| (state.unique_id, state.full_cols, state.full_rows))
}

fn render_kitty_virtual_viewport(
    hash: u64,
    area: Rect,
    buf: &mut Buffer,
    scroll_x: u16,
    scroll_y: u16,
    visible_width: u16,
    visible_height: u16,
) -> bool {
    if visible_width == 0 || visible_height == 0 {
        return true;
    }

    let mut cache = match KITTY_VIEWPORT_STATE.lock() {
        Ok(cache) => cache,
        Err(_) => return false,
    };
    let Some(state) = cache.get_mut(hash) else {
        return false;
    };
    let unique_id = state.unique_id;
    let pending_transmit = state.pending_transmit.take();
    drop(cache);

    if pending_transmit.is_none()
        && let Ok(mut dbg) = MERMAID_DEBUG.lock()
    {
        dbg.stats.viewport_state_reuse_hits += 1;
    }

    let [id_extra, id_r, id_g, id_b] = unique_id.to_be_bytes();
    let id_color = format!("\x1b[38;2;{id_r};{id_g};{id_b}m");
    let right = area.width.saturating_sub(1);
    let down = area.height.saturating_sub(1);

    for row in 0..area.height {
        let y = area.top() + row;
        if row >= visible_height {
            for x in 0..area.width {
                if let Some(cell) = buf.cell_mut((area.left() + x, y)) {
                    cell.set_symbol(" ");
                    cell.set_skip(false);
                }
            }
            continue;
        }

        let mut symbol = if row == 0 {
            pending_transmit.clone().unwrap_or_default()
        } else {
            String::new()
        };
        symbol.push_str("\x1b[s");
        symbol.push_str(&id_color);
        kitty_add_placeholder(
            &mut symbol,
            scroll_x,
            scroll_y.saturating_add(row),
            id_extra,
        );
        for x in 1..area.width {
            if let Some(cell) = buf.cell_mut((area.left() + x, y)) {
                if x < visible_width {
                    symbol.push('\u{10EEEE}');
                    cell.set_skip(true);
                } else {
                    cell.set_symbol(" ");
                    cell.set_skip(false);
                }
            }
        }
        symbol.push_str(&format!("\x1b[u\x1b[{right}C\x1b[{down}B"));
        if let Some(cell) = buf.cell_mut((area.left(), y)) {
            cell.set_symbol(&symbol);
        }
    }

    true
}

fn can_use_kitty_virtual_viewport(
    full_cols: u16,
    full_rows: u16,
    scroll_x: u16,
    scroll_y: u16,
) -> bool {
    let max_index = KITTY_DIACRITICS.len() as u16;
    full_cols < max_index && full_rows < max_index && scroll_x < max_index && scroll_y < max_index
}

fn kitty_add_placeholder(buf: &mut String, x: u16, y: u16, id_extra: u8) {
    buf.push('\u{10EEEE}');
    buf.push(kitty_diacritic(y));
    buf.push(kitty_diacritic(x));
    buf.push(kitty_diacritic(id_extra as u16));
}

#[inline]
fn kitty_diacritic(index: u16) -> char {
    KITTY_DIACRITICS
        .get(index as usize)
        .copied()
        .unwrap_or(KITTY_DIACRITICS[0])
}

/// From https://sw.kovidgoyal.net/kitty/_downloads/1792bad15b12979994cd6ecc54c967a6/rowcolumn-diacritics.txt
static KITTY_DIACRITICS: [char; 297] = [
    '\u{305}',
    '\u{30D}',
    '\u{30E}',
    '\u{310}',
    '\u{312}',
    '\u{33D}',
    '\u{33E}',
    '\u{33F}',
    '\u{346}',
    '\u{34A}',
    '\u{34B}',
    '\u{34C}',
    '\u{350}',
    '\u{351}',
    '\u{352}',
    '\u{357}',
    '\u{35B}',
    '\u{363}',
    '\u{364}',
    '\u{365}',
    '\u{366}',
    '\u{367}',
    '\u{368}',
    '\u{369}',
    '\u{36A}',
    '\u{36B}',
    '\u{36C}',
    '\u{36D}',
    '\u{36E}',
    '\u{36F}',
    '\u{483}',
    '\u{484}',
    '\u{485}',
    '\u{486}',
    '\u{487}',
    '\u{592}',
    '\u{593}',
    '\u{594}',
    '\u{595}',
    '\u{597}',
    '\u{598}',
    '\u{599}',
    '\u{59C}',
    '\u{59D}',
    '\u{59E}',
    '\u{59F}',
    '\u{5A0}',
    '\u{5A1}',
    '\u{5A8}',
    '\u{5A9}',
    '\u{5AB}',
    '\u{5AC}',
    '\u{5AF}',
    '\u{5C4}',
    '\u{610}',
    '\u{611}',
    '\u{612}',
    '\u{613}',
    '\u{614}',
    '\u{615}',
    '\u{616}',
    '\u{617}',
    '\u{657}',
    '\u{658}',
    '\u{659}',
    '\u{65A}',
    '\u{65B}',
    '\u{65D}',
    '\u{65E}',
    '\u{6D6}',
    '\u{6D7}',
    '\u{6D8}',
    '\u{6D9}',
    '\u{6DA}',
    '\u{6DB}',
    '\u{6DC}',
    '\u{6DF}',
    '\u{6E0}',
    '\u{6E1}',
    '\u{6E2}',
    '\u{6E4}',
    '\u{6E7}',
    '\u{6E8}',
    '\u{6EB}',
    '\u{6EC}',
    '\u{730}',
    '\u{732}',
    '\u{733}',
    '\u{735}',
    '\u{736}',
    '\u{73A}',
    '\u{73D}',
    '\u{73F}',
    '\u{740}',
    '\u{741}',
    '\u{743}',
    '\u{745}',
    '\u{747}',
    '\u{749}',
    '\u{74A}',
    '\u{7EB}',
    '\u{7EC}',
    '\u{7ED}',
    '\u{7EE}',
    '\u{7EF}',
    '\u{7F0}',
    '\u{7F1}',
    '\u{7F3}',
    '\u{816}',
    '\u{817}',
    '\u{818}',
    '\u{819}',
    '\u{81B}',
    '\u{81C}',
    '\u{81D}',
    '\u{81E}',
    '\u{81F}',
    '\u{820}',
    '\u{821}',
    '\u{822}',
    '\u{823}',
    '\u{825}',
    '\u{826}',
    '\u{827}',
    '\u{829}',
    '\u{82A}',
    '\u{82B}',
    '\u{82C}',
    '\u{82D}',
    '\u{951}',
    '\u{953}',
    '\u{954}',
    '\u{F82}',
    '\u{F83}',
    '\u{F86}',
    '\u{F87}',
    '\u{135D}',
    '\u{135E}',
    '\u{135F}',
    '\u{17DD}',
    '\u{193A}',
    '\u{1A17}',
    '\u{1A75}',
    '\u{1A76}',
    '\u{1A77}',
    '\u{1A78}',
    '\u{1A79}',
    '\u{1A7A}',
    '\u{1A7B}',
    '\u{1A7C}',
    '\u{1B6B}',
    '\u{1B6D}',
    '\u{1B6E}',
    '\u{1B6F}',
    '\u{1B70}',
    '\u{1B71}',
    '\u{1B72}',
    '\u{1B73}',
    '\u{1CD0}',
    '\u{1CD1}',
    '\u{1CD2}',
    '\u{1CDA}',
    '\u{1CDB}',
    '\u{1CE0}',
    '\u{1DC0}',
    '\u{1DC1}',
    '\u{1DC3}',
    '\u{1DC4}',
    '\u{1DC5}',
    '\u{1DC6}',
    '\u{1DC7}',
    '\u{1DC8}',
    '\u{1DC9}',
    '\u{1DCB}',
    '\u{1DCC}',
    '\u{1DD1}',
    '\u{1DD2}',
    '\u{1DD3}',
    '\u{1DD4}',
    '\u{1DD5}',
    '\u{1DD6}',
    '\u{1DD7}',
    '\u{1DD8}',
    '\u{1DD9}',
    '\u{1DDA}',
    '\u{1DDB}',
    '\u{1DDC}',
    '\u{1DDD}',
    '\u{1DDE}',
    '\u{1DDF}',
    '\u{1DE0}',
    '\u{1DE1}',
    '\u{1DE2}',
    '\u{1DE3}',
    '\u{1DE4}',
    '\u{1DE5}',
    '\u{1DE6}',
    '\u{1DFE}',
    '\u{20D0}',
    '\u{20D1}',
    '\u{20D4}',
    '\u{20D5}',
    '\u{20D6}',
    '\u{20D7}',
    '\u{20DB}',
    '\u{20DC}',
    '\u{20E1}',
    '\u{20E7}',
    '\u{20E9}',
    '\u{20F0}',
    '\u{2CEF}',
    '\u{2CF0}',
    '\u{2CF1}',
    '\u{2DE0}',
    '\u{2DE1}',
    '\u{2DE2}',
    '\u{2DE3}',
    '\u{2DE4}',
    '\u{2DE5}',
    '\u{2DE6}',
    '\u{2DE7}',
    '\u{2DE8}',
    '\u{2DE9}',
    '\u{2DEA}',
    '\u{2DEB}',
    '\u{2DEC}',
    '\u{2DED}',
    '\u{2DEE}',
    '\u{2DEF}',
    '\u{2DF0}',
    '\u{2DF1}',
    '\u{2DF2}',
    '\u{2DF3}',
    '\u{2DF4}',
    '\u{2DF5}',
    '\u{2DF6}',
    '\u{2DF7}',
    '\u{2DF8}',
    '\u{2DF9}',
    '\u{2DFA}',
    '\u{2DFB}',
    '\u{2DFC}',
    '\u{2DFD}',
    '\u{2DFE}',
    '\u{2DFF}',
    '\u{A66F}',
    '\u{A67C}',
    '\u{A67D}',
    '\u{A6F0}',
    '\u{A6F1}',
    '\u{A8E0}',
    '\u{A8E1}',
    '\u{A8E2}',
    '\u{A8E3}',
    '\u{A8E4}',
    '\u{A8E5}',
    '\u{A8E6}',
    '\u{A8E7}',
    '\u{A8E8}',
    '\u{A8E9}',
    '\u{A8EA}',
    '\u{A8EB}',
    '\u{A8EC}',
    '\u{A8ED}',
    '\u{A8EE}',
    '\u{A8EF}',
    '\u{A8F0}',
    '\u{A8F1}',
    '\u{AAB0}',
    '\u{AAB2}',
    '\u{AAB3}',
    '\u{AAB7}',
    '\u{AAB8}',
    '\u{AABE}',
    '\u{AABF}',
    '\u{AAC1}',
    '\u{FE20}',
    '\u{FE21}',
    '\u{FE22}',
    '\u{FE23}',
    '\u{FE24}',
    '\u{FE25}',
    '\u{FE26}',
    '\u{10A0F}',
    '\u{10A38}',
    '\u{1D185}',
    '\u{1D186}',
    '\u{1D187}',
    '\u{1D188}',
    '\u{1D189}',
    '\u{1D1AA}',
    '\u{1D1AB}',
    '\u{1D1AC}',
    '\u{1D1AD}',
    '\u{1D242}',
    '\u{1D243}',
    '\u{1D244}',
];

/// Render an image by cropping a viewport (for pan/scroll in pinned pane).
pub fn render_image_widget_viewport(
    hash: u64,
    area: Rect,
    buf: &mut Buffer,
    scroll_x: i32,
    scroll_y: i32,
    zoom_percent: u8,
    draw_border: bool,
) -> u16 {
    if VIDEO_EXPORT_MODE.load(Ordering::Relaxed) {
        return area.height;
    }

    let buf_area = *buf.area();
    let area = area.intersection(buf_area);

    if area.width == 0 || area.height == 0 {
        return 0;
    }

    let border_width = if draw_border { BORDER_WIDTH } else { 0 };
    if area.width <= border_width {
        return 0;
    }

    if draw_border {
        draw_left_border(buf, area);
    }

    let image_area = Rect {
        x: area.x + border_width,
        y: area.y,
        width: area.width - border_width,
        height: area.height,
    };

    if image_area.width == 0 || image_area.height == 0 {
        return 0;
    }

    let picker = match PICKER.get().and_then(|p| p.as_ref()) {
        Some(picker) => picker,
        None => return 0,
    };

    let cached = match get_cached_diagram(hash, None) {
        Some(cached) => cached,
        None => return 0,
    };
    let source_path = cached.path.clone();

    let source = match load_source_image(hash, &source_path) {
        Some(img) => img,
        None => return 0,
    };

    let font_size = picker.font_size();
    let zoom = zoom_percent.clamp(50, 200) as u32;
    let view_w_px = (image_area.width as u32)
        .saturating_mul(font_size.0 as u32)
        .saturating_mul(100)
        / zoom;
    let view_h_px = (image_area.height as u32)
        .saturating_mul(font_size.1 as u32)
        .saturating_mul(100)
        / zoom;
    if view_w_px == 0 || view_h_px == 0 {
        return 0;
    }

    let img_width = source.width();
    let img_height = source.height();
    let max_scroll_x = img_width.saturating_sub(view_w_px);
    let max_scroll_y = img_height.saturating_sub(view_h_px);

    let cell_w_px = (font_size.0 as u32).saturating_mul(100) / zoom;
    let cell_h_px = (font_size.1 as u32).saturating_mul(100) / zoom;
    let scroll_x_px = (scroll_x.max(0) as u32)
        .saturating_mul(cell_w_px)
        .min(max_scroll_x);
    let scroll_y_px = (scroll_y.max(0) as u32)
        .saturating_mul(cell_h_px)
        .min(max_scroll_y);

    let crop_w = view_w_px.min(img_width.saturating_sub(scroll_x_px));
    let crop_h = view_h_px.min(img_height.saturating_sub(scroll_y_px));
    if crop_w == 0 || crop_h == 0 {
        return 0;
    }

    let viewport = ViewportState {
        scroll_x_px,
        scroll_y_px,
        view_w_px,
        view_h_px,
    };

    if picker.protocol_type() == ProtocolType::Kitty
        && let Some((_, full_cols, full_rows)) = ensure_kitty_viewport_state(
            hash,
            &source_path,
            source.as_ref(),
            zoom_percent,
            font_size,
        )
    {
        let scroll_x_cells = (scroll_x.max(0) as u16).min(full_cols.saturating_sub(1));
        let scroll_y_cells = (scroll_y.max(0) as u16).min(full_rows.saturating_sub(1));
        if can_use_kitty_virtual_viewport(full_cols, full_rows, scroll_x_cells, scroll_y_cells) {
            let visible_width = image_area
                .width
                .min(full_cols.saturating_sub(scroll_x_cells));
            let visible_height = image_area
                .height
                .min(full_rows.saturating_sub(scroll_y_cells));
            if let Ok(mut state) = IMAGE_STATE.lock()
                && let Some(img_state) = state.get_mut(hash)
            {
                img_state.last_area = Some(image_area);
                img_state.last_viewport = Some(viewport);
            }
            if render_kitty_virtual_viewport(
                hash,
                image_area,
                buf,
                scroll_x_cells,
                scroll_y_cells,
                visible_width,
                visible_height,
            ) {
                return area.height;
            }
        }
    }

    {
        let mut state = IMAGE_STATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let needs_reset = state
            .get(&hash)
            .map(|s| {
                s.resize_mode != ResizeMode::Viewport
                    || s.source_path.as_path() != source_path.as_path()
            })
            .unwrap_or(false);
        if needs_reset {
            state.remove(&hash);
        }
        if let Some(img_state) = state.get_mut(hash)
            && img_state.last_viewport == Some(viewport)
        {
            if let Ok(mut dbg) = MERMAID_DEBUG.lock() {
                dbg.stats.viewport_state_reuse_hits += 1;
            }
            if !render_stateful_image_safe(
                hash,
                image_area,
                buf,
                &mut img_state.protocol,
                Resize::Fit(None),
            ) {
                return 0;
            }
            img_state.last_area = Some(image_area);
            return area.height;
        }
    }

    let cropped = source.crop_imm(scroll_x_px, scroll_y_px, crop_w, crop_h);
    if let Ok(mut dbg) = MERMAID_DEBUG.lock() {
        dbg.stats.viewport_protocol_rebuilds += 1;
    }
    let protocol = picker.new_resize_protocol(cropped);

    let mut state = IMAGE_STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.insert(
        hash,
        ImageState {
            protocol,
            source_path,
            last_area: Some(image_area),
            resize_mode: ResizeMode::Viewport,
            last_crop_top: false,
            last_viewport: Some(viewport),
        },
    );

    if let Some(img_state) = state.get_mut(hash) {
        if !render_stateful_image_safe(
            hash,
            image_area,
            buf,
            &mut img_state.protocol,
            Resize::Fit(None),
        ) {
            return 0;
        }
        return area.height;
    }

    0
}

/// Clear an area that previously had an image (removes stale terminal graphics)
/// This is called when an image's marker scrolls off-screen but its area still overlaps
/// the visible region - we need to explicitly clear the terminal graphics layer.
pub fn clear_image_area(area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let clamped = area.intersection(*buf.area());
    if clamped.width == 0 || clamped.height == 0 {
        return;
    }
    if let Ok(mut dbg) = MERMAID_DEBUG.lock() {
        dbg.stats.clear_operations += 1;
    }
    super::color_support::clear_buf(clamped, buf);
}

/// Invalidate last render state for a hash (call when content changes)
pub fn invalidate_render_state(hash: u64) {
    if let Ok(mut last_render) = LAST_RENDER.lock() {
        last_render.remove(&hash);
    }
}

/// Estimate the height needed for an image in terminal rows
pub fn estimate_image_height(width: u32, height: u32, max_width: u16) -> u16 {
    if let Some(Some(picker)) = PICKER.get() {
        let font_size = picker.font_size();
        // Calculate how many rows the image will take
        let img_width_cells = (width as f32 / font_size.0 as f32).ceil() as u16;
        let img_height_cells = (height as f32 / font_size.1 as f32).ceil() as u16;

        // If image is wider than max_width, scale down proportionally
        if img_width_cells > max_width {
            let scale = max_width as f32 / img_width_cells as f32;
            (img_height_cells as f32 * scale).ceil() as u16
        } else {
            img_height_cells
        }
    } else {
        // Fallback: assume ~8x16 font
        let aspect = width as f32 / height as f32;
        let h = (max_width as f32 / aspect / 2.0).ceil() as u16;
        h.min(30) // Cap at reasonable height
    }
}

/// Content that can be rendered - either text lines or an image
#[derive(Clone)]
pub enum MermaidContent {
    /// Regular text lines
    Lines(Vec<Line<'static>>),
    /// Image to be rendered as a widget
    Image { hash: u64, estimated_height: u16 },
}

/// Convert render result to content that can be displayed
pub fn result_to_content(result: RenderResult, max_width: Option<usize>) -> MermaidContent {
    match result {
        RenderResult::Image {
            hash,
            width,
            height,
            ..
        } => {
            // Check if we have picker/protocol support (or video export mode)
            if PICKER.get().and_then(|p| p.as_ref()).is_some()
                || VIDEO_EXPORT_MODE.load(Ordering::Relaxed)
            {
                let max_w = max_width.map(|w| w as u16).unwrap_or(80);
                let estimated_height = estimate_image_height(width, height, max_w);
                MermaidContent::Image {
                    hash,
                    estimated_height,
                }
            } else {
                MermaidContent::Lines(image_placeholder_lines(width, height))
            }
        }
        RenderResult::Error(msg) => MermaidContent::Lines(error_to_lines(&msg)),
    }
}

/// Convert render result to lines (legacy API, uses placeholder for images)
pub fn result_to_lines(result: RenderResult, max_width: Option<usize>) -> Vec<Line<'static>> {
    match result_to_content(result, max_width) {
        MermaidContent::Lines(lines) => lines,
        MermaidContent::Image {
            hash,
            estimated_height,
        } => {
            // Return placeholder lines that will be replaced by image widget
            image_widget_placeholder(hash, estimated_height)
        }
    }
}

/// Marker prefix for mermaid image placeholders
const MERMAID_MARKER_PREFIX: &str = "\x00MERMAID_IMAGE:";
const MERMAID_MARKER_SUFFIX: &str = "\x00";

/// Create placeholder lines for an image widget
/// These will be recognized and replaced during rendering
fn image_widget_placeholder(hash: u64, height: u16) -> Vec<Line<'static>> {
    // Use invisible styling - black on black won't show even if render fails
    // because we only clear on render failure now
    let invisible = Style::default().fg(Color::Black).bg(Color::Black);

    let mut lines = Vec::with_capacity(height as usize);

    // First line contains the hash as a marker
    lines.push(Line::from(Span::styled(
        format!(
            "{}{:016x}{}",
            MERMAID_MARKER_PREFIX, hash, MERMAID_MARKER_SUFFIX
        ),
        invisible,
    )));

    // Fill remaining height with empty lines (will be overwritten by image)
    for _ in 1..height {
        lines.push(Line::from(""));
    }

    lines
}

/// Check if a line is a mermaid image placeholder and extract the hash
pub fn parse_image_placeholder(line: &Line<'_>) -> Option<u64> {
    if line.spans.is_empty() {
        return None;
    }

    let content = &line.spans[0].content;
    if content.starts_with(MERMAID_MARKER_PREFIX) && content.ends_with(MERMAID_MARKER_SUFFIX) {
        // Extract hex between prefix and suffix
        let start = MERMAID_MARKER_PREFIX.len();
        let end = content.len() - MERMAID_MARKER_SUFFIX.len();
        if end > start {
            let hex = &content[start..end];
            return u64::from_str_radix(hex, 16).ok();
        }
    }
    None
}

/// Write a mermaid image marker into a buffer area (for video export mode).
/// This allows the SVG pipeline to detect the region and embed the cached PNG.
pub fn write_video_export_marker(hash: u64, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let invisible = Style::default().fg(Color::Black).bg(Color::Black);
    // Use printable marker characters that won't break SVG XML
    let marker = format!("JMERMAID:{:016x}:END", hash);
    // Write marker on the first row
    let y = area.y;
    for (i, ch) in marker.chars().enumerate() {
        let x = area.x + i as u16;
        if x < area.x + area.width {
            buf[(x, y)].set_char(ch).set_style(invisible);
        }
    }
    // Clear remaining rows (empty for region detection)
    for row in (area.y + 1)..(area.y + area.height) {
        for col in area.x..(area.x + area.width) {
            buf[(col, row)].set_char(' ').set_style(invisible);
        }
    }
}

/// Create placeholder lines for when image protocols aren't available
fn image_placeholder_lines(width: u32, height: u32) -> Vec<Line<'static>> {
    let dim = Style::default().fg(rgb(100, 100, 100));
    let info = Style::default().fg(rgb(140, 170, 200));

    vec![
        Line::from(Span::styled("┌─ mermaid diagram ", dim)),
        Line::from(vec![
            Span::styled("│ ", dim),
            Span::styled(
                format!("{}×{} px (image protocols not available)", width, height),
                info,
            ),
        ]),
        Line::from(Span::styled("└─", dim)),
    ]
}

/// Public helper for pinned diagram pane placeholders
pub fn diagram_placeholder_lines(width: u32, height: u32) -> Vec<Line<'static>> {
    image_placeholder_lines(width, height)
}

/// Convert error to ratatui Lines
pub fn error_to_lines(error: &str) -> Vec<Line<'static>> {
    let dim = Style::default().fg(rgb(100, 100, 100));
    let err_style = Style::default().fg(rgb(200, 80, 80));

    // Calculate box width based on content
    let header = "mermaid error";
    let content_width = error.len().max(header.len());
    let top_padding = content_width.saturating_sub(header.len());
    let bottom_width = content_width + 1;

    vec![
        Line::from(Span::styled(
            format!("┌─ {} {}┐", header, "─".repeat(top_padding)),
            dim,
        )),
        Line::from(vec![
            Span::styled("│ ", dim),
            Span::styled(
                format!("{:<width$}", error, width = content_width),
                err_style,
            ),
            Span::styled("│", dim),
        ]),
        Line::from(Span::styled(
            format!("└─{}─┘", "─".repeat(bottom_width)),
            dim,
        )),
    ]
}

/// Terminal-friendly theme (works on dark backgrounds)
fn terminal_theme() -> Theme {
    Theme {
        // Catppuccin-inspired pastel dark theme tuned for jcode's terminal UI.
        // Uses transparent canvas so the rendered PNG integrates with the TUI,
        // while keeping nodes/labels readable against dark panes.
        background: "#00000000".to_string(),
        font_family: "Inter, ui-sans-serif, system-ui, -apple-system, Segoe UI, sans-serif"
            .to_string(),
        font_size: 15.0,
        primary_color: "#313244".to_string(),
        primary_text_color: "#cdd6f4".to_string(),
        primary_border_color: "#b4befe".to_string(),
        line_color: "#74c7ec".to_string(),
        secondary_color: "#45475a".to_string(),
        tertiary_color: "#1e1e2e".to_string(),
        edge_label_background: "#1e1e2eee".to_string(),
        cluster_background: "#181825d9".to_string(),
        cluster_border: "#6c7086".to_string(),
        text_color: "#cdd6f4".to_string(),
        // Sequence diagram colors: soft surfaces with pastel borders so actor
        // boxes, notes, and activations remain distinct without becoming loud.
        sequence_actor_fill: "#313244".to_string(),
        sequence_actor_border: "#89b4fa".to_string(),
        sequence_actor_line: "#7f849c".to_string(),
        sequence_note_fill: "#45475a".to_string(),
        sequence_note_border: "#f9e2af".to_string(),
        sequence_activation_fill: "#1e1e2e".to_string(),
        sequence_activation_border: "#cba6f7".to_string(),
        // Git/journey/mindmap accent cycle.
        git_colors: [
            "#b4befe".to_string(), // lavender
            "#89b4fa".to_string(), // blue
            "#94e2d5".to_string(), // teal
            "#a6e3a1".to_string(), // green
            "#f9e2af".to_string(), // yellow
            "#fab387".to_string(), // peach
            "#eba0ac".to_string(), // maroon
            "#f5c2e7".to_string(), // pink
        ],
        git_inv_colors: [
            "#cba6f7".to_string(), // mauve
            "#74c7ec".to_string(), // sapphire
            "#89dceb".to_string(), // sky
            "#94e2d5".to_string(), // teal
            "#fab387".to_string(), // peach
            "#f38ba8".to_string(), // red
            "#eba0ac".to_string(), // maroon
            "#f2cdcd".to_string(), // flamingo
        ],
        git_branch_label_colors: [
            "#1e1e2e".to_string(),
            "#1e1e2e".to_string(),
            "#1e1e2e".to_string(),
            "#1e1e2e".to_string(),
            "#1e1e2e".to_string(),
            "#1e1e2e".to_string(),
            "#1e1e2e".to_string(),
            "#1e1e2e".to_string(),
        ],
        git_commit_label_color: "#cdd6f4".to_string(),
        git_commit_label_background: "#313244".to_string(),
        git_tag_label_color: "#1e1e2e".to_string(),
        git_tag_label_background: "#b4befe".to_string(),
        git_tag_label_border: "#cba6f7".to_string(),
        pie_colors: [
            "#cba6f7".to_string(), // mauve
            "#b4befe".to_string(), // lavender
            "#89b4fa".to_string(), // blue
            "#74c7ec".to_string(), // sapphire
            "#89dceb".to_string(), // sky
            "#94e2d5".to_string(), // teal
            "#a6e3a1".to_string(), // green
            "#f9e2af".to_string(), // yellow
            "#fab387".to_string(), // peach
            "#eba0ac".to_string(), // maroon
            "#f38ba8".to_string(), // red
            "#f5c2e7".to_string(), // pink
        ],
        pie_title_text_size: 24.0,
        pie_title_text_color: "#cdd6f4".to_string(),
        pie_section_text_size: 15.0,
        pie_section_text_color: "#1e1e2e".to_string(),
        pie_legend_text_size: 15.0,
        pie_legend_text_color: "#bac2de".to_string(),
        pie_stroke_color: "#181825".to_string(),
        pie_stroke_width: 1.4,
        pie_outer_stroke_width: 1.6,
        pie_outer_stroke_color: "#45475a".to_string(),
        pie_opacity: 0.92,
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
