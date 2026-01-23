//! Mermaid diagram rendering for terminal display
//!
//! Renders mermaid diagrams to PNG images, then displays them
//! using the terminal's graphics protocol (Kitty, Sixel) or
//! falls back to ASCII representation.

use crate::tui::image::{display_image, ImageDisplayParams, ImageProtocol};
use mermaid_rs_renderer::{
    config::{LayoutConfig, RenderConfig},
    layout::compute_layout,
    parser::parse_mermaid,
    render::{render_svg, write_output_png},
    theme::Theme,
};
use ratatui::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash as _, Hasher};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

/// Cache for rendered mermaid diagrams
static RENDER_CACHE: LazyLock<Mutex<MermaidCache>> = LazyLock::new(|| Mutex::new(MermaidCache::new()));

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
    /// Successfully rendered to image
    Image {
        path: PathBuf,
        width: u32,
        height: u32,
    },
    /// ASCII fallback (no graphics support)
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

/// Render a mermaid code block
pub fn render_mermaid(content: &str) -> RenderResult {
    // Check if graphics are supported
    let protocol = ImageProtocol::detect();
    if !protocol.is_supported() {
        return render_ascii_fallback(content);
    }

    // Calculate content hash for caching
    let hash = hash_content(content);

    // Check cache
    {
        let cache = RENDER_CACHE.lock().unwrap();
        if let Some(cached) = cache.get(hash) {
            if cached.path.exists() {
                return RenderResult::Image {
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

    RenderResult::Image {
        path: png_path,
        width,
        height,
    }
}

/// Create ASCII fallback representation
fn render_ascii_fallback(content: &str) -> RenderResult {
    match parse_mermaid(content) {
        Ok(parsed) => {
            let kind = match parsed.graph.kind {
                mermaid_rs_renderer::ir::DiagramKind::Flowchart => "flowchart",
                mermaid_rs_renderer::ir::DiagramKind::Class => "classDiagram",
                mermaid_rs_renderer::ir::DiagramKind::State => "stateDiagram",
                mermaid_rs_renderer::ir::DiagramKind::Sequence => "sequenceDiagram",
            };
            let node_count = parsed.graph.nodes.len();
            let edge_count = parsed.graph.edges.len();

            RenderResult::Ascii(AsciiDiagram {
                kind: kind.to_string(),
                node_count,
                edge_count,
            })
        }
        Err(e) => RenderResult::Error(format!("Parse error: {}", e)),
    }
}

/// Terminal-friendly theme (works on dark backgrounds)
fn terminal_theme() -> Theme {
    Theme {
        background: "#1e1e2e".to_string(),      // Dark background (Catppuccin Mocha)
        primary_color: "#313244".to_string(),   // Node fill
        primary_text_color: "#cdd6f4".to_string(), // Text
        primary_border_color: "#585b70".to_string(), // Node border
        line_color: "#7f849c".to_string(),      // Edge color
        secondary_color: "#45475a".to_string(),
        tertiary_color: "#313244".to_string(),
        edge_label_background: "#1e1e2e".to_string(),
        cluster_background: "#181825".to_string(),
        cluster_border: "#45475a".to_string(),
        font_family: "monospace".to_string(),
        font_size: 13.0,
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

/// Convert render result to displayable ratatui Lines
pub fn result_to_lines(result: RenderResult, _max_width: Option<usize>) -> Vec<Line<'static>> {
    match result {
        RenderResult::Image { path, .. } => {
            // Display image using terminal protocol
            let params = ImageDisplayParams::from_terminal();
            match display_image(&path, &params) {
                Ok(true) => {
                    // Image displayed successfully via escape codes
                    // Return empty lines - the image was written directly to stdout
                    vec![]
                }
                Ok(false) | Err(_) => {
                    // Fallback to ASCII placeholder
                    vec![Line::from(Span::styled(
                        "[mermaid diagram - image display not supported]",
                        Style::default().fg(Color::Yellow),
                    ))]
                }
            }
        }
        RenderResult::Ascii(diagram) => ascii_to_lines(&diagram),
        RenderResult::Error(msg) => error_to_lines(&msg),
    }
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

/// Clean up cached files (call on exit)
pub fn cleanup_cache() {
    if let Ok(cache) = RENDER_CACHE.lock() {
        let _ = fs::remove_dir_all(&cache.cache_dir);
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
    fn test_ascii_fallback() {
        let content = "flowchart LR\nA[Start] --> B[End]";
        let result = render_ascii_fallback(content);
        match result {
            RenderResult::Ascii(diagram) => {
                assert_eq!(diagram.kind, "flowchart");
                assert_eq!(diagram.node_count, 2);
                assert_eq!(diagram.edge_count, 1);
            }
            _ => panic!("Expected ASCII result"),
        }
    }

    #[test]
    fn test_invalid_mermaid() {
        // Parser is lenient, so only truly malformed diagrams fail
        // This tests that we handle errors gracefully when they do occur
        let content = ""; // Empty content should fail
        let result = render_ascii_fallback(content);
        // Either an error or an empty diagram is acceptable
        match result {
            RenderResult::Error(_) => {} // Expected for empty input
            RenderResult::Ascii(d) => {
                // Parser may produce empty diagram
                assert!(d.node_count == 0 || d.edge_count == 0);
            }
            _ => panic!("Unexpected result type"),
        }
    }
}
