//! Help overlay widget

use ratatui::{Frame, layout::Rect, widgets::{Block, Borders, Clear, Paragraph}};
use crate::tui::theme::Theme;

pub fn render_help(f: &mut Frame, area: Rect, theme: &Theme) {
    let help_text = r#"
  CarpAI TUI — Keyboard Shortcuts
  -------------------------------
  Enter     Send message
  Esc       Cancel / Exit (Ctrl-C)
  Ctrl-F    Toggle file tree
  ?         Toggle this help
  Tab       Cycle focus
  "#;
    f.render_widget(Clear, area);
    let para = Paragraph::new(help_text)
        .style(theme.title_style)
        .block(Block::default().borders(Borders::ALL).title(" Help "));
    f.render_widget(para, area);
}
