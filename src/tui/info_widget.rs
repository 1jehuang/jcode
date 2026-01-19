//! InfoWidget - A floating information panel that appears in empty screen space
//!
//! This widget finds the largest empty rectangle on the right side of the
//! visible message area and renders a compact info panel there.

use crate::prompt::ContextInfo;
use crate::todo::TodoItem;
use ratatui::{
    prelude::*,
    widgets::{Block, BorderType, Borders, Paragraph},
};
use std::sync::Mutex;

/// Minimum width needed to show the widget
const MIN_WIDGET_WIDTH: u16 = 24;
/// Maximum width the widget can take
const MAX_WIDGET_WIDTH: u16 = 40;
/// Minimum height needed to show the widget
const MIN_WIDGET_HEIGHT: u16 = 5;

/// Data to display in the info widget
#[derive(Debug, Default, Clone)]
pub struct InfoWidgetData {
    pub todos: Vec<TodoItem>,
    pub context_info: Option<ContextInfo>,
    // TODO: Add swarm/subagent status summary to the info widget.
}

impl InfoWidgetData {
    pub fn is_empty(&self) -> bool {
        self.todos.is_empty() && self.context_info.is_none()
    }
}

/// Cached layout calculation for the widget
#[derive(Debug, Clone)]
struct WidgetState {
    /// Whether the widget should be visible
    visible: bool,
    /// Whether the user has disabled the widget
    enabled: bool,
    /// Calculated position and size
    rect: Rect,
}

impl Default for WidgetState {
    fn default() -> Self {
        Self {
            visible: false,
            enabled: true,
            rect: Rect::default(),
        }
    }
}

/// Global widget state (for polling across frames)
static WIDGET_STATE: Mutex<WidgetState> = Mutex::new(WidgetState {
    visible: false,
    enabled: true,
    rect: Rect {
        x: 0,
        y: 0,
        width: 0,
        height: 0,
    },
});

/// Toggle widget visibility (user preference)
pub fn toggle_enabled() {
    if let Ok(mut state) = WIDGET_STATE.lock() {
        state.enabled = !state.enabled;
    }
}

/// Check if widget is enabled by user
pub fn is_enabled() -> bool {
    WIDGET_STATE.lock().map(|s| s.enabled).unwrap_or(true)
}

/// Calculate the widget layout based on available space
/// Returns the Rect where the widget should be drawn, or None if it shouldn't show
pub fn calculate_layout(
    messages_area: Rect,
    free_widths: &[u16],
    data: &InfoWidgetData,
) -> Option<Rect> {
    let mut state = match WIDGET_STATE.lock() {
        Ok(s) => s,
        Err(_) => return None,
    };

    // User disabled
    if !state.enabled {
        state.visible = false;
        return None;
    }

    // Nothing to show
    if data.is_empty() {
        state.visible = false;
        return None;
    }

    if free_widths.is_empty() || messages_area.height == 0 || messages_area.width == 0 {
        state.visible = false;
        return None;
    }

    let needed_height = calculate_needed_height(data);
    let best = find_largest_empty_rect(free_widths, MIN_WIDGET_WIDTH, MIN_WIDGET_HEIGHT)?;
    let (top, height, max_width) = best;

    let widget_width = max_width.min(MAX_WIDGET_WIDTH);
    let widget_height = needed_height.min(height);

    if widget_height < MIN_WIDGET_HEIGHT || widget_width < MIN_WIDGET_WIDTH {
        state.visible = false;
        return None;
    }

    let x = messages_area.x + messages_area.width.saturating_sub(widget_width);
    let extra_height = height.saturating_sub(widget_height);
    let y = messages_area.y + top + (extra_height / 2);

    let rect = Rect::new(x, y, widget_width, widget_height);

    state.visible = true;
    state.rect = rect;

    Some(rect)
}

fn find_largest_empty_rect(
    free_widths: &[u16],
    min_width: u16,
    min_height: u16,
) -> Option<(u16, u16, u16)> {
    let mut best_area: u32 = 0;
    let mut best: Option<(u16, u16, u16)> = None;

    for start in 0..free_widths.len() {
        let mut min_w = free_widths[start];
        if min_w < min_width {
            continue;
        }
        for end in start..free_widths.len() {
            min_w = min_w.min(free_widths[end]);
            if min_w < min_width {
                break;
            }
            let height = (end - start + 1) as u16;
            if height < min_height {
                continue;
            }
            let width = min_w.min(MAX_WIDGET_WIDTH);
            let area = width as u32 * height as u32;
            if area > best_area {
                best_area = area;
                best = Some((start as u16, height, width));
            }
        }
    }

    best
}

