use super::{accent_color, clear_area, dim_color, tool_color};
use crate::tui::info_widget;
use ratatui::{prelude::*, widgets::Paragraph};
use serde::Serialize;
use std::cell::RefCell;

#[derive(Debug, Clone, Default, Serialize)]
pub struct PinnedDiagramProbeRect {
    pub width_cells: u16,
    pub height_cells: u16,
    pub width_utilization_percent: f64,
    pub height_utilization_percent: f64,
    pub area_utilization_percent: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PinnedDiagramLiveDebugSnapshot {
    pub index: usize,
    pub total: usize,
    pub pane_width_cells: u16,
    pub pane_height_cells: u16,
    pub inner_width_cells: u16,
    pub inner_height_cells: u16,
    pub focused: bool,
    pub scroll_x: i32,
    pub scroll_y: i32,
    pub zoom_percent: u8,
    pub render_mode: String,
    pub rendered_png_width_px: u32,
    pub rendered_png_height_px: u32,
    pub pane_utilization: PinnedDiagramProbeRect,
    pub inner_utilization: PinnedDiagramProbeRect,
    pub log: String,
}

#[derive(Default)]
struct PinnedDiagramDebugState {
    live_snapshot: Option<PinnedDiagramLiveDebugSnapshot>,
}

const PINNED_DIAGRAM_TARGET_UTILIZATION_PERCENT: f64 = 85.0;
const PINNED_DIAGRAM_MIN_READABLE_ZOOM_PERCENT: u16 = 70;
const PINNED_DIAGRAM_MAX_AUTO_FILL_ZOOM_PERCENT: u16 = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PinnedDiagramFitRenderPlan {
    /// Show the whole diagram, centered inside the pane.
    Contain { area: Rect },
    /// Keep text readable by filling the pane and cropping to a centered viewport.
    FillViewport {
        zoom_percent: u16,
        scroll_x: i32,
        scroll_y: i32,
    },
}

impl PinnedDiagramFitRenderPlan {
    fn visible_rect(self, inner: Rect) -> Rect {
        match self {
            Self::Contain { area } => area,
            Self::FillViewport { .. } => inner,
        }
    }

