use ratatui::prelude::*;

pub(crate) use jcode_tui_render::chrome::{
    align_if_unset, centered_content_block_width, left_aligned_content_inset,
    left_pad_lines_to_block_width,
};
pub(super) use jcode_tui_render::chrome::{clear_area, draw_right_rail_chrome};

pub(super) fn right_rail_border_style(focused: bool, focus_color: Color) -> Style {
    jcode_tui_render::chrome::right_rail_border_style(
        focused,
        focus_color,
        super::theme_support::dim_color(),
    )
}

/// Keep the visible dismiss label and its mouse target identical in split and
/// fullscreen layouts. Match the minimum size of the side-panel renderer.
pub(crate) fn side_panel_dismiss_area(area: Rect) -> Option<Rect> {
    (area.width >= 10 && area.height >= 3)
        .then(|| Rect::new(area.right().saturating_sub(8), area.y, 8, 1))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_side_panel_markdown(
    frame: &mut ratatui::Frame,
    area: Rect,
    app: &dyn crate::tui::TuiState,
    snapshot: &crate::side_panel::SidePanelSnapshot,
    scroll: usize,
    focused: bool,
    centered: bool,
) {
    super::pinned_ui::draw_side_panel_markdown(
        frame, area, app, snapshot, scroll, focused, centered,
    );
    if let Some(close_area) = side_panel_dismiss_area(area) {
        frame.render_widget(
            ratatui::widgets::Paragraph::new(" [close]")
                .style(Style::default().fg(super::theme_support::accent_color())),
            close_area,
        );
    }
}
