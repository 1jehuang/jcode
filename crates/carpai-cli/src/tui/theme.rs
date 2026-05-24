//! TUI Color theme definitions

use ratatui::style::{Color, Modifier, Style};

pub struct Theme {
    pub primary: Color,
    pub secondary: Color,
    pub accent: Color,
    pub error: Color,
    pub warning: Color,
    pub success: Color,
    pub text: Color,
    pub text_dim: Color,
    pub border: Style,
    pub title_style: Style,
    pub user_msg_style: Style,
    pub assistant_msg_style: Style,
    pub error_style: Style,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            primary: Color::Blue,
            secondary: Color::DarkGray,
            accent: Color::Cyan,
            error: Color::Red,
            warning: Color::Yellow,
            success: Color::Green,
            text: Color::White,
            text_dim: Color::DarkGray,
            border: Style::default().fg(Color::DarkGray),
            title_style: Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            user_msg_style: Style::default().fg(Color::Blue),
            assistant_msg_style: Style::default().fg(Color::Green),
            error_style: Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        }
    }
}
