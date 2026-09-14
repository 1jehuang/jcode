//! Gallery / grid layout for live swarm-agent viewports.
//!
//! Unlike [`crate::memory_tiles`], which is a ragged masonry bin-packer optimized
//! for boxes whose heights are fixed by their content (a memory entry is however
//! many lines it is), this module lays out a set of *streaming viewports* whose
//! heights we control. The goals are different:
//!
//! - Cells should be uniform and scannable (gallery view), not artfully packed.
//! - Each cell shows the *tail* of an agent's output, bottom-anchored like a
//!   terminal, filling exactly its height budget.
//! - The total height is a knob: more agents -> shorter viewports each, instead
//!   of overflowing.
//!
//! The placement is a simple grid allocator (column count chosen to keep cells
//! near a target aspect ratio), reusing the box-drawing/width helpers shared
//! with the memory tiles.

use ratatui::prelude::*;
use unicode_width::UnicodeWidthStr;

use crate::memory_tiles::split_by_display_width;

/// One agent's viewport to render in the gallery.
#[derive(Clone, Debug)]
pub struct SwarmTile {
    /// Title line (e.g. agent friendly name).
    pub title: String,
    /// Short status badge text (e.g. "running", "done").
    pub status: String,
    /// Color used for the status badge and accents.
    pub accent: Color,
    /// Optional role/prefix glyph drawn before the title (e.g. "★").
    pub role_glyph: Option<String>,
    /// The agent's recent output, oldest first. The renderer shows the tail.
    pub body: Vec<String>,
}

impl SwarmTile {
    pub fn new(title: impl Into<String>, status: impl Into<String>, accent: Color) -> Self {
        Self {
            title: title.into(),
            status: status.into(),
            accent,
            role_glyph: None,
            body: Vec::new(),
        }
    }

    pub fn with_role_glyph(mut self, glyph: impl Into<String>) -> Self {
        self.role_glyph = Some(glyph.into());
        self
    }

    pub fn with_body(mut self, body: Vec<String>) -> Self {
        self.body = body;
        self
    }
}

/// Tunable parameters for the gallery layout.
#[derive(Clone, Copy, Debug)]
pub struct SwarmGalleryConfig {
    /// Total height budget for the whole gallery (excluding an optional header).
    pub max_height: usize,
    /// Minimum inner width per cell (columns) before we reduce the column count.
    pub min_cell_inner_width: usize,
    /// Minimum cell height (including borders). Cells never shrink below this;
    /// if the budget can't fit all agents at this height, we overflow into a
    /// "+N more" strip.
    pub min_cell_height: usize,
    /// Preferred cell height (including borders) when there is room to spare.
    pub preferred_cell_height: usize,
    /// Horizontal gap between cells.
    pub gap: usize,
    /// Target width:height ratio for a cell, used to pick the column count.
    pub target_aspect: f32,
    /// Index of the currently selected/focused tile within the displayed grid,
    /// if any. The selected tile gets a brighter border and dimmed inactive tiles
    /// for visual differentiation.
    pub selected: Option<usize>,
}