    fn mode_label(self) -> String {
        match self {
            Self::Contain { .. } => "fit".to_string(),
            Self::FillViewport { zoom_percent, .. } => format!("fit-fill@{zoom_percent}%"),
        }
    }
}

fn utilization_percent(used: u32, total: u32) -> f64 {
    if total == 0 {
        0.0
    } else {
        (used as f64 * 100.0) / total as f64
    }
}

fn axis_fill_zoom_percent(available_cells: u16, image_px: u32, cell_px: u16) -> u16 {
    if available_cells == 0 || image_px == 0 || cell_px == 0 {
        return 100;
    }

    (available_cells as u32)
        .saturating_mul(cell_px as u32)
        .saturating_mul(100)
        .checked_div(image_px.max(1))
        .unwrap_or(100)
        .clamp(1, PINNED_DIAGRAM_MAX_AUTO_FILL_ZOOM_PERCENT as u32) as u16
}

fn fit_zoom_percent_for_area(
    area: Rect,
    img_w_px: u32,
    img_h_px: u32,
    font_size: Option<(u16, u16)>,
) -> u16 {
    if area.width == 0 || area.height == 0 || img_w_px == 0 || img_h_px == 0 {
        return 100;
    }

    let Some((font_w, font_h)) = font_size else {
        return 100;
    };
    let font_w = font_w.max(1) as u32;
    let font_h = font_h.max(1) as u32;
    let zoom_w = (area.width as u32)
        .saturating_mul(font_w)
        .saturating_mul(100)
        / img_w_px.max(1);
    let zoom_h = (area.height as u32)
        .saturating_mul(font_h)
        .saturating_mul(100)
        / img_h_px.max(1);
    zoom_w
        .min(zoom_h)
        .clamp(1, PINNED_DIAGRAM_MAX_AUTO_FILL_ZOOM_PERCENT as u32) as u16
}

fn centered_viewport_scroll_cells(
    image_px: u32,
    area_cells: u16,
    zoom_percent: u16,
    cell_px: u16,
) -> i32 {
    if image_px == 0 || area_cells == 0 || zoom_percent == 0 || cell_px == 0 {
        return 0;
    }

    let zoom = zoom_percent as u32;
    let view_px = (area_cells as u32)
        .saturating_mul(cell_px as u32)
        .saturating_mul(100)
        / zoom;
    let max_scroll_px = image_px.saturating_sub(view_px);
    if max_scroll_px == 0 {
        return 0;
    }

    let cell_px_at_zoom = div_ceil_u32((cell_px as u32).saturating_mul(100), zoom).max(1);

    ((max_scroll_px / 2) / cell_px_at_zoom).min(i32::MAX as u32) as i32
}

fn plan_pinned_diagram_fit_with_font(
    area: Rect,
    img_w_px: u32,
    img_h_px: u32,
    font_size: Option<(u16, u16)>,
) -> PinnedDiagramFitRenderPlan {
    let contain_area = vcenter_fitted_image_with_font(area, img_w_px, img_h_px, font_size);
    if area.width == 0 || area.height == 0 || img_w_px == 0 || img_h_px == 0 {
        return PinnedDiagramFitRenderPlan::Contain { area: contain_area };
    }

    let Some((font_w, font_h)) = font_size else {
        return PinnedDiagramFitRenderPlan::Contain { area: contain_area };
    };
    let font_w = font_w.max(1);
    let font_h = font_h.max(1);
    let fit_zoom = fit_zoom_percent_for_area(area, img_w_px, img_h_px, Some((font_w, font_h)));
    let width_fill_zoom = axis_fill_zoom_percent(area.width, img_w_px, font_w);
    let height_fill_zoom = axis_fill_zoom_percent(area.height, img_h_px, font_h);
    let preferred_fill_zoom = width_fill_zoom.max(height_fill_zoom).clamp(
        PINNED_DIAGRAM_MIN_READABLE_ZOOM_PERCENT,
        PINNED_DIAGRAM_MAX_AUTO_FILL_ZOOM_PERCENT,
    );

    let width_utilization = utilization_percent(contain_area.width as u32, area.width as u32);
    let height_utilization = utilization_percent(contain_area.height as u32, area.height as u32);
    let contain_area_cells = (contain_area.width as u32).saturating_mul(contain_area.height as u32);
    let available_area_cells = (area.width as u32).saturating_mul(area.height as u32);
    let area_utilization = utilization_percent(contain_area_cells, available_area_cells);
    let underutilized = width_utilization < PINNED_DIAGRAM_TARGET_UTILIZATION_PERCENT
        || height_utilization < PINNED_DIAGRAM_TARGET_UTILIZATION_PERCENT
        || area_utilization < PINNED_DIAGRAM_TARGET_UTILIZATION_PERCENT;
    let meaningfully_larger = preferred_fill_zoom > fit_zoom.saturating_add(5);

    if underutilized && meaningfully_larger {
        return PinnedDiagramFitRenderPlan::FillViewport {
            zoom_percent: preferred_fill_zoom,
            scroll_x: centered_viewport_scroll_cells(
                img_w_px,
                area.width,
                preferred_fill_zoom,
                font_w,
            ),
            scroll_y: centered_viewport_scroll_cells(
                img_h_px,
                area.height,
                preferred_fill_zoom,
                font_h,
            ),
        };
    }

    PinnedDiagramFitRenderPlan::Contain { area: contain_area }
}

fn plan_pinned_diagram_fit(area: Rect, img_w_px: u32, img_h_px: u32) -> PinnedDiagramFitRenderPlan {
    plan_pinned_diagram_fit_with_font(
        area,
        img_w_px,
        img_h_px,
        super::super::mermaid::get_font_size(),
    )
}

fn pinned_diagram_content_area_for_title(
    area: Rect,
    pane_position: crate::config::DiagramPanePosition,
) -> Option<Rect> {
    use ratatui::widgets::{Block, Borders};

    match pane_position {
        crate::config::DiagramPanePosition::Side => {
            let inner = Block::default().borders(Borders::LEFT).inner(area);
            if inner.width == 0 || inner.height <= 1 {
                None
            } else {
                Some(Rect {
                    x: inner.x,
                    y: inner.y + 1,
                    width: inner.width,
                    height: inner.height - 1,
                })
            }
        }
        crate::config::DiagramPanePosition::Top => {
            let inner = Block::default().borders(Borders::ALL).inner(area);
            if inner.width == 0 || inner.height == 0 {
                None
            } else {
                Some(inner)
            }
        }
    }
}

pub(crate) fn content_area_preferred_aspect_ratio(area: Rect) -> Option<f32> {
    if area.width == 0 || area.height == 0 {
        return None;
    }
    let (font_w, font_h) = super::super::mermaid::get_font_size().unwrap_or((8, 16));
    let width_px = area.width as f32 * font_w.max(1) as f32;
    let height_px = area.height as f32 * font_h.max(1) as f32;
    if width_px > 0.0 && height_px > 0.0 {
        Some(width_px / height_px)
    } else {
        None
    }
}

pub(crate) fn pinned_diagram_preferred_aspect_ratio(
    area: Rect,
    pane_position: crate::config::DiagramPanePosition,
) -> Option<f32> {
    pinned_diagram_content_area_for_title(area, pane_position)
        .and_then(content_area_preferred_aspect_ratio)
}

fn planned_pinned_diagram_mode_label(
    diagram: &info_widget::DiagramInfo,
    area: Rect,
    pane_position: crate::config::DiagramPanePosition,
    fit_mode: bool,
    zoom_percent: u8,
) -> String {
    if !fit_mode {
        return "pan".to_string();
    }

    pinned_diagram_content_area_for_title(area, pane_position)
        .map(|inner| plan_pinned_diagram_fit(inner, diagram.width, diagram.height).mode_label())
        .unwrap_or_else(|| pinned_diagram_render_mode_label(fit_mode, zoom_percent))
}

fn probe_rect(
    rendered_width: u16,
    rendered_height: u16,
    total_width: u16,
    total_height: u16,
) -> PinnedDiagramProbeRect {
    PinnedDiagramProbeRect {
        width_cells: rendered_width,
        height_cells: rendered_height,
        width_utilization_percent: utilization_percent(rendered_width as u32, total_width as u32),
        height_utilization_percent: utilization_percent(
            rendered_height as u32,
            total_height as u32,
        ),
        area_utilization_percent: utilization_percent(
            rendered_width as u32 * rendered_height as u32,
            total_width as u32 * total_height as u32,
        ),
    }
}

fn pinned_diagram_render_mode_label(fit_mode: bool, zoom_percent: u8) -> String {
    if fit_mode {
        "fit".to_string()
    } else {
        format!("scrollable-viewport@{zoom_percent}%")
    }
}

#[derive(Clone, Copy)]
struct PinnedDiagramSnapshotLayout {
    area: Rect,
    inner: Rect,
    index: usize,
    total: usize,
}

#[derive(Clone, Copy)]
struct PinnedDiagramSnapshotView {
    focused: bool,
    scroll_x: i32,
    scroll_y: i32,
    zoom_percent: u8,
}

fn build_pinned_diagram_live_snapshot(
    diagram: &info_widget::DiagramInfo,
    layout: PinnedDiagramSnapshotLayout,
    view: PinnedDiagramSnapshotView,
) -> PinnedDiagramLiveDebugSnapshot {
    build_pinned_diagram_live_snapshot_with_font(
        diagram,
        layout,
        view,
        super::super::mermaid::get_font_size(),
    )
}

fn build_pinned_diagram_live_snapshot_with_font(
    diagram: &info_widget::DiagramInfo,
    layout: PinnedDiagramSnapshotLayout,
    view: PinnedDiagramSnapshotView,
    font_size: Option<(u16, u16)>,
) -> PinnedDiagramLiveDebugSnapshot {
    let PinnedDiagramSnapshotLayout {
        area,
        inner,
        index,
        total,
    } = layout;
    let PinnedDiagramSnapshotView {
        focused,
        scroll_x,
        scroll_y,
        zoom_percent,
    } = view;
    let fit_mode = diagram_view_uses_fit_mode(focused, scroll_x, scroll_y, zoom_percent);
    let fit_plan = if fit_mode {
        Some(plan_pinned_diagram_fit_with_font(
            inner,
            diagram.width,
            diagram.height,
            font_size,
        ))
    } else {
        None
    };
    let visible_rect = fit_plan.map_or(inner, |plan| plan.visible_rect(inner));
    let pane_utilization = probe_rect(
        visible_rect.width,
        visible_rect.height,
        area.width,
        area.height,
    );
    let inner_utilization = probe_rect(
        visible_rect.width,
        visible_rect.height,
        inner.width,
        inner.height,
    );
    let render_mode = fit_plan.map_or_else(
        || pinned_diagram_render_mode_label(fit_mode, zoom_percent),
        PinnedDiagramFitRenderPlan::mode_label,
    );

    PinnedDiagramLiveDebugSnapshot {
        index,
        total,
        pane_width_cells: area.width,
        pane_height_cells: area.height,
        inner_width_cells: inner.width,
        inner_height_cells: inner.height,
        focused,
        scroll_x,
        scroll_y,
        zoom_percent,
        render_mode: render_mode.clone(),
        rendered_png_width_px: diagram.width,
        rendered_png_height_px: diagram.height,
        pane_utilization,
        inner_utilization: inner_utilization.clone(),
        log: format!(
            "diagram#{}/{} {} visible={}x{} cells ({:.1}% inner area)",
            index + 1,
            total.max(1),
            render_mode,
            inner_utilization.width_cells,
            inner_utilization.height_cells,
            inner_utilization.area_utilization_percent,
        ),
    }
}

pub fn debug_probe_pinned_diagram(
    diagram: &info_widget::DiagramInfo,
    area: Rect,
    inner: Rect,
    focused: bool,
    scroll_x: i32,
    scroll_y: i32,
    zoom_percent: u8,
) -> PinnedDiagramLiveDebugSnapshot {
    build_pinned_diagram_live_snapshot(
        diagram,
        PinnedDiagramSnapshotLayout {
            area,
            inner,
            index: 0,
            total: 1,
        },
        PinnedDiagramSnapshotView {
            focused,
            scroll_x,
            scroll_y,
            zoom_percent,
        },
    )
}

#[cfg(test)]
pub(crate) fn debug_probe_pinned_diagram_with_font(
    diagram: &info_widget::DiagramInfo,
    area: Rect,
    inner: Rect,
    focused: bool,
    scroll_x: i32,
    scroll_y: i32,
    zoom_percent: u8,
    font_size: Option<(u16, u16)>,
) -> PinnedDiagramLiveDebugSnapshot {
    build_pinned_diagram_live_snapshot_with_font(
        diagram,
        PinnedDiagramSnapshotLayout {
            area,
            inner,
            index: 0,
            total: 1,
        },
        PinnedDiagramSnapshotView {
            focused,
            scroll_x,
            scroll_y,
            zoom_percent,
        },
        font_size,
    )
}

thread_local! {
    static PINNED_DIAGRAM_DEBUG_STATE: RefCell<PinnedDiagramDebugState> = RefCell::new(PinnedDiagramDebugState::default());
}

fn with_pinned_diagram_debug<R>(f: impl FnOnce(&PinnedDiagramDebugState) -> R) -> R {
    PINNED_DIAGRAM_DEBUG_STATE.with(|state| f(&state.borrow()))
}

fn with_pinned_diagram_debug_mut<R>(f: impl FnOnce(&mut PinnedDiagramDebugState) -> R) -> R {
    PINNED_DIAGRAM_DEBUG_STATE.with(|state| f(&mut state.borrow_mut()))
}

pub(crate) fn pinned_diagram_debug_json() -> Option<serde_json::Value> {
    let live_snapshot = with_pinned_diagram_debug(|state| state.live_snapshot.clone());
    serde_json::to_value(serde_json::json!({
        "live": live_snapshot,
    }))
    .ok()
}

pub(crate) fn clear_pinned_diagram_debug_snapshot() {
    with_pinned_diagram_debug_mut(|debug| {
        debug.live_snapshot = None;
    });
}

pub(crate) fn reset_pinned_diagram_debug_snapshot() {
    clear_pinned_diagram_debug_snapshot();
}

pub(crate) fn div_ceil_u32(value: u32, divisor: u32) -> u32 {
    if divisor == 0 {
        return value;
    }
    value.saturating_add(divisor - 1) / divisor
}

#[cfg(test)]
mod tests {
    use super::{
        PinnedDiagramFitRenderPlan, diagram_view_uses_fit_mode, plan_pinned_diagram_fit_with_font,
        vcenter_fitted_image_with_font,
    };
    use ratatui::prelude::Rect;

