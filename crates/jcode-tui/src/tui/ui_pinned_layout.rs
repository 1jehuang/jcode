use super::{
    FitImageRenderPlan, SIDE_PANEL_INLINE_IMAGE_MIN_ROWS, SIDE_PANEL_INLINE_IMAGE_MIN_ZOOM_PERCENT,
    SidePanelImageLayout, SidePanelImageRenderMode,
};
use crate::tui::mermaid;
use ratatui::prelude::Rect;

const SIDE_PANEL_INLINE_IMAGE_TARGET_UTILIZATION_PERCENT: u16 = 85;
const SIDE_PANEL_INLINE_IMAGE_MAX_AUTO_FILL_ZOOM_PERCENT: u16 = 1000;

pub(super) fn estimate_side_panel_image_layout(
    hash: u64,
    inner: Rect,
    lines_before_image: usize,
    has_following_content: bool,
) -> SidePanelImageLayout {
    let Some((_, width, height)) = mermaid::get_cached_png(hash) else {
        return SidePanelImageLayout {
            rows: clamp_side_panel_image_rows(
                inner.height.clamp(SIDE_PANEL_INLINE_IMAGE_MIN_ROWS, 12),
                inner.height,
                lines_before_image,
                has_following_content,
            ),
            render_mode: SidePanelImageRenderMode::Fit,
        };
    };

    estimate_side_panel_image_layout_with_font(
        width,
        height,
        inner.width,
        inner.height,
        lines_before_image,
        has_following_content,
        mermaid::get_font_size(),
    )
}

pub(super) fn estimate_side_panel_image_layout_with_font(
    width: u32,
    height: u32,
    available_width: u16,
    inner_height: u16,
    lines_before_image: usize,
    has_following_content: bool,
    font_size: Option<(u16, u16)>,
) -> SidePanelImageLayout {
    if width == 0 || height == 0 || available_width == 0 {
        return SidePanelImageLayout {
            rows: clamp_side_panel_image_rows(
                SIDE_PANEL_INLINE_IMAGE_MIN_ROWS,
                inner_height,
                lines_before_image,
                has_following_content,
            ),
            render_mode: SidePanelImageRenderMode::Fit,
        };
    }

    let (cell_w, cell_h) = font_size.unwrap_or((8, 16));
    let cell_w = cell_w.max(1) as u32;
    let cell_h = cell_h.max(1) as u32;
    let image_h_cells = super::diagram_pane::div_ceil_u32(height.max(1), cell_h).max(1);
    let available_width = available_width.max(1) as u32;
    let inner_height = inner_height.max(1);
    let fit_area = Rect::new(0, 0, available_width as u16, inner_height);

    let fit_zoom = fit_zoom_percent_for_area(
        fit_area,
        width,
        height,
        Some((cell_w as u16, cell_h as u16)),
    ) as u16;
    let fit_rect = fit_image_area_with_font(
        fit_area,
        width,
        height,
        Some((cell_w as u16, cell_h as u16)),
        true,
        false,
    );
    let width_fill_zoom = axis_fill_zoom_percent(available_width, width, cell_w);
    let height_fill_zoom = axis_fill_zoom_percent(inner_height as u32, height, cell_h);
    let preferred_viewport_zoom = width_fill_zoom.max(height_fill_zoom).clamp(
        SIDE_PANEL_INLINE_IMAGE_MIN_ZOOM_PERCENT,
        SIDE_PANEL_INLINE_IMAGE_MAX_AUTO_FILL_ZOOM_PERCENT,
    );
    let fit_underutilized = rect_utilization_percent(fit_rect.width, fit_area.width)
        < SIDE_PANEL_INLINE_IMAGE_TARGET_UTILIZATION_PERCENT
        || rect_utilization_percent(fit_rect.height, fit_area.height)
            < SIDE_PANEL_INLINE_IMAGE_TARGET_UTILIZATION_PERCENT
        || area_utilization_percent(fit_rect, fit_area)
            < SIDE_PANEL_INLINE_IMAGE_TARGET_UTILIZATION_PERCENT;

    if fit_underutilized && preferred_viewport_zoom > fit_zoom {
        let zoom_percent = preferred_viewport_zoom;
        return SidePanelImageLayout {
            rows: scaled_image_rows(image_h_cells, zoom_percent)
                .max(SIDE_PANEL_INLINE_IMAGE_MIN_ROWS),
            render_mode: SidePanelImageRenderMode::ScrollableViewport { zoom_percent },
        };
    }

    let needed = scaled_image_rows(image_h_cells, fit_zoom);
    SidePanelImageLayout {
        rows: clamp_side_panel_image_rows(
            needed
                .max(SIDE_PANEL_INLINE_IMAGE_MIN_ROWS)
                .min(inner_height.max(SIDE_PANEL_INLINE_IMAGE_MIN_ROWS)),
            inner_height,
            lines_before_image,
            has_following_content,
        ),
        render_mode: SidePanelImageRenderMode::Fit,
    }
}

