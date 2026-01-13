use super::app::{App, ProcessingStatus};
use super::markdown;
use crate::message::ToolCall;
use ratatui::{
    prelude::*,
    widgets::Paragraph,
};
use std::time::SystemTime;

// Minimal color palette
const USER_COLOR: Color = Color::Rgb(138, 180, 248);    // Soft blue (caret)
const AI_COLOR: Color = Color::Rgb(129, 199, 132);      // Soft green (unused)
const TOOL_COLOR: Color = Color::Rgb(120, 120, 120);    // Gray
const DIM_COLOR: Color = Color::Rgb(80, 80, 80);        // Dimmer gray
const ACCENT_COLOR: Color = Color::Rgb(186, 139, 255);  // Purple accent
const QUEUED_COLOR: Color = Color::Rgb(255, 193, 7);    // Amber/yellow for queued
const USER_TEXT: Color = Color::Rgb(245, 245, 255);     // Bright cool white (user messages)
const USER_BG: Color = Color::Rgb(35, 40, 50);          // Subtle dark blue background for user
const AI_TEXT: Color = Color::Rgb(220, 220, 215);       // Softer warm white (AI messages)

// Spinner frames for animated status
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Generate an animated color that pulses between two colors
fn animated_tool_color(elapsed: f32) -> Color {
    // Cycle period of ~1.5 seconds
    let t = (elapsed * 2.0).sin() * 0.5 + 0.5; // 0.0 to 1.0

    // Interpolate between cyan and purple
    let r = (80.0 + t * 106.0) as u8;  // 80 -> 186
    let g = (200.0 - t * 61.0) as u8;  // 200 -> 139
    let b = (220.0 + t * 35.0) as u8;  // 220 -> 255

    Color::Rgb(r, g, b)
}

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

    // Calculate queued messages (full count for numbering)
    let queued_count = app.queued_messages().len();
    let queued_height = queued_count.min(3) as u16;

    // Calculate input height based on content (max 5 lines to not overwhelm)
    let available_width = area.width.saturating_sub(3) as usize; // prompt chars
    let input_len = app.input().len();
    let input_height = if available_width > 0 {
        ((input_len / available_width) + 1).min(5) as u16
    } else {
        1
    };

    // Count user messages to show next prompt number
    let user_count = app.display_messages().iter().filter(|m| m.role == "user").count();

    // Estimate message content height (no margin, full width)
    let content_height = estimate_content_height(app, area.width);
    let fixed_height = 1 + queued_height + input_height; // status + queued + input
    let available_height = area.height;

    // Use packed layout when content fits, scrolling layout otherwise
    let use_packed = content_height + fixed_height <= available_height;

    // Both layouts use same structure: messages, status, queued, input
    // This keeps chunk indices consistent
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if use_packed {
            [
                Constraint::Length(content_height.max(1)), // Messages (exact height)
                Constraint::Length(1),                     // Status line
                Constraint::Length(queued_height),         // Queued messages
                Constraint::Length(input_height),          // Input
            ]
        } else {
            [
                Constraint::Min(3),                // Messages (scrollable)
                Constraint::Length(1),             // Status line
                Constraint::Length(queued_height), // Queued messages
                Constraint::Length(input_height),  // Input
            ]
        })
        .split(area);

    draw_messages(frame, app, chunks[0]);
    draw_status(frame, app, chunks[1]);
    if queued_height > 0 {
        draw_queued(frame, app, chunks[2], user_count + 1);
    }
    draw_input(frame, app, chunks[3], user_count + queued_count + 1);
}

