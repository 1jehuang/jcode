use super::app::{App, ProcessingStatus};
use super::markdown;
use crate::message::ToolCall;
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

    // Calculate input height based on content (max 5 lines to not overwhelm)
    let available_width = area.width.saturating_sub(4) as usize; // margin + prompt
    let input_len = app.input().len();
    let input_height = if available_width > 0 {
        ((input_len / available_width) + 1).min(5) as u16
    } else {
        1
    };

    // Layout: messages + status + queued + input
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Min(3),              // Messages
            Constraint::Length(1),           // Status line
            Constraint::Length(queued_height), // Queued messages
            Constraint::Length(input_height), // Input (dynamic height)
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
                // AI messages: render markdown with syntax highlighting
                let md_lines = markdown::render_markdown(&msg.content);
                for md_line in md_lines {
                    // Prepend indent to each line
                    let mut spans = vec![Span::raw("  ")];
                    spans.extend(md_line.spans);
                    lines.push(Line::from(spans));
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
                // Tool calls are shown inline during streaming, this is kept for backwards compat
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
        // Show streaming tool calls with details as they are detected
        let streaming_tools = app.streaming_tool_calls();
        for tc in streaming_tools {
            lines.push(Line::from(""));
            // Tool header with name and key info
            let summary = get_tool_summary(tc);
            lines.push(Line::from(vec![
                Span::styled("  ◦ ", Style::default().fg(TOOL_COLOR)),
                Span::styled(&tc.name, Style::default().fg(TOOL_COLOR)),
                Span::styled(format!(" {}", summary), Style::default().fg(DIM_COLOR)),
            ]));
        }
    }

    // Calculate scroll position
    // Note: We don't use Wrap because it makes scroll calculation unpredictable
    let total_lines = lines.len();
    let visible_height = area.height as usize;
    let max_scroll = total_lines.saturating_sub(visible_height);
    let user_scroll = app.scroll_offset().min(max_scroll); // Cap to available content

    // scroll_offset = 0 means bottom (auto-scroll), higher = further up
    // ratatui scroll = lines from top to hide
    let scroll = if user_scroll > 0 {
        max_scroll.saturating_sub(user_scroll)
    } else {
        max_scroll
    };

    let paragraph = Paragraph::new(lines)
        .scroll((scroll as u16, 0));

    frame.render_widget(paragraph, area);

    // Show scroll indicators
    if scroll > 0 {
        // Content above indicator (top-right)
        let indicator = format!("↑{}", scroll);
        let indicator_area = Rect {
            x: area.x + area.width.saturating_sub(indicator.len() as u16 + 1),
            y: area.y,
            width: indicator.len() as u16 + 1,
            height: 1,
        };
        let indicator_widget = Paragraph::new(Line::from(vec![
            Span::styled(indicator, Style::default().fg(DIM_COLOR)),
        ]));
        frame.render_widget(indicator_widget, indicator_area);
    }

    // Content below indicator (bottom-right) when user has scrolled up
    if user_scroll > 0 {
        let indicator = format!("↓{}", user_scroll);
        let indicator_area = Rect {
            x: area.x + area.width.saturating_sub(indicator.len() as u16 + 1),
            y: area.y + area.height.saturating_sub(1),
            width: indicator.len() as u16 + 1,
            height: 1,
        };
        let indicator_widget = Paragraph::new(Line::from(vec![
            Span::styled(indicator, Style::default().fg(QUEUED_COLOR)),
        ]));
        frame.render_widget(indicator_widget, indicator_area);
    }
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let (input_tokens, output_tokens) = app.streaming_tokens();
    let elapsed = app.elapsed().map(|d| d.as_secs_f32()).unwrap_or(0.0);
    let stale_secs = app.time_since_activity().map(|d| d.as_secs_f32());

    let line = if app.is_processing() {
        // Animated spinner based on elapsed time (cycles every 80ms per frame)
        let spinner_idx = (elapsed * 12.5) as usize % SPINNER_FRAMES.len();
        let spinner = SPINNER_FRAMES[spinner_idx];

        let (status_text, progress_bar) = match app.status() {
            ProcessingStatus::Idle => (String::new(), None),
            ProcessingStatus::Sending => (format!("sending… {:.1}s", elapsed), None),
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
                (format!("{}{} {:.1}s", tokens_str, stale_str, elapsed), None)
            }
            ProcessingStatus::RunningTool(name) => {
                let tokens_str = if input_tokens > 0 || output_tokens > 0 {
                    format!("↑{} ↓{} ", input_tokens, output_tokens)
                } else {
                    String::new()
                };
                // Animated progress bar for tool execution
                let bar_width = 10;
                let progress = ((elapsed * 2.0) % 1.0) as f32; // Cycle every 0.5s
                let filled = ((progress * bar_width as f32) as usize) % bar_width;
                let bar: String = (0..bar_width)
                    .map(|i| if i == filled { '●' } else { '·' })
                    .collect();
                (format!("{}{}… {:.1}s", tokens_str, name, elapsed), Some(bar))
            }
        };

        let mut spans = vec![
            Span::styled(spinner, Style::default().fg(AI_COLOR)),
            Span::styled(format!(" {}", status_text), Style::default().fg(DIM_COLOR)),
        ];
        if let Some(bar) = progress_bar {
            spans.push(Span::styled(format!(" {}", bar), Style::default().fg(AI_COLOR)));
        }
        Line::from(spans)
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

    // Build prompt
    let prompt_str = if app.is_processing() {
        "… "
    } else if app.active_skill().is_some() {
        "» "
    } else {
        "> "
    };
    let prompt_style = if app.is_processing() {
        Style::default().fg(QUEUED_COLOR)
    } else if app.active_skill().is_some() {
        Style::default().fg(ACCENT_COLOR)
    } else {
        Style::default().fg(DIM_COLOR)
    };

    let prompt_len = 2;
    let line_width = (area.width as usize).saturating_sub(prompt_len);

    if line_width == 0 {
        return;
    }

    // Wrap text into lines
    let chars: Vec<char> = input_text.chars().collect();
    let mut lines: Vec<Line> = Vec::new();
    let mut pos = 0;

    while pos < chars.len() || lines.is_empty() {
        let end = (pos + line_width).min(chars.len());
        let line_text: String = chars[pos..end].iter().collect();

        if lines.is_empty() {
            // First line has prompt
            lines.push(Line::from(vec![
                Span::styled(prompt_str, prompt_style),
                Span::raw(line_text),
            ]));
        } else {
            // Continuation lines have indent
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::raw(line_text),
            ]));
        }

        if end == pos {
            break; // Empty input case
        }
        pos = end;
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);

    // Calculate cursor position in wrapped text
    let cursor_line = cursor_pos / line_width;
    let cursor_col = cursor_pos % line_width;
    let cursor_y = area.y + (cursor_line as u16).min(area.height.saturating_sub(1));
    let cursor_x = area.x + prompt_len as u16 + cursor_col as u16;

    frame.set_cursor_position(Position::new(cursor_x, cursor_y));
}

/// Extract a brief summary from a tool call input (file path, command, etc.)
fn get_tool_summary(tool: &ToolCall) -> String {
    match tool.name.as_str() {
        "bash" => {
            if let Some(cmd) = tool.input.get("command").and_then(|v| v.as_str()) {
                let short = if cmd.len() > 50 {
                    format!("{}...", &cmd[..50])
                } else {
                    cmd.to_string()
                };
                format!("$ {}", short)
            } else {
                String::new()
            }
        }
        "read" | "write" | "edit" => {
            if let Some(path) = tool.input.get("file_path").and_then(|v| v.as_str()) {
                path.to_string()
            } else {
                String::new()
            }
        }
        "glob" | "grep" => {
            if let Some(pattern) = tool.input.get("pattern").and_then(|v| v.as_str()) {
                format!("'{}'", pattern)
            } else {
                String::new()
            }
        }
        "ls" => {
            tool.input
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".")
                .to_string()
        }
        _ => String::new()
    }
}
