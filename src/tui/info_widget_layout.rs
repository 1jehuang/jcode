use super::info_widget::{
    calculate_widget_height, is_overview_mergeable, InfoWidgetData, Side, WidgetKind,
    WidgetPlacement,
};
use ratatui::layout::Rect;
use std::collections::HashSet;

/// Minimum width needed to show the widget.
const MIN_WIDGET_WIDTH: u16 = 24;
/// Maximum width the widget can take.
const MAX_WIDGET_WIDTH: u16 = 40;
/// Minimum height needed to show the widget.
const MIN_WIDGET_HEIGHT: u16 = 5;
/// How much width shrinkage to tolerate before forcing a widget to reposition.
const STICKY_WIDTH_TOLERANCE: u16 = 4;

/// Margin information for layout calculation.
#[derive(Debug, Clone)]
pub struct Margins {
    /// Free widths on the right side for each row.
    pub right_widths: Vec<u16>,
    /// Free widths on the left side for each row (only populated in centered mode).
    pub left_widths: Vec<u16>,
    /// Whether we're in centered mode.
    pub centered: bool,
}

/// Available margin space on one side.
#[derive(Debug, Clone)]
struct MarginSpace {
    side: Side,
    /// Free width for each row (index = row from top of messages area).
    widths: Vec<u16>,
    /// X offset where this margin starts.
    x_offset: u16,
}

/// Compute widget placements while keeping the caller-owned widget state stable.
pub(crate) fn calculate_placements(
    messages_area: Rect,
    margins: &Margins,
    data: &InfoWidgetData,
    enabled: bool,
    prev_placements: &[WidgetPlacement],
) -> Vec<WidgetPlacement> {
    if !enabled || messages_area.height == 0 || messages_area.width == 0 {
        return Vec::new();
    }

    let available = data.available_widgets();
    if available.is_empty() {
        return Vec::new();
    }
    let overview_requested = available.contains(&WidgetKind::Overview);

    let mut margin_spaces: Vec<MarginSpace> = Vec::new();
    if !margins.right_widths.is_empty() {
        margin_spaces.push(MarginSpace {
            side: Side::Right,
            widths: margins.right_widths.clone(),
            x_offset: messages_area.x + messages_area.width,
        });
    }
    if margins.centered && !margins.left_widths.is_empty() {
        margin_spaces.push(MarginSpace {
            side: Side::Left,
            widths: margins.left_widths.clone(),
            x_offset: messages_area.x,
        });
    }

    // Format: (side, top, height, width, x_offset, margin_index)
    let mut all_rects: Vec<(Side, u16, u16, u16, u16, usize)> = Vec::new();
    for (margin_idx, margin) in margin_spaces.iter().enumerate() {
        let rects = find_all_empty_rects(&margin.widths, MIN_WIDGET_WIDTH, MIN_WIDGET_HEIGHT);
        for (top, height, width) in rects {
            let clamped_width = width.min(MAX_WIDGET_WIDTH);
            let x = match margin.side {
                Side::Right => margin.x_offset.saturating_sub(clamped_width),
                Side::Left => margin.x_offset,
            };
            all_rects.push((margin.side, top, height, clamped_width, x, margin_idx));
        }
    }

    let mut placements: Vec<WidgetPlacement> = Vec::new();
    let mut kept: HashSet<WidgetKind> = HashSet::new();

    // Phase 1: keep prior placements where the current margins still support them.
    for prev in prev_placements {
        if !available.contains(&prev.kind) || prev.rect.height <= 2 {
            continue;
        }
        if overview_requested && is_overview_mergeable(prev.kind) {
            continue;
        }

        let row_start = prev.rect.y.saturating_sub(messages_area.y) as usize;
        let row_end = row_start + prev.rect.height as usize;
        let widths = match prev.side {
            Side::Right => &margins.right_widths,
            Side::Left => &margins.left_widths,
        };

        let still_fits = row_end <= widths.len()
            && (row_start..row_end)
                .all(|row| widths[row] + STICKY_WIDTH_TOLERANCE >= prev.rect.width);
        if !still_fits {
            continue;
        }

        let actual_fit_width = widths[row_start..row_end]
            .iter()
            .copied()
            .min()
            .unwrap_or(0)
            .min(MAX_WIDGET_WIDTH);
        if actual_fit_width < MIN_WIDGET_WIDTH {
            continue;
        }

        let kept_width = prev.rect.width.min(actual_fit_width);
        let kept_x = match prev.side {
            Side::Right => messages_area
                .x
                .saturating_add(messages_area.width)
                .saturating_sub(kept_width),
            Side::Left => messages_area.x,
        };
        placements.push(WidgetPlacement {
            kind: prev.kind,
            rect: Rect::new(kept_x, prev.rect.y, kept_width, prev.rect.height),
            side: prev.side,
        });
        kept.insert(prev.kind);

        for rect in all_rects.iter_mut() {
            if rect.2 == 0 || rect.0 != prev.side {
                continue;
            }
            let rect_start = rect.1 as usize;
            let rect_end = rect_start + rect.2 as usize;
            if row_start >= rect_end || row_end <= rect_start {
                continue;
            }

            if row_start <= rect_start && row_end >= rect_end {
                rect.2 = 0;
            } else if row_start <= rect_start {
                let trim = (row_end - rect_start) as u16;
                rect.1 += trim;
                rect.2 = rect.2.saturating_sub(trim);
            } else {
                rect.2 = (row_start - rect_start) as u16;
            }
        }
    }

    // Phase 2: greedily place remaining widgets.
    let mut overview_placed = placements.iter().any(|p| p.kind == WidgetKind::Overview);
    for kind in available {
        if kept.contains(&kind) || (overview_placed && is_overview_mergeable(kind)) {
            continue;
        }

        let min_h = kind.min_height() + 2;
        let preferred = kind.preferred_side();
        let mut best_idx: Option<usize> = None;
        let mut best_score = i32::MIN;

        for (idx, &(side, _top, height, width, _x, _margin_idx)) in all_rects.iter().enumerate() {
            if height < min_h || width < MIN_WIDGET_WIDTH {
                continue;
            }

            let mut score = -((height as i32 * width as i32) / 10);
            if side == preferred {
                score += 1000;
            }
            if score > best_score {
                best_score = score;
                best_idx = Some(idx);
            }
        }

        let Some(idx) = best_idx else {
            continue;
        };

        let (side, top, height, width, x, margin_idx) = all_rects[idx];
        let widget_height = calculate_widget_height(kind, data, width, height);
        if widget_height <= 2 {
            continue;
        }

        placements.push(WidgetPlacement {
            kind,
            rect: Rect::new(x, messages_area.y + top, width, widget_height),
            side,
        });
        if kind == WidgetKind::Overview {
            overview_placed = true;
        }

        let remaining_height = height.saturating_sub(widget_height);
        if remaining_height < MIN_WIDGET_HEIGHT {
            all_rects[idx].2 = 0;
            continue;
        }

        let new_top = top + widget_height;
        all_rects[idx].1 = new_top;
        all_rects[idx].2 = remaining_height;

        let margin = &margin_spaces[margin_idx];
        let new_end = (new_top as usize + remaining_height as usize).min(margin.widths.len());
        if new_top as usize >= new_end {
            all_rects[idx].2 = 0;
            continue;
        }

        let actual_min_width = margin.widths[new_top as usize..new_end]
            .iter()
            .copied()
            .min()
            .unwrap_or(0);
        let new_min_width = actual_min_width.min(MAX_WIDGET_WIDTH);
        all_rects[idx].3 = new_min_width;
        all_rects[idx].4 = match side {
            Side::Right => margin.x_offset.saturating_sub(new_min_width),
            Side::Left => margin.x_offset,
        };
    }

    placements
}