/// Estimate how many lines the message content will take
fn estimate_content_height(app: &App, width: u16) -> u16 {
    let width = width as usize;
    if width == 0 {
        return 1;
    }

    let mut lines = 0u16;

    // Header is always visible: version + model + blank = 3 lines minimum
    lines += 3;
    // Plus optional MCP line
    if !app.mcp_servers().is_empty() {
        lines += 1;
    }
    // Plus optional skills line
    if !app.available_skills().is_empty() {
        lines += 1;
    }

    for msg in app.display_messages() {
        // Spacing between messages
        if lines > 0 && msg.role != "tool" {
            lines += 1;
        }

        match msg.role.as_str() {
            "user" => {
                // User messages can wrap - estimate based on content length
                // Format is "N› content" so add ~4 chars for prefix
                let msg_len = msg.content.len() + 4;
                let wrap_lines = (msg_len / width).max(1);
                lines += wrap_lines as u16;
            }
            "assistant" => {
                // Rough estimate: count newlines + wrap estimate
                let content_lines = msg.content.lines().count().max(1);
                let avg_line_len = msg.content.len() / content_lines.max(1);
                let wrap_factor = if avg_line_len > width { (avg_line_len / width) + 1 } else { 1 };
                lines += (content_lines * wrap_factor) as u16;

                // Tool badges
                if !msg.tool_calls.is_empty() {
                    lines += 1;
                }
                // Duration
                if msg.duration_secs.is_some() {
                    lines += 1;
                }
            }
            "tool" => {
                lines += 1;
                // Diff lines for edit tools
                if let Some(ref tc) = msg.tool_data {
                    if tc.name == "edit" || tc.name == "Edit" {
                        lines += 10; // Rough estimate for diff
                    }
                }
            }
            _ => {
                lines += 1;
            }
        }
    }

    // Streaming content
    if app.is_processing() {
        let streaming = app.streaming_text();
        if !streaming.is_empty() {
            // Estimate with wrapping
            let content_lines = streaming.lines().count().max(1);
            let avg_line_len = streaming.len() / content_lines.max(1);
            let wrap_factor = if avg_line_len > width { (avg_line_len / width) + 1 } else { 1 };
            lines += (content_lines * wrap_factor) as u16;
        }
        // Active tool calls
        lines += app.streaming_tool_calls().len() as u16;
    }

    // Add small buffer for estimation errors (prevents micro-scrolling)
    lines += 2;

    lines
}

