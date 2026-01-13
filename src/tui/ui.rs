use super::app::{App, ProcessingStatus};
use ratatui::{
    prelude::*,
    widgets::{Paragraph, Wrap},
};
use std::time::SystemTime;

// Minimal color palette
const USER_COLOR: Color = Color::Rgb(138, 180, 248);    // Soft blue
const AI_COLOR: Color = Color::Rgb(129, 199, 132);      // Soft green
const TOOL_COLOR: Color = Color::Rgb(120, 120, 120);    // Gray
const DIM_COLOR: Color = Color::Rgb(80, 80, 80);        // Dimmer gray
const ACCENT_COLOR: Color = Color::Rgb(186, 139, 255);  // Purple accent
const QUEUED_COLOR: Color = Color::Rgb(255, 193, 7);    // Amber/yellow for queued

// Spinner frames for animated status
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Get how long ago the binary was last modified
fn binary_age() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let metadata = std::fs::metadata(&exe).ok()?;
    let modified = metadata.modified().ok()?;
    let elapsed = SystemTime::now().duration_since(modified).ok()?;
    let secs = elapsed.as_secs();

    let age_str = if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    };

    Some(age_str)
}

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Calculate queued messages height (1 line per message, max 3)
    let queued_height = app.queued_messages().len().min(3) as u16;

    // Layout: messages + status + queued + input
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Min(3),              // Messages
            Constraint::Length(1),           // Status line
            Constraint::Length(queued_height), // Queued messages
            Constraint::Length(1),           // Input
        ])
        .split(area);

    draw_messages(frame, app, chunks[0]);
    draw_status(frame, app, chunks[1]);
    if queued_height > 0 {
        draw_queued(frame, app, chunks[2]);
    }
    draw_input(frame, app, chunks[3]);
}

