use chrono::{DateTime, Utc};
use ratatui::prelude::*;

#[derive(Clone)]
pub(super) struct MemoryTilePlan {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) score: usize,
}

pub(super) struct MemoryTile {
    category: String,
    items: Vec<MemoryTileItem>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MemoryTileItem {
    pub(super) content: String,
    pub(super) updated_at: Option<DateTime<Utc>>,
}

impl From<String> for MemoryTileItem {
    fn from(content: String) -> Self {
        Self {
            content,
            updated_at: None,
        }
    }
}

impl From<&str> for MemoryTileItem {
    fn from(content: &str) -> Self {
        Self::from(content.to_string())
    }
}

pub(super) fn parse_memory_display_entries(content: &str) -> Vec<(String, MemoryTileItem)> {
    let mut entries: Vec<(String, MemoryTileItem)> = Vec::new();
    let mut current_category = String::new();
    let mut last_entry_idx: Option<usize> = None;

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.starts_with("# ") || line.is_empty() {
            continue;
        }
        if let Some(category) = line.strip_prefix("## ") {
            current_category = category.trim().to_string();
            continue;
        }
        if let Some(updated_at_raw) = line
            .strip_prefix("<!-- updated_at: ")
            .and_then(|value| value.strip_suffix(" -->"))
        {
            if let (Some(idx), Ok(updated_at)) = (
                last_entry_idx,
                DateTime::parse_from_rfc3339(updated_at_raw.trim()),
            ) {
                entries[idx].1.updated_at = Some(updated_at.with_timezone(&Utc));
            }
            continue;
        }

        let content = if let Some(dot_pos) = line.find(". ") {
            let prefix = &line[..dot_pos];
            if prefix.trim().chars().all(|c| c.is_ascii_digit()) {
                line[dot_pos + 2..].trim()
            } else {
                line
            }
        } else {
            line
        };
        if content.is_empty() {
            continue;
        }

        let category = if current_category.is_empty() {
            "memory".to_string()
        } else {
            current_category.clone()
        };
        entries.push((
            category,
            MemoryTileItem {
                content: content.to_string(),
                updated_at: None,
            },
        ));
        last_entry_idx = Some(entries.len() - 1);
    }

    entries
}

pub(super) fn group_into_tiles<T>(entries: Vec<(String, T)>) -> Vec<MemoryTile>
where
    T: Into<MemoryTileItem>,
{
    let mut order: Vec<String> = Vec::new();
    let mut map: std::collections::HashMap<String, Vec<MemoryTileItem>> =
        std::collections::HashMap::new();
    for (cat, content) in entries {
        if !map.contains_key(&cat) {
            order.push(cat.clone());
        }
        map.entry(cat).or_default().push(content.into());
    }
    order
        .into_iter()
        .filter_map(|cat| {
            map.remove(&cat).map(|items| MemoryTile {
                category: cat,
                items,
            })
        })
        .collect()
}

/// Split a string into chunks that each fit within `max_width` display columns,
/// respecting multi-column characters (CJK characters take 2 columns, etc.).
pub(super) fn split_by_display_width(s: &str, max_width: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthChar;
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for ch in s.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if current_width + cw > max_width && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
            current_width = 0;
        }
        current.push(ch);
        current_width += cw;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    if chunks.is_empty() {
        chunks.push(String::new());
    }
    chunks
}

fn truncate_to_display_width(s: &str, max_width: usize) -> String {
    use unicode_width::UnicodeWidthChar;

    if max_width == 0 {
        return String::new();
    }

    let full_width = unicode_width::UnicodeWidthStr::width(s);
    if full_width <= max_width {
        return s.to_string();
    }

    let ellipsis = "…";
    let ellipsis_width = unicode_width::UnicodeWidthStr::width(ellipsis);
    if ellipsis_width >= max_width {
        return ellipsis.to_string();
    }

    let target_width = max_width - ellipsis_width;
    let mut truncated = String::new();
    let mut width = 0usize;
    for ch in s.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_width > target_width {
            break;
        }
        truncated.push(ch);
        width += ch_width;
    }
    truncated.push('…');
    truncated
}