fn draw_messages(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    let mut user_line_indices: Vec<usize> = Vec::new(); // Track which lines are user prompts

    // Header - always visible
    let age = binary_age().unwrap_or_else(|| "unknown".to_string());
    let provider = app.provider_name();

    // Line 1: Version
    lines.push(Line::from(Span::styled(
        format!("jcode {} (built {})", env!("JCODE_VERSION"), age),
        Style::default().fg(DIM_COLOR),
    )));

    // Line 2: Provider/Model
    lines.push(Line::from(Span::styled(
        format!("model: {}", provider),
        Style::default().fg(DIM_COLOR),
    )));

    // Line 3: MCPs (if any)
    let mcps = app.mcp_servers();
    if !mcps.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("mcp: {}", mcps.join(", ")),
            Style::default().fg(DIM_COLOR),
        )));
    }

    // Line 4: Skills (if any)
    let skills = app.available_skills();
    if !skills.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("skills: {}", skills.iter().map(|s| format!("/{}", s)).collect::<Vec<_>>().join(" ")),
            Style::default().fg(DIM_COLOR),
        )));
    }

    // Blank line after header
    lines.push(Line::from(""));

    let mut prompt_num = 0usize;

    for msg in app.display_messages() {
        // Add spacing between messages
        if !lines.is_empty() && msg.role != "tool" {
            lines.push(Line::from(""));
        }

        match msg.role.as_str() {
            "user" => {
                prompt_num += 1;
                user_line_indices.push(lines.len()); // Track this line index
                // User messages: dim number, blue caret, bright text
                lines.push(Line::from(vec![
                    Span::styled(format!("{}", prompt_num), Style::default().fg(DIM_COLOR)),
                    Span::styled("› ", Style::default().fg(USER_COLOR)),
                    Span::styled(msg.content.clone(), Style::default().fg(USER_TEXT)),
                ]));
            }
            "assistant" => {
                // AI messages: render markdown flush left
                let md_lines = markdown::render_markdown(&msg.content);
                for md_line in md_lines {
                    lines.push(md_line);
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
                // Show tool call with full details
                if let Some(ref tc) = msg.tool_data {
                    let summary = get_tool_summary(tc);
                    lines.push(Line::from(vec![
                        Span::styled("  ◦ ", Style::default().fg(TOOL_COLOR)),
                        Span::styled(tc.name.clone(), Style::default().fg(TOOL_COLOR)),
                        Span::styled(format!(" {}", summary), Style::default().fg(DIM_COLOR)),
                    ]));

                    // Show diff for edit tools
                    if tc.name == "edit" || tc.name == "Edit" {
                        let diff_lines = get_edit_diff_lines(tc);
                        lines.extend(diff_lines);
                    }
                }
            }
            "system" => {
                lines.push(Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(msg.content.clone(), Style::default().fg(ACCENT_COLOR).italic()),
                ]));
            }
            "usage" => {
                lines.push(Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(msg.content.clone(), Style::default().fg(DIM_COLOR)),
                ]));
            }
            "error" => {
                lines.push(Line::from(vec![
                    Span::styled("  ✗ ", Style::default().fg(Color::Red)),
                    Span::styled(msg.content.clone(), Style::default().fg(Color::Red)),
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
                    Span::raw(line.to_string()),
                ]));
            }
        }
        // Tool calls are now shown inline in display_messages
    }

    // Wrap lines and track which wrapped indices correspond to user lines
    let wrap_width = area.width.saturating_sub(2) as usize; // Leave margin for right bar
    let mut wrapped_user_indices: Vec<usize> = Vec::new();
    let mut wrapped_idx = 0usize;

    let mut wrapped_lines: Vec<Line> = Vec::new();
    for (orig_idx, line) in lines.into_iter().enumerate() {
        let is_user_line = user_line_indices.contains(&orig_idx);
        let new_lines = markdown::wrap_line(line, wrap_width);
        let count = new_lines.len();

        if is_user_line {
            // All wrapped lines from a user message get the right bar
            for i in 0..count {
                wrapped_user_indices.push(wrapped_idx + i);
            }
        }

        wrapped_lines.extend(new_lines);
        wrapped_idx += count;
    }

    // Calculate scroll position
    let total_lines = wrapped_lines.len();
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

    let paragraph = Paragraph::new(wrapped_lines)
        .scroll((scroll as u16, 0));

    frame.render_widget(paragraph, area);

    // Draw right bar for visible user lines
    let right_x = area.x + area.width.saturating_sub(1);
    for &line_idx in &wrapped_user_indices {
        // Check if this line is visible after scroll
        if line_idx >= scroll && line_idx < scroll + visible_height {
            let screen_y = area.y + (line_idx - scroll) as u16;
            let bar_area = Rect { x: right_x, y: screen_y, width: 1, height: 1 };
            let bar = Paragraph::new(Span::styled("│", Style::default().fg(USER_COLOR)));
            frame.render_widget(bar, bar_area);
        }
    }

    // Show scroll indicators
    if scroll > 0 {
        // Content above indicator (top-right, offset to not overlap bar)
        let indicator = format!("↑{}", scroll);
        let indicator_area = Rect {
            x: area.x + area.width.saturating_sub(indicator.len() as u16 + 2),
            y: area.y,
            width: indicator.len() as u16,
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
            x: area.x + area.width.saturating_sub(indicator.len() as u16 + 2),
            y: area.y + area.height.saturating_sub(1),
            width: indicator.len() as u16,
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

        match app.status() {
            ProcessingStatus::Idle => Line::from(""),
            ProcessingStatus::Sending => {
                Line::from(vec![
                    Span::styled(spinner, Style::default().fg(AI_COLOR)),
                    Span::styled(format!(" sending… {:.1}s", elapsed), Style::default().fg(DIM_COLOR)),
                ])
            }
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
                Line::from(vec![
                    Span::styled(spinner, Style::default().fg(AI_COLOR)),
                    Span::styled(format!(" {}{} {:.1}s", tokens_str, stale_str, elapsed), Style::default().fg(DIM_COLOR)),
                ])
            }
            ProcessingStatus::RunningTool(ref name) => {
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
                // Use animated color for the tool name
                let anim_color = animated_tool_color(elapsed);
                Line::from(vec![
                    Span::styled(spinner, Style::default().fg(anim_color)),
                    Span::styled(format!(" {}", tokens_str), Style::default().fg(DIM_COLOR)),
                    Span::styled(name.to_string(), Style::default().fg(anim_color).bold()),
                    Span::styled(format!("… {:.1}s ", elapsed), Style::default().fg(DIM_COLOR)),
                    Span::styled(bar, Style::default().fg(anim_color)),
                ])
            }
        }
    } else {
        // Idle - show nothing or minimal info
        Line::from(Span::styled("", Style::default().fg(DIM_COLOR)))
    };

    let paragraph = Paragraph::new(line);
    frame.render_widget(paragraph, area);
}

