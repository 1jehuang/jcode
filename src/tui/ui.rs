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

/// Duration of the startup header animation in seconds
const HEADER_ANIM_DURATION: f32 = 1.5;

/// Calculate smooth animated color for the header.
/// Uses a gentle shimmer effect that pulses through colors uniformly.
fn header_animation_color(elapsed: f32) -> Color {
    if elapsed >= HEADER_ANIM_DURATION {
        return ACCENT_COLOR;
    }

    // Smooth easing function (ease-out cubic)
    let progress = elapsed / HEADER_ANIM_DURATION;
    let eased = 1.0 - (1.0 - progress).powi(3);

    // Color journey: cyan -> purple (accent)
    // Start bright cyan, smoothly transition to accent purple
    let start = (100.0, 220.0, 255.0);  // Bright cyan
    let end = (186.0, 139.0, 255.0);    // Accent purple

    // Add a subtle pulse/shimmer during transition
    let pulse = (elapsed * 8.0).sin() * 0.15 * (1.0 - eased);

    let r = start.0 + (end.0 - start.0) * eased + pulse * 50.0;
    let g = start.1 + (end.1 - start.1) * eased - pulse * 30.0;
    let b = start.2 + (end.2 - start.2) * eased;

    Color::Rgb(
        r.clamp(0.0, 255.0) as u8,
        g.clamp(0.0, 255.0) as u8,
        b.clamp(0.0, 255.0) as u8,
    )
}

/// Create animated span for the header text during startup
fn animated_header_span(text: &str, elapsed: f32) -> Span<'static> {
    let color = header_animation_color(elapsed);
    Span::styled(text.to_string(), Style::default().fg(color))
}

/// Capitalize first letter of a string
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().chain(chars).collect(),
    }
}

/// Format model name nicely (e.g., "claude4.5opus" -> "Claude 4.5 Opus")
fn format_model_name(short: &str) -> String {
    if short.contains("opus") {
        if short.contains("4.5") {
            return "Claude 4.5 Opus".to_string();
        }
        return "Claude Opus".to_string();
    }
    if short.contains("sonnet") {
        if short.contains("3.5") {
            return "Claude 3.5 Sonnet".to_string();
        }
        return "Claude Sonnet".to_string();
    }
    if short.contains("haiku") {
        return "Claude Haiku".to_string();
    }
    if short.starts_with("gpt") {
        return short.to_uppercase();
    }
    short.to_string()
}

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

/// Shorten model name for display (e.g., "claude-opus-4-5-20251101" -> "claude4.5opus")
fn shorten_model_name(model: &str) -> String {
    // Handle common Claude model patterns
    if model.contains("opus") {
        if model.contains("4-5") || model.contains("4.5") {
            return "claude4.5opus".to_string();
        }
        return "claudeopus".to_string();
    }
    if model.contains("sonnet") {
        if model.contains("3-5") || model.contains("3.5") {
            return "claude3.5sonnet".to_string();
        }
        return "claudesonnet".to_string();
    }
    if model.contains("haiku") {
        return "claudehaiku".to_string();
    }
    // Handle OpenAI models
    if model.starts_with("gpt-4") {
        return model.replace("gpt-", "").replace("-", "");
    }
    if model.starts_with("gpt-3") {
        return "gpt3.5".to_string();
    }
    // Fallback: remove common suffixes and dashes
    model
        .split('-')
        .take(3)
        .collect::<Vec<_>>()
        .join("")
}

/// Calculate the number of visual lines an input string will occupy
/// when wrapped to a given width, accounting for explicit newlines.
fn calculate_input_lines(input: &str, line_width: usize) -> usize {
    if line_width == 0 {
        return 1;
    }
    if input.is_empty() {
        return 1;
    }

    let mut total_lines = 0;
    for line in input.split('\n') {
        let chars: Vec<char> = line.chars().collect();
        if chars.is_empty() {
            total_lines += 1;
        } else {
            // Calculate wrapped lines for this segment
            total_lines += (chars.len() + line_width - 1) / line_width;
        }
    }
    total_lines.max(1)
}

