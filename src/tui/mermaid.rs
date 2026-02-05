//! Mermaid diagram rendering for terminal display
//!
//! Renders mermaid diagrams to PNG images, then displays them using
//! ratatui-image which supports Kitty, Sixel, iTerm2, and halfblock protocols.
//! The protocol is auto-detected based on terminal capabilities.

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
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash as _, Hasher};
use std::panic;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::Instant;

/// Global picker for terminal capability detection
/// Initialized once on first use
static PICKER: OnceLock<Option<Picker>> = OnceLock::new();

/// Track whether cache eviction has run
static CACHE_EVICTED: OnceLock<()> = OnceLock::new();

/// Cache for rendered mermaid diagrams
static RENDER_CACHE: LazyLock<Mutex<MermaidCache>> =
    LazyLock::new(|| Mutex::new(MermaidCache::new()));

/// Image state cache - holds StatefulProtocol for each rendered image
static IMAGE_STATE: LazyLock<Mutex<HashMap<u64, StatefulProtocol>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

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
    pub last_image_render_ms: Option<f32>,
    pub cache_entries: usize,
    pub cache_dir: Option<String>,
    pub protocol: Option<String>,
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

pub fn clear_cache() -> Result<(), String> {
    let cache_dir = if let Ok(cache) = RENDER_CACHE.lock() {
        cache.cache_dir.clone()
    } else {
        PathBuf::from("/tmp")
    };

    // Clear in-memory caches
    if let Ok(mut cache) = RENDER_CACHE.lock() {
        cache.entries.clear();
    }
    if let Ok(mut state) = IMAGE_STATE.lock() {
        state.clear();
    }

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
    PICKER.get().and_then(|p| p.map(|p| p.protocol_type()))
}

/// Mermaid rendering cache
struct MermaidCache {
    /// Map from content hash to rendered PNG path
    entries: HashMap<u64, CachedDiagram>,
    /// Cache directory
    cache_dir: PathBuf,
}

struct CachedDiagram {
    path: PathBuf,
    width: u32,
    height: u32,
}

impl MermaidCache {
    fn new() -> Self {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("jcode")
            .join("mermaid");

        // Create cache dir if needed
        let _ = fs::create_dir_all(&cache_dir);

        Self {
            entries: HashMap::new(),
            cache_dir,
        }
    }

    fn get(&self, hash: u64) -> Option<&CachedDiagram> {
        self.entries.get(&hash)
    }

    fn insert(&mut self, hash: u64, diagram: CachedDiagram) {
        self.entries.insert(hash, diagram);
    }

