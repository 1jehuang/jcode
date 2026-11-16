use ratatui::prelude::*;

pub(crate) const SIDE_PANEL_DISMISSED_HINT: &str = "Side panel hidden. Alt+M brings it back";

thread_local! {
    static RESTORE_AREA: std::cell::Cell<Option<Rect>> = const { std::cell::Cell::new(None) };
}

pub(crate) fn side_panel_restore_area() -> Option<Rect> {
    RESTORE_AREA.with(|area| area.get())
}

// Share the existing status-notice lifetime rather than leaving a permanent
// header control after dismissal. Record the rendered bounds so mouse handling
// cannot activate a clipped button on narrow terminals.
pub(crate) fn draw_side_panel_restore(frame: &mut ratatui::Frame, app: &dyn crate::tui::TuiState) {
    RESTORE_AREA.with(|area| area.set(None));
    let area = frame.area();
    let label = "[show side panel]";
    if app.status_notice().as_deref() != Some(SIDE_PANEL_DISMISSED_HINT)
        || area.width < label.len() as u16
        || area.height == 0
    {
        return;
    }
    let target = Rect::new(
        area.right() - label.len() as u16,
        area.y,
        label.len() as u16,
        1,
    );
    frame.render_widget(
        ratatui::widgets::Paragraph::new(label)
            .style(Style::default().fg(super::theme_support::accent_color())),
        target,
    );
    RESTORE_AREA.with(|area| area.set(Some(target)));
}

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