fn axis_fill_zoom_percent(available_cells: u32, image_px: u32, cell_px: u32) -> u16 {
    if available_cells == 0 || image_px == 0 || cell_px == 0 {
        return 100;
    }

    available_cells
        .saturating_mul(cell_px)
        .saturating_mul(100)
        .checked_div(image_px.max(1))
        .unwrap_or(100)
        .clamp(1, SIDE_PANEL_INLINE_IMAGE_MAX_AUTO_FILL_ZOOM_PERCENT as u32) as u16
}

fn rect_utilization_percent(used: u16, total: u16) -> u16 {
    if total == 0 {
        return 0;
    }
    ((used as u32).saturating_mul(100) / total as u32) as u16
}

fn area_utilization_percent(used: Rect, total: Rect) -> u16 {
    let used_area = (used.width as u32).saturating_mul(used.height as u32);
    let total_area = (total.width as u32).saturating_mul(total.height as u32);
    if total_area == 0 {
        return 0;
    }
    (used_area.saturating_mul(100) / total_area) as u16
}

pub(super) fn scaled_image_rows(image_h_cells: u32, zoom_percent: u16) -> u16 {
    if image_h_cells == 0 || zoom_percent == 0 {
        return 0;
    }

    super::diagram_pane::div_ceil_u32(image_h_cells.saturating_mul(zoom_percent as u32), 100)
        .min(u16::MAX as u32) as u16
}

#[cfg(test)]
pub(super) fn estimate_side_panel_image_rows_with_font(
    width: u32,
    height: u32,
    available_width: u16,
    font_size: Option<(u16, u16)>,
) -> u16 {
    if width == 0 || height == 0 || available_width == 0 {
        return 0;
    }

    let (cell_w, cell_h) = font_size.unwrap_or((8, 16));
    let cell_w = cell_w.max(1) as u32;
    let cell_h = cell_h.max(1) as u32;

    let image_w_cells = super::diagram_pane::div_ceil_u32(width.max(1), cell_w).max(1);
    let image_h_cells = super::diagram_pane::div_ceil_u32(height.max(1), cell_h).max(1);
    let available_width = available_width.max(1) as u32;

    let fitted_h_cells = if image_w_cells > available_width {
        super::diagram_pane::div_ceil_u32(
            image_h_cells.saturating_mul(available_width),
            image_w_cells,
        )
    } else {
        image_h_cells
    }
    .max(1);

    fitted_h_cells.min(u16::MAX as u32) as u16
}

pub(super) fn side_panel_viewport_scroll_x(
    img_w_px: u32,
    area_width: u16,
    zoom_percent: u16,
    centered: bool,
    font_size: Option<(u16, u16)>,
    pan_x_cells: i32,
) -> i32 {
    if img_w_px == 0 || area_width == 0 || zoom_percent == 0 {
        return 0;
    }

    let (font_w, _) = font_size.unwrap_or((8, 16));
    let font_w = font_w.max(1) as u32;
    let zoom = zoom_percent as u32;
    let view_w_px = (area_width as u32)
        .saturating_mul(font_w)
        .saturating_mul(100)
        / zoom;
    let max_scroll_x_px = img_w_px.saturating_sub(view_w_px);
    if max_scroll_x_px == 0 {
        return 0;
    }

    let cell_w_px = super::diagram_pane::div_ceil_u32(font_w.saturating_mul(100), zoom).max(1);

    let base_cells = if centered {
        ((max_scroll_x_px / 2) / cell_w_px).min(i32::MAX as u32) as i32
    } else {
        0
    };
    let max_cells = (max_scroll_x_px / cell_w_px).min(i32::MAX as u32) as i32;
    base_cells.saturating_add(pan_x_cells).clamp(0, max_cells)
}

fn fit_zoom_percent_for_area(
    area: Rect,
    img_w_px: u32,
    img_h_px: u32,
    font_size: Option<(u16, u16)>,
) -> u8 {
    if area.width == 0 || area.height == 0 || img_w_px == 0 || img_h_px == 0 {
        return 100;
    }

    let (font_w, font_h) = font_size.unwrap_or((8, 16));
    let font_w = font_w.max(1) as u32;
    let font_h = font_h.max(1) as u32;
    let zoom_w = area.width as u32 * font_w * 100 / img_w_px.max(1);
    let zoom_h = area.height as u32 * font_h * 100 / img_h_px.max(1);
    zoom_w.min(zoom_h).clamp(1, 200) as u8
}