impl Default for SwarmGalleryConfig {
    fn default() -> Self {
        Self {
            max_height: 16,
            min_cell_inner_width: 18,
            min_cell_height: 4,
            preferred_cell_height: 7,
            gap: 2,
            target_aspect: 4.5,
            selected: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GridShape {
    cols: usize,
    rows: usize,
    /// Number of tiles actually shown (rest go into the overflow strip).
    shown: usize,
}

/// Choose how many columns/rows to use for `tile_count` agents in the given
/// budget. Picks the column count whose resulting cell aspect ratio is closest
/// to `target_aspect`, subject to width/height minimums.
fn choose_grid(
    tile_count: usize,
    total_width: usize,
    cfg: &SwarmGalleryConfig,
) -> Option<GridShape> {
    if tile_count == 0 || total_width == 0 || cfg.max_height == 0 {
        return None;
    }

    // Max columns that still give each cell at least the minimum inner width.
    let min_cell_total = cfg.min_cell_inner_width + 2; // borders
    let max_cols_by_width =
        ((total_width + cfg.gap) / (min_cell_total + cfg.gap)).clamp(1, tile_count);
    if max_cols_by_width == 0 {
        return None;
    }

    // Max rows that fit at least the minimum cell height. Guard against a
    // degenerate `min_cell_height` of 0 (division by zero).
    let max_rows_by_height = (cfg.max_height / cfg.min_cell_height.max(1)).max(1);
    let max_visible_cells = (max_cols_by_width * max_rows_by_height).max(1);

    let mut best: Option<(f32, GridShape)> = None;
    for cols in 1..=max_cols_by_width {
        let cell_w = (total_width.saturating_sub((cols - 1) * cfg.gap)) / cols;
        if cell_w < min_cell_total {
            continue;
        }
        // Rows needed for this column count, capped by what fits vertically.
        let rows_needed = tile_count.div_ceil(cols);
        let rows = rows_needed.min(max_rows_by_height).max(1);
        let shown = (cols * rows).min(tile_count).min(max_visible_cells);
        let cell_h = (cfg.max_height.saturating_sub((rows - 1) * cfg.gap)) / rows;
        if cell_h == 0 {
            continue;
        }
        let aspect = cell_w as f32 / cell_h as f32;
        // Penalize empty slots in the last row (orphan tiles look unbalanced).
        let empty_slots = (cols * rows).saturating_sub(shown);
        // Prefer aspect close to target; penalize raggedness; tie-break toward
        // showing more tiles.
        let cost =
            (aspect - cfg.target_aspect).abs() + (empty_slots as f32) * 0.6 - (shown as f32) * 0.01;
        let shape = GridShape { cols, rows, shown };
        match &best {
            Some((best_cost, _)) if *best_cost <= cost => {}
            _ => best = Some((cost, shape)),
        }
    }

    best.map(|(_, shape)| shape)
}

/// Render a single cell box of the given inner dimensions. `inner_w`/`inner_h`
/// are the content area (excluding borders).
fn render_cell(tile: &SwarmTile, inner_w: usize, inner_h: usize, selected: bool) -> Vec<Line<'static>> {
    // --- Color palette based on selection state ---
    let (border_color, title_color, badge_color, text_color) = if selected {
        (
            tile.accent,                                       // bright accent border
            tile.accent,                                       // full brightness title
            tile.accent,                                       // accent badge
            Color::Rgb(170, 172, 180),                         // normal body text
        )
    } else {
        (
            Color::Rgb(80, 80, 92),                            // dim border (default)
            Color::Rgb(130, 130, 145),                         // slightly dim title
            Color::Rgb(100, 100, 115),                         // dim badge
            Color::Rgb(170, 170, 185),                         // slightly dimmed body text
        )
    };
    let border_style = Style::default().fg(border_color);
    let accent_style = Style::default().fg(badge_color);

    // ---- Title bar (drawn into the top border line) ----
    let glyph = tile.role_glyph.as_deref().unwrap_or("");
    let title_prefix = if selected { "▸ " } else { "" };
    let title_raw = if glyph.is_empty() {
        format!("{}{}", title_prefix, tile.title)
    } else {
        format!("{}{} {}", title_prefix, glyph, tile.title)
    };
    let title_style = if selected {
        Style::default().fg(title_color).bold()
    } else {
        Style::default().fg(title_color)
    };
    let box_width = inner_w + 2;

    // Top border, total width == box_width, structured as:
    //   ╭─ <title> <dashes> <badge> ─╮
    // Fixed cost: ╭(1) ─(1) space(1) ... space(1) ─(1) ╮(1) = 6 columns plus
    // the dash-fill segment of at least 1. Badge is included only if it fits.
    let fixed = 6usize; // corners + two leading dashes/space wrappers
    let mut badge = format!("[{}]", tile.status);
    let mut badge_w = UnicodeWidthStr::width(badge.as_str());

    // Reserve room: at least 1 title char, 1 dash filler, and a space before
    // the badge. If the badge can't fit, drop it.
    let min_dashes = 1usize;
    let title_budget = box_width
        .saturating_sub(fixed + min_dashes + badge_w + 1)
        .max(1);
    let mut title_text = truncate_w(&title_raw, title_budget);
    let mut title_w = UnicodeWidthStr::width(title_text.as_str());

    if fixed + title_w + min_dashes + badge_w + 1 > box_width {
        // No room for the badge; drop it and give the title the space.
        badge = String::new();
        badge_w = 0;
        let title_budget = box_width.saturating_sub(fixed + min_dashes).max(1);
        title_text = truncate_w(&title_raw, title_budget);
        title_w = UnicodeWidthStr::width(title_text.as_str());
    }

    // Remaining columns become the dash filler. Subtract: corners(2),
    // leading "─ "(2), title, trailing " ─"(2), and "[badge] " if present.
    let badge_segment = if badge.is_empty() { 0 } else { badge_w + 1 };
    let dashes = box_width
        .saturating_sub(2 + 2 + title_w + 2 + badge_segment)
        .max(1);

    let mut top_spans: Vec<Span<'static>> = vec![
        Span::styled("╭─ ".to_string(), border_style),
        Span::styled(title_text, title_style),
        Span::styled(" ".to_string(), border_style),
        Span::styled("─".repeat(dashes), border_style),
    ];
    if !badge.is_empty() {
        top_spans.push(Span::styled(" ".to_string(), border_style));
        top_spans.push(Span::styled(badge, accent_style));
    }
    top_spans.push(Span::styled("─╮".to_string(), border_style));
    // Final guard against off-by-one from width rounding and against boxes
    // narrower than the fixed title-bar chrome.
    normalize_top_width(&mut top_spans, box_width, border_style);

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(inner_h + 2);
    lines.push(Line::from(top_spans));

    // ---- Body: bottom-anchored tail of the stream ----
    let body_lines = wrap_tail(&tile.body, inner_w, inner_h);
    let blank_top = inner_h.saturating_sub(body_lines.len());
    let text_style = Style::default().fg(text_color);
    for _ in 0..blank_top {
        lines.push(content_line("", inner_w, border_style, text_style));
    }
    for text in body_lines {
        lines.push(content_line(&text, inner_w, border_style, text_style));
    }

    // ---- Bottom border ----
    lines.push(Line::from(Span::styled(
        format!("╰{}╯", "─".repeat(box_width.saturating_sub(2))),
        border_style,
    )));

    lines
}

fn normalize_top_width(spans: &mut Vec<Span<'static>>, box_width: usize, border_style: Style) {
    let cur: usize = spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum();
    if cur < box_width {
        // Pad just before the trailing " ─╮".
        let pad = box_width - cur;
        let insert_at = spans.len().saturating_sub(1);
        spans.insert(insert_at, Span::styled("─".repeat(pad), border_style));
    } else if cur > box_width {
        // The box is too narrow for the titled border (fixed chrome is ~8
        // columns). Fall back to a plain border of exactly `box_width`.
        let fill = "─".repeat(box_width.saturating_sub(2));
        spans.clear();
        spans.push(Span::styled(format!("╭{fill}╮"), border_style));
    }
}

fn content_line(
    text: &str,
    inner_w: usize,
    border_style: Style,
    text_style: Style,
) -> Line<'static> {
    let truncated = truncate_w(text, inner_w);
    let w = UnicodeWidthStr::width(truncated.as_str());
    let pad = inner_w.saturating_sub(w);
    Line::from(vec![
        Span::styled("│".to_string(), border_style),
        Span::styled(truncated, text_style),
        Span::raw(" ".repeat(pad)),
        Span::styled("│".to_string(), border_style),
    ])
}

