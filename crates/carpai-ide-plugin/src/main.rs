//! CarpAI IDE Plugin - Native Rust implementation
//!
//! High-performance IDE integration using TUI + gRPC

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Frame, Terminal,
};
use std::io;
use tracing_subscriber;

mod grpc_client;
mod chat_state;
mod lsp_integration;

use grpc_client::CarpAiGrpcClient;
use chat_state::{ChatMessage, ChatState, Role};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    // Initialize gRPC client
    let grpc_addr = std::env::var("CARPAI_GRPC_ADDR")
        .unwrap_or_else(|_| "http://[::1]:50051".to_string());

    tracing::info!("Connecting to CarpAI server at {}", grpc_addr);
    let mut grpc_client = CarpAiGrpcClient::connect(&grpc_addr).await?;
    tracing::info!("Connected to CarpAI server");

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Initialize chat state
    let mut chat_state = ChatState::new();

    // Main loop
    let result = run_app(&mut terminal, &mut chat_state, &mut grpc_client).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = result {
        eprintln!("Error: {:?}", err);
        std::process::exit(1);
    }

    Ok(())
}

async fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    chat_state: &mut ChatState,
    grpc_client: &mut CarpAiGrpcClient,
) -> Result<()> {
    loop {
        terminal.draw(|f| ui(f, chat_state))?;

        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                match key.code {
                    KeyCode::Char('q') => return Ok(()),
                    KeyCode::Enter => {
                        if !chat_state.input.is_empty() {
                            // Send message
                            let user_msg = chat_state.input.clone();
                            chat_state.add_message(Role::User, user_msg.clone());
                            chat_state.input.clear();

                            // Get AI response via gRPC
                            chat_state.is_loading = true;
                            match grpc_client.chat(&user_msg).await {
                                Ok(response) => {
                                    chat_state.add_message(Role::Assistant, response.content);
                                }
                                Err(e) => {
                                    chat_state.add_message(
                                        Role::System,
                                        format!("Error: {}", e),
                                    );
                                }
                            }
                            chat_state.is_loading = false;
                        }
                    }
                    KeyCode::Backspace => {
                        chat_state.input.pop();
                    }
                    KeyCode::Char(c) => {
                        chat_state.input.push(c);
                    }
                    _ => {}
                }
            }
        }
    }
}

fn ui(f: &mut Frame, chat_state: &ChatState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(3),
        ])
        .split(f.area());

    // Chat messages area
    let messages_block = Block::default()
        .title(" CarpAI Chat (q to quit) ")
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Cyan));

    let messages: Vec<ListItem> = chat_state
        .messages
        .iter()
        .map(|msg| {
            let style = match msg.role {
                Role::User => Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
                Role::Assistant => Style::default().fg(Color::White),
                Role::System => Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::DIM),
            };

            let prefix = match msg.role {
                Role::User => "You: ",
                Role::Assistant => "AI: ",
                Role::System => "[System] ",
            };

            ListItem::new(Text::from(vec![Line::from(vec![
                Span::styled(prefix, style),
                Span::raw(&msg.content),
            ])]))
        })
        .collect();

    let messages_list = List::new(messages).block(messages_block);
    f.render_widget(messages_list, chunks[0]);

    // Input area
    let input_text = if chat_state.is_loading {
        format!("{} 🤔 Thinking...", chat_state.input)
    } else {
        format!("> {}", chat_state.input)
    };

    let input_block = Block::default()
        .title(" Input ")
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Yellow));

    let input_paragraph = Paragraph::new(input_text)
        .block(input_block)
        .wrap(Wrap { trim: false });

    f.render_widget(input_paragraph, chunks[1]);

    // Set cursor position
    let cursor_x = chunks[1].x + 2 + chat_state.input.len() as u16 + 2;
    let cursor_y = chunks[1].y + 1;
    f.set_cursor(cursor_x, cursor_y);
}