    #[test]
    fn diagram_view_uses_fit_mode_when_unfocused_or_reset() {
        assert!(diagram_view_uses_fit_mode(false, 0, 0, 100));
        assert!(diagram_view_uses_fit_mode(true, 0, 0, 100));
        assert!(!diagram_view_uses_fit_mode(true, 1, 0, 100));
        assert!(!diagram_view_uses_fit_mode(true, 0, 1, 100));
        assert!(!diagram_view_uses_fit_mode(true, 0, 0, 90));
    }

    #[test]
    fn vcenter_fitted_image_uses_the_full_inner_area_without_extra_padding() {
        let area = Rect::new(10, 5, 48, 38);
        let fitted = vcenter_fitted_image_with_font(area, 600, 300, Some((8, 16)));

        assert_eq!(fitted.x, area.x);
        assert_eq!(fitted.width, area.width);
        assert!(
            fitted.y > area.y,
            "wide image should be vertically centered"
        );
        assert!(fitted.y + fitted.height <= area.y + area.height);
    }

    #[test]
    fn pinned_diagram_fit_plan_fills_pane_when_contain_would_be_tiny() {
        let inner = Rect::new(1, 1, 44, 49);
        let plan = plan_pinned_diagram_fit_with_font(inner, 614, 743, Some((15, 34)));

        match plan {
            PinnedDiagramFitRenderPlan::FillViewport {
                zoom_percent,
                scroll_x,
                scroll_y,
            } => {
                assert!(
                    zoom_percent > 200,
                    "auto fit-fill should not be capped at 200%, got {zoom_percent}%"
                );
                assert!(
                    scroll_x > 0,
                    "fill viewport should start horizontally centered"
                );
                assert_eq!(
                    scroll_y, 0,
                    "the full diagram height fits at the selected zoom in this pane"
                );
                assert_eq!(plan.visible_rect(inner), inner);
                assert_eq!(plan.mode_label(), format!("fit-fill@{zoom_percent}%"));
            }
            other => panic!("expected fill viewport for underutilized contain fit, got {other:?}"),
        }
    }