fn draw_queued(frame: &mut Frame, app: &App, area: Rect, start_num: usize) {
    let queued = app.queued_messages();
    let lines: Vec<Line> = queued.iter()
        .take(3)
        .enumerate()
        .map(|(i, msg)| {
            Line::from(vec![
                Span::styled(format!("{}", start_num + i), Style::default().fg(DIM_COLOR)),
                Span::styled("… ", Style::default().fg(QUEUED_COLOR)),
                Span::styled(msg.as_str(), Style::default().fg(QUEUED_COLOR).dim()),
            ])
        })
        .collect();

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}

fn draw_input(frame: &mut Frame, app: &App, area: Rect, next_prompt: usize) {
    let input_text = app.input();
    let cursor_pos = app.cursor_pos();

    // Build prompt parts: number (dim) + caret (colored) + space
    let (prompt_char, caret_color) = if app.is_processing() {
        ("… ", QUEUED_COLOR)
    } else if app.active_skill().is_some() {
        ("» ", ACCENT_COLOR)
    } else {
        ("> ", USER_COLOR)
    };
    let num_str = format!("{}", next_prompt);
    let prompt_len = num_str.len() + prompt_char.len();

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
            // First line has prompt: dim number + colored caret
            lines.push(Line::from(vec![
                Span::styled(num_str.clone(), Style::default().fg(DIM_COLOR)),
                Span::styled(prompt_char, Style::default().fg(caret_color)),
                Span::raw(line_text),
            ]));
        } else {
            // Continuation lines have indent to match prompt length
            lines.push(Line::from(vec![
                Span::raw(" ".repeat(prompt_len)),
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

// Colors for diff display
const DIFF_ADD_COLOR: Color = Color::Rgb(100, 200, 100);    // Green for additions
const DIFF_DEL_COLOR: Color = Color::Rgb(200, 100, 100);    // Red for deletions

/// Generate diff lines for an edit tool call
fn get_edit_diff_lines(tool: &ToolCall) -> Vec<Line<'static>> {
    let mut diff_lines = Vec::new();

    let old_str = tool.input.get("old_string").and_then(|v| v.as_str()).unwrap_or("");
    let new_str = tool.input.get("new_string").and_then(|v| v.as_str()).unwrap_or("");

    if old_str.is_empty() && new_str.is_empty() {
        return diff_lines;
    }

    // Show removed lines (limited to 5 lines)
    for (i, line) in old_str.lines().take(5).enumerate() {
        let truncated = if line.len() > 60 {
            format!("{}...", &line[..60])
        } else {
            line.to_string()
        };
        diff_lines.push(Line::from(vec![
            Span::styled("     ", Style::default()),
            Span::styled(format!("- {}", truncated), Style::default().fg(DIFF_DEL_COLOR)),
        ]));
        if i == 4 && old_str.lines().count() > 5 {
            diff_lines.push(Line::from(vec![
                Span::styled("     ", Style::default()),
                Span::styled(format!("  ... ({} more lines)", old_str.lines().count() - 5), Style::default().fg(DIM_COLOR)),
            ]));
            break;
        }
    }

    // Show added lines (limited to 5 lines)
    for (i, line) in new_str.lines().take(5).enumerate() {
        let truncated = if line.len() > 60 {
            format!("{}...", &line[..60])
        } else {
            line.to_string()
        };
        diff_lines.push(Line::from(vec![
            Span::styled("     ", Style::default()),
            Span::styled(format!("+ {}", truncated), Style::default().fg(DIFF_ADD_COLOR)),
        ]));
        if i == 4 && new_str.lines().count() > 5 {
            diff_lines.push(Line::from(vec![
                Span::styled("     ", Style::default()),
                Span::styled(format!("  ... ({} more lines)", new_str.lines().count() - 5), Style::default().fg(DIM_COLOR)),
            ]));
            break;
        }
    }

    diff_lines
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