    fn cache_path(&self, hash: u64) -> PathBuf {
        self.cache_dir.join(format!("{:016x}.png", hash))
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
        if trimmed.contains("-->") || trimmed.contains("-.->") || trimmed.contains("==>") {
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

    (nodes, edges)
}

/// Render a mermaid code block to PNG (cached)
pub fn render_mermaid(content: &str) -> RenderResult {
    if let Ok(mut state) = MERMAID_DEBUG.lock() {
        state.stats.total_requests += 1;
        state.stats.last_content_len = Some(content.len());
        state.stats.last_error = None;
    }

    // Calculate content hash for caching
    let hash = hash_content(content);

    // Check cache
    {
        let cache = RENDER_CACHE.lock().unwrap();
        if let Some(cached) = cache.get(hash) {
            if cached.path.exists() {
                if let Ok(mut state) = MERMAID_DEBUG.lock() {
                    state.stats.cache_hits += 1;
                    state.stats.last_hash = Some(format!("{:016x}", hash));
                }
                return RenderResult::Image {
                    hash,
                    path: cached.path.clone(),
                    width: cached.width,
                    height: cached.height,
                };
            }
        }
    }
    if let Ok(mut state) = MERMAID_DEBUG.lock() {
        state.stats.cache_misses += 1;
        state.stats.last_hash = Some(format!("{:016x}", hash));
    }

    // Get cache path early (needed outside catch_unwind)
    let png_path = {
        let cache = RENDER_CACHE.lock().unwrap();
        cache.cache_path(hash)
    };
    let png_path_clone = png_path.clone();

    // Wrap mermaid library calls in catch_unwind for defense-in-depth
    // This protects against any panics in the external library
    // We temporarily install a no-op panic hook to suppress the default output
    let content_owned = content.to_string();

    // Check diagram size before attempting expensive layout
    // This prevents OOM on complex diagrams (e.g., full system architecture)
    let (node_count, edge_count) = estimate_diagram_size(&content_owned);
    if let Ok(mut state) = MERMAID_DEBUG.lock() {
        state.stats.last_nodes = Some(node_count);
        state.stats.last_edges = Some(edge_count);
    }
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

        // Use larger spacing for better readability in terminal
        let layout_config = LayoutConfig {
            node_spacing: 80.0,   // Default is 50
            rank_spacing: 80.0,   // Default is 50
            node_padding_x: 40.0, // Default is 30
            node_padding_y: 20.0, // Default is 15
            ..Default::default()
        };

        // Compute layout
        let layout = compute_layout(&parsed.graph, &theme, &layout_config);

        // Render to SVG
        let svg = render_svg(&layout, &theme, &layout_config);

        // Convert SVG to PNG with larger dimensions for readability
        let render_config = RenderConfig {
            width: 1600.0,  // Larger than default 1200
            height: 1200.0, // Larger than default 800
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
        } // Success, continue below
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

    // Get dimensions
    let (width, height) = get_png_dimensions(&png_path).unwrap_or((400, 300));

    // Cache the result
    {
        let mut cache = RENDER_CACHE.lock().unwrap();
        cache.insert(
            hash,
            CachedDiagram {
                path: png_path.clone(),
                width,
                height,
            },
        );
    }

    // Pre-create the StatefulProtocol for this image
    if let Some(Some(picker)) = PICKER.get() {
        if let Ok(img) = image::open(&png_path) {
            let protocol = picker.new_resize_protocol(img);
            let mut state = IMAGE_STATE.lock().unwrap();
            state.insert(hash, protocol);
        }
    }

    RenderResult::Image {
        hash,
        path: png_path,
        width,
        height,
    }
}

/// Render an image at the given area using ratatui-image
/// If centered is true, the image will be horizontally centered within the area
/// Returns the number of rows used
///
/// OPTIMIZATION: Uses try_lock for non-critical operations to reduce scroll lag.
/// The terminal graphics protocol maintains the image display between frames.
pub fn render_image_widget(hash: u64, area: Rect, buf: &mut Buffer, centered: bool) -> u16 {
    // Skip if area is too small
    if area.width == 0 || area.height == 0 {
        return 0;
    }

    // Get the cached image dimensions to calculate centered area
    // Use try_lock to avoid blocking during scroll
    let img_width = RENDER_CACHE
        .try_lock()
        .ok()
        .and_then(|cache| cache.get(hash).map(|c| c.width))
        .unwrap_or(0);

    // Calculate the actual render area (potentially centered)
    let render_area = if centered && img_width > 0 {
        // Calculate actual rendered width in terminal cells
        let rendered_width = if let Some(Some(picker)) = PICKER.get() {
            let font_size = picker.font_size();
            let img_width_cells = (img_width as f32 / font_size.0 as f32).ceil() as u16;
            img_width_cells.min(area.width)
        } else {
            area.width
        };

        // Center horizontally
        let x_offset = (area.width.saturating_sub(rendered_width)) / 2;
        Rect {
            x: area.x + x_offset,
            y: area.y,
            width: rendered_width,
            height: area.height,
        }
    } else {
        area
    };

    // Use Crop to clip the image when partially visible
    // clip_top/clip_left control which edge to clip from
    let make_resize = || {
        Resize::Crop(Some(CropOptions {
            clip_top: true,  // Allow clipping from top for partial visibility
            clip_left: true, // Allow clipping from left
        }))
    };

    // Try to render from existing state (fast path)
    let render_result = if let Ok(mut state) = IMAGE_STATE.try_lock() {
        if let Some(protocol) = state.get_mut(&hash) {
            let widget = StatefulImage::default().resize(make_resize());
            widget.render(render_area, buf, protocol);
            true
        } else {
            false
        }
    } else {
        // Couldn't acquire lock, skip this frame
        return area.height;
    };

    if render_result {
        // Update debug stats non-blocking
        if let Ok(mut debug) = MERMAID_DEBUG.try_lock() {
            debug.stats.image_state_hits += 1;
        }
        return area.height;
    }

    // Update debug stats non-blocking
    if let Ok(mut debug) = MERMAID_DEBUG.try_lock() {
        debug.stats.image_state_misses += 1;
    }

    // No state available, try to load from cache (slow path - blocking is OK)
    let cached_path = RENDER_CACHE
        .try_lock()
        .ok()
        .and_then(|cache| cache.get(hash).map(|c| c.path.clone()));

    if let Some(path) = cached_path {
        if let Some(Some(picker)) = PICKER.get() {
            if let Ok(img) = image::open(&path) {
                let protocol = picker.new_resize_protocol(img);

                if let Ok(mut state) = IMAGE_STATE.try_lock() {
                    state.insert(hash, protocol);

                    if let Some(protocol) = state.get_mut(&hash) {
                        let widget = StatefulImage::default().resize(make_resize());
                        widget.render(render_area, buf, protocol);
                        return area.height;
                    }
                }
            }
        }
    }

    0
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
            // Check if we have picker/protocol support
            if PICKER.get().and_then(|p| *p).is_some() {
                let max_w = max_width.map(|w| w as u16).unwrap_or(80);
                let estimated_height = estimate_image_height(width, height, max_w);
                MermaidContent::Image {
                    hash,
                    estimated_height,
                }
            } else {
                // No image protocol support, fall back to placeholder
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
    // Use black foreground on default background to make marker invisible
    // The marker will be completely overwritten by the image rendering
    let invisible = Style::default().fg(Color::Black).bg(Color::Reset);

    let mut lines = Vec::with_capacity(height as usize);

    // First line contains the hash as a marker (invisible, zero-width prefix)
    // We use null characters which won't render but can be detected
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

/// Convert error to ratatui Lines
pub fn error_to_lines(error: &str) -> Vec<Line<'static>> {
    let dim = Style::default().fg(Color::Rgb(100, 100, 100));
    let err_style = Style::default().fg(Color::Rgb(200, 80, 80));

    // Calculate box width based on content
    let header = "mermaid error";
    let content_width = error.len().max(header.len());
    let top_padding = content_width.saturating_sub(header.len());
    let bottom_width = content_width + 1; // +1 for the space after │

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
        edge_label_background: "#00000000".to_string(), // Transparent edge labels
        cluster_background: "#18182580".to_string(),    // Semi-transparent cluster bg
        cluster_border: "#45475a".to_string(),
        font_family: "monospace".to_string(),
        font_size: 18.0, // Larger font for terminal readability (default was 13)
        text_color: "#cdd6f4".to_string(),
        // Sequence diagram colors (dark theme)
        sequence_actor_fill: "#313244".to_string(),
        sequence_actor_border: "#585b70".to_string(),
        sequence_actor_line: "#7f849c".to_string(),
        sequence_note_fill: "#45475a".to_string(),
        sequence_note_border: "#585b70".to_string(),
        sequence_activation_fill: "#313244".to_string(),
        sequence_activation_border: "#7f849c".to_string(),
        // Use defaults from modern theme for git/pie chart fields
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
fn get_png_dimensions(path: &PathBuf) -> Option<(u32, u32)> {
    let data = fs::read(path).ok()?;
    if data.len() > 24 && &data[0..8] == b"\x89PNG\r\n\x1a\n" {
        let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
        let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
        return Some((width, height));
    }
    None
}

/// Maximum age for cached files (7 days)
const CACHE_MAX_AGE_SECS: u64 = 7 * 24 * 60 * 60;

/// Maximum total cache size (100 MB)
const CACHE_MAX_SIZE_BYTES: u64 = 100 * 1024 * 1024;

/// Evict old cache files on startup.
/// Removes files older than CACHE_MAX_AGE_SECS and enforces CACHE_MAX_SIZE_BYTES limit.
/// Called automatically during init_picker().
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

    // Collect file info
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

    let mut deleted_count = 0;
    let mut deleted_bytes: u64 = 0;

    for (path, size, modified) in &files {
        let age = now.duration_since(*modified).unwrap_or_default();
        let should_delete = age.as_secs() > CACHE_MAX_AGE_SECS
            || (total_size - deleted_bytes) > CACHE_MAX_SIZE_BYTES;

        if should_delete {
            if fs::remove_file(path).is_ok() {
                deleted_count += 1;
                deleted_bytes += size;
            }
        }
    }

    // Silently evict - logging not needed for routine cache maintenance
    let _ = (deleted_count, deleted_bytes);
}

/// Clear image state (call on app exit to free memory)
pub fn clear_image_state() {
    if let Ok(mut state) = IMAGE_STATE.lock() {
        state.clear();
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
}