pub fn draw(frame: &mut Frame, app: &dyn TuiState) {
    let area = frame.area();

    // Calculate queued messages (full count for numbering)
    let queued_count = app.queued_messages().len();
    let queued_height = queued_count.min(3) as u16;

    // Calculate input height based on content (max 10 lines visible, scrolls if more)
    let available_width = area.width.saturating_sub(3) as usize; // prompt chars
    let base_input_height = calculate_input_lines(app.input(), available_width).min(10) as u16;
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

    // Header is always visible: agent name + model/build + changelog box (up to 7 lines) + blank = 10 lines minimum
    lines += 10;
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
                // Diff lines for edit tools (only if diffs are shown)
                if app.show_diffs() {
                    if let Some(ref tc) = msg.tool_data {
                        if tc.name == "edit" || tc.name == "Edit" {
                            lines += 10; // Rough estimate for diff
                        }
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
    let _provider = app.provider_name();
    let model = app.provider_model();
    let anim_elapsed = app.animation_elapsed();

    // Line 1: Full agent name (icon jcode-session-model) + Mode indicators
    let mut mode_parts: Vec<Span> = Vec::new();

    // Build full agent name: jcode-{session}-{model}
    let session_name = app.session_display_name().unwrap_or_default();
    let short_model = shorten_model_name(&model);
    let icon = crate::id::session_icon(&session_name);

    if !session_name.is_empty() {
        // Full agent name with animated color during startup
        // Format: "JCode Fox · Claude 4.5 Opus"
        let nice_model = format_model_name(&short_model);
        let header_text = format!("{} JCode {} · {}", icon, capitalize(&session_name), nice_model);
        mode_parts.push(animated_header_span(&header_text, anim_elapsed));
    } else {
        mode_parts.push(Span::styled(
            format!("JCode {}", env!("JCODE_VERSION")),
            Style::default().fg(DIM_COLOR),
        ));
    }

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
            " client ",
            Style::default().fg(Color::Black).bg(Color::Rgb(100, 149, 237)), // Cornflower blue badge
        ));
    }

    lines.push(Line::from(mode_parts));

    // Line 2: Model ID and build age (dimmed)
    let build_info = binary_age().unwrap_or_else(|| "unknown".to_string());
    lines.push(Line::from(Span::styled(
        format!("{} · built {}", model, build_info),
        Style::default().fg(DIM_COLOR),
    )));

    // Line 3+: Recent changes in a box (from git log, embedded at build time)
    let changelog = env!("JCODE_CHANGELOG");
    let term_width = area.width as usize;
    if !changelog.is_empty() && term_width > 20 {
        let changelog_lines: Vec<&str> = changelog.lines().collect();
        if !changelog_lines.is_empty() {
            // Determine box width based on terminal width (leave some margin)
            let available_width = term_width.saturating_sub(2); // Leave margin
            const MAX_LINES: usize = 5;

            // Cap content width to available space minus box chars (│ + space + space + │ = 4)
            let max_content_width = changelog_lines.iter()
                .take(MAX_LINES)
                .map(|l| l.chars().count())
                .max()
                .unwrap_or(0)
                .min(available_width.saturating_sub(4));

            // Minimum usable width
            if max_content_width < 10 {
                // Too narrow - skip the box
            } else {
                let box_width = max_content_width + 4; // +4 for "│ " and " │"

                // Top border with title centered: ──── Updates ────
                let title = " Updates ";
                let title_len = title.chars().count();
                let border_chars = box_width.saturating_sub(title_len + 2); // -2 for corners
                let left_border = "─".repeat(border_chars / 2);
                let right_border = "─".repeat(border_chars - border_chars / 2);
                lines.push(Line::from(Span::styled(
                    format!("┌{}{}{}┐", left_border, title, right_border),
                    Style::default().fg(DIM_COLOR),
                )));

                // Content lines (truncate each line if too long, limit total lines)
                let display_lines = changelog_lines.len().min(MAX_LINES);
                let has_more = changelog_lines.len() > MAX_LINES;

                for line in changelog_lines.iter().take(display_lines) {
                    let truncated = if line.chars().count() > max_content_width {
                        format!("{}…", line.chars().take(max_content_width.saturating_sub(1)).collect::<String>())
                    } else {
                        line.to_string()
                    };
                    let padding = max_content_width.saturating_sub(truncated.chars().count());
                    lines.push(Line::from(Span::styled(
                        format!("│ {}{} │", truncated, " ".repeat(padding)),
                        Style::default().fg(DIM_COLOR),
                    )));
                }

                // Show truncation indicator if there are more
                if has_more {
                    let more_text = format!("…{} more", changelog_lines.len() - MAX_LINES);
                    let padding = max_content_width.saturating_sub(more_text.chars().count());
                    lines.push(Line::from(Span::styled(
                        format!("│ {}{} │", more_text, " ".repeat(padding)),
                        Style::default().fg(DIM_COLOR),
                    )));
                }

                // Bottom border
                let bottom_border = "─".repeat(box_width.saturating_sub(2));
                lines.push(Line::from(Span::styled(
                    format!("└{}┘", bottom_border),
                    Style::default().fg(DIM_COLOR),
                )));
            }
        }
    }

    // Line 4: MCPs (if any)
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
                    if app.show_diffs() && matches!(tc.name.as_str(), "edit" | "Edit" | "write" | "multiedit") {
                        // Extract file extension for syntax highlighting
                        let file_ext = tc.input.get("file_path")
                            .and_then(|v| v.as_str())
                            .and_then(|p| std::path::Path::new(p).extension())
                            .and_then(|e| e.to_str());

                        // Collect only actual change lines (+ and -)
                        let change_lines: Vec<&str> = msg.content.lines()
                            .skip(1)
                            .filter(|line| {
                                let trimmed = line.trim();
                                !trimmed.is_empty() &&
                                trimmed != "..." &&
                                (trimmed.contains("+ ") || trimmed.contains("- "))
                            })
                            .collect();

                        const MAX_DIFF_LINES: usize = 12;
                        let total_changes = change_lines.len();

                        // Count additions and deletions for summary
                        let additions = change_lines.iter().filter(|l| l.contains("+ ")).count();
                        let deletions = change_lines.iter().filter(|l| l.contains("- ")).count();

                        // Determine which lines to show
                        let (display_lines, truncated): (Vec<&str>, bool) = if total_changes <= MAX_DIFF_LINES {
                            (change_lines, false)
                        } else {
                            // Show first half and last half, with truncation indicator
                            let half = MAX_DIFF_LINES / 2;
                            let mut result: Vec<&str> = change_lines.iter().take(half).copied().collect();
                            result.extend(change_lines.iter().skip(total_changes - half).copied());
                            (result, true)
                        };

                        let mut shown_truncation = false;
                        let half_point = if truncated { MAX_DIFF_LINES / 2 } else { usize::MAX };

                        for (i, line) in display_lines.iter().enumerate() {
                            // Show truncation marker at the midpoint
                            if truncated && !shown_truncation && i >= half_point {
                                let skipped = total_changes - MAX_DIFF_LINES;
                                lines.push(Line::from(Span::styled(
                                    format!("    ... {} more changes ...", skipped),
                                    Style::default().fg(DIM_COLOR),
                                )));
                                shown_truncation = true;
                            }

                            let trimmed = line.trim();
                            let is_add = trimmed.contains("+ ");
                            let base_color = if is_add { DIFF_ADD_COLOR } else { DIFF_DEL_COLOR };

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
                                for span in highlighted {
                                    let tinted = tint_span_with_diff_color(span, base_color);
                                    spans.push(tinted);
                                }
                            }

                            lines.push(Line::from(spans));
                        }

                        // Show summary if there were changes
                        if total_changes > 0 && truncated {
                            lines.push(Line::from(Span::styled(
                                format!("    (+{} -{} total)", additions, deletions),
                                Style::default().fg(DIM_COLOR),
                            )));
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

    let line = if let Some(notice) = app.status_notice() {
        Line::from(vec![
            Span::styled(notice, Style::default().fg(ACCENT_COLOR)),
        ])
    } else if app.is_processing() {
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
        // Idle - show token warning if high usage, otherwise nothing
        if let Some((total_in, total_out)) = app.total_session_tokens() {
            let total = total_in + total_out;
            if total > 100_000 {
                // High usage warning (>100k tokens)
                let warning_color = if total > 150_000 {
                    Color::Rgb(255, 100, 100) // Red for very high
                } else {
                    Color::Rgb(255, 193, 7) // Amber for high
                };
                Line::from(vec![
                    Span::styled("⚠ ", Style::default().fg(warning_color)),
                    Span::styled(
                        format!("Session: {}k tokens ", total / 1000),
                        Style::default().fg(warning_color)
                    ),
                    Span::styled(
                        "(consider /clear for fresh context)",
                        Style::default().fg(DIM_COLOR)
                    ),
                ])
            } else {
                Line::from(Span::styled("", Style::default().fg(DIM_COLOR)))
            }
        } else {
            Line::from(Span::styled("", Style::default().fg(DIM_COLOR)))
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

    // Build all wrapped lines with cursor tracking
    let (all_lines, cursor_line, cursor_col) = wrap_input_text(
        input_text,
        cursor_pos,
        line_width,
        &num_str,
        prompt_char,
        caret_color,
        prompt_len,
    );

    // Show command suggestions if available (prepended to lines)
    let mut lines: Vec<Line> = Vec::new();
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
    } else if app.is_processing() && !input_text.is_empty() {
        // Show hint for Shift+Enter when processing and user has typed something
        lines.push(Line::from(Span::styled(
            "  Shift+Enter to send now",
            Style::default().fg(DIM_COLOR),
        )));
    }

    let suggestions_offset = lines.len();
    let total_input_lines = all_lines.len();
    let visible_height = area.height as usize;

    // Calculate scroll offset to keep cursor visible
    // The cursor_line is relative to input lines (0-indexed)
    let scroll_offset = if total_input_lines + suggestions_offset <= visible_height {
        // Everything fits, no scrolling needed
        0
    } else {
        // Need to scroll - ensure cursor line is visible
        let available_for_input = visible_height.saturating_sub(suggestions_offset);
        if cursor_line < available_for_input {
            0
        } else {
            // Scroll so cursor is near the bottom of visible area
            cursor_line.saturating_sub(available_for_input.saturating_sub(1))
        }
    };

    // Add visible input lines (after scroll offset)
    for line in all_lines.into_iter().skip(scroll_offset) {
        lines.push(line);
        if lines.len() >= visible_height {
            break;
        }
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);

    // Calculate cursor screen position
    let cursor_screen_line = cursor_line.saturating_sub(scroll_offset) + suggestions_offset;
    let cursor_y = area.y + (cursor_screen_line as u16).min(area.height.saturating_sub(1));
    let cursor_x = area.x + prompt_len as u16 + cursor_col as u16;

    frame.set_cursor_position(Position::new(cursor_x, cursor_y));
}

/// Wrap input text into lines, handling explicit newlines and tracking cursor position.
/// Returns (lines, cursor_line, cursor_col) where cursor_line/col are in wrapped coordinates.
fn wrap_input_text<'a>(
    input: &str,
    cursor_pos: usize,
    line_width: usize,
    num_str: &str,
    prompt_char: &'a str,
    caret_color: Color,
    prompt_len: usize,
) -> (Vec<Line<'a>>, usize, usize) {
    let mut lines: Vec<Line> = Vec::new();
    let mut cursor_line = 0;
    let mut cursor_col = 0;
    let mut char_count = 0;
    let mut found_cursor = false;

    let chars: Vec<char> = input.chars().collect();

    // Handle empty input
    if chars.is_empty() {
        let num_color = rainbow_prompt_color(0);
        lines.push(Line::from(vec![
            Span::styled(num_str.to_string(), Style::default().fg(num_color)),
            Span::styled(prompt_char.to_string(), Style::default().fg(caret_color)),
        ]));
        return (lines, 0, 0);
    }

    // Split by newlines first, then wrap each segment
    let mut pos = 0;
    while pos <= chars.len() {
        // Find next newline or end
        let newline_pos = chars[pos..].iter().position(|&c| c == '\n');
        let segment_end = match newline_pos {
            Some(rel_pos) => pos + rel_pos,
            None => chars.len(),
        };

        let segment: Vec<char> = chars[pos..segment_end].to_vec();

        // Wrap this segment
        let mut seg_pos = 0;
        loop {
            let end = (seg_pos + line_width).min(segment.len());
            let line_text: String = segment[seg_pos..end].iter().collect();

            // Track cursor position
            let line_start_char = char_count;
            let line_end_char = char_count + (end - seg_pos);

            if !found_cursor && cursor_pos >= line_start_char && cursor_pos <= line_end_char {
                cursor_line = lines.len();
                cursor_col = cursor_pos - line_start_char;
                found_cursor = true;
            }
            char_count = line_end_char;

            if lines.is_empty() {
                // First line has prompt
                let num_color = rainbow_prompt_color(0);
                lines.push(Line::from(vec![
                    Span::styled(num_str.to_string(), Style::default().fg(num_color)),
                    Span::styled(prompt_char.to_string(), Style::default().fg(caret_color)),
                    Span::raw(line_text),
                ]));
            } else {
                // Continuation lines
                lines.push(Line::from(vec![
                    Span::raw(" ".repeat(prompt_len)),
                    Span::raw(line_text),
                ]));
            }

            if end >= segment.len() {
                break;
            }
            seg_pos = end;
        }

        // Account for the newline character itself in cursor tracking
        if newline_pos.is_some() {
            if !found_cursor && cursor_pos == char_count {
                cursor_line = lines.len().saturating_sub(1);
                cursor_col = lines.last().map(|l| {
                    l.spans.iter().skip(1).map(|s| s.content.chars().count()).sum::<usize>()
                }).unwrap_or(0);
                found_cursor = true;
            }
            char_count += 1; // newline char
            pos = segment_end + 1;
        } else {
            break;
        }
    }

    // Handle cursor at very end
    if !found_cursor {
        cursor_line = lines.len().saturating_sub(1);
        cursor_col = lines.last().map(|l| {
            // Skip the prompt spans and count content
            l.spans.iter().skip(if cursor_line == 0 { 2 } else { 1 })
                .map(|s| s.content.chars().count()).sum::<usize>()
        }).unwrap_or(0);
    }

    (lines, cursor_line, cursor_col)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_input_lines_empty() {
        assert_eq!(calculate_input_lines("", 80), 1);
    }

    #[test]
    fn test_calculate_input_lines_single_line() {
        assert_eq!(calculate_input_lines("hello", 80), 1);
        assert_eq!(calculate_input_lines("hello world", 80), 1);
    }

    #[test]
    fn test_calculate_input_lines_wrapped() {
        // 10 chars with width 5 = 2 lines
        assert_eq!(calculate_input_lines("aaaaaaaaaa", 5), 2);
        // 15 chars with width 5 = 3 lines
        assert_eq!(calculate_input_lines("aaaaaaaaaaaaaaa", 5), 3);
    }

    #[test]
    fn test_calculate_input_lines_with_newlines() {
        // Two lines separated by newline
        assert_eq!(calculate_input_lines("hello\nworld", 80), 2);
        // Three lines
        assert_eq!(calculate_input_lines("a\nb\nc", 80), 3);
        // Trailing newline
        assert_eq!(calculate_input_lines("hello\n", 80), 2);
    }

    #[test]
    fn test_calculate_input_lines_newlines_and_wrapping() {
        // First line wraps (10 chars / 5 = 2), second line is short (1)
        assert_eq!(calculate_input_lines("aaaaaaaaaa\nb", 5), 3);
    }

    #[test]
    fn test_calculate_input_lines_zero_width() {
        assert_eq!(calculate_input_lines("hello", 0), 1);
    }

    #[test]
    fn test_wrap_input_text_empty() {
        let (lines, cursor_line, cursor_col) = wrap_input_text(
            "", 0, 80, "1", "> ", USER_COLOR, 3
        );
        assert_eq!(lines.len(), 1);
        assert_eq!(cursor_line, 0);
        assert_eq!(cursor_col, 0);
    }

    #[test]
    fn test_wrap_input_text_simple() {
        let (lines, cursor_line, cursor_col) = wrap_input_text(
            "hello", 5, 80, "1", "> ", USER_COLOR, 3
        );
        assert_eq!(lines.len(), 1);
        assert_eq!(cursor_line, 0);
        assert_eq!(cursor_col, 5); // cursor at end
    }

    #[test]
    fn test_wrap_input_text_cursor_middle() {
        let (lines, cursor_line, cursor_col) = wrap_input_text(
            "hello world", 6, 80, "1", "> ", USER_COLOR, 3
        );
        assert_eq!(lines.len(), 1);
        assert_eq!(cursor_line, 0);
        assert_eq!(cursor_col, 6); // cursor at 'w'
    }

    #[test]
    fn test_wrap_input_text_wrapping() {
        // 10 chars with width 5 = 2 lines
        let (lines, cursor_line, cursor_col) = wrap_input_text(
            "aaaaaaaaaa", 7, 5, "1", "> ", USER_COLOR, 3
        );
        assert_eq!(lines.len(), 2);
        assert_eq!(cursor_line, 1); // second line
        assert_eq!(cursor_col, 2);  // 7 - 5 = 2
    }

    #[test]
    fn test_wrap_input_text_with_newlines() {
        let (lines, cursor_line, cursor_col) = wrap_input_text(
            "hello\nworld", 6, 80, "1", "> ", USER_COLOR, 3
        );
        assert_eq!(lines.len(), 2);
        assert_eq!(cursor_line, 1); // second line (after newline)
        assert_eq!(cursor_col, 0);  // at start of 'world'
    }

    #[test]
    fn test_wrap_input_text_cursor_at_end_of_wrapped() {
        // 10 chars with width 5, cursor at position 10 (end)
        let (lines, cursor_line, cursor_col) = wrap_input_text(
            "aaaaaaaaaa", 10, 5, "1", "> ", USER_COLOR, 3
        );
        assert_eq!(lines.len(), 2);
        assert_eq!(cursor_line, 1);
        assert_eq!(cursor_col, 5);
    }

    #[test]
    fn test_wrap_input_text_many_lines() {
        // Create text that spans 15 lines when wrapped to width 10
        let text = "a".repeat(150);
        let (lines, cursor_line, cursor_col) = wrap_input_text(
            &text, 145, 10, "1", "> ", USER_COLOR, 3
        );
        assert_eq!(lines.len(), 15);
        assert_eq!(cursor_line, 14); // last line
        assert_eq!(cursor_col, 5);   // 145 % 10 = 5
    }

    #[test]
    fn test_wrap_input_text_multiple_newlines() {
        let (lines, cursor_line, cursor_col) = wrap_input_text(
            "a\nb\nc\nd", 6, 80, "1", "> ", USER_COLOR, 3
        );
        assert_eq!(lines.len(), 4);
        assert_eq!(cursor_line, 3); // on 'd' line
        assert_eq!(cursor_col, 0);
    }
}
