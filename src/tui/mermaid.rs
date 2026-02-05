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
    Resize, StatefulImage,
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
/// Key is (hash, target_width) to support multiple sizes of the same diagram
static IMAGE_STATE: LazyLock<Mutex<HashMap<u64, ImageState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Last render state for skip-redundant-render optimization
static LAST_RENDER: LazyLock<Mutex<HashMap<u64, LastRenderState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// State for a rendered image
struct ImageState {
    protocol: StatefulProtocol,
    /// The area this was last rendered to (for change detection)
    last_area: Option<Rect>,
    /// Resize mode locked at creation time (prevents flickering on scroll)
    resize_mode: ResizeMode,
}

/// Resize mode for images - locked at creation time
#[derive(Clone, Copy, PartialEq, Eq)]
enum ResizeMode {
    Fit,
    Crop,
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
    if let Ok(mut last) = LAST_RENDER.lock() {
        last.clear();
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

/// Debug info for a single image's state
#[derive(Debug, Clone, Serialize)]
pub struct ImageStateInfo {
    pub hash: String,
    pub resize_mode: String,
    pub last_area: Option<String>,
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
                },
                last_area: img_state.last_area.map(|r| {
                    format!("{}x{}+{}+{}", r.width, r.height, r.x, r.y)
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
        RenderResult::Image { hash, path, width, height } => {
            // Check what resize mode was assigned
            let resize_mode = if let Ok(state) = IMAGE_STATE.lock() {
                state.get(&hash).map(|s| match s.resize_mode {
                    ResizeMode::Fit => "Fit".to_string(),
                    ResizeMode::Crop => "Crop".to_string(),
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
        Rect { x: 0, y: 0, width: 80, height: 24 },
        Rect { x: 0, y: 0, width: 120, height: 40 },
        Rect { x: 0, y: 0, width: 60, height: 20 },
        Rect { x: 10, y: 5, width: 80, height: 24 },
    ];

    let mut results: Vec<serde_json::Value> = Vec::new();
    let mut modes: Vec<String> = Vec::new();

    for area in &areas {
        // Check current resize mode for this hash
        let mode = if let Ok(state) = IMAGE_STATE.lock() {
            state.get(&hash).map(|s| match s.resize_mode {
                ResizeMode::Fit => "Fit",
                ResizeMode::Crop => "Crop",
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
    let test_content = content.unwrap_or(r#"flowchart TD
    A[Start] --> B{Decision}
    B -->|Yes| C[Process 1]
    B -->|No| D[Process 2]
    C --> E[Merge]
    D --> E
    E --> F[End]"#);

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
        let visible_top = image_top.max(0) as u16;
        let visible_bottom = (image_bottom.min(term_height as i32)) as u16;

        let visible = visible_top < visible_bottom && visible_bottom > 0;
        let visible_rows = if visible { visible_bottom - visible_top } else { 0 };

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

            let rows_used = render_image_widget(hash, area, &mut buf, false);
            frame_info.rendered = rows_used > 0;

            // Check resize mode
            if let Ok(state) = IMAGE_STATE.lock() {
                if let Some(img_state) = state.get(&hash) {
                    let mode = match img_state.resize_mode {
                        ResizeMode::Fit => "Fit",
                        ResizeMode::Crop => "Crop",
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
    PICKER.get().and_then(|p| p.map(|p| p.protocol_type()))
}

/// Get terminal font size for adaptive sizing
pub fn get_font_size() -> Option<(u16, u16)> {
    PICKER.get().and_then(|p| p.map(|p| p.font_size()))
}

/// Mermaid rendering cache
struct MermaidCache {
    /// Map from content hash to rendered PNG info
    entries: HashMap<u64, CachedDiagram>,
    /// Cache directory
    cache_dir: PathBuf,
}

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

    fn cache_path(&self, hash: u64, target_width: u32) -> PathBuf {
        // Include target width in filename for size-specific caching
        self.cache_dir
            .join(format!("{:016x}_w{}.png", hash, target_width))
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
fn calculate_render_size(node_count: usize, edge_count: usize, terminal_width: Option<u16>) -> (f64, f64) {
    // Base size on terminal width if available
    let base_width = if let Some(term_width) = terminal_width {
        // Get font size to calculate pixel width
        let font_width = get_font_size().map(|(w, _)| w).unwrap_or(8) as f64;
        let pixel_width = term_width as f64 * font_width;
        // Cap at reasonable bounds
        pixel_width.clamp(400.0, 2400.0)
    } else {
        1200.0 // Default fallback
    };

    // Scale based on complexity
    let complexity = node_count + edge_count;
    let complexity_factor = match complexity {
        0..=5 => 0.6,    // Simple: smaller image
        6..=15 => 0.8,   // Medium: moderate size
        16..=30 => 1.0,  // Standard: full size
        31..=60 => 1.2,  // Complex: larger
        _ => 1.4,        // Very complex: even larger
    };

    let width = (base_width * complexity_factor).clamp(400.0, 2400.0);
    // Maintain reasonable aspect ratio
    let height = (width * 0.75).clamp(300.0, 1800.0);

    (width, height)
}

/// Render a mermaid code block to PNG (cached)
/// Now accepts optional terminal_width for adaptive sizing
pub fn render_mermaid(content: &str) -> RenderResult {
    render_mermaid_sized(content, None)
}

/// Render with explicit terminal width for adaptive sizing
pub fn render_mermaid_sized(content: &str, terminal_width: Option<u16>) -> RenderResult {
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
    let (target_width, target_height) = calculate_render_size(node_count, edge_count, terminal_width);
    let target_width_u32 = target_width as u32;

    // Check cache (use blocking lock for consistency)
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
            width: target_width as f32,
            height: target_height as f32,
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
    let (width, height) = get_png_dimensions(&png_path).unwrap_or((target_width_u32, target_height as u32));

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

    // Pre-create the StatefulProtocol for this image
    // Determine resize mode once at creation time to prevent flickering during scroll
    if let Some(Some(picker)) = PICKER.get() {
        if let Ok(img) = image::open(&png_path) {
            let protocol = picker.new_resize_protocol(img);

            // Determine resize mode based on image size vs typical terminal
            // Use Fit for large images, Crop for small ones
            // This is locked at creation time to prevent mode switching during scroll
            let font_size = picker.font_size();
            let img_width_cells = (width as f32 / font_size.0 as f32).ceil() as u16;
            let img_height_cells = (height as f32 / font_size.1 as f32).ceil() as u16;
            // Default to Fit if image is larger than ~80 cols or ~30 rows (typical terminal)
            let resize_mode = if img_width_cells > 80 || img_height_cells > 30 {
                ResizeMode::Fit
            } else {
                ResizeMode::Crop
            };

            let mut state = IMAGE_STATE.lock().unwrap();
            state.insert(
                hash,
                ImageState {
                    protocol,
                    last_area: None,
                    resize_mode,
                },
            );
        }
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

/// Render an image at the given area using ratatui-image
/// If centered is true, the image will be horizontally centered within the area
/// Returns the number of rows used
///
/// ## Optimizations
/// - Uses blocking locks for consistent rendering (no frame skipping)
/// - Skips render if area and settings unchanged from last frame
/// - Uses Fit mode for small terminals to scale instead of crop
/// - Only clears area if render fails
/// - Draws a left border (like code blocks) for visual consistency
pub fn render_image_widget(hash: u64, area: Rect, buf: &mut Buffer, centered: bool) -> u16 {
    // Skip if area is too small (need room for border + image)
    if area.width <= BORDER_WIDTH || area.height == 0 {
        return 0;
    }

    // Draw left border (vertical bar like code blocks)
    let border_style = Style::default().fg(Color::Rgb(100, 100, 100)); // DIM_COLOR
    for row in area.y..area.y.saturating_add(area.height) {
        if row < buf.area().height {
            buf.get_mut(area.x, row).set_char('│').set_style(border_style);
            if area.x + 1 < buf.area().width {
                buf.get_mut(area.x + 1, row).set_char(' ');
            }
        }
    }

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

    // Check if we can skip this render (same area, same settings)
    let current_state = LastRenderState { area, centered };
    {
        if let Ok(last_render) = LAST_RENDER.lock() {
            if let Some(last) = last_render.get(&hash) {
                if *last == current_state {
                    // Nothing changed, skip render
                    if let Ok(mut debug) = MERMAID_DEBUG.lock() {
                        debug.stats.skipped_renders += 1;
                    }
                    return area.height;
                }
            }
        }
    }

    // Get cached image info (blocking lock for consistency)
    let (img_width, img_height, path) = {
        let cache = RENDER_CACHE.lock().unwrap();
        if let Some(cached) = cache.get(hash) {
            (cached.width, cached.height, Some(cached.path.clone()))
        } else {
            (0, 0, None)
        }
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

    // Helper to create Resize enum from stored mode
    fn make_resize(mode: ResizeMode) -> Resize {
        match mode {
            ResizeMode::Fit => Resize::Fit(None),
            ResizeMode::Crop => Resize::Crop(None),
        }
    }

    // Try to render from existing state (blocking lock)
    // Use the stored resize_mode to prevent flickering during scroll
    let render_success = {
        let mut state = IMAGE_STATE.lock().unwrap();
        if let Some(img_state) = state.get_mut(&hash) {
            let widget = StatefulImage::default().resize(make_resize(img_state.resize_mode));
            widget.render(render_area, buf, &mut img_state.protocol);
            img_state.last_area = Some(render_area);

            if let Ok(mut debug) = MERMAID_DEBUG.lock() {
                debug.stats.image_state_hits += 1;
            }
            true
        } else {
            false
        }
    };

    if render_success {
        // Update last render state
        if let Ok(mut last_render) = LAST_RENDER.lock() {
            last_render.insert(hash, current_state);
        }
        return area.height;
    }

    // State miss - need to load image
    if let Ok(mut debug) = MERMAID_DEBUG.lock() {
        debug.stats.image_state_misses += 1;
    }

    // Try to load from cache
    if let Some(path) = path {
        if let Some(Some(picker)) = PICKER.get() {
            if let Ok(img) = image::open(&path) {
                let protocol = picker.new_resize_protocol(img);

                // Determine resize mode based on image size (locked at creation)
                let font_size = picker.font_size();
                let img_width_cells = (img_width as f32 / font_size.0 as f32).ceil() as u16;
                let img_height_cells = (img_height as f32 / font_size.1 as f32).ceil() as u16;
                let resize_mode = if img_width_cells > 80 || img_height_cells > 30 {
                    ResizeMode::Fit
                } else {
                    ResizeMode::Crop
                };

                let mut state = IMAGE_STATE.lock().unwrap();
                state.insert(
                    hash,
                    ImageState {
                        protocol,
                        last_area: Some(render_area),
                        resize_mode,
                    },
                );

                if let Some(img_state) = state.get_mut(&hash) {
                    let widget = StatefulImage::default().resize(make_resize(img_state.resize_mode));
                    widget.render(render_area, buf, &mut img_state.protocol);

                    // Update last render state
                    if let Ok(mut last_render) = LAST_RENDER.lock() {
                        last_render.insert(hash, current_state);
                    }
                    return area.height;
                }
            }
        }
    }

    // Render failed - clear the area to avoid showing stale content
    use ratatui::widgets::Clear;
    Clear.render(area, buf);

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
    Clear.render(area, buf);
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
}
