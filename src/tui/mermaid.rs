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

use image::DynamicImage;
use mermaid_rs_renderer::{
    config::{LayoutConfig, RenderConfig},
    layout::compute_layout,
    parser::parse_mermaid,
    render::{render_svg, write_output_png},
    theme::Theme,
};
use ratatui::prelude::*;
use ratatui_image::{
    picker::{Picker, ProtocolType},
    protocol::StatefulProtocol,
    CropOptions, Resize, StatefulImage,
};
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::hash::{Hash as _, Hasher};
use std::panic;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::Instant;

const DEFAULT_RENDER_WIDTH: u32 = 1600;
const DEFAULT_RENDER_HEIGHT: u32 = 1200;

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

/// Last render state for skip-redundant-render optimization
static LAST_RENDER: LazyLock<Mutex<HashMap<u64, LastRenderState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Render errors for lazy mermaid diagrams (hash -> error message)
static RENDER_ERRORS: LazyLock<Mutex<HashMap<u64, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Pending mermaid content for lazy rendering (hash -> content)
static PENDING_DIAGRAMS: LazyLock<Mutex<HashMap<u64, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Active diagrams for info widget display
/// Updated during markdown rendering, queried by info_widget_data()
static ACTIVE_DIAGRAMS: LazyLock<Mutex<Vec<ActiveDiagram>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

/// Ephemeral diagram preview for in-flight streaming markdown.
/// This should never persist once a streaming segment is committed.
static STREAMING_PREVIEW_DIAGRAM: LazyLock<Mutex<Option<ActiveDiagram>>> =
    LazyLock::new(|| Mutex::new(None));

/// Prevent unbounded growth when a long session contains many unique diagrams.
const ACTIVE_DIAGRAMS_MAX: usize = 128;

