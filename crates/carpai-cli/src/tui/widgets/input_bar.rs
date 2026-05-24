//! Input bar widget

use ratatui::{Frame, layout::Rect, widgets::{Block, Borders, Paragraph}};
use crate::tui::theme::Theme;

pub fn render_input(f: &mut Frame, area: Rect, input: &str, _theme: &Theme) {
    let input_text = if input.is_empty() { "Type a message...".to_string() } else { input.to_string() };
    let para = Paragraph::new(input_text)
        .block(Block::default().borders(Borders::ALL).title(" Input "));
    f.render_widget(para, area);
}
