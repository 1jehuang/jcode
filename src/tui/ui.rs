#![allow(dead_code)]

#![allow(dead_code)]

use super::{ProcessingStatus, TuiState};
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

/// Calculate rainbow color for prompt index with exponential decay to gray.
/// `distance` is how many prompts back from the most recent (0 = most recent).
fn rainbow_prompt_color(distance: usize) -> Color {
    // Rainbow colors (hue progression): red -> orange -> yellow -> green -> cyan -> blue -> violet
    const RAINBOW: [(u8, u8, u8); 7] = [
        (255, 80, 80),   // Red (softened)
        (255, 160, 80),  // Orange
        (255, 230, 80),  // Yellow
        (80, 220, 100),  // Green
        (80, 200, 220),  // Cyan
        (100, 140, 255), // Blue
        (180, 100, 255), // Violet
    ];

    // Gray target (DIM_COLOR)
    const GRAY: (u8, u8, u8) = (80, 80, 80);

    // Exponential decay factor - how quickly we fade to gray
    // decay = e^(-distance * rate), rate of ~0.4 gives nice falloff
    let decay = (-0.4 * distance as f32).exp();

    // Select rainbow color based on distance (cycle through)
    let rainbow_idx = distance.min(RAINBOW.len() - 1);
    let (r, g, b) = RAINBOW[rainbow_idx];

    // Blend rainbow color with gray based on decay
    // At distance 0: 100% rainbow, as distance increases: approaches gray
    let blend = |rainbow: u8, gray: u8| -> u8 {
        (rainbow as f32 * decay + gray as f32 * (1.0 - decay)) as u8
    };

    Color::Rgb(blend(r, GRAY.0), blend(g, GRAY.1), blend(b, GRAY.2))
}

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

