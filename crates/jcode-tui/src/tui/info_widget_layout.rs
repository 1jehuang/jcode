use super::info_widget::{
    InfoWidgetData, Side, WidgetKind, WidgetPlacement, calculate_widget_height,
    is_overview_mergeable,
};
use ratatui::layout::Rect;
use std::collections::HashSet;

/// Minimum width needed to show the widget.
const MIN_WIDGET_WIDTH: u16 = 24;
/// Maximum width the widget can take.
const MAX_WIDGET_WIDTH: u16 = 40;
/// Minimum height needed to show the widget.
const MIN_WIDGET_HEIGHT: u16 = 5;
/// How many consecutive frames a widget may stay hidden-in-place before its
/// anchor is abandoned and Phase 2 is allowed to re-home it elsewhere.
///
/// Under the resident model a widget's rows are bound to transcript lines, so
/// the content beneath it is normally invariant and hiding only happens when
/// that content actively re-renders (streaming region, collapse/expand,
/// markdown refresh). Residents wait that churn out rather than teleporting,
/// so this is generous: it exists only as a backstop for content that
/// permanently grew over the slot.
const MAX_HIDDEN_FRAMES: u16 = 120;

/// Persistent memory of where a widget lives, so it can hold a fixed screen slot
/// across frames (HUD-style) instead of being re-solved from scratch each frame.
#[derive(Debug, Clone)]
pub(crate) struct WidgetAnchor {
    pub placement: WidgetPlacement,
    /// Consecutive frames this anchor has been retained but not rendered.
    pub hidden_frames: u16,
    /// Absolute transcript line the widget's top row is pinned to. Residents
    /// always ride with this content line, so they stick to the same pocket of
    /// negative space and scroll along with the text. When the view is pinned at
    /// the bottom and new lines append, the resident drifts up with its content.
    pub content_top: usize,
}

/// Result of a placement pass: what to render now, plus the anchor memory to feed
/// back in next frame.
pub(crate) struct PlacementOutcome {
    pub visible: Vec<WidgetPlacement>,
    pub anchors: Vec<WidgetAnchor>,
}

/// Margin information for layout calculation.
#[derive(Debug, Clone, Default)]
pub struct Margins {
    /// Free widths on the right side for each row.
    pub right_widths: Vec<u16>,
    /// Free widths on the left side for each row (only populated in centered mode).
    pub left_widths: Vec<u16>,
    /// Whether we're in centered mode.
    pub centered: bool,
    /// Look-ahead "reliable" free widths: per row, the width that stays free across
    /// a small band of upcoming/recent scroll lines. When non-empty these gate where
    /// *new* widgets may dock (Phase 2) so a freshly placed widget won't be covered
    /// by a long line a frame later. Pinned widgets (Phase 1) still size themselves
    /// to the instantaneous `right_widths`/`left_widths` for full coverage. Empty =
    /// fall back to the instantaneous widths (no look-ahead).
    pub right_reliable: Vec<u16>,
    pub left_reliable: Vec<u16>,
    /// Absolute transcript line shown on the first visible row this frame. Lets the
    /// placement engine translate a content-anchored widget by the scroll delta so
    /// it rides the transcript instead of holding a fixed screen row.
    pub scroll_top: usize,
}

