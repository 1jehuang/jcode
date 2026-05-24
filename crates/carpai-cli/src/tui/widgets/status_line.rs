//! Status line widget

use ratatui::{Frame, layout::Rect, widgets::{Block, Borders, Paragraph}};
use crate::tui::theme::Theme;

pub fn render_status(f: &mut Frame, area: Rect, model: &str, mode: &str, theme: &Theme) {
    let status = format!(" {} | Mode: {} | Model: {} ", "CarpAI", mode, model);
    let para = Paragraph::new(status)
        .style(theme.title_style)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(para, area);
}