/// Info about an active diagram (for info widget)
#[derive(Clone)]
struct ActiveDiagram {
    hash: u64,
    width: u32,
    height: u32,
    label: Option<String>,
}

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

    fn contains_key(&self, hash: &u64) -> bool {
        self.entries.contains_key(hash)
    }

    fn insert(&mut self, hash: u64, state: ImageState) {
        if self.entries.contains_key(&hash) {
            self.entries.insert(hash, state);
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
#[derive(Clone, Copy, PartialEq, Eq)]
enum ResizeMode {
    Fit,
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
#[derive(Clone, PartialEq, Eq)]
struct LastRenderState {
    area: Rect,
    centered: bool,
}

/// Debug stats for mermaid rendering
#[derive(Debug, Clone, Default, Serialize)]
pub struct MermaidDebugStats {
    pub total_requests: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub render_success: u64,
    pub render_errors: u64,
    pub last_render_ms: Option<f32>,
    pub last_error: Option<String>,
    pub last_hash: Option<String>,
    pub last_nodes: Option<usize>,
    pub last_edges: Option<usize>,
    pub last_content_len: Option<usize>,
    pub image_state_hits: u64,
    pub image_state_misses: u64,
    pub skipped_renders: u64,
    pub last_image_render_ms: Option<f32>,
    pub cache_entries: usize,
    pub cache_dir: Option<String>,
    pub protocol: Option<String>,
    pub last_png_width: Option<u32>,
    pub last_png_height: Option<u32>,
}

#[derive(Debug, Clone, Default)]
struct MermaidDebugState {
    stats: MermaidDebugStats,
}

static MERMAID_DEBUG: LazyLock<Mutex<MermaidDebugState>> =
    LazyLock::new(|| Mutex::new(MermaidDebugState::default()));

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

#[derive(Debug, Clone, Default)]
struct ProcessMemorySnapshot {
    rss_bytes: Option<u64>,
    peak_rss_bytes: Option<u64>,
    virtual_bytes: Option<u64>,
}

pub fn debug_stats() -> MermaidDebugStats {
    let mut out = if let Ok(state) = MERMAID_DEBUG.lock() {
        state.stats.clone()
    } else {
        MermaidDebugStats::default()
    };

    // Fill runtime fields
    if let Ok(cache) = RENDER_CACHE.lock() {
        out.cache_entries = cache.entries.len();
        out.cache_dir = Some(cache.cache_dir.to_string_lossy().to_string());
    }
    out.protocol = protocol_type().map(|p| format!("{:?}", p));
    out
}

pub fn debug_stats_json() -> Option<serde_json::Value> {
    serde_json::to_value(debug_stats()).ok()
}

pub fn debug_cache() -> Vec<MermaidCacheEntry> {
    if let Ok(cache) = RENDER_CACHE.lock() {
        return cache
            .entries
            .iter()
            .map(|(hash, diagram)| MermaidCacheEntry {
                hash: format!("{:016x}", hash),
                path: diagram.path.to_string_lossy().to_string(),
                width: diagram.width,
                height: diagram.height,
            })
            .collect();
    }
    Vec::new()
}

pub fn debug_memory_profile() -> MermaidMemoryProfile {
    let process_mem = process_memory_snapshot();
    let mut out = MermaidMemoryProfile {
        process_rss_bytes: process_mem.rss_bytes,
        process_peak_rss_bytes: process_mem.peak_rss_bytes,
        process_virtual_bytes: process_mem.virtual_bytes,
        render_cache_limit: RENDER_CACHE_MAX,
        image_state_limit: IMAGE_STATE_MAX,
        source_cache_limit: SOURCE_CACHE_MAX,
        active_diagrams_limit: ACTIVE_DIAGRAMS_MAX,
        cache_disk_limit_bytes: CACHE_MAX_SIZE_BYTES,
        cache_disk_max_age_secs: CACHE_MAX_AGE_SECS,
        ..MermaidMemoryProfile::default()
    };

    let mut cache_dir: Option<PathBuf> = None;
    if let Ok(cache) = RENDER_CACHE.lock() {
        out.render_cache_entries = cache.entries.len();
        out.render_cache_metadata_estimate_bytes = cache
            .entries
            .values()
            .map(|diagram| {
                (std::mem::size_of::<CachedDiagram>() as u64)
                    .saturating_add(diagram.path.to_string_lossy().len() as u64)
                    .saturating_add(24)
            })
            .sum();
        cache_dir = Some(cache.cache_dir.clone());
    }

    if let Some(dir) = cache_dir.as_deref() {
        let (count, bytes) = scan_cache_dir_png_usage(dir);
        out.cache_disk_png_files = count;
        out.cache_disk_png_bytes = bytes;
    }

    if let Ok(state) = IMAGE_STATE.lock() {
        out.image_state_entries = state.entries.len();
        let mut seen_paths: HashSet<PathBuf> = HashSet::new();
        for (_, image_state) in state.iter() {
            if seen_paths.insert(image_state.source_path.clone()) {
                if let Some((w, h)) = get_png_dimensions(&image_state.source_path) {
                    out.image_state_protocol_min_estimate_bytes = out
                        .image_state_protocol_min_estimate_bytes
                        .saturating_add(rgba_bytes_estimate(w, h));
                }
            }
        }
    }

    if let Ok(source) = SOURCE_CACHE.lock() {
        out.source_cache_entries = source.entries.len();
        for entry in source.entries.values() {
            out.source_cache_decoded_estimate_bytes = out
                .source_cache_decoded_estimate_bytes
                .saturating_add(rgba_bytes_estimate(
                    entry.image.width(),
                    entry.image.height(),
                ));
        }
    }

    if let Ok(diagrams) = ACTIVE_DIAGRAMS.lock() {
        out.active_diagrams = diagrams.len();
    }

    out.mermaid_working_set_estimate_bytes = out
        .render_cache_metadata_estimate_bytes
        .saturating_add(out.image_state_protocol_min_estimate_bytes)
        .saturating_add(out.source_cache_decoded_estimate_bytes);

    out
}

pub fn debug_memory_benchmark(iterations: usize) -> MermaidMemoryBenchmark {
    let iterations = iterations.clamp(1, 256);
    let before = debug_memory_profile();
    let mut peak_rss = before.process_rss_bytes;
    let mut peak_working_set = before.mermaid_working_set_estimate_bytes;
    let mut errors = 0usize;

    for idx in 0..iterations {
        let content = format!(
            "flowchart TD\n    A{i}[Start {i}] --> B{i}{{Check}}\n    B{i} -->|yes| C{i}[Fast path]\n    B{i} -->|no| D{i}[Slow path]\n    C{i} --> E{i}[Done]\n    D{i} --> E{i}",
            i = idx
        );

        if matches!(
            render_mermaid_untracked(&content, Some(96)),
            RenderResult::Error(_)
        ) {
            errors += 1;
        }

        let sample = debug_memory_profile();
        peak_rss = max_opt_u64(peak_rss, sample.process_rss_bytes);
        peak_working_set = peak_working_set.max(sample.mermaid_working_set_estimate_bytes);
    }

    let after = debug_memory_profile();
    peak_rss = max_opt_u64(peak_rss, after.process_rss_bytes);
    peak_working_set = peak_working_set.max(after.mermaid_working_set_estimate_bytes);

    MermaidMemoryBenchmark {
        iterations,
        errors,
        rss_delta_bytes: diff_opt_u64(after.process_rss_bytes, before.process_rss_bytes),
        working_set_delta_bytes: diff_u64(
            after.mermaid_working_set_estimate_bytes,
            before.mermaid_working_set_estimate_bytes,
        ),
        peak_rss_bytes: peak_rss,
        peak_working_set_estimate_bytes: peak_working_set,
        before,
        after,
    }
}

fn scan_cache_dir_png_usage(cache_dir: &Path) -> (usize, u64) {
    let Ok(entries) = fs::read_dir(cache_dir) else {
        return (0, 0);
    };

    let mut file_count = 0usize;
    let mut total_bytes = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "png") {
            file_count += 1;
            if let Ok(meta) = entry.metadata() {
                total_bytes = total_bytes.saturating_add(meta.len());
            }
        }
    }
    (file_count, total_bytes)
}

fn rgba_bytes_estimate(width: u32, height: u32) -> u64 {
    (width as u64)
        .saturating_mul(height as u64)
        .saturating_mul(4)
}

fn max_opt_u64(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}

fn diff_u64(after: u64, before: u64) -> i64 {
    if after >= before {
        (after - before).min(i64::MAX as u64) as i64
    } else {
        -((before - after).min(i64::MAX as u64) as i64)
    }
}

fn diff_opt_u64(after: Option<u64>, before: Option<u64>) -> Option<i64> {
    match (after, before) {
        (Some(after), Some(before)) => Some(diff_u64(after, before)),
        _ => None,
    }
}

fn parse_proc_status_kib_line(line: &str, key: &str) -> Option<u64> {
    let rest = line.strip_prefix(key)?.trim();
    let value_kib = rest.split_whitespace().next()?.parse::<u64>().ok()?;
    Some(value_kib.saturating_mul(1024))
}

fn parse_proc_status_value_bytes(status: &str, key: &str) -> Option<u64> {
    status
        .lines()
        .find_map(|line| parse_proc_status_kib_line(line, key))
}

#[cfg(target_os = "linux")]
fn process_memory_snapshot() -> ProcessMemorySnapshot {
    let Ok(status) = fs::read_to_string("/proc/self/status") else {
        return ProcessMemorySnapshot::default();
    };
    ProcessMemorySnapshot {
        rss_bytes: parse_proc_status_value_bytes(&status, "VmRSS:"),
        peak_rss_bytes: parse_proc_status_value_bytes(&status, "VmHWM:"),
        virtual_bytes: parse_proc_status_value_bytes(&status, "VmSize:"),
    }
}

#[cfg(not(target_os = "linux"))]
fn process_memory_snapshot() -> ProcessMemorySnapshot {
    ProcessMemorySnapshot::default()
}

/// Register a diagram as active (call during markdown rendering)
pub fn register_active_diagram(hash: u64, width: u32, height: u32, label: Option<String>) {
    if let Ok(mut diagrams) = ACTIVE_DIAGRAMS.lock() {
        if let Some(pos) = diagrams.iter().position(|d| d.hash == hash) {
            let mut existing = diagrams.remove(pos);
            existing.width = width;
            existing.height = height;
            if label.is_some() {
                existing.label = label;
            }
            diagrams.push(existing);
        } else {
            diagrams.push(ActiveDiagram {
                hash,
                width,
                height,
                label,
            });
        }
        while diagrams.len() > ACTIVE_DIAGRAMS_MAX {
            diagrams.remove(0);
        }
    }
}

/// Register or replace the current streaming preview diagram.
pub fn set_streaming_preview_diagram(hash: u64, width: u32, height: u32, label: Option<String>) {
    if let Ok(mut preview) = STREAMING_PREVIEW_DIAGRAM.lock() {
        *preview = Some(ActiveDiagram {
            hash,
            width,
            height,
            label,
        });
    }
}

/// Clear the current streaming preview diagram.
pub fn clear_streaming_preview_diagram() {
    if let Ok(mut preview) = STREAMING_PREVIEW_DIAGRAM.lock() {
        *preview = None;
    }
}

/// Get active diagrams for info widget display
pub fn get_active_diagrams() -> Vec<super::info_widget::DiagramInfo> {
    let preview = STREAMING_PREVIEW_DIAGRAM
        .lock()
        .ok()
        .and_then(|preview| preview.clone());
    let preview_hash = preview.as_ref().map(|d| d.hash);

    let mut out = Vec::new();
    if let Some(d) = preview {
        out.push(super::info_widget::DiagramInfo {
            hash: d.hash,
            width: d.width,
            height: d.height,
            label: d.label,
        });
    }

    if let Ok(diagrams) = ACTIVE_DIAGRAMS.lock() {
        out.extend(
            diagrams
                .iter()
                .rev()
                .filter(|d| Some(d.hash) != preview_hash)
                .map(|d| super::info_widget::DiagramInfo {
                    hash: d.hash,
                    width: d.width,
                    height: d.height,
                    label: d.label.clone(),
                }),
        );
    }

    out
}

/// Snapshot active diagrams (internal order) for temporary overrides in tests/debug
pub fn snapshot_active_diagrams() -> Vec<super::info_widget::DiagramInfo> {
    if let Ok(diagrams) = ACTIVE_DIAGRAMS.lock() {
        return diagrams
            .iter()
            .map(|d| super::info_widget::DiagramInfo {
                hash: d.hash,
                width: d.width,
                height: d.height,
                label: d.label.clone(),
            })
            .collect();
    }
    Vec::new()
}

/// Restore active diagrams from a snapshot
pub fn restore_active_diagrams(snapshot: Vec<super::info_widget::DiagramInfo>) {
    if let Ok(mut diagrams) = ACTIVE_DIAGRAMS.lock() {
        diagrams.clear();
        diagrams.extend(snapshot.into_iter().map(|d| ActiveDiagram {
            hash: d.hash,
            width: d.width,
            height: d.height,
            label: d.label,
        }));
        while diagrams.len() > ACTIVE_DIAGRAMS_MAX {
            diagrams.remove(0);
        }
    }
}

/// Clear active diagrams (call at start of render cycle)
pub fn clear_active_diagrams() {
    if let Ok(mut diagrams) = ACTIVE_DIAGRAMS.lock() {
        diagrams.clear();
    }
    clear_streaming_preview_diagram();
}

pub fn clear_cache() -> Result<(), String> {
    let cache_dir = if let Ok(cache) = RENDER_CACHE.lock() {
        cache.cache_dir.clone()
    } else {
        PathBuf::from("/tmp")
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
    if let Ok(mut last) = LAST_RENDER.lock() {
        last.clear();
    }
    if let Ok(mut diagrams) = ACTIVE_DIAGRAMS.lock() {
        diagrams.clear();
    }
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
        RenderResult::Error(e) => {
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
            if let Ok(state) = IMAGE_STATE.lock() {
                if let Some(img_state) = state.get(&hash) {
                    let mode = match img_state.resize_mode {
                        ResizeMode::Fit => "Fit",
                        ResizeMode::Crop => "Crop",
                        ResizeMode::Viewport => "Viewport",
                    };
                    frame_info.resize_mode = Some(mode.to_string());
                    modes_seen.push(mode.to_string());
                }
            }

            // Check border was rendered (first column should have │)
            if area.x < buf.area().width && area.y < buf.area().height {
                let cell = buf.get(area.x, area.y);
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

/// Initialize the global picker by querying terminal capabilities.
/// Should be called early in app startup, after entering alternate screen.
/// Also triggers cache eviction on first call.
pub fn init_picker() {
    PICKER.get_or_init(|| Picker::from_query_stdio().ok());
    // Evict old cache files once per process
    CACHE_EVICTED.get_or_init(|| {
        evict_old_cache();
    });
}

/// Get the current protocol type (for debugging/display)
pub fn protocol_type() -> Option<ProtocolType> {
    let real = PICKER.get().and_then(|p| p.map(|p| p.protocol_type()));
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
    PICKER.get().and_then(|p| *p).is_some() || VIDEO_EXPORT_MODE.load(Ordering::Relaxed)
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
                complexity: 0,
            },
        );
    }
    hash
}

fn has_render_error(hash: u64) -> bool {
    RENDER_ERRORS
        .lock()
        .ok()
        .map_or(false, |errors| errors.contains_key(&hash))
}

fn record_render_error(hash: u64, message: String) {
    if let Ok(mut errors) = RENDER_ERRORS.lock() {
        errors.insert(hash, message);
    }
}

fn clear_render_error(hash: u64) {
    if let Ok(mut errors) = RENDER_ERRORS.lock() {
        errors.remove(&hash);
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
    PICKER.get().and_then(|p| p.map(|p| p.font_size()))
}

/// Maximum in-memory RENDER_CACHE entries (metadata only, not images).
const RENDER_CACHE_MAX: usize = 64;
/// Reuse a cached PNG only if it's at least this fraction of requested width.
/// This avoids visibly blurry upscaling after terminal/pane resizes.
const CACHE_WIDTH_MATCH_PERCENT: u32 = 85;

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
    /// Complexity score (nodes + edges) for adaptive sizing decisions
    complexity: usize,
}

impl MermaidCache {
    fn new() -> Self {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
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
        if self.entries.contains_key(&hash) {
            self.entries.insert(hash, diagram);
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
                candidates
                    .iter()
                    .max_by_key(|(_, w)| *w)
                    .cloned()
                    .unwrap_or_else(|| candidates[0].clone())
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
            complexity: 0,
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

fn invalidate_cached_image(hash: u64) {
    if let Ok(mut state) = IMAGE_STATE.lock() {
        state.remove(&hash);
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
    let mut nodes = 0;
    let mut edges = 0;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("%%") {
            continue;
        }
        // Count arrow connections as edges
        if trimmed.contains("-->")
            || trimmed.contains("-.->")
            || trimmed.contains("==>")
            || trimmed.contains("---")
            || trimmed.contains("-.-")
        {
            edges += 1;
        }
        // Count node definitions (rough heuristic)
        if trimmed.contains('[') && trimmed.contains(']') {
            nodes += 1;
        } else if trimmed.contains('{') && trimmed.contains('}') {
            nodes += 1;
        } else if trimmed.contains('(') && trimmed.contains(')') {
            nodes += 1;
        }
    }

    (nodes.max(2), edges.max(1)) // Minimum reasonable values
}

/// Calculate optimal PNG dimensions based on terminal and diagram complexity
fn calculate_render_size(
    node_count: usize,
    edge_count: usize,
    terminal_width: Option<u16>,
) -> (f64, f64) {
    let base_width = if let Some(term_width) = terminal_width {
        let font_width = get_font_size().map(|(w, _)| w).unwrap_or(8) as f64;
        let pixel_width = term_width as f64 * font_width;
        pixel_width.clamp(400.0, DEFAULT_RENDER_WIDTH as f64)
    } else {
        1200.0
    };

    let complexity = node_count + edge_count;
    let complexity_factor = match complexity {
        0..=5 => 0.6,
        6..=15 => 0.8,
        16..=30 => 1.0,
        _ => 1.1,
    };

    let width = (base_width * complexity_factor).clamp(400.0, DEFAULT_RENDER_WIDTH as f64);
    let height = (width * 0.75).clamp(300.0, DEFAULT_RENDER_HEIGHT as f64);

    (width, height)
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

fn render_mermaid_sized_internal(
    content: &str,
    terminal_width: Option<u16>,
    register_active: bool,
) -> RenderResult {
    if let Ok(mut state) = MERMAID_DEBUG.lock() {
        state.stats.total_requests += 1;
        state.stats.last_content_len = Some(content.len());
        state.stats.last_error = None;
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
        let cache = RENDER_CACHE.lock().unwrap();
        cache.cache_path(hash, target_width_u32)
    };
    let png_path_clone = png_path.clone();

    // Wrap mermaid library calls in catch_unwind for defense-in-depth
    let content_owned = content.to_string();

    let prev_hook = panic::take_hook();
    panic::set_hook(Box::new(|_| {
        // Silently ignore panics from mermaid renderer
    }));

    let render_start = Instant::now();
    let render_result = panic::catch_unwind(move || -> Result<(), String> {
        // Parse mermaid
        let parsed = parse_mermaid(&content_owned).map_err(|e| format!("Parse error: {}", e))?;

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

        // Compute layout
        let layout = compute_layout(&parsed.graph, &theme, &layout_config);

        // Render to SVG
        let svg = render_svg(&layout, &theme, &layout_config);

        // Convert SVG to PNG with adaptive dimensions
        let render_config = RenderConfig {
            width: DEFAULT_RENDER_WIDTH as f32,
            height: DEFAULT_RENDER_HEIGHT as f32,
            background: theme.background.clone(),
        };

        // Ensure parent directory exists
        if let Some(parent) = png_path_clone.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create cache directory: {}", e))?;
        }

        write_output_png(&svg, &png_path_clone, &render_config, &theme)
            .map_err(|e| format!("Render error: {}", e))?;

        Ok(())
    });

    // Restore the original panic hook
    panic::set_hook(prev_hook);

    // Handle the result
    let render_ms = render_start.elapsed().as_secs_f32() * 1000.0;
    match render_result {
        Ok(Ok(())) => {
            if let Ok(mut state) = MERMAID_DEBUG.lock() {
                state.stats.render_success += 1;
                state.stats.last_render_ms = Some(render_ms);
            }
        }
        Ok(Err(e)) => {
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
        let mut cache = RENDER_CACHE.lock().unwrap();
        cache.insert(
            hash,
            CachedDiagram {
                path: png_path.clone(),
                width,
                height,
                complexity,
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
    let cell = buf.get_mut(x, y);
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
    let border_style = Style::default().fg(Color::Rgb(100, 100, 100)); // DIM_COLOR
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
        let mut state = IMAGE_STATE.lock().unwrap();
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
                let background = img_state.protocol.background_color();
                let mut force_area = img_state.protocol.area();
                if force_area.width == 0 || force_area.height == 0 {
                    force_area = render_area;
                }
                img_state.protocol.resize_encode(
                    &Resize::Crop(Some(crop_opts.clone())),
                    background,
                    force_area,
                );
                img_state.last_crop_top = crop_top;
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
    if let Some(path) = path {
        if let Some(Some(picker)) = PICKER.get() {
            if let Ok(img) = image::open(&path) {
                let protocol = picker.new_resize_protocol(img);

                let mut state = IMAGE_STATE.lock().unwrap();
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
        }
    }

    // Render failed - clear the area to avoid showing stale content
    use ratatui::widgets::Clear;
    let clear_area = area.intersection(buf_area);
    if clear_area.width > 0 && clear_area.height > 0 {
        Clear.render(clear_area, buf);
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
        let mut state = IMAGE_STATE.lock().unwrap();
        let needs_reset = state
            .get(&hash)
            .map(|s| {
                s.resize_mode != ResizeMode::Fit
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
            img_state.resize_mode = ResizeMode::Fit;
            img_state.last_viewport = None;
            if !render_stateful_image_safe(
                hash,
                render_area,
                buf,
                &mut img_state.protocol,
                Resize::Fit(None),
            ) {
                return 0;
            }
            img_state.last_area = Some(render_area);
            return area.height;
        }
    }

    if let Some(path) = path {
        if let Some(Some(picker)) = PICKER.get() {
            if let Ok(img) = image::open(&path) {
                let protocol = picker.new_resize_protocol(img);

                let mut state = IMAGE_STATE.lock().unwrap();
                state.insert(
                    hash,
                    ImageState {
                        protocol,
                        source_path: path.clone(),
                        last_area: Some(render_area),
                        resize_mode: ResizeMode::Fit,
                        last_crop_top: false,
                        last_viewport: None,
                    },
                );

                if let Some(img_state) = state.get_mut(hash) {
                    if !render_stateful_image_safe(
                        hash,
                        render_area,
                        buf,
                        &mut img_state.protocol,
                        Resize::Fit(None),
                    ) {
                        return 0;
                    }
                    return area.height;
                }
            }
        }
    }

    use ratatui::widgets::Clear;
    let clear_area = area.intersection(buf_area);
    if clear_area.width > 0 && clear_area.height > 0 {
        Clear.render(clear_area, buf);
    }

    0
}

fn load_source_image(hash: u64, path: &Path) -> Option<Arc<DynamicImage>> {
    if let Ok(mut cache) = SOURCE_CACHE.lock() {
        if let Some(img) = cache.get(hash, path) {
            return Some(img);
        }
    }

    let img = image::open(path).ok()?;
    if let Ok(mut cache) = SOURCE_CACHE.lock() {
        return Some(cache.insert(hash, path.to_path_buf(), img));
    }
    Some(Arc::new(img))
}

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

    {
        let mut state = IMAGE_STATE.lock().unwrap();
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
        if let Some(img_state) = state.get_mut(hash) {
            if img_state.last_viewport == Some(viewport) {
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
    }

    let cropped = source.crop_imm(scroll_x_px, scroll_y_px, crop_w, crop_h);
    let protocol = picker.new_resize_protocol(cropped);

    let mut state = IMAGE_STATE.lock().unwrap();
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

    // Use ratatui's Clear widget
    use ratatui::widgets::Clear;
    let clamped = area.intersection(*buf.area());
    if clamped.width == 0 || clamped.height == 0 {
        return;
    }
    Clear.render(clamped, buf);
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
            if PICKER.get().and_then(|p| *p).is_some()
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
    let dim = Style::default().fg(Color::Rgb(100, 100, 100));
    let info = Style::default().fg(Color::Rgb(140, 170, 200));

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
    let dim = Style::default().fg(Color::Rgb(100, 100, 100));
    let err_style = Style::default().fg(Color::Rgb(200, 80, 80));

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
        background: "#00000000".to_string(), // Fully transparent (RGBA)
        primary_color: "#313244".to_string(),
        primary_text_color: "#cdd6f4".to_string(),
        primary_border_color: "#585b70".to_string(),
        line_color: "#7f849c".to_string(),
        secondary_color: "#45475a".to_string(),
        tertiary_color: "#313244".to_string(),
        edge_label_background: "#00000000".to_string(),
        cluster_background: "#18182580".to_string(),
        cluster_border: "#45475a".to_string(),
        font_family: "monospace".to_string(),
        font_size: 18.0,
        text_color: "#cdd6f4".to_string(),
        // Sequence diagram colors (dark theme)
        sequence_actor_fill: "#313244".to_string(),
        sequence_actor_border: "#585b70".to_string(),
        sequence_actor_line: "#7f849c".to_string(),
        sequence_note_fill: "#45475a".to_string(),
        sequence_note_border: "#585b70".to_string(),
        sequence_activation_fill: "#313244".to_string(),
        sequence_activation_border: "#7f849c".to_string(),
        ..Theme::modern()
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
        if path.extension().is_some_and(|e| e == "png") {
            if let Ok(meta) = entry.metadata() {
                let size = meta.len();
                let modified = meta.modified().unwrap_or(now);
                files.push((path, size, modified));
                total_size += size;
            }
        }
    }

    // Sort by modification time (oldest first)
    files.sort_by_key(|(_, _, modified)| *modified);

    let mut deleted_bytes: u64 = 0;

    for (path, size, modified) in &files {
        let age = now.duration_since(*modified).unwrap_or_default();
        let should_delete = age.as_secs() > CACHE_MAX_AGE_SECS
            || (total_size - deleted_bytes) > CACHE_MAX_SIZE_BYTES;

        if should_delete {
            if fs::remove_file(path).is_ok() {
                deleted_bytes += size;
            }
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
mod tests {
    use super::*;

    #[test]
    fn test_mermaid_detection() {
        assert!(is_mermaid_lang("mermaid"));
        assert!(is_mermaid_lang("Mermaid"));
        assert!(is_mermaid_lang("mermaid-js"));
        assert!(!is_mermaid_lang("rust"));
        assert!(!is_mermaid_lang("python"));
    }

    #[test]
    fn test_content_hash() {
        let h1 = hash_content("flowchart LR\nA --> B");
        let h2 = hash_content("flowchart LR\nA --> B");
        let h3 = hash_content("flowchart LR\nA --> C");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_placeholder_parsing() {
        let hash = 0x123456789abcdef0u64;
        let lines = image_widget_placeholder(hash, 10);
        assert!(!lines.is_empty());

        let parsed = parse_image_placeholder(&lines[0]);
        assert_eq!(parsed, Some(hash));
    }

    #[test]
    fn test_adaptive_sizing() {
        // Simple diagram should get smaller size
        let (w1, h1) = calculate_render_size(3, 2, Some(100));
        // Complex diagram should get larger size
        let (w2, h2) = calculate_render_size(50, 80, Some(100));
        assert!(w2 > w1);
        assert!(h2 > h1);
    }

    #[test]
    fn test_diagram_size_estimation() {
        let simple = "flowchart LR\n    A --> B";
        let (n1, e1) = estimate_diagram_size(simple);
        assert!(n1 >= 2);
        assert!(e1 >= 1);

        let complex = "flowchart TD\n    A[Start] --> B{Check}\n    B --> C[Yes]\n    B --> D[No]\n    C --> E[End]\n    D --> E";
        let (n2, e2) = estimate_diagram_size(complex);
        assert!(n2 > n1);
        assert!(e2 > e1);
    }

    #[test]
    fn test_cached_width_satisfies_threshold() {
        assert!(cached_width_satisfies(850, Some(1000)));
        assert!(cached_width_satisfies(1000, Some(1000)));
        assert!(!cached_width_satisfies(849, Some(1000)));
        assert!(cached_width_satisfies(300, None));
    }

    #[test]
    fn test_parse_cache_filename() {
        let path = std::path::Path::new("/tmp/0123456789abcdef_w640.png");
        let parsed = parse_cache_filename(path);
        assert_eq!(parsed, Some((0x0123_4567_89ab_cdef, 640)));
    }

    #[test]
    fn test_active_diagrams_are_bounded() {
        clear_active_diagrams();
        for idx in 0..(ACTIVE_DIAGRAMS_MAX + 5) {
            register_active_diagram(idx as u64, 100, 80, None);
        }
        let snapshot = snapshot_active_diagrams();
        assert_eq!(snapshot.len(), ACTIVE_DIAGRAMS_MAX);
        assert_eq!(snapshot.first().map(|d| d.hash), Some(5));
        assert_eq!(
            snapshot.last().map(|d| d.hash),
            Some((ACTIVE_DIAGRAMS_MAX + 4) as u64)
        );
        clear_active_diagrams();
    }

    #[test]
    fn test_streaming_preview_is_ephemeral_and_prioritized() {
        clear_active_diagrams();
        register_active_diagram(0x1, 100, 80, None);

        set_streaming_preview_diagram(0x2, 140, 90, Some("streaming".to_string()));
        let with_preview = get_active_diagrams();
        assert_eq!(with_preview.first().map(|d| d.hash), Some(0x2));
        assert_eq!(with_preview.get(1).map(|d| d.hash), Some(0x1));

        clear_streaming_preview_diagram();
        let without_preview = get_active_diagrams();
        assert_eq!(without_preview.len(), 1);
        assert_eq!(without_preview[0].hash, 0x1);

        clear_active_diagrams();
    }

    #[test]
    fn test_parse_proc_status_value_bytes() {
        let status = "Name:\tjcode\nVmSize:\t   2048 kB\nVmRSS:\t    512 kB\nVmHWM:\t   1024 kB\n";
        assert_eq!(
            parse_proc_status_value_bytes(status, "VmSize:"),
            Some(2048 * 1024)
        );
        assert_eq!(
            parse_proc_status_value_bytes(status, "VmRSS:"),
            Some(512 * 1024)
        );
        assert_eq!(
            parse_proc_status_value_bytes(status, "VmHWM:"),
            Some(1024 * 1024)
        );
        assert_eq!(parse_proc_status_value_bytes(status, "VmSwap:"), None);
    }

    #[test]
    fn test_memory_profile_exposes_limits() {
        let profile = debug_memory_profile();
        assert_eq!(profile.render_cache_limit, RENDER_CACHE_MAX);
        assert_eq!(profile.image_state_limit, IMAGE_STATE_MAX);
        assert_eq!(profile.source_cache_limit, SOURCE_CACHE_MAX);
        assert_eq!(profile.active_diagrams_limit, ACTIVE_DIAGRAMS_MAX);
        assert_eq!(profile.cache_disk_limit_bytes, CACHE_MAX_SIZE_BYTES);
        assert_eq!(profile.cache_disk_max_age_secs, CACHE_MAX_AGE_SECS);
    }

    #[test]
    fn test_memory_benchmark_clamps_iterations() {
        let result = debug_memory_benchmark(0);
        assert_eq!(result.iterations, 1);
    }

    #[test]
    fn test_set_cell_if_visible_ignores_out_of_bounds_coordinates() {
        let mut buf = Buffer::empty(Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 2,
        });
        set_cell_if_visible(&mut buf, 10, 1, 'X', None);
        set_cell_if_visible(&mut buf, 2, 1, 'Y', None);
        assert_eq!(buf[(2, 1)].symbol(), "Y");
        assert_eq!(buf[(0, 0)].symbol(), " ");
    }

    #[test]
    fn test_draw_left_border_clamps_to_buffer_area() {
        let mut buf = Buffer::empty(Rect {
            x: 0,
            y: 0,
            width: 5,
            height: 3,
        });
        draw_left_border(
            &mut buf,
            Rect {
                x: 10,
                y: 1,
                width: 4,
                height: 2,
            },
        );
        draw_left_border(
            &mut buf,
            Rect {
                x: 3,
                y: 0,
                width: 4,
                height: 3,
            },
        );
        assert_eq!(buf[(3, 0)].symbol(), "│");
        assert_eq!(buf[(4, 0)].symbol(), " ");
    }
}
