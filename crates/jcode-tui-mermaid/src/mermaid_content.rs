use super::*;

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
pub(super) fn image_widget_placeholder(hash: u64, height: u16) -> Vec<Line<'static>> {
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

/// Create a markdown/text marker line that side-panel rendering recognizes as an
/// inline image placeholder for an already-registered image hash.
pub fn image_widget_placeholder_markdown(hash: u64) -> String {
    format!(
        "{}{:016x}{}\n",
        MERMAID_MARKER_PREFIX, hash, MERMAID_MARKER_SUFFIX
    )
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
pub fn terminal_theme() -> Theme {
    Theme {
        // Catppuccin-inspired pastel dark theme tuned for jcode's terminal UI.
        // Uses transparent canvas so the rendered PNG integrates with the TUI,
        // while keeping nodes/labels readable against dark panes.
        background: "#00000000".to_string(),
        font_family: "Inter, ui-sans-serif, system-ui, -apple-system, Segoe UI, \"DejaVu Sans\", \"Liberation Sans\", sans-serif"
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