pub fn draw(frame: &mut Frame, app: &dyn TuiState) {
    let area = frame.area();

    // Calculate queued messages (full count for numbering)
    let queued_count = app.queued_messages().len();
    let queued_height = queued_count.min(3) as u16;

    // Calculate input height based on content (max 5 lines to not overwhelm)
    let available_width = area.width.saturating_sub(3) as usize; // prompt chars
    let input_len = app.input().len();
    let base_input_height = if available_width > 0 {
        ((input_len / available_width) + 1).min(5) as u16
    } else {
        1
    };
    // Add 1 line for command suggestions when typing /
    let suggestions = app.command_suggestions();
    let suggestions_height = if !suggestions.is_empty() && !app.is_processing() { 1 } else { 0 };
    let input_height = base_input_height + suggestions_height;

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
fn estimate_content_height(app: &dyn TuiState, width: u16) -> u16 {
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
    // Plus optional server stats line
    let client_count = app.connected_clients().unwrap_or(0);
    let session_count = app.server_sessions().len();
    if client_count > 0 || session_count > 1 {
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

fn draw_messages(frame: &mut Frame, app: &dyn TuiState, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    let mut user_line_indices: Vec<usize> = Vec::new(); // Track which lines are user prompts

    // Header - always visible
    let age = binary_age().unwrap_or_else(|| "unknown".to_string());
    let provider = app.provider_name();
    let model = app.provider_model();

    // Line 1: Version + Mode indicators
    let mut mode_parts: Vec<Span> = vec![
        Span::styled(
            format!("jcode {} (built {})", env!("JCODE_VERSION"), age),
            Style::default().fg(DIM_COLOR),
        ),
    ];

    // Add mode badges
    if app.is_canary() {
        mode_parts.push(Span::styled(" ", Style::default()));
        mode_parts.push(Span::styled(
            " self-dev ",
            Style::default().fg(Color::Black).bg(Color::Rgb(255, 193, 7)), // Amber badge
        ));
    }
    if app.is_remote_mode() {
        mode_parts.push(Span::styled(" ", Style::default()));
        mode_parts.push(Span::styled(
            " remote ",
            Style::default().fg(Color::Black).bg(Color::Rgb(100, 149, 237)), // Cornflower blue badge
        ));
    }

    lines.push(Line::from(mode_parts));

    // Line 2: Provider/Model (show full model identifier)
    lines.push(Line::from(Span::styled(
        format!("{}: {}", provider, model),
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

    // Line 5: Server stats (if running as server with clients)
    let client_count = app.connected_clients().unwrap_or(0);
    let session_count = app.server_sessions().len();
    if client_count > 0 || session_count > 1 {
        let mut parts = Vec::new();
        if client_count > 0 {
            parts.push(format!("{} client{}", client_count, if client_count == 1 { "" } else { "s" }));
        }
        if session_count > 1 {
            parts.push(format!("{} sessions", session_count));
        }
        lines.push(Line::from(Span::styled(
            format!("server: {}", parts.join(", ")),
            Style::default().fg(DIM_COLOR),
        )));
    }

    // Blank line after header
    lines.push(Line::from(""));

    let mut prompt_num = 0usize;
    // Count total user prompts and queued messages for rainbow coloring
    // The input prompt is distance 0, queued messages are 1..queued_count,
    // existing messages continue from there
    let total_prompts = app.display_messages().iter().filter(|m| m.role == "user").count();
    let queued_count = app.queued_messages().len();
    // Input prompt number is total_prompts + queued_count + 1, so distance for
    // existing prompt N is: (total_prompts + queued_count + 1) - N

    for msg in app.display_messages() {
        // Add spacing between messages
        if !lines.is_empty() && msg.role != "tool" {
            lines.push(Line::from(""));
        }

        match msg.role.as_str() {
            "user" => {
                prompt_num += 1;
                user_line_indices.push(lines.len()); // Track this line index
                // Calculate distance from input prompt (distance 0)
                let distance = total_prompts + queued_count + 1 - prompt_num;
                let num_color = rainbow_prompt_color(distance);
                // User messages: rainbow number, blue caret, bright text
                lines.push(Line::from(vec![
                    Span::styled(format!("{}", prompt_num), Style::default().fg(num_color)),
                    Span::styled("› ", Style::default().fg(USER_COLOR)),
                    Span::styled(msg.content.clone(), Style::default().fg(USER_TEXT)),
                ]));
            }
            "assistant" => {
                // AI messages: render markdown flush left
                // Pass width for table rendering (leave some margin)
                let content_width = area.width.saturating_sub(4) as usize;
                let md_lines = markdown::render_markdown_with_width(&msg.content, Some(content_width));
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

                    // Determine status: error if content starts with error prefix
                    // Be specific to avoid false positives (e.g., "No matches found" is not an error)
                    let is_error = msg.content.starts_with("Error:")
                        || msg.content.starts_with("error:")
                        || msg.content.starts_with("Failed:");

                    let (icon, icon_color) = if is_error {
                        ("✗", Color::Rgb(220, 100, 100)) // Red for errors
                    } else {
                        ("✓", Color::Rgb(100, 180, 100)) // Green for success
                    };

                    lines.push(Line::from(vec![
                        Span::styled(format!("  {} ", icon), Style::default().fg(icon_color)),
                        Span::styled(tc.name.clone(), Style::default().fg(TOOL_COLOR)),
                        Span::styled(format!(" {}", summary), Style::default().fg(DIM_COLOR)),
                    ]));

                    // Show diff output for editing tools with syntax highlighting
                    if matches!(tc.name.as_str(), "edit" | "Edit" | "write" | "multiedit") {
                        // Extract file extension for syntax highlighting
                        let file_ext = tc.input.get("file_path")
                            .and_then(|v| v.as_str())
                            .and_then(|p| std::path::Path::new(p).extension())
                            .and_then(|e| e.to_str());

                        let diff_lines: Vec<&str> = msg.content.lines().skip(1).collect();
                        for line in diff_lines {
                            let trimmed = line.trim();
                            if trimmed.is_empty() || trimmed == "..." {
                                continue;
                            }

                            // Determine if addition or deletion
                            let is_add = trimmed.contains("+ ");
                            let is_del = trimmed.contains("- ");
                            let base_color = if is_add {
                                DIFF_ADD_COLOR
                            } else if is_del {
                                DIFF_DEL_COLOR
                            } else {
                                DIM_COLOR
                            };

                            // Extract prefix (line number + sign) and content
                            let (prefix, content) = extract_diff_prefix_and_content(trimmed);

                            // Build the line with syntax-highlighted content
                            let mut spans: Vec<Span<'static>> = vec![
                                Span::styled("    ", Style::default()),
                                Span::styled(prefix.to_string(), Style::default().fg(base_color)),
                            ];

                            // Apply syntax highlighting to content
                            if !content.is_empty() {
                                let highlighted = markdown::highlight_line(content, file_ext);
                                // Tint the highlighted spans with the diff color
                                for span in highlighted {
                                    let tinted = tint_span_with_diff_color(span, base_color);
                                    spans.push(tinted);
                                }
                            }

                            lines.push(Line::from(spans));
                        }
                    }

                    // Show task output (sub-agent result, truncated)
                    if tc.name == "task" {
                        let mut line_count = 0;
                        const MAX_TASK_LINES: usize = 10;
                        for line in msg.content.lines() {
                            // Skip metadata section
                            if line.contains("<task_metadata>") {
                                break;
                            }
                            if line.trim().is_empty() {
                                continue;
                            }
                            if line_count >= MAX_TASK_LINES {
                                lines.push(Line::from(Span::styled(
                                    "    ...(truncated)".to_string(),
                                    Style::default().fg(DIM_COLOR),
                                )));
                                break;
                            }
                            lines.push(Line::from(Span::styled(
                                format!("    {}", line),
                                Style::default().fg(DIM_COLOR),
                            )));
                            line_count += 1;
                        }
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

    // Streaming text - render with markdown for consistent formatting
    if app.is_processing() {
        if !app.streaming_text().is_empty() {
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            // Use markdown rendering to match final display
            let content_width = area.width.saturating_sub(4) as usize;
            let md_lines = markdown::render_markdown_with_width(app.streaming_text(), Some(content_width));
            lines.extend(md_lines);
        }
        // Tool calls are now shown inline in display_messages
    }

    // Wrap lines and track which wrapped indices correspond to user lines
    let full_width = area.width as usize;
    let user_width = area.width.saturating_sub(2) as usize; // Leave margin for right bar
    let mut wrapped_user_indices: Vec<usize> = Vec::new();
    let mut wrapped_idx = 0usize;

    let mut wrapped_lines: Vec<Line> = Vec::new();
    for (orig_idx, line) in lines.into_iter().enumerate() {
        let is_user_line = user_line_indices.contains(&orig_idx);
        // User lines need margin for bar, AI lines use full width
        let wrap_width = if is_user_line { user_width } else { full_width };
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

fn draw_status(frame: &mut Frame, app: &dyn TuiState, area: Rect) {
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
                // Show subagent status if available (e.g., "calling API", "running grep")
                let status_suffix = app.subagent_status()
                    .map(|s| format!(" ({})", s))
                    .unwrap_or_default();
                // Get tool details (command, file path, etc.) from the current tool call
                let tool_detail = app.streaming_tool_calls()
                    .last()
                    .map(|tc| get_tool_summary(tc))
                    .filter(|s| !s.is_empty())
                    .map(|s| format!(" {}", s))
                    .unwrap_or_default();
                Line::from(vec![
                    Span::styled(spinner, Style::default().fg(anim_color)),
                    Span::styled(format!(" {}", tokens_str), Style::default().fg(DIM_COLOR)),
                    Span::styled(name.to_string(), Style::default().fg(anim_color).bold()),
                    Span::styled(status_suffix, Style::default().fg(DIM_COLOR)),
                    Span::styled(tool_detail, Style::default().fg(DIM_COLOR)),
                    Span::styled(format!(" {:.1}s ", elapsed), Style::default().fg(DIM_COLOR)),
                    Span::styled(bar, Style::default().fg(anim_color)),
                ])
            }
        }
    } else {
        // Idle - show model info and session stats
        let model = app.provider_model();
        let msg_count = app.display_messages().len();
        let (total_in, total_out) = app.total_session_tokens().unwrap_or((0, 0));
        let total_tokens = total_in + total_out;

        // Format token count nicely
        let tokens_str = if total_tokens >= 1000 {
            format!("{}k", total_tokens / 1000)
        } else {
            format!("{}", total_tokens)
        };

        // Calculate approximate cost (rough estimate based on typical Claude pricing)
        // Input: ~$3/1M tokens, Output: ~$15/1M tokens for Opus
        let cost_estimate = (total_in as f64 * 3.0 + total_out as f64 * 15.0) / 1_000_000.0;

        // High usage warning color
        let (token_color, warning) = if total_tokens > 150_000 {
            (Color::Rgb(255, 100, 100), " ⚠ consider /clear")
        } else if total_tokens > 100_000 {
            (Color::Rgb(255, 193, 7), "")
        } else {
            (DIM_COLOR, "")
        };

        if app.is_remote_mode() {
            // Remote mode: show session ID + stats
            let session_part = app.current_session_id()
                .map(|id| if id.len() > 8 { format!("{}… ", &id[..8]) } else { format!("{} ", id) })
                .unwrap_or_default();
            Line::from(vec![
                Span::styled("⚡ ", Style::default().fg(ACCENT_COLOR)),
                Span::styled(session_part, Style::default().fg(DIM_COLOR)),
                Span::styled(format!("{} ", model), Style::default().fg(DIM_COLOR)),
                Span::styled(format!("{}msg ", msg_count), Style::default().fg(DIM_COLOR)),
                Span::styled(format!("{}tok ", tokens_str), Style::default().fg(token_color)),
                Span::styled(format!("${:.2}", cost_estimate), Style::default().fg(DIM_COLOR)),
                Span::styled(warning, Style::default().fg(token_color)),
            ])
        } else {
            // Local mode: show model + session stats
            Line::from(vec![
                Span::styled(format!("{} ", model), Style::default().fg(DIM_COLOR)),
                Span::styled(format!("{}msg ", msg_count), Style::default().fg(DIM_COLOR)),
                Span::styled(format!("{}tok ", tokens_str), Style::default().fg(token_color)),
                Span::styled(format!("${:.2}", cost_estimate), Style::default().fg(DIM_COLOR)),
                Span::styled(warning, Style::default().fg(token_color)),
            ])
        }
    };

    let paragraph = Paragraph::new(line);
    frame.render_widget(paragraph, area);
}

fn draw_queued(frame: &mut Frame, app: &dyn TuiState, area: Rect, start_num: usize) {
    let queued = app.queued_messages();
    let queued_count = queued.len();
    let lines: Vec<Line> = queued.iter()
        .take(3)
        .enumerate()
        .map(|(i, msg)| {
            // Distance from input prompt: queued_count - i (first queued is furthest from input)
            // +1 because the input prompt itself is distance 0
            let distance = queued_count.saturating_sub(i);
            let num_color = rainbow_prompt_color(distance);
            Line::from(vec![
                Span::styled(format!("{}", start_num + i), Style::default().fg(num_color)),
                Span::styled("… ", Style::default().fg(QUEUED_COLOR)),
                Span::styled(msg.as_str(), Style::default().fg(QUEUED_COLOR).dim()),
            ])
        })
        .collect();

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}

fn draw_input(frame: &mut Frame, app: &dyn TuiState, area: Rect, next_prompt: usize) {
    let input_text = app.input();
    let cursor_pos = app.cursor_pos();

    // Check for command suggestions
    let suggestions = app.command_suggestions();
    let has_suggestions = !suggestions.is_empty() && !app.is_processing();

    // Build prompt parts: number (dim) + caret (colored) + space
    let (prompt_char, caret_color) = if app.is_processing() {
        ("… ", QUEUED_COLOR)
    } else if app.active_skill().is_some() {
        ("» ", ACCENT_COLOR)
    } else {
        ("> ", USER_COLOR)
    };
    let num_str = format!("{}", next_prompt);
    // Use char count, not byte count (ellipsis is 3 bytes but 1 char)
    let prompt_len = num_str.chars().count() + prompt_char.chars().count();

    let line_width = (area.width as usize).saturating_sub(prompt_len);

    if line_width == 0 {
        return;
    }

    let mut lines: Vec<Line> = Vec::new();

    // Show command suggestions if available
    if has_suggestions {
        let suggestion_text: String = suggestions
            .iter()
            .map(|(cmd, desc)| format!("  {} - {}", cmd, desc))
            .collect::<Vec<_>>()
            .join("  │  ");
        lines.push(Line::from(Span::styled(
            suggestion_text,
            Style::default().fg(DIM_COLOR),
        )));
    }

    // Wrap text into lines
    let chars: Vec<char> = input_text.chars().collect();
    let mut pos = 0;
    let input_start_line = lines.len();

    while pos < chars.len() || lines.len() == input_start_line {
        let end = (pos + line_width).min(chars.len());
        let line_text: String = chars[pos..end].iter().collect();

        if lines.len() == input_start_line {
            // First line has prompt: rainbow number (most recent = distance 0) + colored caret
            let num_color = rainbow_prompt_color(0);
            lines.push(Line::from(vec![
                Span::styled(num_str.clone(), Style::default().fg(num_color)),
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
const DIFF_HIGHLIGHT_ADD: Color = Color::Rgb(150, 255, 150); // Brighter green for changed parts
const DIFF_HIGHLIGHT_DEL: Color = Color::Rgb(255, 130, 130); // Brighter red for changed parts

/// Extract prefix (line number + sign) and content from diff line
/// "42- content" -> ("42- ", "content")
fn extract_diff_prefix_and_content(line: &str) -> (&str, &str) {
    // Format is "42- content" or "42+ content"
    if let Some(pos) = line.find("- ") {
        (&line[..pos + 2], &line[pos + 2..])
    } else if let Some(pos) = line.find("+ ") {
        (&line[..pos + 2], &line[pos + 2..])
    } else {
        (line, "")
    }
}

/// Tint a syntax-highlighted span with a diff color (green/red)
/// Blends the syntax color with the diff color for a subtle tint
fn tint_span_with_diff_color(span: Span<'static>, diff_color: Color) -> Span<'static> {
    let (dr, dg, db) = match diff_color {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => return span,
    };

    // Get the span's foreground color
    let fg = span.style.fg.unwrap_or(Color::White);
    let (sr, sg, sb) = match fg {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::White => (255, 255, 255),
        Color::Black => (0, 0, 0),
        _ => return span, // Can't tint indexed colors easily
    };

    // Blend: 70% syntax color + 30% diff color
    let blend = |s: u8, d: u8| -> u8 {
        ((s as u16 * 70 + d as u16 * 30) / 100) as u8
    };

    let tinted = Color::Rgb(blend(sr, dr), blend(sg, dg), blend(sb, db));
    Span::styled(span.content, span.style.fg(tinted))
}

/// Render a diff line with word-level highlighting for changed parts
fn render_diff_line_with_highlights(
    full_line: &str,
    this_content: &str,
    other_content: &str,
    is_deletion: bool,
) -> Line<'static> {
    use similar::{ChangeTag, TextDiff};

    let (base_color, highlight_color) = if is_deletion {
        (DIFF_DEL_COLOR, DIFF_HIGHLIGHT_DEL)
    } else {
        (DIFF_ADD_COLOR, DIFF_HIGHLIGHT_ADD)
    };

    // Get prefix (line number and +/-)
    let prefix = if let Some(pos) = full_line.find(if is_deletion { "- " } else { "+ " }) {
        &full_line[..pos + 2]
    } else {
        ""
    };

    // Do word-level diff
    let diff = TextDiff::from_words(
        if is_deletion { this_content } else { other_content },
        if is_deletion { other_content } else { this_content },
    );

    let mut spans: Vec<Span<'static>> = vec![
        Span::styled("    ".to_string(), Style::default()),
        Span::styled(prefix.to_string(), Style::default().fg(base_color)),
    ];

    // Build spans with highlighting for changed words
    for change in diff.iter_all_changes() {
        let text = change.value().to_string();
        let style = match change.tag() {
            ChangeTag::Equal => Style::default().fg(base_color),
            ChangeTag::Insert if !is_deletion => {
                // This is a new word in the addition - highlight it
                Style::default().fg(highlight_color).bold()
            }
            ChangeTag::Delete if is_deletion => {
                // This is a removed word in the deletion - highlight it
                Style::default().fg(highlight_color).bold()
            }
            _ => Style::default().fg(base_color),
        };
        spans.push(Span::styled(text, style));
    }

    Line::from(spans)
}

/// Extract a brief summary from a tool call input (file path, command, etc.)
fn get_tool_summary(tool: &ToolCall) -> String {
    let truncate = |s: &str, max: usize| {
        if s.len() > max {
            format!("{}...", &s[..max])
        } else {
            s.to_string()
        }
    };

    match tool.name.as_str() {
        "bash" => {
            tool.input.get("command").and_then(|v| v.as_str())
                .map(|cmd| format!("$ {}", truncate(cmd, 50)))
                .unwrap_or_default()
        }
        "read" | "write" | "edit" => {
            tool.input.get("file_path").and_then(|v| v.as_str())
                .map(|p| p.to_string())
                .unwrap_or_default()
        }
        "multiedit" => {
            let path = tool.input.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            let count = tool.input.get("edits").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
            format!("{} ({} edits)", path, count)
        }
        "glob" => {
            tool.input.get("pattern").and_then(|v| v.as_str())
                .map(|p| format!("'{}'", p))
                .unwrap_or_default()
        }
        "grep" => {
            let pattern = tool.input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            let path = tool.input.get("path").and_then(|v| v.as_str());
            if let Some(p) = path {
                format!("'{}' in {}", truncate(pattern, 30), p)
            } else {
                format!("'{}'", truncate(pattern, 40))
            }
        }
        "ls" => {
            tool.input.get("path").and_then(|v| v.as_str())
                .unwrap_or(".")
                .to_string()
        }
        "task" => {
            let desc = tool.input.get("description").and_then(|v| v.as_str()).unwrap_or("task");
            let agent_type = tool.input.get("subagent_type").and_then(|v| v.as_str()).unwrap_or("agent");
            format!("{} ({})", desc, agent_type)
        }
        "patch" | "apply_patch" => {
            tool.input.get("patch_text").and_then(|v| v.as_str())
                .map(|p| {
                    let lines = p.lines().count();
                    format!("({} lines)", lines)
                })
                .unwrap_or_default()
        }
        "webfetch" => {
            tool.input.get("url").and_then(|v| v.as_str())
                .map(|u| truncate(u, 50))
                .unwrap_or_default()
        }
        "websearch" => {
            tool.input.get("query").and_then(|v| v.as_str())
                .map(|q| format!("'{}'", truncate(q, 40)))
                .unwrap_or_default()
        }
        "mcp" => {
            let action = tool.input.get("action").and_then(|v| v.as_str()).unwrap_or("");
            let server = tool.input.get("server_name").and_then(|v| v.as_str());
            if let Some(s) = server {
                format!("{} {}", action, s)
            } else {
                action.to_string()
            }
        }
        "todowrite" | "todoread" => {
            "todos".to_string()
        }
        "skill" => {
            tool.input.get("skill").and_then(|v| v.as_str())
                .map(|s| format!("/{}", s))
                .unwrap_or_default()
        }
        "codesearch" => {
            tool.input.get("query").and_then(|v| v.as_str())
                .map(|q| format!("'{}'", truncate(q, 40)))
                .unwrap_or_default()
        }
        // MCP tools (prefixed with mcp__)
        name if name.starts_with("mcp__") => {
            // Show first string parameter as summary
            tool.input.as_object()
                .and_then(|obj| obj.iter().find(|(_, v)| v.is_string()))
                .and_then(|(_, v)| v.as_str())
                .map(|s| truncate(s, 40))
                .unwrap_or_default()
        }
        _ => String::new()
    }
}