    #[test]
    fn pinned_diagram_fit_plan_fills_wide_short_lr_flowchart() {
        // Regression for a side-pane Mermaid LR flowchart that rendered as a
        // tiny horizontal strip with large blank space below it. A 200% cap is
        // still too low here; matching the pane aspect requires a much higher
        // centered viewport zoom.
        let inner = Rect::new(75, 1, 118, 70);
        let plan = plan_pinned_diagram_fit_with_font(inner, 1440, 110, Some((8, 16)));

        match plan {
            PinnedDiagramFitRenderPlan::FillViewport {
                zoom_percent,
                scroll_x,
                scroll_y,
            } => {
                assert!(
                    zoom_percent >= 700,
                    "wide short diagrams need high fit-fill zoom, got {zoom_percent}%"
                );
                assert!(scroll_x > 0, "wide diagram should be horizontally centered");
                assert_eq!(scroll_y, 0, "short diagram should use its full height");
                assert_eq!(plan.visible_rect(inner), inner);
                assert_eq!(plan.mode_label(), format!("fit-fill@{zoom_percent}%"));
            }
            other => panic!("expected fit-fill viewport for wide short diagram, got {other:?}"),
        }
    }

    #[test]
    fn pinned_diagram_fit_plan_keeps_contain_when_diagram_already_fills_pane() {
        let inner = Rect::new(0, 0, 36, 30);
        let plan = plan_pinned_diagram_fit_with_font(inner, 219, 360, Some((8, 16)));

        match plan {
            PinnedDiagramFitRenderPlan::Contain { area } => {
                assert_eq!(area.width, inner.width);
                assert_eq!(area.height, inner.height);
                assert_eq!(plan.visible_rect(inner), inner);
                assert_eq!(plan.mode_label(), "fit");
            }
            other => panic!("expected contain fit for well-utilized diagram, got {other:?}"),
        }
    }

