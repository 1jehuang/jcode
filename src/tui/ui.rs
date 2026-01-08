use super::app::{App, ProcessingStatus, QueueMode};
use ratatui::{
    prelude::*,
    widgets::{Paragraph, Wrap},
};

// Minimal color palette
const USER_COLOR: Color = Color::Rgb(138, 180, 248);    // Soft blue
const AI_COLOR: Color = Color::Rgb(129, 199, 132);      // Soft green
const TOOL_COLOR: Color = Color::Rgb(120, 120, 120);    // Gray
const DIM_COLOR: Color = Color::Rgb(80, 80, 80);        // Dimmer gray
const ACCENT_COLOR: Color = Color::Rgb(186, 139, 255);  // Purple accent
const QUEUED_COLOR: Color = Color::Rgb(255, 193, 7);    // Amber/yellow for queued

// Spinner frames for animated status
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Layout: messages + status + input
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Min(3),      // Messages
            Constraint::Length(1),   // Status line
            Constraint::Length(1),   // Input
        ])
        .split(area);

    draw_messages(frame, app, chunks[0]);
    draw_status(frame, app, chunks[1]);
    draw_input(frame, app, chunks[2]);
}

fn draw_messages(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();

    // Header - minimal
    if app.display_messages().is_empty() && !app.is_processing() {
        lines.push(Line::from(vec![
            Span::styled(
                format!("jcode v{}", env!("CARGO_PKG_VERSION")),
                Style::default().fg(DIM_COLOR),
            ),
        ]));
        lines.push(Line::from(Span::styled(
            "new: message queueing, live status & tokens",
            Style::default().fg(DIM_COLOR).italic(),
        )));
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
            }
            "tool" => {
                // Tool output with diff coloring
                for (i, line) in msg.content.lines().take(15).enumerate() {
                    let (prefix, style) = if line.starts_with('+') && !line.starts_with("++") {
                        ("    ", Style::default().fg(Color::Green))
                    } else if line.starts_with('-') && !line.starts_with("--") {
                        ("    ", Style::default().fg(Color::Red))
                    } else if line.starts_with("...(truncated)") {
                        ("    ", Style::default().fg(DIM_COLOR).italic())
                    } else {
                        ("    ", Style::default().fg(TOOL_COLOR).dim())
                    };
                    let display_line = if line.len() > 80 {
                        format!("{}…", &line[..80])
                    } else {
                        line.to_string()
                    };
                    lines.push(Line::from(vec![
                        Span::raw(prefix),
                        Span::styled(display_line, style),
                    ]));
                    // Show truncation indicator if we hit the limit
                    if i == 14 && msg.content.lines().count() > 15 {
                        lines.push(Line::from(vec![
                            Span::raw("    "),
                            Span::styled("...(more)", Style::default().fg(DIM_COLOR).italic()),
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
            "queued" => {
                // Queued message: show with amber color and mode indicator
                let mode_tag = msg.tool_calls.first()
                    .map(|s| s.as_str())
                    .unwrap_or("after");
                lines.push(Line::from(vec![
                    Span::styled("⏳ ", Style::default().fg(QUEUED_COLOR)),
                    Span::styled(&msg.content, Style::default().fg(QUEUED_COLOR).dim()),
                    Span::styled(format!(" [{}]", mode_tag), Style::default().fg(DIM_COLOR)),
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

    // Auto-scroll to bottom
    let visible_height = area.height as usize;
    let scroll = lines.len().saturating_sub(visible_height);

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll as u16, 0));

    frame.render_widget(paragraph, area);
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let (input_tokens, output_tokens) = app.streaming_tokens();
    let elapsed = app.elapsed().map(|d| d.as_secs_f32()).unwrap_or(0.0);

    let line = if app.is_processing() {
        // Animated spinner based on elapsed time (cycles every 80ms per frame)
        let spinner_idx = (elapsed * 12.5) as usize % SPINNER_FRAMES.len();
        let spinner = SPINNER_FRAMES[spinner_idx];

        let status_text = match app.status() {
            ProcessingStatus::Idle => String::new(),
            ProcessingStatus::Sending => format!("sending… {:.1}s", elapsed),
            ProcessingStatus::Streaming => {
                if input_tokens > 0 || output_tokens > 0 {
                    format!("↑{} ↓{} {:.1}s", input_tokens, output_tokens, elapsed)
                } else {
                    format!("streaming… {:.1}s", elapsed)
                }
            }
            ProcessingStatus::RunningTool(name) => format!("running {}… {:.1}s", name, elapsed),
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

fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    let input_text = app.input();
    let cursor_pos = app.cursor_pos();

    // Build input line with prompt
    let (prompt, suffix) = if app.is_processing() {
        // Show queue mode indicator when processing
        let mode_indicator = match app.queue_mode() {
            QueueMode::Interleave => Span::styled(" [Tab:⚡]", Style::default().fg(QUEUED_COLOR)),
            QueueMode::AfterCompletion => Span::styled(" [Tab:⏳]", Style::default().fg(DIM_COLOR)),
        };
        (Span::styled("… ", Style::default().fg(QUEUED_COLOR)), Some(mode_indicator))
    } else if app.active_skill().is_some() {
        (Span::styled("» ", Style::default().fg(ACCENT_COLOR)), None)
    } else {
        (Span::styled("> ", Style::default().fg(DIM_COLOR)), None)
    };

    let mut spans = vec![prompt, Span::raw(input_text)];
    if let Some(s) = suffix {
        spans.push(s);
    }

    let input_line = Line::from(spans);
    let paragraph = Paragraph::new(input_line);
    frame.render_widget(paragraph, area);

    // Always show cursor - user can type even during processing
    frame.set_cursor_position(Position::new(
        area.x + 2 + cursor_pos as u16,
        area.y,
    ));
}