/// Calculate how much height the widget needs based on its content
fn calculate_needed_height(data: &InfoWidgetData) -> u16 {
    let mut height: u16 = 2; // Border top/bottom

    // Todos section
    if !data.todos.is_empty() {
        height += 1; // Header "Todos"
        height += data.todos.len().min(8) as u16; // Show up to 8 todos
        height += 1; // Spacing
    }

    if let Some(info) = &data.context_info {
        if info.total_chars > 0 {
            height += 1; // Header "Context"
            height += 1; // Total tokens
            height += context_entries(info).len().min(MAX_CONTEXT_LINES) as u16;
            height += 1; // Spacing
        }
    }

    height.max(MIN_WIDGET_HEIGHT)
}

/// Render the widget to the frame
pub fn render(frame: &mut Frame, rect: Rect, data: &InfoWidgetData) {
    // Semi-transparent looking border (using dim colors)
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(70, 70, 80)).dim());

    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let mut lines: Vec<Line> = Vec::new();

    // Todos section
    if !data.todos.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            "Todos",
            Style::default().fg(Color::Rgb(180, 180, 190)).bold(),
        )]));

        for todo in data.todos.iter().take(8) {
            let (icon, color) = match todo.status.as_str() {
                "completed" => ("", Color::Rgb(100, 180, 100)),
                "in_progress" => ("", Color::Rgb(255, 200, 100)),
                _ => ("", Color::Rgb(120, 120, 130)),
            };

            // Truncate content to fit
            let max_len = inner.width.saturating_sub(3) as usize;
            let content = if todo.content.len() > max_len && max_len > 3 {
                format!("{}...", &todo.content[..max_len.saturating_sub(3)])
            } else {
                todo.content.clone()
            };

            lines.push(Line::from(vec![
                Span::styled(format!("{} ", icon), Style::default().fg(color)),
                Span::styled(content, Style::default().fg(Color::Rgb(160, 160, 170))),
            ]));
        }

        if data.todos.len() > 8 {
            lines.push(Line::from(vec![Span::styled(
                format!("  +{} more", data.todos.len() - 8),
                Style::default().fg(Color::Rgb(100, 100, 110)),
            )]));
        }
    }

    if let Some(info) = &data.context_info {
        if info.total_chars > 0 {
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }

            lines.push(Line::from(vec![Span::styled(
                "Context",
                Style::default().fg(Color::Rgb(180, 180, 190)).bold(),
            )]));

            let total_k = info.estimated_tokens() / 1000;
            lines.push(Line::from(vec![
                Span::styled("∑ ", Style::default().fg(Color::Rgb(160, 160, 170))),
                Span::styled(
                    format!("{}k tokens", total_k),
                    Style::default().fg(Color::Rgb(140, 140, 150)),
                ),
            ]));

            let max_items = MAX_CONTEXT_LINES;
            let max_len = inner.width.saturating_sub(2) as usize;
            for (icon, label, tokens) in context_entries(info).into_iter().take(max_items) {
                let mut content = format!("{} {} {}k", icon, label, tokens / 1000);
                if content.len() > max_len && max_len > 3 {
                    content.truncate(max_len.saturating_sub(3));
                    content.push_str("...");
                }
                lines.push(Line::from(Span::styled(
                    content,
                    Style::default().fg(Color::Rgb(140, 140, 150)),
                )));
            }
        }
    }

    let para = Paragraph::new(lines);
    frame.render_widget(para, inner);
}

const MAX_CONTEXT_LINES: usize = 5;

fn context_entries(info: &ContextInfo) -> Vec<(&'static str, &'static str, usize)> {
    let docs_chars = info.project_agents_md_chars
        + info.project_claude_md_chars
        + info.global_agents_md_chars
        + info.global_claude_md_chars;
    let skills_chars = info.skills_chars + info.selfdev_chars;
    let msgs_chars = info.user_messages_chars + info.assistant_messages_chars;
    let tool_io_chars = info.tool_calls_chars + info.tool_results_chars;

    let mut entries: Vec<(&'static str, &'static str, usize)> = Vec::new();
    if info.system_prompt_chars > 0 {
        entries.push(("⚙", "sys", info.system_prompt_chars / 4));
    }
    if info.env_context_chars > 0 {
        entries.push(("🌍", "env", info.env_context_chars / 4));
    }
    if docs_chars > 0 {
        entries.push(("📄", "docs", docs_chars / 4));
    }
    if skills_chars > 0 {
        entries.push(("🛠", "skills", skills_chars / 4));
    }
    if info.tool_defs_chars > 0 {
        entries.push(("🔨", "tools", info.tool_defs_chars / 4));
    }
    if msgs_chars > 0 {
        entries.push(("💬", "msgs", msgs_chars / 4));
    }
    if tool_io_chars > 0 {
        entries.push(("⚡", "tool io", tool_io_chars / 4));
    }

    entries.sort_by(|a, b| b.2.cmp(&a.2));
    entries
}
