use ratatui::prelude::Rect;

pub(super) fn clipped_band_area(
    render_area: Rect,
    text_render_area: Rect,
    prompt_preview_lines: u16,
    band_lines: usize,
) -> Rect {
    Rect {
        x: text_render_area.x,
        y: render_area.y.saturating_add(prompt_preview_lines),
        width: text_render_area.width,
        height: band_lines.min(u16::MAX as usize) as u16,
    }
    .intersection(text_render_area)
}
