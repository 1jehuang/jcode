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

fn utilization_percent(used: u32, total: u32) -> f64 {
    if total == 0 {
        0.0
    } else {
        (used as f64 * 100.0) / total as f64
    }
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
    let visible_rect = if fit_mode {
        vcenter_fitted_image(inner, diagram.width, diagram.height)
    } else {
        inner
    };
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
    let render_mode = pinned_diagram_render_mode_label(fit_mode, zoom_percent);

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

pub(crate) fn readable_image_target_area(area: Rect) -> Rect {
    if area.width <= 6 || area.height <= 4 {
        return area;
    }

    let horizontal_padding = (area.width / 12).clamp(1, 3);
    let vertical_padding = (area.height / 14).clamp(1, 2);

    let max_horizontal_padding = area.width.saturating_sub(1) / 2;
    let max_vertical_padding = area.height.saturating_sub(1) / 2;
    let horizontal_padding = horizontal_padding.min(max_horizontal_padding);
    let vertical_padding = vertical_padding.min(max_vertical_padding);

    Rect {
        x: area.x + horizontal_padding,
        y: area.y + vertical_padding,
        width: area
            .width
            .saturating_sub(horizontal_padding.saturating_mul(2))
            .max(1),
        height: area
            .height
            .saturating_sub(vertical_padding.saturating_mul(2))
            .max(1),
    }
}

#[cfg(test)]
mod tests {
    use super::diagram_view_uses_fit_mode;

    #[test]
    fn diagram_view_uses_fit_mode_when_unfocused_or_reset() {
        assert!(diagram_view_uses_fit_mode(false, 0, 0, 100));
        assert!(diagram_view_uses_fit_mode(true, 0, 0, 100));
        assert!(!diagram_view_uses_fit_mode(true, 1, 0, 100));
        assert!(!diagram_view_uses_fit_mode(true, 0, 1, 100));
        assert!(!diagram_view_uses_fit_mode(true, 0, 0, 90));
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

pub(crate) fn vcenter_fitted_image(area: Rect, img_w_px: u32, img_h_px: u32) -> Rect {
    vcenter_fitted_image_with_font(
        area,
        img_w_px,
        img_h_px,
        super::super::mermaid::get_font_size(),
    )
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
    let target_area = readable_image_target_area(area);
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
    let mode_label = if fit_mode { " fit " } else { " pan " };
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
                let render_area = vcenter_fitted_image(inner, diagram.width, diagram.height);
                rendered = super::super::mermaid::render_image_widget_scale(
                    diagram.hash,
                    render_area,
                    frame.buffer_mut(),
                    false,
                );
            }
        }

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