impl Margins {
    /// Reliable right-side widths for dock gating, falling back to instantaneous.
    fn right_dock_widths(&self) -> &[u16] {
        if self.right_reliable.is_empty() {
            &self.right_widths
        } else {
            &self.right_reliable
        }
    }
    /// Reliable left-side widths for dock gating, falling back to instantaneous.
    fn left_dock_widths(&self) -> &[u16] {
        if self.left_reliable.is_empty() {
            &self.left_widths
        } else {
            &self.left_reliable
        }
    }
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

/// Choose the per-row widths Phase 2 should dock against: prefer the look-ahead
/// `reliable` profile, but if it admits no placeable region at all (e.g. dense long
/// lines cover every candidate row) fall back to `instant` so we still show
/// something. Returns an owned profile to store in the `MarginSpace`.
fn dock_widths_with_fallback(reliable: &[u16], instant: &[u16]) -> Vec<u16> {
    if reliable.is_empty() {
        return instant.to_vec();
    }
    let reliable_has_dock =
        !find_all_empty_rects(reliable, MIN_WIDGET_WIDTH, MIN_WIDGET_HEIGHT).is_empty();
    if reliable_has_dock {
        reliable.to_vec()
    } else {
        instant.to_vec()
    }
}

/// Anchor-aware placement. Widgets behave like a pinned HUD: each holds the exact
/// screen slot recorded in its anchor for as long as that slot can show it, only
/// shrinking (with hysteresis) or hiding-in-place when a wide line scrolls under
/// it, and only re-homing via Phase 2 after the slot has been unusable for
/// [`MAX_HIDDEN_FRAMES`]. This is what stops widgets from jumping while scrolling.
pub(crate) fn calculate_placements_anchored(
    messages_area: Rect,
    margins: &Margins,
    data: &InfoWidgetData,
    enabled: bool,
    prev_anchors: &[WidgetAnchor],
) -> PlacementOutcome {
    if !enabled || messages_area.height == 0 || messages_area.width == 0 {
        return PlacementOutcome {
            visible: Vec::new(),
            anchors: Vec::new(),
        };
    }

    let available = data.available_widgets();
    if available.is_empty() {
        return PlacementOutcome {
            visible: Vec::new(),
            anchors: Vec::new(),
        };
    }
    let mut margin_spaces: Vec<MarginSpace> = Vec::new();
    if !margins.right_widths.is_empty() {
        // Phase 2 docks on the *reliable* (look-ahead) widths so a newly placed
        // widget lands only where space stays free for a few scroll lines. But if
        // the reliable profile is so covered that it yields no dock at all (dense
        // long lines), fall back to the instantaneous widths so we still show
        // something rather than nothing. Phase 1 always sizes pinned widgets to the
        // instantaneous widths for full coverage.
        let mut dock =
            dock_widths_with_fallback(margins.right_dock_widths(), &margins.right_widths);
        // Never dock below the messages area: a margin profile longer than the
        // area (caller bug or stale data) must not let a widget escape the
        // viewport and draw over the input/status rows beneath it.
        dock.truncate(messages_area.height as usize);
        margin_spaces.push(MarginSpace {
            side: Side::Right,
            widths: dock,
            x_offset: messages_area.x + messages_area.width,
        });
    }
    if margins.centered && !margins.left_widths.is_empty() {
        let mut dock = dock_widths_with_fallback(margins.left_dock_widths(), &margins.left_widths);
        dock.truncate(messages_area.height as usize);
        margin_spaces.push(MarginSpace {
            side: Side::Left,
            widths: dock,
            x_offset: messages_area.x,
        });
    }

    // Format: (side, top, height, width, x_offset, margin_index)
    let mut all_rects: Vec<(Side, u16, u16, u16, u16, usize)> = Vec::new();
    for (margin_idx, margin) in margin_spaces.iter().enumerate() {
        let rects = find_all_empty_rects(&margin.widths, MIN_WIDGET_WIDTH, MIN_WIDGET_HEIGHT);
        for (top, height, width) in rects {
            // Clamp to the area width as well: a margin profile reporting more
            // free width than the area is wide (caller bug) must not produce a
            // rect that pokes out of the viewport and panics the renderer.
            let clamped_width = width.min(MAX_WIDGET_WIDTH).min(messages_area.width);
            let x = match margin.side {
                Side::Right => margin.x_offset.saturating_sub(clamped_width),
                Side::Left => margin.x_offset,
            };
            all_rects.push((margin.side, top, height, clamped_width, x, margin_idx));
        }
    }

    let mut placements: Vec<WidgetPlacement> = Vec::new();
    // Anchors to carry into the next frame, keyed by widget kind.
    let mut next_anchors: Vec<WidgetAnchor> = Vec::new();
    let mut kept: HashSet<WidgetKind> = HashSet::new();
    // Widgets whose anchor is still alive (visible or hidden-in-place) and which
    // therefore must NOT be re-homed by Phase 2 this frame.
    let mut anchored: HashSet<WidgetKind> = HashSet::new();
    // Overview "merges" several smaller widgets (model, context, ...). Those
    // mergeable widgets must be suppressed only when Overview is *actually shown*
    // this frame, not merely requested. While Overview is hidden-in-place (its slot
    // is temporarily covered by a wide line) the small widgets are NOT the right
    // fallback either, because Overview will pop back; but if Overview never had a
    // slot at all, the small widgets are the real content and must anchor normally.
    // We process the Overview anchor first so this flag is known before the small
    // widgets are considered.
    let mut overview_active = false;

    // Order anchors so Overview is resolved before any mergeable widget.
    let mut ordered_anchors: Vec<&WidgetAnchor> = prev_anchors.iter().collect();
    ordered_anchors.sort_by_key(|a| match a.placement.kind {
        WidgetKind::Overview => 0,
        _ => 1,
    });

    // Phase 1: hold each anchored widget in its exact recorded slot.
    //
    // The viewport's free-width profile churns line-by-line as ragged content
    // scrolls under fixed screen rows. The old code bailed as soon as the content
    // under a widget grew a few columns, which sent the widget to Phase 2 where it
    // teleported into whatever the largest empty pocket happened to be that frame -
    // the distracting jump. Instead, a placed widget now holds its exact screen
    // slot as long as that slot can still show it at >= MIN_WIDGET_WIDTH, so it
    // never moves for width reasons. Width only *shrinks* immediately when the slot
    // narrows and *grows back* with hysteresis, so the left edge does not jitter.
    for anchor in ordered_anchors {
        let prev = &anchor.placement;
        if !available.contains(&prev.kind) || prev.rect.height <= 2 {
            continue;
        }
        if overview_active && is_overview_mergeable(prev.kind) {
            continue;
        }
        if kept.contains(&prev.kind) || anchored.contains(&prev.kind) {
            // Already handled (duplicate anchor for the same kind).
            continue;
        }

        // Resolve which screen row the widget occupies this frame.
        //
        // Content-anchored (the user is scrolling): the widget is pinned to a
        // transcript line (`anchor.content_top`) and rides with it, so it sticks to
        // the same pocket of negative space and simply scrolls along with the text
        // rather than churning against a fixed screen row. Because the rows it now
        // covers map back to the *same* content lines, the free-width profile under
        // it is invariant frame-to-frame, so its width is stable too. If its content
        // line has scrolled above the viewport, drop the anchor and let Phase 2 home
        // a fresh widget into the newly exposed space.
        //
        // Screen-anchored (pinned at the bottom / streaming): hold the exact screen
        // row as before, and refresh `content_top` so a later switch into scrolling
        // hands off seamlessly.
        let height = prev.rect.height as usize;
        // Resident model: every anchor is bound to a transcript line and rides
        // with it, in both scrolling and pinned-at-bottom (streaming) modes. When
        // new lines append while pinned, `scroll_top` advances and the widget
        // drifts up with the content it belongs to instead of holding a screen
        // row while text churns under it. If its content line has scrolled above
        // the viewport the resident retires, and Phase 2 may house a fresh
        // instance in newly settled space.
        let (row_start, target_y, content_top) = {
            if anchor.content_top < margins.scroll_top {
                continue;
            }
            let row = anchor.content_top - margins.scroll_top;
            (
                row,
                messages_area.y.saturating_add(row as u16),
                anchor.content_top,
            )
        };
        let row_end = row_start + height;
        let widths = match prev.side {
            Side::Right => &margins.right_widths,
            Side::Left => &margins.left_widths,
        };
        if height == 0 || row_end > widths.len() || row_end > messages_area.height as usize {
            continue;
        }

        // Widest the widget can be without overrunning the text on any of its rows.
        let fit_width = widths[row_start..row_end]
            .iter()
            .copied()
            .min()
            .unwrap_or(0)
            .min(MAX_WIDGET_WIDTH)
            .min(messages_area.width);
        let renderable = fit_width >= MIN_WIDGET_WIDTH;

        // Width is monotonic non-increasing for the life of an anchor: it shrinks to
        // clear newly-wide content but never grows back while pinned. Growing would
        // move a right-anchored widget's left edge inward every time the margin
        // breathed, which reads as horizontal jitter on ragged content. The widget
        // settles to the narrowest width its slot ever required and then holds. It
        // only widens again by re-homing (a fresh Phase 2 placement).
        let kept_width = if !renderable {
            prev.rect.width
        } else {
            fit_width.min(prev.rect.width)
        };

        if !renderable || kept_width < MIN_WIDGET_WIDTH {
            // The slot can't show the widget this frame (a wide line scrolled under
            // it). Keep the anchor and hide in place so it returns to the same spot
            // when the wide line passes - unless it has been hidden too long, in
            // which case we abandon the anchor and let Phase 2 re-home it.
            let hidden_frames = anchor.hidden_frames.saturating_add(1);
            if hidden_frames <= MAX_HIDDEN_FRAMES {
                anchored.insert(prev.kind);
                next_anchors.push(WidgetAnchor {
                    placement: prev.clone(),
                    hidden_frames,
                    content_top,
                });
                // Overview will pop back into its slot, so keep suppressing its
                // mergeable widgets while it is only transiently hidden.
                if prev.kind == WidgetKind::Overview {
                    overview_active = true;
                }
                // Reserve the hidden widget's rows so Phase 2 cannot drop another
                // widget into the slot it will reclaim next frame; otherwise the
                // returning widget would overlap whatever took its place.
                reserve_rows(&mut all_rects, prev.side, row_start, row_end);
            }
            continue;
        }

        let kept_x = match prev.side {
            Side::Right => messages_area
                .x
                .saturating_add(messages_area.width)
                .saturating_sub(kept_width),
            Side::Left => messages_area.x,
        };
        let placement = WidgetPlacement {
            kind: prev.kind,
            rect: Rect::new(kept_x, target_y, kept_width, prev.rect.height),
            side: prev.side,
        };
        placements.push(placement.clone());
        next_anchors.push(WidgetAnchor {
            placement,
            hidden_frames: 0,
            content_top,
        });
        kept.insert(prev.kind);
        anchored.insert(prev.kind);
        if prev.kind == WidgetKind::Overview {
            overview_active = true;
        }

        reserve_rows(&mut all_rects, prev.side, row_start, row_end);
    }

    // Phase 2: greedily place remaining widgets.
    //
    // `overview_active` already covers the case where Overview is shown OR only
    // hidden-in-place; in both cases its mergeable widgets (model/context/...) are
    // suppressed so they don't pop in at a *different* location while Overview is
    // momentarily covered. `overview_placed` additionally covers a brand-new
    // Overview placed within this very Phase 2 pass.
    let mut overview_placed = overview_active;
    for kind in available {
        if kept.contains(&kind)
            || anchored.contains(&kind)
            || (overview_placed && is_overview_mergeable(kind))
        {
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

        // Seat the widget at the *bottom* of the pocket so it has the maximum
        // runway to ride upward with the transcript before scrolling off the top -
        // a widget born at the pocket's top row would fall off and re-home every
        // frame (a constant recycle). The leftover free space is the region above.
        let placed_top = top + height.saturating_sub(widget_height);

        let placement = WidgetPlacement {
            kind,
            rect: Rect::new(x, messages_area.y + placed_top, width, widget_height),
            side,
        };
        placements.push(placement.clone());
        next_anchors.push(WidgetAnchor {
            placement,
            hidden_frames: 0,
            // Bind this fresh widget to the transcript line currently under its top
            // row, so the moment the user keeps scrolling it rides with the content.
            content_top: margins.scroll_top + placed_top as usize,
        });
        if kind == WidgetKind::Overview {
            overview_placed = true;
        }

        let remaining_height = height.saturating_sub(widget_height);
        if remaining_height < MIN_WIDGET_HEIGHT {
            all_rects[idx].2 = 0;
            continue;
        }

        // The leftover pocket is the region above the bottom-seated widget.
        let new_top = top;
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
        let new_min_width = actual_min_width
            .min(MAX_WIDGET_WIDTH)
            .min(messages_area.width);
        all_rects[idx].3 = new_min_width;
        all_rects[idx].4 = match side {
            Side::Right => margin.x_offset.saturating_sub(new_min_width),
            Side::Left => margin.x_offset,
        };
    }

    PlacementOutcome {
        visible: placements,
        anchors: next_anchors,
    }
}

/// Carve the rows `[row_start, row_end)` on `side` out of the candidate empty
/// rectangles so Phase 2 cannot place another widget into space already claimed by
/// an anchored widget - whether that widget is visible this frame or only
/// hidden-in-place. `all_rects` entries are `(side, top, height, width, x, margin)`.
fn reserve_rows(
    all_rects: &mut [(Side, u16, u16, u16, u16, usize)],
    side: Side,
    row_start: usize,
    row_end: usize,
) {
    for rect in all_rects.iter_mut() {
        if rect.2 == 0 || rect.0 != side {
            continue;
        }
        let rect_start = rect.1 as usize;
        let rect_end = rect_start + rect.2 as usize;
        if row_start >= rect_end || row_end <= rect_start {
            continue;
        }

        if row_start <= rect_start && row_end >= rect_end {
            // Fully covered: remove the rect.
            rect.2 = 0;
        } else if row_start <= rect_start {
            // Covered from the top: push the top down past the reserved rows.
            let trim = (row_end - rect_start) as u16;
            rect.1 += trim;
            rect.2 = rect.2.saturating_sub(trim);
        } else {
            // Covered from the bottom (or middle): keep only the top portion.
            rect.2 = (row_start - rect_start) as u16;
        }
    }
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

#[cfg(test)]
#[path = "info_widget_layout_tests.rs"]
mod tests;
