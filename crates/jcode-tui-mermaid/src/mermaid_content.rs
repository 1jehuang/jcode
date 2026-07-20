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

/// Minimum placeholder height for an inline-fit image or diagram.
pub const INLINE_FIT_MIN_ROWS: u16 = 3;

/// Row cap for mermaid diagrams embedded inline in the transcript. Generous so
/// diagrams keep near-natural (readable) size, but bounded below Kitty's
/// virtual-placement row limit (296 diacritic slots) so stable-fit rendering
/// keeps working.
pub const INLINE_DIAGRAM_MAX_ROWS: u16 = 200;

/// Explanation shown immediately next to images rendered through the Unicode
/// half-block fallback. Keep this centralized so raster images, Mermaid, and
/// LaTeX use the same wording.
pub const TERMINAL_IMAGE_FALLBACK_NOTE: &str =
    "Your terminal cannot render inline images; showing a text fallback.";

pub fn text_image_fallback_note_line() -> Line<'static> {
    let style = Style::default()
        .fg(rgb(140, 170, 200))
        .add_modifier(Modifier::DIM | Modifier::ITALIC);
    Line::from(vec![
        Span::styled("  ⓘ ", Style::default().fg(rgb(100, 100, 100))),
        Span::styled(TERMINAL_IMAGE_FALLBACK_NOTE, style),
    ])
}

/// Rows assumed consumed by non-transcript chrome (status line, input box,
/// hint line, spacing) when converting the chat column height into a
/// readable-height budget for the inline aspect goal.
const INLINE_ASPECT_CHROME_ROWS: u16 = 4;

/// Coarse quantization step for the inline transcript aspect goal. Per-mille
/// bucketing alone would mint a new render (and a new on-disk cache entry)
/// for nearly every one-cell terminal resize; snapping the goal to 0.25-wide
/// steps keeps resize jitter from re-rendering diagrams constantly.
const INLINE_ASPECT_QUANT_STEP: f32 = 0.25;

/// Never request an aspect goal taller (narrower) than the 4:3 sizing
/// default: `calculate_render_size` only adjusts height from width, so a
/// portrait goal in a narrow terminal would just produce a taller render
/// than today's default behavior.
const INLINE_ASPECT_MIN: f32 = 4.0 / 3.0;

/// Cap on how flat an inline aspect goal can get in very wide, short
/// terminals; beyond this the layout goal stops improving readability.
const INLINE_ASPECT_MAX: f32 = 6.0;

/// Best-effort aspect-ratio goal (width / height) for mermaid diagrams
/// rendered inline in the chat transcript, so a wide terminal asks the
/// renderer for a landscape layout that fits a visible-viewport height
/// budget instead of the tall 4:3 default.
///
/// The goal is optional and advisory: the renderer fits the diagram's
/// natural aspect toward it but may not hit it. Returns `None` when font or
/// terminal geometry is unknown (no picker), preserving today's behavior.
pub fn inline_transcript_aspect_goal_with_font(
    chat_width: u16,
    chat_height: u16,
    font_size: Option<(u16, u16)>,
) -> Option<f32> {
    let (cell_w, cell_h) = font_size?;
    if chat_width == 0 || chat_height == 0 {
        return None;
    }
    // Mirror inline_fit_geometry: the border bar + padding take 2 cells.
    let avail_cells = chat_width.saturating_sub(2).max(1);
    let width_px = avail_cells as f32 * cell_w.max(1) as f32;
    let goal_rows = chat_height
        .saturating_sub(INLINE_ASPECT_CHROME_ROWS)
        .clamp(INLINE_FIT_MIN_ROWS, INLINE_DIAGRAM_MAX_ROWS);
    let height_px = goal_rows as f32 * cell_h.max(1) as f32;
    let raw = (width_px / height_px).clamp(INLINE_ASPECT_MIN, INLINE_ASPECT_MAX);
    let quantized = (raw / INLINE_ASPECT_QUANT_STEP).round() * INLINE_ASPECT_QUANT_STEP;
    Some(quantized.clamp(INLINE_ASPECT_MIN, INLINE_ASPECT_MAX))
}

/// [`inline_transcript_aspect_goal_with_font`] using the global picker's font
/// size. Without a picker (no image protocol) inline diagrams render as text
/// placeholders anyway, so no goal is produced.
pub fn inline_transcript_aspect_goal(chat_width: u16, chat_height: u16) -> Option<f32> {
    inline_transcript_aspect_goal_with_font(chat_width, chat_height, get_font_size())
}