fn format_memory_updated_age(updated_at: DateTime<Utc>) -> String {
    let age = Utc::now().signed_duration_since(updated_at);
    if age.num_seconds() < 2 {
        "updated now".to_string()
    } else if age.num_minutes() < 1 {
        format!("updated {}s ago", age.num_seconds().max(1))
    } else if age.num_hours() < 1 {
        format!("updated {}m ago", age.num_minutes())
    } else if age.num_days() < 1 {
        format!("updated {}h ago", age.num_hours())
    } else if age.num_days() < 7 {
        format!("updated {}d ago", age.num_days())
    } else if age.num_days() < 30 {
        format!("updated {}w ago", (age.num_days() / 7).max(1))
    } else {
        format!("updated {}mo ago", (age.num_days() / 30).max(1))
    }
}

fn memory_age_text_tint(updated_at: Option<DateTime<Utc>>) -> Color {
    let Some(updated_at) = updated_at else {
        return Color::Rgb(140, 144, 152);
    };
    let age = Utc::now().signed_duration_since(updated_at);
    if age.num_hours() < 1 {
        Color::Rgb(146, 156, 149)
    } else if age.num_days() < 1 {
        Color::Rgb(142, 148, 156)
    } else if age.num_days() < 7 {
        Color::Rgb(145, 144, 154)
    } else if age.num_days() < 30 {
        Color::Rgb(150, 143, 147)
    } else {
        Color::Rgb(154, 144, 144)
    }
}

fn memory_tile_content_lines(
    items: &[MemoryTileItem],
    inner_width: usize,
    border_style: Style,
    text_style: Style,
) -> Vec<Line<'static>> {
    let bullet = "· ";
    let bullet_width = unicode_width::UnicodeWidthStr::width(bullet);
    let item_width = inner_width.saturating_sub(bullet_width);

    let mut content_lines: Vec<Line<'static>> = Vec::new();
    for item in items {
        let text_fill_style = text_style.fg(memory_age_text_tint(item.updated_at));
        let meta_fill_style = Style::default().fg(Color::Rgb(160, 165, 172));
        let text_display_width = unicode_width::UnicodeWidthStr::width(item.content.as_str());
        if text_display_width <= item_width {
            let text = item.content.to_string();
            let padding = inner_width.saturating_sub(bullet_width + text_display_width);
            let mut spans = vec![
                Span::styled("│ ", border_style),
                Span::styled(bullet.to_string(), text_fill_style),
                Span::styled(text, text_fill_style),
            ];
            if padding > 0 {
                spans.push(Span::raw(" ".repeat(padding)));
            }
            spans.push(Span::styled(" │", border_style));
            content_lines.push(Line::from(spans));
        } else {
            let indent = bullet_width;
            let cont_width = inner_width.saturating_sub(indent);
            let first_chunk_width = item_width;
            let mut all_chunks: Vec<String> = Vec::new();
            let first_chunks = split_by_display_width(&item.content, first_chunk_width);
            if let Some(first) = first_chunks.first() {
                all_chunks.push(first.clone());
                let remainder: String = item.content.chars().skip(first.chars().count()).collect();
                if !remainder.is_empty() {
                    all_chunks.extend(split_by_display_width(&remainder, cont_width));
                }
            }
            for (ci, chunk) in all_chunks.iter().enumerate() {
                let chunk_width = unicode_width::UnicodeWidthStr::width(chunk.as_str());
                if ci == 0 {
                    let padding = inner_width.saturating_sub(bullet_width + chunk_width);
                    let mut spans = vec![
                        Span::styled("│ ", border_style),
                        Span::styled(bullet.to_string(), text_fill_style),
                        Span::styled(chunk.clone(), text_fill_style),
                    ];
                    if padding > 0 {
                        spans.push(Span::raw(" ".repeat(padding)));
                    }
                    spans.push(Span::styled(" │", border_style));
                    content_lines.push(Line::from(spans));
                } else {
                    let padding = inner_width.saturating_sub(indent + chunk_width);
                    let mut spans = vec![
                        Span::styled("│ ", border_style),
                        Span::raw(" ".repeat(indent)),
                        Span::styled(chunk.clone(), text_fill_style),
                    ];
                    if padding > 0 {
                        spans.push(Span::raw(" ".repeat(padding)));
                    }
                    spans.push(Span::styled(" │", border_style));
                    content_lines.push(Line::from(spans));
                }
            }
        }

        if let Some(updated_at) = item.updated_at {
            let meta = format_memory_updated_age(updated_at);
            let indent = bullet_width;
            let meta_width = inner_width.saturating_sub(indent).max(1);
            for chunk in split_by_display_width(&meta, meta_width) {
                let chunk_width = unicode_width::UnicodeWidthStr::width(chunk.as_str());
                let padding = inner_width.saturating_sub(indent + chunk_width);
                content_lines.push(Line::from(vec![
                    Span::styled("│ ", border_style),
                    Span::raw(" ".repeat(indent)),
                    Span::styled(chunk, meta_fill_style),
                    Span::raw(" ".repeat(padding)),
                    Span::styled(" │", border_style),
                ]));
            }
        }
    }

    if content_lines.is_empty() {
        content_lines.push(Line::from(vec![
            Span::styled("│ ", border_style),
            Span::raw(" ".repeat(inner_width)),
            Span::styled(" │", border_style),
        ]));
    }

    content_lines
}