    #[test]
    fn pinned_diagram_fit_plan_keeps_contain_for_full_height_beetle_repro() {
        // Repro from a live Beetle/Harbor TUI frame: a simple TD flowchart
        // rendered as 1180x1470 in a 73x46-cell side pane. Contain uses almost
        // the whole pane, so auto fit must show the complete diagram. The old
        // readability-floor rule forced fit-fill@70%, cropping the top node
        // while leaving visible blank space below the chart.
        let inner = Rect::new(96, 1, 73, 46);
        let plan = plan_pinned_diagram_fit_with_font(inner, 1180, 1470, Some((8, 16)));

        match plan {
            PinnedDiagramFitRenderPlan::Contain { area } => {
                assert_eq!(area.width, 73);
                assert_eq!(area.height, 46);
                assert_eq!(area.y, inner.y);
                assert_eq!(plan.mode_label(), "fit");
            }
            other => panic!("expected full contain fit for Beetle repro, got {other:?}"),
        }
    }
}

pub(crate) fn estimate_pinned_diagram_pane_width_with_font(
    diagram: &info_widget::DiagramInfo,
    pane_height: u16,
    min_width: u16,
    font_size: Option<(u16, u16)>,
) -> u16 {
    const PANE_BORDER_WIDTH: u32 = 2;
    let inner_height = pane_height.saturating_sub(PANE_BORDER_WIDTH as u16).max(1) as u32;
    let (cell_w, cell_h) = font_size.unwrap_or((8, 16));
    let cell_w = cell_w.max(1) as u32;
    let cell_h = cell_h.max(1) as u32;

    let image_w_cells = div_ceil_u32(diagram.width.max(1), cell_w);
    let image_h_cells = div_ceil_u32(diagram.height.max(1), cell_h);
    let fit_w_cells = if image_h_cells > inner_height {
        div_ceil_u32(image_w_cells.saturating_mul(inner_height), image_h_cells)
    } else {
        image_w_cells
    }
    .max(1);

    let pane_width = fit_w_cells.saturating_add(PANE_BORDER_WIDTH);
    pane_width.max(min_width as u32).min(u16::MAX as u32) as u16
}

pub(crate) fn estimate_pinned_diagram_pane_width(
    diagram: &info_widget::DiagramInfo,
    pane_height: u16,
    min_width: u16,
) -> u16 {
    estimate_pinned_diagram_pane_width_with_font(
        diagram,
        pane_height,
        min_width,
        super::super::mermaid::get_font_size(),
    )
}

pub(crate) fn estimate_pinned_diagram_pane_height(
    diagram: &info_widget::DiagramInfo,
    pane_width: u16,
    min_height: u16,
) -> u16 {
    const PANE_BORDER: u32 = 2;
    let inner_width = pane_width.saturating_sub(PANE_BORDER as u16).max(1) as u32;
    let (cell_w, cell_h) = super::super::mermaid::get_font_size().unwrap_or((8, 16));
    let cell_w = cell_w.max(1) as u32;
    let cell_h = cell_h.max(1) as u32;

    let image_w_cells = div_ceil_u32(diagram.width.max(1), cell_w);
    let image_h_cells = div_ceil_u32(diagram.height.max(1), cell_h);
    let fit_h_cells = if image_w_cells > inner_width {
        div_ceil_u32(image_h_cells.saturating_mul(inner_width), image_w_cells)
    } else {
        image_h_cells
    }
    .max(1);

    let pane_height = fit_h_cells.saturating_add(PANE_BORDER);
    pane_height.max(min_height as u32).min(u16::MAX as u32) as u16
}

pub(crate) fn vcenter_fitted_image_with_font(
    area: Rect,
    img_w_px: u32,
    img_h_px: u32,
    font_size: Option<(u16, u16)>,
) -> Rect {
    if area.width == 0 || area.height == 0 || img_w_px == 0 || img_h_px == 0 {
        return area;
    }
    let target_area = area;
    let (font_w, font_h) = match font_size {
        Some(fs) => (fs.0.max(1) as f64, fs.1.max(1) as f64),
        None => return target_area,
    };

    let area_w_px = target_area.width as f64 * font_w;
    let area_h_px = target_area.height as f64 * font_h;
    let scale = (area_w_px / img_w_px as f64).min(area_h_px / img_h_px as f64);

    let fitted_w_cells = ((img_w_px as f64 * scale) / font_w).ceil() as u16;
    let fitted_h_cells = ((img_h_px as f64 * scale) / font_h).ceil() as u16;
    let fitted_w_cells = fitted_w_cells.min(target_area.width);
    let fitted_h_cells = fitted_h_cells.min(target_area.height);

    let x_offset = (target_area.width - fitted_w_cells) / 2;
    let y_offset = (target_area.height - fitted_h_cells) / 2;
    Rect {
        x: target_area.x + x_offset,
        y: target_area.y + y_offset,
        width: fitted_w_cells,
        height: fitted_h_cells,
    }
}

pub(crate) fn is_diagram_poor_fit(
    diagram: &info_widget::DiagramInfo,
    area: Rect,
    position: crate::config::DiagramPanePosition,
) -> bool {
    if diagram.width == 0 || diagram.height == 0 || area.width < 5 || area.height < 3 {
        return false;
    }
    let (cell_w, cell_h) = super::super::mermaid::get_font_size().unwrap_or((8, 16));
    let cell_w = cell_w.max(1) as f64;
    let cell_h = cell_h.max(1) as f64;
    let inner_w = area.width.saturating_sub(2).max(1) as f64 * cell_w;
    let inner_h = area.height.saturating_sub(2).max(1) as f64 * cell_h;
    let img_w = diagram.width as f64;
    let img_h = diagram.height as f64;
    let aspect = img_w / img_h.max(1.0);
    let scale = (inner_w / img_w).min(inner_h / img_h);

    if scale < 0.3 {
        return true;
    }

    match position {
        crate::config::DiagramPanePosition::Side => {
            let used_w = img_w * scale;
            let used_h = img_h * scale;
            let utilization = (used_w * used_h) / (inner_w * inner_h);
            aspect > 2.0 && utilization < 0.35
        }
        crate::config::DiagramPanePosition::Top => {
            let used_w = img_w * scale;
            let used_h = img_h * scale;
            let utilization = (used_w * used_h) / (inner_w * inner_h);
            aspect < 0.5 && utilization < 0.35
        }
    }
}

pub(crate) fn diagram_view_uses_fit_mode(
    focused: bool,
    scroll_x: i32,
    scroll_y: i32,
    zoom_percent: u8,
) -> bool {
    !focused || (scroll_x == 0 && scroll_y == 0 && zoom_percent == 100)
}

#[expect(
    clippy::too_many_arguments,
    reason = "pinned diagram rendering needs layout, focus, scroll, zoom, pane placement, and animation state"
)]
pub(crate) fn draw_pinned_diagram(
    frame: &mut Frame,
    diagram: &info_widget::DiagramInfo,
    area: Rect,
    index: usize,
    total: usize,
    focused: bool,
    scroll_x: i32,
    scroll_y: i32,
    zoom_percent: u8,
    pane_position: crate::config::DiagramPanePosition,
    pane_animating: bool,
) {
    use ratatui::widgets::{Block, BorderType, Borders, Wrap};

    if area.width < 5 || area.height < 3 {
        return;
    }

    let border_style = super::right_rail_border_style(focused, accent_color());
    let mut title_parts = vec![Span::styled(" pinned ", Style::default().fg(tool_color()))];
    let fit_mode = diagram_view_uses_fit_mode(focused, scroll_x, scroll_y, zoom_percent);
    if total > 0 {
        title_parts.push(Span::styled(
            format!("{}/{}", index + 1, total),
            Style::default().fg(tool_color()),
        ));
    }
    let planned_mode =
        planned_pinned_diagram_mode_label(diagram, area, pane_position, fit_mode, zoom_percent);
    let mode_label = format!(" {planned_mode} ");
    title_parts.push(Span::styled(
        mode_label,
        Style::default().fg(if focused { accent_color() } else { dim_color() }),
    ));
    if focused || zoom_percent != 100 {
        title_parts.push(Span::styled(
            format!(" zoom {}%", zoom_percent),
            Style::default().fg(if focused { accent_color() } else { dim_color() }),
        ));
    }
    if total > 1 {
        title_parts.push(Span::styled(" Ctrl+←/→", Style::default().fg(dim_color())));
    }
    title_parts.push(Span::styled(
        " Ctrl+H/L focus",
        Style::default().fg(dim_color()),
    ));
    title_parts.push(Span::styled(
        " Alt+M side panel",
        Style::default().fg(dim_color()),
    ));

    let poor_fit = is_diagram_poor_fit(diagram, area, pane_position);
    if poor_fit {
        let hint = match pane_position {
            crate::config::DiagramPanePosition::Side => " Alt+T \u{21c4} top",
            crate::config::DiagramPanePosition::Top => " Alt+T \u{21c4} side",
        };
        title_parts.push(Span::styled(
            hint,
            Style::default()
                .fg(accent_color())
                .add_modifier(ratatui::style::Modifier::BOLD),
        ));
    }
    if focused {
        title_parts.push(Span::styled(
            " o open",
            Style::default().fg(if poor_fit {
                accent_color()
            } else {
                dim_color()
            }),
        ));
    } else if poor_fit {
        title_parts.push(Span::styled(
            " focus+o open",
            Style::default()
                .fg(accent_color())
                .add_modifier(ratatui::style::Modifier::BOLD),
        ));
    }

    let inner = if pane_position == crate::config::DiagramPanePosition::Side {
        let Some(content_area) =
            super::draw_right_rail_chrome(frame, area, Line::from(title_parts), border_style)
        else {
            return;
        };
        content_area
    } else {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(border_style)
            .title(Line::from(title_parts));

        let inner = block.inner(area);
        frame.render_widget(block, area);
        inner
    };

    if inner.width > 0 && inner.height > 0 {
        let debug_snapshot = build_pinned_diagram_live_snapshot(
            diagram,
            PinnedDiagramSnapshotLayout {
                area,
                inner,
                index,
                total,
            },
            PinnedDiagramSnapshotView {
                focused,
                scroll_x,
                scroll_y,
                zoom_percent,
            },
        );
        with_pinned_diagram_debug_mut(|debug| {
            debug.live_snapshot = Some(debug_snapshot);
        });

        let mut rendered = 0u16;
        let mermaid_aspect_ratio = content_area_preferred_aspect_ratio(inner);
        super::super::mermaid::with_preferred_aspect_ratio(mermaid_aspect_ratio, || {
            if pane_animating {
                clear_area(frame, inner);
                let placeholder =
                    super::super::mermaid::diagram_placeholder_lines(diagram.width, diagram.height);
                let paragraph = Paragraph::new(placeholder).wrap(Wrap { trim: true });
                frame.render_widget(paragraph, inner);
                rendered = inner.height;
            } else if super::super::mermaid::protocol_type().is_some() {
                if focused && !fit_mode {
                    rendered = super::super::mermaid::render_image_widget_viewport(
                        diagram.hash,
                        inner,
                        frame.buffer_mut(),
                        scroll_x,
                        scroll_y,
                        zoom_percent,
                        false,
                    );
                } else {
                    match plan_pinned_diagram_fit(inner, diagram.width, diagram.height) {
                        PinnedDiagramFitRenderPlan::Contain { area: render_area } => {
                            rendered = super::super::mermaid::render_image_widget_scale(
                                diagram.hash,
                                render_area,
                                frame.buffer_mut(),
                                false,
                            );
                        }
                        PinnedDiagramFitRenderPlan::FillViewport {
                            zoom_percent,
                            scroll_x,
                            scroll_y,
                        } => {
                            rendered = super::super::mermaid::render_image_widget_viewport_precise(
                                diagram.hash,
                                inner,
                                frame.buffer_mut(),
                                scroll_x,
                                scroll_y,
                                zoom_percent,
                                false,
                            );
                        }
                    }
                }
            }
        });

        if rendered > 0 && super::super::mermaid::is_video_export_mode() {
            super::super::mermaid::write_video_export_marker(
                diagram.hash,
                inner,
                frame.buffer_mut(),
            );
        } else if rendered == 0 {
            clear_area(frame, inner);
            let placeholder =
                super::super::mermaid::diagram_placeholder_lines(diagram.width, diagram.height);
            let paragraph = Paragraph::new(placeholder).wrap(Wrap { trim: true });
            frame.render_widget(paragraph, inner);
        }
    } else {
        clear_pinned_diagram_debug_snapshot();
    }
}