/// Take the tail of `body`, wrapping each logical line to `width`, and return at
/// most `height` rendered rows (the most recent ones).
fn wrap_tail(body: &[String], width: usize, height: usize) -> Vec<String> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let mut rows: Vec<String> = Vec::new();
    // Wrap from the end until we have enough rows.
    for logical in body.iter().rev() {
        let mut wrapped = if logical.is_empty() {
            vec![String::new()]
        } else {
            split_by_display_width(logical, width)
        };
        // Keep wrapped chunks in natural (top-to-bottom) order, but we are
        // walking logical lines bottom-up, so prepend.
        let mut prefix = std::mem::take(&mut wrapped);
        prefix.extend(rows);
        rows = prefix;
        if rows.len() >= height {
            break;
        }
    }
    if rows.len() > height {
        let start = rows.len() - height;
        rows = rows.split_off(start);
    }
    rows
}

fn truncate_w(s: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let full = UnicodeWidthStr::width(s);
    if full <= max_width {
        return s.to_string();
    }
    let ellipsis = "…";
    let target = max_width.saturating_sub(1);
    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > target {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push_str(ellipsis);
    out
}

/// Render a single tile filling exactly `width` x `height` (including borders).
/// Used by the list+detail swarm panel to draw the focused agent's viewport.
/// `focused` controls whether the tile border uses the bright accent color
/// (focused) or the dim default border (unfocused). Returns an empty vec when
/// the area is too small to draw a bordered box.
pub fn render_single_tile(tile: &SwarmTile, width: usize, height: usize, focused: bool) -> Vec<Line<'static>> {
    if width < 4 || height < 3 {
        return Vec::new();
    }
    let inner_w = width - 2;
    let inner_h = height - 2;
    let mut lines = render_cell(tile, inner_w, inner_h, focused);
    // render_cell already yields `height` lines, but pad/truncate defensively so
    // callers can rely on the exact height.
    while lines.len() < height {
        lines.push(Line::from(Span::raw(" ".repeat(width))));
    }
    lines.truncate(height);
    lines
}