fn render_memory_tile_box(
    tile: &MemoryTile,
    box_width: usize,
    border_style: Style,
    text_style: Style,
) -> Vec<Line<'static>> {
    let inner_width = box_width.saturating_sub(4);
    if inner_width < 4 {
        return Vec::new();
    }

    let title_max_width = box_width.saturating_sub(4);
    let title_label = truncate_to_display_width(&tile.category.to_lowercase(), title_max_width);
    let title_text = format!(" {} ", title_label);
    let title_len = unicode_width::UnicodeWidthStr::width(title_text.as_str());
    let border_chars = box_width.saturating_sub(title_len + 2);
    let left_border = "─".repeat(border_chars / 2);
    let right_border = "─".repeat(border_chars - border_chars / 2);

    let top = Line::from(Span::styled(
        format!("╭{}{}{}╮", left_border, title_text, right_border),
        border_style,
    ));
    let content_lines =
        memory_tile_content_lines(&tile.items, inner_width, border_style, text_style);
    let bottom = Line::from(Span::styled(
        format!("╰{}╯", "─".repeat(box_width.saturating_sub(2))),
        border_style,
    ));

    let mut lines = Vec::with_capacity(content_lines.len() + 2);
    lines.push(top);
    lines.extend(content_lines);
    lines.push(bottom);
    lines
}

pub(super) fn plan_memory_tile(
    tile: &MemoryTile,
    box_width: usize,
    border_style: Style,
    text_style: Style,
) -> Option<MemoryTilePlan> {
    let lines = render_memory_tile_box(tile, box_width, border_style, text_style);
    if lines.is_empty() {
        return None;
    }
    let width = lines.first().map(Line::width).unwrap_or(box_width);
    let height = lines.len();
    let score = tile.items.len() * 10
        + tile
            .items
            .iter()
            .map(|item| unicode_width::UnicodeWidthStr::width(item.content.as_str()).min(80))
            .sum::<usize>();
    Some(MemoryTilePlan {
        lines,
        width,
        height,
        score,
    })
}

pub(super) fn choose_memory_tile_span(
    tile: &MemoryTile,
    column_width: usize,
    gap: usize,
    max_span: usize,
    border_style: Style,
    text_style: Style,
) -> Option<(MemoryTilePlan, usize)> {
    let single = plan_memory_tile(tile, column_width, border_style, text_style)?;
    let mut best_plan = single.clone();
    let mut best_span = 1usize;

    for span in 2..=max_span.max(1) {
        let width = column_width * span + gap * span.saturating_sub(1);
        let Some(plan) = plan_memory_tile(tile, width, border_style, text_style) else {
            continue;
        };

        let single_area = single.width * single.height;
        let span_area = plan.width * plan.height;
        let height_gain = single.height.saturating_sub(plan.height);
        let area_gain = single_area.saturating_sub(span_area);

        if height_gain >= 2 || (height_gain >= 1 && area_gain > column_width) {
            best_plan = plan;
            best_span = span;
            break;
        }
    }

    Some((best_plan, best_span))
}

