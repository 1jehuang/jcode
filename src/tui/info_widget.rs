//! InfoWidget - A floating information panel that appears in empty screen space
//!
//! This widget polls periodically to detect available empty space on the right side
//! of the terminal and displays contextual information like todos, client count, etc.
//!
//! Design: Check every ~10 seconds for empty space, resize/show/hide accordingly.
//! Uses a global state to track polling across frames.

use crate::todo::TodoItem;
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph},
};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Minimum width needed to show the widget
const MIN_WIDGET_WIDTH: u16 = 24;
/// Maximum width the widget can take
const MAX_WIDGET_WIDTH: u16 = 40;
/// Minimum height needed to show the widget
const MIN_WIDGET_HEIGHT: u16 = 5;
/// How often to recalculate widget visibility/position
const POLL_INTERVAL: Duration = Duration::from_secs(10);
/// Minimum content width before we consider showing the widget
const MIN_CONTENT_WIDTH: u16 = 60;

/// Data to display in the info widget
#[derive(Debug, Default, Clone)]
pub struct InfoWidgetData {
    pub todos: Vec<TodoItem>,
    pub client_count: Option<usize>,
    pub session_tokens: Option<(u64, u64)>,
}

impl InfoWidgetData {
    pub fn is_empty(&self) -> bool {
        self.todos.is_empty() && self.client_count.is_none()
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
    /// Terminal size at time of calculation
    term_size: (u16, u16),
    /// Max content width at time of calculation
    max_content_width: u16,
    /// Last time we recalculated the layout
    last_poll: Option<Instant>,
}

impl Default for WidgetState {
    fn default() -> Self {
        Self {
            visible: false,
            enabled: true,
            rect: Rect::default(),
            term_size: (0, 0),
            max_content_width: 0,
            last_poll: None,
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
    term_size: (0, 0),
    max_content_width: 0,
    last_poll: None,
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

/// Check if we should recalculate the layout (poll interval elapsed or inputs changed)
fn should_recalculate(state: &WidgetState, term_width: u16, term_height: u16, max_content_width: u16) -> bool {
    // Always recalculate if terminal size changed
    if state.term_size != (term_width, term_height) {
        return true;
    }

    // Recalculate if content width changed significantly
    if (state.max_content_width as i32 - max_content_width as i32).abs() > 10 {
        return true;
    }

    // Otherwise, check poll interval
    match state.last_poll {
        None => true,
        Some(last) => last.elapsed() >= POLL_INTERVAL,
    }
}

/// Calculate the widget layout based on available space
/// Returns the Rect where the widget should be drawn, or None if it shouldn't show
pub fn calculate_layout(
    term_width: u16,
    term_height: u16,
    messages_area: Rect,
    max_content_width: u16,
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

    // Check if we need to recalculate
    if !should_recalculate(&state, term_width, term_height, max_content_width) {
        return if state.visible {
            Some(state.rect)
        } else {
            None
        };
    }

    // Update poll time and cache
    state.last_poll = Some(Instant::now());
    state.term_size = (term_width, term_height);
    state.max_content_width = max_content_width;

    // Calculate available empty space on the right
    // Only show widget if there's clearly empty space
    let effective_content_width = max_content_width.max(MIN_CONTENT_WIDTH);
    let empty_on_right = term_width.saturating_sub(effective_content_width);

    if empty_on_right < MIN_WIDGET_WIDTH {
        state.visible = false;
        return None;
    }

    // Calculate widget dimensions
    let widget_width = empty_on_right.min(MAX_WIDGET_WIDTH);

    // Calculate needed height based on content
    let needed_height = calculate_needed_height(data);

    // Position in the messages area, right-aligned with some padding
    let available_height = messages_area.height.saturating_sub(4); // Leave room at top/bottom

    if available_height < MIN_WIDGET_HEIGHT {
        state.visible = false;
        return None;
    }

    let widget_height = needed_height.min(available_height);

    // Position: right side with 1 char padding, vertically centered in upper portion
    let x = term_width.saturating_sub(widget_width).saturating_sub(1);
    let y = messages_area.y + 3; // Below header

    let rect = Rect::new(x, y, widget_width, widget_height);

    state.visible = true;
    state.rect = rect;

    Some(rect)
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

    // Client count
    if data.client_count.is_some() {
        height += 1;
    }

    // Token usage
    if data.session_tokens.is_some() {
        height += 1;
    }

    height.max(MIN_WIDGET_HEIGHT)
}

/// Render the widget to the frame
pub fn render(frame: &mut Frame, rect: Rect, data: &InfoWidgetData) {
    // Semi-transparent looking border (using dim colors)
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(60, 60, 70)))
        .style(Style::default().bg(Color::Rgb(25, 25, 30)));

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

    // Client count
    if let Some(count) = data.client_count {
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        let icon = if count > 1 { "" } else { "" };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{} ", icon),
                Style::default().fg(Color::Rgb(100, 160, 200)),
            ),
            Span::styled(
                format!("{} client{}", count, if count == 1 { "" } else { "s" }),
                Style::default().fg(Color::Rgb(140, 140, 150)),
            ),
        ]));
    }

    // Token usage
    if let Some((input, output)) = data.session_tokens {
        if !lines.is_empty() && data.client_count.is_none() {
            lines.push(Line::from(""));
        }
        let total_k = (input + output) / 1000;
        lines.push(Line::from(vec![
            Span::styled(" ", Style::default().fg(Color::Rgb(180, 140, 200))),
            Span::styled(
                format!("{}k tokens", total_k),
                Style::default().fg(Color::Rgb(140, 140, 150)),
            ),
        ]));
    }

    let para = Paragraph::new(lines);
    frame.render_widget(para, inner);
}