fn draw_messages(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();

    // Header - minimal
    if app.display_messages().is_empty() && !app.is_processing() {
        let age = binary_age().unwrap_or_else(|| "unknown".to_string());
        lines.push(Line::from(vec![
            Span::styled(
                format!("jcode v{}", env!("CARGO_PKG_VERSION")),
                Style::default().fg(DIM_COLOR),
            ),
            Span::styled(
                format!(" (updated {})", age),
                Style::default().fg(DIM_COLOR).dim(),
            ),
        ]));
        lines.push(Line::from(""));

        // Show skill hint if available
        let skills = app.available_skills();
        if !skills.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("skills: {}", skills.iter().map(|s| format!("/{}", s)).collect::<Vec<_>>().join(" ")),
                Style::default().fg(DIM_COLOR),
            )));
        }
    }

    for msg in app.display_messages() {
        // Add spacing between messages
        if !lines.is_empty() && msg.role != "tool" {
            lines.push(Line::from(""));
        }

        match msg.role.as_str() {
            "user" => {
                // User messages: blue prefix, then content
                lines.push(Line::from(vec![
                    Span::styled("› ", Style::default().fg(USER_COLOR)),
                    Span::raw(&msg.content),
                ]));
            }
            "assistant" => {
                // AI messages: white/default color
                for line in msg.content.lines() {
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::raw(line),
                    ]));
                }
                // Tool badges inline
                if !msg.tool_calls.is_empty() {
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(
                            msg.tool_calls.join(" "),
                            Style::default().fg(ACCENT_COLOR).dim(),
                        ),
                    ]));
                }
                // Show duration if available
                if let Some(secs) = msg.duration_secs {
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(
                            format!("{:.1}s", secs),
                            Style::default().fg(DIM_COLOR),
                        ),
                    ]));
                }
            }
            "tool" => {
                // Tool header with name/title
                let tool_name = msg.title.as_deref().unwrap_or("tool");
                lines.push(Line::from(vec![
                    Span::styled("  ◦ ", Style::default().fg(TOOL_COLOR)),
                    Span::styled(tool_name, Style::default().fg(TOOL_COLOR)),
                ]));

                // Tool output with diff coloring (compact: max 10 lines)
                let line_count = msg.content.lines().count();
                let max_lines = 10;
                for (i, line) in msg.content.lines().take(max_lines).enumerate() {
                    let style = if line.starts_with('+') && !line.starts_with("++") {
                        Style::default().fg(Color::Green)
                    } else if line.starts_with('-') && !line.starts_with("--") {
                        Style::default().fg(Color::Red)
                    } else if line.starts_with("@@") {
                        Style::default().fg(Color::Cyan).dim()
                    } else {
                        Style::default().fg(TOOL_COLOR).dim()
                    };
                    let display_line = if line.len() > 100 {
                        format!("{}…", &line[..100])
                    } else {
                        line.to_string()
                    };
                    lines.push(Line::from(vec![
                        Span::raw("    "),
                        Span::styled(display_line, style),
                    ]));
                    // Show truncation indicator if we hit the limit
                    if i == max_lines - 1 && line_count > max_lines {
                        lines.push(Line::from(vec![
                            Span::raw("    "),
                            Span::styled(
                                format!("...({} more lines)", line_count - max_lines),
                                Style::default().fg(DIM_COLOR).italic()
                            ),
                        ]));
                    }
                }
            }
            "system" => {
                lines.push(Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(&msg.content, Style::default().fg(ACCENT_COLOR).italic()),
                ]));
            }
            "usage" => {
                lines.push(Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(&msg.content, Style::default().fg(DIM_COLOR)),
                ]));
            }
            "error" => {
                lines.push(Line::from(vec![
                    Span::styled("  ✗ ", Style::default().fg(Color::Red)),
                    Span::styled(&msg.content, Style::default().fg(Color::Red)),
                ]));
            }
            _ => {}
        }
    }

    // Streaming text
    if app.is_processing() {
        if !app.streaming_text().is_empty() {
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            for line in app.streaming_text().lines() {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::raw(line),
                ]));
            }
        }
        // Show streaming tool calls as they are detected
        let streaming_tools = app.streaming_tool_calls();
        if !streaming_tools.is_empty() {
            let tools_str = streaming_tools.iter()
                .map(|t| format!("[{}]", t))
                .collect::<Vec<_>>()
                .join(" ");
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(tools_str, Style::default().fg(ACCENT_COLOR).dim()),
            ]));
        }
    }

    // Calculate scroll position
    let visible_height = area.height as usize;
    let max_scroll = lines.len().saturating_sub(visible_height);
    let user_scroll = app.scroll_offset();

    // Use user's scroll offset, but clamp to valid range
    // When user_scroll is 0, auto-scroll to bottom
    let scroll = if user_scroll > 0 {
        max_scroll.saturating_sub(user_scroll).min(max_scroll)
    } else {
        max_scroll
    };

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll as u16, 0));

    frame.render_widget(paragraph, area);
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let (input_tokens, output_tokens) = app.streaming_tokens();
    let elapsed = app.elapsed().map(|d| d.as_secs_f32()).unwrap_or(0.0);
    let stale_secs = app.time_since_activity().map(|d| d.as_secs_f32());

    let line = if app.is_processing() {
        // Animated spinner based on elapsed time (cycles every 80ms per frame)
        let spinner_idx = (elapsed * 12.5) as usize % SPINNER_FRAMES.len();
        let spinner = SPINNER_FRAMES[spinner_idx];

        let status_text = match app.status() {
            ProcessingStatus::Idle => String::new(),
            ProcessingStatus::Sending => format!("sending… {:.1}s", elapsed),
            ProcessingStatus::Streaming => {
                let tokens_str = if input_tokens > 0 || output_tokens > 0 {
                    format!("↑{} ↓{}", input_tokens, output_tokens)
                } else {
                    String::new()
                };
                // Show stale indicator if no activity for >2s
                let stale_str = match stale_secs {
                    Some(s) if s > 2.0 => format!(" (idle {:.0}s)", s),
                    _ => String::new(),
                };
                format!("{}{} {:.1}s", tokens_str, stale_str, elapsed)
            }
            ProcessingStatus::RunningTool(name) => {
                let tokens_str = if input_tokens > 0 || output_tokens > 0 {
                    format!("↑{} ↓{} ", input_tokens, output_tokens)
                } else {
                    String::new()
                };
                format!("{}{}… {:.1}s", tokens_str, name, elapsed)
            }
        };

        Line::from(vec![
            Span::styled(spinner, Style::default().fg(AI_COLOR)),
            Span::styled(format!(" {}", status_text), Style::default().fg(DIM_COLOR)),
        ])
    } else {
        // Idle - show nothing or minimal info
        Line::from(Span::styled("", Style::default().fg(DIM_COLOR)))
    };

    let paragraph = Paragraph::new(line);
    frame.render_widget(paragraph, area);
}

fn draw_queued(frame: &mut Frame, app: &App, area: Rect) {
    let queued = app.queued_messages();
    let lines: Vec<Line> = queued.iter()
        .take(3)
        .map(|msg| {
            Line::from(vec![
                Span::styled("⏳ ", Style::default().fg(QUEUED_COLOR)),
                Span::styled(msg.as_str(), Style::default().fg(QUEUED_COLOR).dim()),
            ])
        })
        .collect();

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}

fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    let input_text = app.input();
    let cursor_pos = app.cursor_pos();

    // Build input line with prompt
    let prompt = if app.is_processing() {
        Span::styled("… ", Style::default().fg(QUEUED_COLOR))
    } else if app.active_skill().is_some() {
        Span::styled("» ", Style::default().fg(ACCENT_COLOR))
    } else {
        Span::styled("> ", Style::default().fg(DIM_COLOR))
    };

    let spans = vec![prompt, Span::raw(input_text)];
    let input_line = Line::from(spans);
    let paragraph = Paragraph::new(input_line);
    frame.render_widget(paragraph, area);

    // Always show cursor - user can type even during processing
    frame.set_cursor_position(Position::new(
        area.x + 2 + cursor_pos as u16,
        area.y,
    ));
}
