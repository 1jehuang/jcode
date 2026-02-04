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
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash as _, Hasher};
use std::panic;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, OnceLock};

/// Global picker for terminal capability detection
/// Initialized once on first use
static PICKER: OnceLock<Option<Picker>> = OnceLock::new();

/// Cache for rendered mermaid diagrams
static RENDER_CACHE: LazyLock<Mutex<MermaidCache>> =
    LazyLock::new(|| Mutex::new(MermaidCache::new()));

/// Image state cache - holds StatefulProtocol for each rendered image
static IMAGE_STATE: LazyLock<Mutex<HashMap<u64, StatefulProtocol>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Initialize the global picker by querying terminal capabilities.
/// Should be called early in app startup, after entering alternate screen.
pub fn init_picker() {
    PICKER.get_or_init(|| Picker::from_query_stdio().ok());
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
    /// ASCII fallback (parsing info only)
    Ascii(AsciiDiagram),
    /// Error during rendering
    Error(String),
}

/// ASCII representation of a diagram
pub struct AsciiDiagram {
    pub kind: String,
    pub node_count: usize,
    pub edge_count: usize,
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
    // Calculate content hash for caching
    let hash = hash_content(content);

    // Check cache
    {
        let cache = RENDER_CACHE.lock().unwrap();
        if let Some(cached) = cache.get(hash) {
            if cached.path.exists() {
                return RenderResult::Image {
                    hash,
                    path: cached.path.clone(),
                    width: cached.width,
                    height: cached.height,
                };
            }
        }
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
    if node_count > MAX_NODES || edge_count > MAX_EDGES {
        return RenderResult::Error(format!(
            "Diagram too complex ({} nodes, {} edges). Max: {} nodes, {} edges.",
            node_count, edge_count, MAX_NODES, MAX_EDGES
        ));
    }
    let prev_hook = panic::take_hook();
    panic::set_hook(Box::new(|_| {
        // Silently ignore panics from mermaid renderer
    }));

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
    match render_result {
        Ok(Ok(())) => {} // Success, continue below
        Ok(Err(e)) => return RenderResult::Error(e),
        Err(panic_info) => {
            let msg = if let Some(s) = panic_info.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = panic_info.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic in mermaid renderer".to_string()
            };
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
/// OPTIMIZATION: This function tracks the last rendered position for each image hash.
/// If the image was already rendered at the exact same position, we skip re-rendering
/// to avoid scroll lag. The terminal graphics protocol maintains the image display.
pub fn render_image_widget(hash: u64, area: Rect, buf: &mut Buffer, centered: bool) -> u16 {
    // Get the cached image dimensions to calculate centered area
    let img_width = {
        let cache = RENDER_CACHE.lock().unwrap();
        cache.get(hash).map(|c| c.width).unwrap_or(0)
    };

    // Calculate the actual render area (potentially centered)
    let render_area = if centered && img_width > 0 {
        // Calculate actual rendered width in terminal cells
        let rendered_width = if let Some(Some(picker)) = PICKER.get() {
            let font_size = picker.font_size();
            let img_width_cells = (img_width as f32 / font_size.0 as f32).ceil() as u16;
            // If image is wider than area, it will be scaled to fit
            img_width_cells.min(area.width)
        } else {
            area.width // Fallback: assume full width
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

    // Note: We intentionally do NOT clear the buffer here.
    // ratatui-image handles clearing internally via cell.set_skip(true).
    // Manual clearing causes flicker during scroll because it wipes the
    // Unicode placeholders before ratatui-image can redraw them.

    // Get image dimensions from cache
    let (img_width, img_height) = {
        let cache = RENDER_CACHE.lock().unwrap();
        cache
            .get(hash)
            .map(|c| (c.width, c.height))
            .unwrap_or((0, 0))
    };

    // Calculate image dimensions in terminal cells
    let (img_cols, img_rows) = if let Some(Some(picker)) = PICKER.get() {
        let font_size = picker.font_size();
        let cols = (img_width as f32 / font_size.0 as f32).ceil() as u16;
        let rows = (img_height as f32 / font_size.1 as f32).ceil() as u16;
        (cols, rows)
    } else {
        (render_area.width, render_area.height)
    };

    // Always use Crop to clip - never resize the image
    // This prevents flickering during scroll when the available area changes
    let make_resize = || {
        Resize::Crop(Some(CropOptions {
            clip_top: false,
            clip_left: false,
        }))
    };

    // Try to render from existing state
    let render_result = {
        let mut state = IMAGE_STATE.lock().unwrap();
        if let Some(protocol) = state.get_mut(&hash) {
            let widget = StatefulImage::default().resize(make_resize());
            widget.render(render_area, buf, protocol);
            true
        } else {
            false
        }
    };

    if render_result {
        return area.height;
    }

    // No state available, try to load from cache
    let cached_path = {
        let cache = RENDER_CACHE.lock().unwrap();
        cache.get(hash).map(|c| c.path.clone())
    };

    if let Some(path) = cached_path {
        if let Some(Some(picker)) = PICKER.get() {
            if let Ok(img) = image::open(&path) {
                let protocol = picker.new_resize_protocol(img);

                let mut state = IMAGE_STATE.lock().unwrap();
                state.insert(hash, protocol);

                if let Some(protocol) = state.get_mut(&hash) {
                    let widget = StatefulImage::default().resize(make_resize());
                    widget.render(render_area, buf, protocol);
                    return area.height;
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
        RenderResult::Ascii(diagram) => MermaidContent::Lines(ascii_to_lines(&diagram)),
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

/// Create placeholder lines for an image widget
/// These will be recognized and replaced during rendering
fn image_widget_placeholder(hash: u64, height: u16) -> Vec<Line<'static>> {
    let dim = Style::default().fg(Color::Rgb(40, 40, 40));

    // First line contains the hash as a marker (invisible)
    let mut lines = Vec::with_capacity(height as usize + 1);

    // Header with hash marker
    lines.push(Line::from(Span::styled(
        format!("\x00MERMAID_IMAGE:{:016x}\x00", hash),
        dim,
    )));

    // Fill remaining height with blank lines
    for _ in 1..height {
        lines.push(Line::from(Span::styled(" ", dim)));
    }

    lines
}

/// Check if a line is a mermaid image placeholder and extract the hash
pub fn parse_image_placeholder(line: &Line<'_>) -> Option<u64> {
    if line.spans.is_empty() {
        return None;
    }

    let content = &line.spans[0].content;
    // Prefix "\x00MERMAID_IMAGE:" is 15 bytes, then 16 hex digits, then "\x00"
    if content.starts_with("\x00MERMAID_IMAGE:") && content.ends_with("\x00") {
        let hex = &content[15..31]; // Extract the 16 hex digits (bytes 15-30)
        u64::from_str_radix(hex, 16).ok()
    } else {
        None
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

/// Convert ASCII diagram to ratatui Lines
pub fn ascii_to_lines(diagram: &AsciiDiagram) -> Vec<Line<'static>> {
    let dim = Style::default().fg(Color::Rgb(100, 100, 100));
    let label = Style::default().fg(Color::Rgb(180, 180, 180));
    let info = Style::default().fg(Color::Rgb(140, 140, 140));

    vec![
        Line::from(Span::styled("┌─ mermaid ", dim)),
        Line::from(vec![
            Span::styled("│ ", dim),
            Span::styled(format!("[{}]", diagram.kind), label),
        ]),
        Line::from(vec![
            Span::styled("│ ", dim),
            Span::styled(
                format!("{} nodes, {} edges", diagram.node_count, diagram.edge_count),
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

/// Clean up cached files (call on exit)
pub fn cleanup_cache() {
    if let Ok(cache) = RENDER_CACHE.lock() {
        let _ = fs::remove_dir_all(&cache.cache_dir);
    }
}

/// Clear image state (call when switching sessions or on memory pressure)
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