/// Render the swarm gallery. Returns lines ready to draw, fitting within
/// `total_width` columns and roughly `cfg.max_height` rows (plus header).
pub fn render_swarm_gallery(
    tiles: &[SwarmTile],
    total_width: usize,
    cfg: &SwarmGalleryConfig,
    header: Option<Line<'static>>,
) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    if let Some(header) = header {
        out.push(header);
    }
    if tiles.is_empty() {
        return out;
    }

    let Some(shape) = choose_grid(tiles.len(), total_width, cfg) else {
        return out;
    };

    let cols = shape.cols;
    let rows = shape.rows;
    let shown = shape.shown;
    let cell_total_w = (total_width.saturating_sub((cols - 1) * cfg.gap)) / cols;
    let cell_inner_w = cell_total_w.saturating_sub(2);

    // Distribute the height budget across rows; cap at preferred height.
    // Guard against inverted config (preferred < min) which would panic clamp.
    let max_cell_h = cfg.preferred_cell_height.max(cfg.min_cell_height);
    let cell_total_h = (cfg.max_height / rows.max(1)).clamp(cfg.min_cell_height, max_cell_h);
    let cell_inner_h = cell_total_h.saturating_sub(2);

    // Render each shown tile into a grid of line buffers.
    let visible = &tiles[..shown];
    for row in 0..rows {
        let row_start = row * cols;
        if row_start >= shown {
            break;
        }
        let row_end = (row_start + cols).min(shown);
        let row_tiles = &visible[row_start..row_end];

        // Render each cell in this row.
        let cell_blocks: Vec<Vec<Line<'static>>> = row_tiles
            .iter()
            .enumerate()
            .map(|(ci, tile)| {
                let tile_idx = row * cols + ci;
                let is_selected = cfg.selected == Some(tile_idx);
                render_cell(tile, cell_inner_w, cell_inner_h, is_selected)
            })
            .collect();

        for line_idx in 0..cell_total_h {
            let mut spans: Vec<Span<'static>> = Vec::new();
            for (ci, block) in cell_blocks.iter().enumerate() {
                if ci > 0 {
                    spans.push(Span::raw(" ".repeat(cfg.gap)));
                }
                if let Some(line) = block.get(line_idx) {
                    spans.extend(line.spans.clone());
                } else {
                    spans.push(Span::raw(" ".repeat(cell_total_w)));
                }
            }
            out.push(Line::from(spans));
        }
    }

    let hidden = tiles.len().saturating_sub(shown);
    if hidden > 0 {
        out.push(Line::from(Span::styled(
            format!(
                "  +{} more agent{}",
                hidden,
                if hidden == 1 { "" } else { "s" }
            ),
            Style::default().fg(Color::Rgb(140, 140, 150)),
        )));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(name: &str, body: &[&str]) -> SwarmTile {
        SwarmTile::new(name, "running", Color::Rgb(255, 200, 100))
            .with_body(body.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn single_agent_uses_one_column() {
        let tiles = vec![tile("alpha", &["hello", "world"])];
        let cfg = SwarmGalleryConfig::default();
        let lines = render_swarm_gallery(&tiles, 60, &cfg, None);
        assert!(!lines.is_empty());
        // Top border present.
        let top = plain(&lines[0]);
        assert!(top.starts_with("╭─ alpha"), "got: {top}");
        assert!(top.contains("[running]"), "got: {top}");
    }

    #[test]
    fn many_agents_form_multiple_columns() {
        let tiles: Vec<SwarmTile> = (0..4).map(|i| tile(&format!("a{i}"), &["x"])).collect();
        let shape = choose_grid(4, 100, &SwarmGalleryConfig::default()).unwrap();
        assert!(shape.cols >= 2, "expected multi-column, got {shape:?}");
        let lines = render_swarm_gallery(&tiles, 100, &SwarmGalleryConfig::default(), None);
        assert!(!lines.is_empty());
    }

    #[test]
    fn tail_shows_most_recent_lines() {
        let body: Vec<String> = (0..20).map(|i| format!("line{i}")).collect();
        let rows = wrap_tail(&body, 20, 3);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2], "line19");
        assert_eq!(rows[0], "line17");
    }

    #[test]
    fn overflow_emits_more_strip() {
        let tiles: Vec<SwarmTile> = (0..50).map(|i| tile(&format!("a{i}"), &["x"])).collect();
        let cfg = SwarmGalleryConfig {
            max_height: 8,
            ..Default::default()
        };
        let lines = render_swarm_gallery(&tiles, 60, &cfg, None);
        let last = plain(lines.last().unwrap());
        assert!(last.contains("more agent"), "got: {last}");
    }

    #[test]
    fn cells_are_width_bounded() {
        let tiles: Vec<SwarmTile> = (0..3)
            .map(|i| {
                tile(
                    &format!("a{i}"),
                    &["a very long line that should be truncated nicely"],
                )
            })
            .collect();
        let cfg = SwarmGalleryConfig::default();
        let lines = render_swarm_gallery(&tiles, 80, &cfg, None);
        for line in &lines {
            assert!(line.width() <= 80, "line too wide: {}", plain(line));
        }
    }

    #[test]
    fn empty_tiles_render_nothing_but_header() {
        let cfg = SwarmGalleryConfig::default();
        assert!(render_swarm_gallery(&[], 80, &cfg, None).is_empty());
        let lines = render_swarm_gallery(&[], 80, &cfg, Some(Line::from("hdr")));
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn zero_sized_budgets_do_not_panic() {
        let tiles = vec![tile("a", &["x"])];
        for (w, h) in [(0usize, 0usize), (0, 16), (60, 0), (1, 1), (3, 3)] {
            let cfg = SwarmGalleryConfig {
                max_height: h,
                ..Default::default()
            };
            let _ = render_swarm_gallery(&tiles, w, &cfg, None);
        }
    }

    #[test]
    fn degenerate_config_does_not_panic() {
        let tiles = vec![tile("a", &["x"])];
        // min_cell_height = 0 previously divided by zero in choose_grid.
        let cfg = SwarmGalleryConfig {
            min_cell_height: 0,
            ..Default::default()
        };
        let _ = render_swarm_gallery(&tiles, 60, &cfg, None);
        // preferred < min previously panicked in clamp().
        let cfg = SwarmGalleryConfig {
            min_cell_height: 4,
            preferred_cell_height: 3,
            ..Default::default()
        };
        let _ = render_swarm_gallery(&tiles, 60, &cfg, None);
    }

    #[test]
    fn hundreds_of_tiles_small_terminal_stay_bounded() {
        let tiles: Vec<SwarmTile> = (0..300)
            .map(|i| tile(&format!("agent-{i}"), &["node gate-7 running", "ok"]))
            .collect();
        for width in [1usize, 5, 10, 20, 21, 40, 80] {
            for mh in [1usize, 2, 4, 8, 16] {
                let cfg = SwarmGalleryConfig {
                    max_height: mh,
                    ..Default::default()
                };
                let lines = render_swarm_gallery(&tiles, width, &cfg, None);
                for line in &lines {
                    assert!(
                        line.width() <= width.max(1),
                        "width={width} mh={mh}: line {} wider than {width}: {:?}",
                        line.width(),
                        plain(line)
                    );
                }
            }
        }
    }

    #[test]
    fn single_tile_tiny_sizes_stay_bounded() {
        let t = SwarmTile::new("日本語のタイトル", "実行中", Color::Cyan).with_body(vec![
            "こんにちは世界こんにちは世界".to_string(),
            "🐝🐝🐝🐝🐝🐝".to_string(),
            "e\u{301}e\u{301} combining".to_string(),
        ]);
        for w in 0..24usize {
            for h in 0..8usize {
                let lines = render_single_tile(&t, w, h, true);
                if w < 4 || h < 3 {
                    assert!(lines.is_empty(), "expected empty at {w}x{h}");
                    continue;
                }
                assert_eq!(lines.len(), h, "height mismatch at {w}x{h}");
                for line in &lines {
                    assert!(
                        line.width() <= w,
                        "{w}x{h}: line {} too wide: {:?}",
                        line.width(),
                        plain(line)
                    );
                }
            }
        }
    }

    #[test]
    fn wrap_tail_handles_zero_and_empty() {
        assert!(wrap_tail(&[], 10, 3).is_empty());
        assert!(wrap_tail(&["x".to_string()], 0, 3).is_empty());
        assert!(wrap_tail(&["x".to_string()], 10, 0).is_empty());
    }

    #[test]
    fn truncate_w_handles_wide_chars() {
        assert_eq!(truncate_w("", 5), "");
        assert_eq!(truncate_w("abc", 0), "");
        // 2-column chars never split mid-glyph and result stays within budget.
        for max in 1..8usize {
            let out = truncate_w("日本語テスト", max);
            assert!(
                UnicodeWidthStr::width(out.as_str()) <= max,
                "max={max} out={out}"
            );
        }
    }

    fn plain(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }
}
