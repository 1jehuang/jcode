use super::*;
use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize)]
pub struct SidePanelDebugStats {
    pub markdown_cache_hits: u64,
    pub markdown_cache_misses: u64,
    pub render_cache_hits: u64,
    pub render_cache_misses: u64,
    pub markdown_cache_entries: usize,
    pub render_cache_entries: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SidePanelVisibleMermaidDebug {
    pub image_index: usize,
    pub hash: String,
    pub reserved_rows: u16,
    pub visible_rows: u16,
    pub render_mode: String,
    pub rendered_png_width_px: u32,
    pub rendered_png_height_px: u32,
    pub layout_fit: SidePanelMermaidProbeRect,
    pub widget_fit: SidePanelMermaidProbeRect,
    pub visible_widget: SidePanelMermaidProbeRect,
    pub log: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SidePanelLiveDebugSnapshot {
    pub page_id: String,
    pub page_title: String,
    pub pane_width_cells: u16,
    pub pane_height_cells: u16,
    pub total_lines: usize,
    pub scroll_offset: usize,
    pub max_scroll: usize,
    pub total_mermaids: usize,
    pub visible_mermaids: Vec<SidePanelVisibleMermaidDebug>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SidePanelMermaidProbeRect {
    pub width_cells: u16,
    pub height_cells: u16,
    pub width_utilization_percent: f64,
    pub height_utilization_percent: f64,
    pub area_utilization_percent: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SidePanelMermaidProbe {
    pub pane_width_cells: u16,
    pub pane_height_cells: u16,
    pub font_width_px: u16,
    pub font_height_px: u16,
    pub rendered_png_width_px: u32,
    pub rendered_png_height_px: u32,
    pub estimated_rows: u16,
    pub render_mode: String,
    pub layout_fit: SidePanelMermaidProbeRect,
    pub widget_fit: SidePanelMermaidProbeRect,
}

fn utilization_percent(used: u32, total: u32) -> f64 {
    if total == 0 {
        0.0
    } else {
        (used as f64 * 100.0) / total as f64
    }
}

pub(super) fn probe_rect(
    rect: Rect,
    pane_width_cells: u16,
    pane_height_cells: u16,
) -> SidePanelMermaidProbeRect {
    SidePanelMermaidProbeRect {
        width_cells: rect.width,
        height_cells: rect.height,
        width_utilization_percent: utilization_percent(rect.width as u32, pane_width_cells as u32),
        height_utilization_percent: utilization_percent(
            rect.height as u32,
            pane_height_cells as u32,
        ),
        area_utilization_percent: utilization_percent(
            rect.width as u32 * rect.height as u32,
            pane_width_cells as u32 * pane_height_cells as u32,
        ),
    }
}

fn side_panel_render_mode_label(render_mode: SidePanelImageRenderMode) -> String {
    match render_mode {
        SidePanelImageRenderMode::Fit => "fit".to_string(),
        SidePanelImageRenderMode::ScrollableViewport { zoom_percent } => {
            format!("scrollable-viewport@{zoom_percent}%")
        }
    }
}

fn widget_fit_rect_for_layout(
    layout: SidePanelImageLayout,
    pane_width_cells: u16,
    pane_height_cells: u16,
    layout_fit: Rect,
) -> Rect {
    match layout.render_mode {
        SidePanelImageRenderMode::Fit => layout_fit,
        SidePanelImageRenderMode::ScrollableViewport { .. } => Rect::new(
            0,
            0,
            pane_width_cells,
            pane_height_cells.min(layout.rows.max(1)),
        ),
    }
}

pub(super) fn build_side_panel_mermaid_probe_from_image(
    width: u32,
    height: u32,
    pane_width_cells: u16,
    pane_height_cells: u16,
    font_size_px: (u16, u16),
    centered: bool,
) -> SidePanelMermaidProbe {
    let layout = estimate_side_panel_image_layout_with_font(
        width,
        height,
        pane_width_cells,
        pane_height_cells,
        0,
        false,
        Some(font_size_px),
    );
    let layout_fit = fit_image_area_with_font(
        Rect::new(0, 0, pane_width_cells, pane_height_cells.max(1)),
        width,
        height,
        Some(font_size_px),
        centered,
        false,
    );
    let widget_fit =
        widget_fit_rect_for_layout(layout, pane_width_cells, pane_height_cells, layout_fit);

    SidePanelMermaidProbe {
        pane_width_cells,
        pane_height_cells,
        font_width_px: font_size_px.0,
        font_height_px: font_size_px.1,
        rendered_png_width_px: width,
        rendered_png_height_px: height,
        estimated_rows: layout.rows,
        render_mode: side_panel_render_mode_label(layout.render_mode),
        layout_fit: probe_rect(layout_fit, pane_width_cells, pane_height_cells),
        widget_fit: probe_rect(widget_fit, pane_width_cells, pane_height_cells),
    }
}

pub fn debug_probe_side_panel_mermaid(
    mermaid_source: &str,
    pane_width_cells: u16,
    pane_height_cells: u16,
    font_size_px: Option<(u16, u16)>,
    centered: bool,
) -> anyhow::Result<SidePanelMermaidProbe> {
    let font_size_px = font_size_px.unwrap_or((8, 16));
    let render = mermaid::render_mermaid_untracked(mermaid_source, Some(pane_width_cells));
    let mermaid::RenderResult::Image { width, height, .. } = render else {
        let mermaid::RenderResult::Error(error) = render else {
            unreachable!("non-image mermaid render result")
        };
        anyhow::bail!(error);
    };

    Ok(build_side_panel_mermaid_probe_from_image(
        width,
        height,
        pane_width_cells,
        pane_height_cells,
        font_size_px,
        centered,
    ))
}
