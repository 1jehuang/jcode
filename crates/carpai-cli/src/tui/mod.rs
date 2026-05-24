//! TUI — Pure rendering layer (ratatui)
//!
//! **Critical rule**: This module contains ZERO agent business logic.
//! All logic is delegated to `agent_bridge::AgentBridge`.

pub mod app;
pub mod event;
pub mod handler;
pub mod theme;
pub mod widgets;

use crate::config::CliConfig;
use crate::agent_bridge::AgentBridge;
use anyhow::Result;

/// Run the TUI application
pub async fn run(config: CliConfig) -> Result<()> {
    // Build agent context with all Local* implementations
    let ctx = carpai_core::build_local_agent_context(&config.core);
    let bridge = AgentBridge::new_local(ctx);

    let mut app = app::App::new(config, bridge);

    // Initialize terminal
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen, crossterm::cursor::Hide)?;

    use ratatui::{Terminal, backend::CrosstermBackend};
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    // Main loop
    loop {
        terminal.draw(|f| {
            render_app(f, &mut app);
        })?;

        if app.should_quit { break; }

        if crossterm::event::poll(std::time::Duration::from_millis(16))? {
            match crossterm::event::read()? {
                crossterm::event::Event::Key(key) => {
                    app.handle_event(event::Event::Key(key)).await;
                }
                crossterm::event::Event::Resize(_, _) => {}
                _ => {}
            }
        } else {
            app.handle_event(event::Event::Tick).await;
        }
    }

    // Restore terminal
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::cursor::Show
    )?;

    Ok(())
}

fn render_app(f: &mut ratatui::Frame, app: &mut app::App) {
    let theme = theme::Theme::default();

    // Determine layout — with or without file tree
    let (main_area, file_tree_area) = if app.file_tree.visible {
        let horizontal = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Horizontal)
            .constraints([
                ratatui::layout::Constraint::Percentage(25),
                ratatui::layout::Constraint::Percentage(75),
            ])
            .split(f.area());
        (horizontal[1], Some(horizontal[0]))
    } else {
        (f.area(), None)
    };

    // Render file tree on the left if visible
    if let Some(ft_area) = file_tree_area {
        widgets::file_tree::render_file_tree(f, ft_area, &mut app.file_tree, &theme);
    }

    // Split main area into vertical sections
    let chunks = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            ratatui::layout::Constraint::Min(10),   // Chat area
            ratatui::layout::Constraint::Length(3),   // Input bar
            ratatui::layout::Constraint::Length(1),   // Status line
        ])
        .split(main_area);

    widgets::chat_view::render_chat(f, chunks[0], &app.messages, &mut Default::default(), &theme);
    widgets::input_bar::render_input(f, chunks[1], &app.input, &theme);
    widgets::status_line::render_status(f, chunks[2], "local", "cli", &theme);

    // Render help overlay on top if active
    if app.show_help {
        let popup_area = centered_rect(60, 40, main_area);
        widgets::help_overlay::render_help(f, popup_area, &theme);
    }
}

/// Calculate a centered rectangle for popup/overlay
fn centered_rect(percent_x: u16, percent_y: u16, area: ratatui::layout::Rect) -> ratatui::layout::Rect {
    let popup_layout = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            ratatui::layout::Constraint::Percentage((100 - percent_y) / 2),
            ratatui::layout::Constraint::Percentage(percent_y),
            ratatui::layout::Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Horizontal)
        .constraints([
            ratatui::layout::Constraint::Percentage((100 - percent_x) / 2),
            ratatui::layout::Constraint::Percentage(percent_x),
            ratatui::layout::Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