/// Find all valid empty rectangles in the margin.
/// Returns a list of `(top_row, height, width)`.
fn find_all_empty_rects(
    free_widths: &[u16],
    min_width: u16,
    min_height: u16,
) -> Vec<(u16, u16, u16)> {
    let mut rects: Vec<(u16, u16, u16)> = Vec::new();
    if free_widths.is_empty() {
        return rects;
    }

    let mut region_start: Option<usize> = None;
    for (i, &width) in free_widths.iter().enumerate() {
        if width >= min_width {
            if region_start.is_none() {
                region_start = Some(i);
            }
        } else if let Some(start) = region_start {
            add_region_rects(&mut rects, free_widths, start, i, min_width, min_height);
            region_start = None;
        }
    }

    if let Some(start) = region_start {
        add_region_rects(
            &mut rects,
            free_widths,
            start,
            free_widths.len(),
            min_width,
            min_height,
        );
    }

    rects
}

fn add_region_rects(
    rects: &mut Vec<(u16, u16, u16)>,
    free_widths: &[u16],
    start: usize,
    end: usize,
    min_width: u16,
    min_height: u16,
) {
    let region_height = end - start;
    if region_height < min_height as usize {
        return;
    }

    let min_w = free_widths[start..end]
        .iter()
        .copied()
        .min()
        .unwrap_or(0)
        .min(MAX_WIDGET_WIDTH);
    if min_w >= min_width {
        rects.push((start as u16, region_height as u16, min_w));
    }
}
