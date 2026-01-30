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
    Resize, StatefulImage,
};
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash as _, Hasher};
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

    // Parse mermaid
    let parsed = match parse_mermaid(content) {
        Ok(p) => p,
        Err(e) => return RenderResult::Error(format!("Parse error: {}", e)),
    };

    // Configure theme for terminal (dark background friendly)
    let theme = terminal_theme();
    let layout_config = LayoutConfig::default();

    // Compute layout
    let layout = compute_layout(&parsed.graph, &theme, &layout_config);

    // Render to SVG
    let svg = render_svg(&layout, &theme, &layout_config);

    // Get cache path
    let png_path = {
        let cache = RENDER_CACHE.lock().unwrap();
        cache.cache_path(hash)
    };

    // Convert SVG to PNG
    let render_config = RenderConfig {
        background: theme.background.clone(),
        ..Default::default()
    };

    if let Err(e) = write_output_png(&svg, &png_path, &render_config, &theme) {
        return RenderResult::Error(format!("Render error: {}", e));
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
/// Returns the number of rows used
pub fn render_image_widget(hash: u64, area: Rect, buf: &mut Buffer) -> u16 {
    // First try to render from existing state
    {
        let mut state = IMAGE_STATE.lock().unwrap();
        if let Some(protocol) = state.get_mut(&hash) {
            let widget = StatefulImage::default().resize(Resize::Fit(None));
            widget.render(area, buf, protocol);
            return area.height;
        }
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
                    let widget = StatefulImage::default().resize(Resize::Fit(None));
                    widget.render(area, buf, protocol);
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

    vec![
        Line::from(Span::styled("┌─ mermaid error ", dim)),
        Line::from(vec![
            Span::styled("│ ", dim),
            Span::styled(error.to_string(), err_style),
        ]),
        Line::from(Span::styled("└─", dim)),
    ]
}

/// Terminal-friendly theme (works on dark backgrounds)
fn terminal_theme() -> Theme {
    Theme {
        background: "#1e1e2e".to_string(),
        primary_color: "#313244".to_string(),
        primary_text_color: "#cdd6f4".to_string(),
        primary_border_color: "#585b70".to_string(),
        line_color: "#7f849c".to_string(),
        secondary_color: "#45475a".to_string(),
        tertiary_color: "#313244".to_string(),
        edge_label_background: "#1e1e2e".to_string(),
        cluster_background: "#181825".to_string(),
        cluster_border: "#45475a".to_string(),
        font_family: "monospace".to_string(),
        font_size: 13.0,
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