/// Aspect profile for transcript renders: an explicit pinned-pane aspect
/// always wins (inline and pane then share one cached PNG); otherwise fall
/// back to the terminal-friendly inline goal.
pub fn transcript_preferred_aspect_ratio_with_font(
    pinned_pane_aspect: Option<f32>,
    chat_width: u16,
    chat_height: u16,
    font_size: Option<(u16, u16)>,
) -> Option<f32> {
    pinned_pane_aspect
        .or_else(|| inline_transcript_aspect_goal_with_font(chat_width, chat_height, font_size))
}

/// [`transcript_preferred_aspect_ratio_with_font`] using the global picker's
/// font size.
pub fn transcript_preferred_aspect_ratio(
    pinned_pane_aspect: Option<f32>,
    chat_width: u16,
    chat_height: u16,
) -> Option<f32> {
    transcript_preferred_aspect_ratio_with_font(
        pinned_pane_aspect,
        chat_width,
        chat_height,
        get_font_size(),
    )
}

/// Compute `(rows, cols)` for an image/diagram scaled to fit `chat_width`
/// cells wide (including the 2-cell left border) and at most `cap_rows` tall,
/// preserving aspect ratio. This is the single source of placeholder geometry
/// for the inline-fit pipeline: prepare-time placeholders and the draw-time
/// scale use the same math so borders and labels hug the rendered pixels.
pub fn inline_fit_geometry(width: u32, height: u32, chat_width: u16, cap_rows: u16) -> (u16, u16) {
    if width == 0 || height == 0 {
        return (INLINE_FIT_MIN_ROWS, chat_width.min(2));
    }
    let (cell_w, cell_h) = get_font_size().unwrap_or((8, 16));
    let cell_w = cell_w.max(1) as u32;
    let cell_h = cell_h.max(1) as u32;

    // Available width in pixels (border bar + padding take 2 cells, matching
    // the renderer's BORDER_WIDTH).
    let avail_cells = chat_width.saturating_sub(2).max(1) as u32;
    let avail_px = avail_cells * cell_w;

    let cap_rows_u32 = (cap_rows as u32).max(INLINE_FIT_MIN_ROWS as u32);
    let cap_px = cap_rows_u32 * cell_h;

    // Scale to fit *both* the width and the row cap, preserving aspect ratio,
    // exactly like the draw-time fit does.
    let scale_num_w = avail_px.min(width);
    let scaled_h_by_w = height.saturating_mul(scale_num_w) / width.max(1);
    let (final_w_px, final_h_px) = if scaled_h_by_w <= cap_px {
        (scale_num_w, scaled_h_by_w)
    } else {
        // Height-bound: shrink further so the height fits the cap.
        let w = width.saturating_mul(cap_px) / height.max(1);
        (w.min(avail_px).max(1), cap_px)
    };

    let rows = final_h_px
        .max(1)
        .div_ceil(cell_h)
        .max(INLINE_FIT_MIN_ROWS as u32) as u16;
    let cols = (final_w_px.max(1).div_ceil(cell_w) as u16)
        .saturating_add(2)
        .min(chat_width);
    (
        rows.min(cap_rows_u32.min(u16::MAX as u32) as u16)
            .max(INLINE_FIT_MIN_ROWS),
        cols,
    )
}

/// Convert render result to lines. Diagrams emit the same inline-fit
/// placeholder raster images use, so they share the fit/border/stable-scroll
/// draw pipeline. Video export keeps the legacy crop marker, whose draw path
/// writes the printable region markers the SVG exporter scans for.
pub fn result_to_lines(result: RenderResult, max_width: Option<usize>) -> Vec<Line<'static>> {
    result_to_lines_with_capabilities(
        result,
        max_width,
        VIDEO_EXPORT_MODE.load(Ordering::Relaxed),
        PICKER.get().and_then(|p| p.as_ref()).is_some(),
        uses_text_image_fallback(),
    )
}

fn result_to_lines_with_capabilities(
    result: RenderResult,
    max_width: Option<usize>,
    video_export_mode: bool,
    image_renderer_available: bool,
    uses_text_fallback: bool,
) -> Vec<Line<'static>> {
    match result {
        RenderResult::Image {
            hash,
            width,
            height,
            ..
        } => {
            if video_export_mode {
                let max_w = max_width.map(|w| w as u16).unwrap_or(80);
                let estimated_height = estimate_image_height(width, height, max_w);
                return image_widget_placeholder(hash, estimated_height);
            }
            if !image_renderer_available {
                return image_placeholder_lines(width, height);
            }
            let chat_width = max_width.map(|w| w as u16).unwrap_or(80);
            let (rows, cols) =
                inline_fit_geometry(width, height, chat_width, INLINE_DIAGRAM_MAX_ROWS);
            let mut lines = inline_image_placeholder_lines(hash, rows, cols);
            if uses_text_fallback {
                lines.insert(0, text_image_fallback_note_line());
            }
            lines
        }
        RenderResult::Error(msg) => error_to_lines(&msg),
    }
}

