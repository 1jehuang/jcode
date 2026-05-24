//! Chat message list widget

use ratatui::{Frame, layout::Rect, widgets::{Block, Borders, List, ListItem}};
use crate::tui::{app::UIMessage, theme::Theme};

pub fn render_chat(f: &mut Frame, area: Rect, messages: &[UIMessage], _state: &mut (), theme: &Theme) {
    let items: Vec<ListItem> = messages.iter().map(|m| match m {
        UIMessage::User(t) => ListItem::new(format!("> {}", t)).style(theme.user_msg_style),
        UIMessage::Assistant(t) => ListItem::new(format!("  {}", t)).style(theme.assistant_msg_style),
        UIMessage::ToolCall { name, params } => ListItem::new(format!("  \u{1f527} {}({})", name, params)),
        UIMessage::ToolResult { name, result } => ListItem::new(format!("  \u{2713} {}: {}", name, result)),
        UIMessage::System(t) => ListItem::new(format!("  [{}] {}", "SYS", t)).style(theme.text_dim),
        UIMessage::Error(e) => ListItem::new(format!("  \u{2717} {}", e)).style(theme.error_style),
    }).collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Chat "));

    f.render_widget(list, area);
}