pub(super) fn render_memory_tiles(
    tiles: &[MemoryTile],
    total_width: usize,
    border_style: Style,
    text_style: Style,
    header_line: Option<Line<'static>>,
) -> Vec<Line<'static>> {
    if tiles.is_empty() {
        return Vec::new();
    }

    let mut all_lines: Vec<Line<'static>> = Vec::new();

    if let Some(header) = header_line {
        all_lines.push(header);
    }

    let min_box_inner = 16usize;
    let min_box_width = min_box_inner + 4;
    let gap = 2usize;
    let row_gap = 0usize;
    let usable_width = total_width.max(min_box_width);

    #[derive(Clone)]
    struct Placement {
        x: usize,
        y: usize,
        plan: MemoryTilePlan,
    }

    #[derive(Clone)]
    struct PlannedTile {
        span: usize,
        plan: MemoryTilePlan,
    }

    let max_cols = ((usable_width + gap) / (min_box_width + gap)).clamp(1, 4);
    let mut best_layout: Option<(Vec<Placement>, usize, usize)> = None;

    for column_count in 1..=max_cols {
        let column_width = (usable_width.saturating_sub((column_count - 1) * gap)) / column_count;
        if column_width < min_box_width {
            continue;
        }

        let max_span = if column_count >= 2 { 2 } else { 1 };
        let mut planned: Vec<PlannedTile> = tiles
            .iter()
            .filter_map(|tile| {
                let (plan, span) = choose_memory_tile_span(
                    tile,
                    column_width,
                    gap,
                    max_span,
                    border_style,
                    text_style,
                )?;
                Some(PlannedTile { span, plan })
            })
            .collect();

        if planned.is_empty() {
            continue;
        }

        planned.sort_by(|a, b| {
            b.plan
                .score
                .cmp(&a.plan.score)
                .then_with(|| b.span.cmp(&a.span))
                .then_with(|| b.plan.height.cmp(&a.plan.height))
                .then_with(|| b.plan.width.cmp(&a.plan.width))
        });

        let mut column_heights = vec![0usize; column_count];
        let mut placements: Vec<Placement> = Vec::with_capacity(planned.len());

        for planned_tile in planned {
            let mut best_start = 0usize;
            let mut best_y = usize::MAX;

            for start_col in 0..=column_count.saturating_sub(planned_tile.span) {
                let y = column_heights[start_col..start_col + planned_tile.span]
                    .iter()
                    .copied()
                    .max()
                    .unwrap_or(0);

                if y < best_y || (y == best_y && start_col < best_start) {
                    best_start = start_col;
                    best_y = y;
                }
            }

            let x = best_start * (column_width + gap);
            let next_height = best_y + planned_tile.plan.height + row_gap;
            for height in &mut column_heights[best_start..best_start + planned_tile.span] {
                *height = next_height;
            }

            placements.push(Placement {
                x,
                y: best_y,
                plan: planned_tile.plan,
            });
        }

        let total_height = column_heights
            .iter()
            .copied()
            .max()
            .unwrap_or(0)
            .saturating_sub(row_gap);
        let imbalance = column_heights.iter().copied().max().unwrap_or(0)
            - column_heights.iter().copied().min().unwrap_or(0);
        let used_width = column_count * column_width + gap * column_count.saturating_sub(1);
        let leftover_width = usable_width.saturating_sub(used_width);

        // Vertical centering: if this column arrangement has imbalanced columns,
        // center shorter columns' tiles vertically within the available space.
        let max_col_height = *column_heights.iter().max().unwrap_or(&0);
        for (col_idx, col_height) in column_heights.iter().enumerate() {
            if *col_height < max_col_height {
                let extra = max_col_height - col_height;
                let offset = extra / 2;
                if offset > 0 {
                    for placed in placements.iter_mut() {
                        let start_col = placed.x / (column_width + gap);
                        if start_col == col_idx {
                            placed.y += offset;
                        }
                    }
                }
            }
        }

        let layout_score = total_height * 100 + imbalance * 3 + leftover_width;

        match &best_layout {
            Some((_, _, best_score)) if *best_score <= layout_score => {}
            _ => best_layout = Some((placements, total_height, layout_score)),
        }
    }

    let Some((mut placements, total_height, _)) = best_layout else {
        return all_lines;
    };

    placements.sort_by(|a, b| a.x.cmp(&b.x).then_with(|| a.y.cmp(&b.y)));

    for y in 0..total_height {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut cursor = 0usize;
        let mut row_has_content = false;
        for placed in placements
            .iter()
            .filter(|placed| y >= placed.y && y < placed.y + placed.plan.height)
        {
            if placed.x > cursor {
                spans.push(Span::raw(" ".repeat(placed.x - cursor)));
            }
            spans.extend(placed.plan.lines[y - placed.y].spans.clone());
            cursor = placed.x + placed.plan.width;
            row_has_content = true;
        }
        if row_has_content {
            if cursor < usable_width {
                spans.push(Span::raw(" ".repeat(usable_width - cursor)));
            }
            all_lines.push(Line::from(spans));
        }
    }

    all_lines
}