#[cfg(test)]
mod fallback_note_tests {
    use super::*;

    #[test]
    fn fallback_note_explains_why_the_image_is_text() {
        let text = text_image_fallback_note_line()
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(text.contains("terminal cannot render inline images"));
        assert!(text.contains("text fallback"));
    }

    fn image_result() -> RenderResult {
        RenderResult::Image {
            hash: 0x1234,
            path: PathBuf::from("test.png"),
            width: 640,
            height: 480,
        }
    }

    #[test]
    fn halfblock_result_attaches_note_before_image_placeholder() {
        let lines = result_to_lines_with_capabilities(image_result(), Some(80), false, true, true);
        let note = lines[0]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(note.contains(TERMINAL_IMAGE_FALLBACK_NOTE));
        assert!(parse_inline_image_placeholder(&lines[1]).is_some());
    }

    #[test]
    fn native_protocol_result_starts_with_image_placeholder_without_note() {
        let lines = result_to_lines_with_capabilities(image_result(), Some(80), false, true, false);
        assert!(parse_inline_image_placeholder(&lines[0]).is_some());
        assert!(lines.iter().all(|line| {
            !line
                .spans
                .iter()
                .any(|span| span.content.contains(TERMINAL_IMAGE_FALLBACK_NOTE))
        }));
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

/// First non-blank span of a line: centering/padding passes may insert a
/// leading whitespace span before a marker, so marker parsing skips
/// whitespace-only spans instead of assuming the marker sits in `spans[0]`.
fn first_content_span<'a>(line: &'a Line<'_>) -> Option<&'a str> {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .find(|content| !content.trim().is_empty())
}

/// Check if a line is a mermaid image placeholder and extract the hash
pub fn parse_image_placeholder(line: &Line<'_>) -> Option<u64> {
    let content = first_content_span(line)?;
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

/// Marker prefix for inline raster images anchored in the transcript flow.
/// Unlike mermaid placeholders, the marker encodes the placeholder geometry
/// (`rows`/`cols`) so the scan step can build an exact scale-to-fit region.
/// Kept short so the marker line (prefix + 16 hex + 2×(1+4) + suffix = 33
/// cells) survives wrapping at the same narrow widths the mermaid marker does.
const INLINE_IMAGE_MARKER_PREFIX: &str = "\x00IIMG:";

/// Create placeholder lines for an inline raster image embedded in the
/// transcript body: a marker line encoding `(hash, rows, cols)` followed by
/// `rows - 1` blank lines that the draw step paints the image over.
pub fn inline_image_placeholder_lines(hash: u64, rows: u16, cols: u16) -> Vec<Line<'static>> {
    let invisible = Style::default().fg(Color::Black).bg(Color::Black);
    let rows = rows.max(1);
    let mut lines = Vec::with_capacity(rows as usize);
    lines.push(Line::from(Span::styled(
        format!(
            "{}{:016x}:{:04x}:{:04x}{}",
            INLINE_IMAGE_MARKER_PREFIX, hash, rows, cols, MERMAID_MARKER_SUFFIX
        ),
        invisible,
    )));
    for _ in 1..rows {
        lines.push(Line::from(""));
    }
    lines
}

/// Check if a line is an inline raster image placeholder and extract
/// `(hash, rows, cols)`.
pub fn parse_inline_image_placeholder(line: &Line<'_>) -> Option<(u64, u16, u16)> {
    let content = first_content_span(line)?;
    let rest = content.strip_prefix(INLINE_IMAGE_MARKER_PREFIX)?;
    let rest = rest.strip_suffix(MERMAID_MARKER_SUFFIX)?;
    let mut parts = rest.split(':');
    let hash = u64::from_str_radix(parts.next()?, 16).ok()?;
    let rows = u16::from_str_radix(parts.next()?, 16).ok()?;
    let cols = u16::from_str_radix(parts.next()?, 16).ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((hash, rows, cols))
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
#[cfg(feature = "renderer")]
pub fn terminal_theme() -> Theme {
    Theme {
        // Catppuccin-inspired pastel dark theme tuned for jcode's terminal UI.
        // Uses transparent canvas so the rendered PNG integrates with the TUI,
        // while keeping nodes/labels readable against dark panes.
        background: "#00000000".to_string(),
        // Include common Linux-native sans families (DejaVu/Liberation/Noto) so
        // label glyphs still resolve a face when Inter/Segoe UI are not installed
        // (the typical Linux case). resvg matches the first family in this list
        // that exists in the loaded font DB.
        font_family: "Inter, ui-sans-serif, system-ui, -apple-system, Segoe UI, \
                      \"Noto Sans\", \"DejaVu Sans\", \"Liberation Sans\", sans-serif"
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