pub(super) fn plan_fit_image_render(
    viewport_area: Rect,
    viewport_start: usize,
    image_start: usize,
    reserved_rows: u16,
    img_w_px: u32,
    img_h_px: u32,
    centered: bool,
) -> Option<FitImageRenderPlan> {
    if viewport_area.width == 0
        || viewport_area.height == 0
        || reserved_rows == 0
        || img_w_px == 0
        || img_h_px == 0
    {
        return None;
    }

    let reserved_template = Rect {
        x: viewport_area.x,
        y: 0,
        width: viewport_area.width,
        height: reserved_rows,
    };
    let fitted = fit_side_panel_image_area(reserved_template, img_w_px, img_h_px, centered);
    if fitted.width == 0 || fitted.height == 0 {
        return None;
    }

    let reserved_top = viewport_area.y as i32 + image_start as i32 - viewport_start as i32;
    let fitted_top = reserved_top + fitted.y as i32;
    let fitted_bottom = fitted_top + fitted.height as i32;
    let viewport_top = viewport_area.y as i32;
    let viewport_bottom = viewport_top + viewport_area.height as i32;

    if fitted_bottom <= viewport_top || fitted_top >= viewport_bottom {
        return None;
    }

    let visible_top = fitted_top.max(viewport_top);
    let visible_bottom = fitted_bottom.min(viewport_bottom);
    let visible_height = (visible_bottom - visible_top) as u16;
    if visible_height == 0 {
        return None;
    }

    if visible_height == fitted.height && fitted_top >= 0 {
        return Some(FitImageRenderPlan::Full {
            area: Rect {
                x: fitted.x,
                y: fitted_top as u16,
                width: fitted.width,
                height: fitted.height,
            },
        });
    }

    Some(FitImageRenderPlan::ClippedViewport {
        area: Rect {
            x: fitted.x,
            y: visible_top.max(0) as u16,
            width: fitted.width,
            height: visible_height,
        },
        scroll_y: visible_top.saturating_sub(fitted_top),
        zoom_percent: fit_zoom_percent_for_area(
            fitted,
            img_w_px,
            img_h_px,
            mermaid::get_font_size(),
        ),
    })
}

pub(super) fn fit_side_panel_image_area(
    area: Rect,
    img_w_px: u32,
    img_h_px: u32,
    centered: bool,
) -> Rect {
    fit_image_area_with_font(
        area,
        img_w_px,
        img_h_px,
        mermaid::get_font_size(),
        centered,
        false,
    )
}

pub(super) fn fit_image_area_with_font(
    area: Rect,
    img_w_px: u32,
    img_h_px: u32,
    font_size: Option<(u16, u16)>,
    centered: bool,
    vcenter: bool,
) -> Rect {
    if area.width == 0 || area.height == 0 || img_w_px == 0 || img_h_px == 0 {
        return area;
    }

    let (font_w, font_h) = match font_size {
        Some(fs) => (fs.0.max(1) as f64, fs.1.max(1) as f64),
        None => return area,
    };

    let area_w_px = area.width as f64 * font_w;
    let area_h_px = area.height as f64 * font_h;
    let scale = (area_w_px / img_w_px as f64).min(area_h_px / img_h_px as f64);
    if !scale.is_finite() || scale <= 0.0 {
        return area;
    }

    let fitted_w_cells = ((img_w_px as f64 * scale) / font_w)
        .ceil()
        .max(1.0)
        .min(area.width as f64) as u16;
    let fitted_h_cells = ((img_h_px as f64 * scale) / font_h)
        .ceil()
        .max(1.0)
        .min(area.height as f64) as u16;

    let x_offset = if centered {
        area.width.saturating_sub(fitted_w_cells) / 2
    } else {
        0
    };
    let y_offset = if vcenter {
        area.height.saturating_sub(fitted_h_cells) / 2
    } else {
        0
    };

    Rect {
        x: area.x + x_offset,
        y: area.y + y_offset,
        width: fitted_w_cells,
        height: fitted_h_cells,
    }
}

pub(super) fn clamp_side_panel_image_rows(
    estimated_rows: u16,
    inner_height: u16,
    _lines_before_image: usize,
    has_following_content: bool,
) -> u16 {
    let min_rows = SIDE_PANEL_INLINE_IMAGE_MIN_ROWS.min(inner_height.max(1));
    let max_rows = inner_height.max(min_rows);
    let estimated_rows = estimated_rows.max(min_rows).min(max_rows);

    if !has_following_content {
        return estimated_rows;
    }

    estimated_rows.min(max_rows.saturating_sub(1).max(min_rows))
}
