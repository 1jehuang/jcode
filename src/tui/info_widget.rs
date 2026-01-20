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
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Memory statistics for the info widget
#[derive(Debug, Default, Clone)]
pub struct MemoryInfo {
    /// Total memory count (project + global)
    pub total_count: usize,
    /// Project-specific memory count
    pub project_count: usize,
    /// Global memory count
    pub global_count: usize,
    /// Count by category
    pub by_category: HashMap<String, usize>,
    /// Whether sidecar is available
    pub sidecar_available: bool,
}

/// Minimum width needed to show the widget
const MIN_WIDGET_WIDTH: u16 = 24;
/// Maximum width the widget can take
const MAX_WIDGET_WIDTH: u16 = 40;
/// Minimum height needed to show the widget
const MIN_WIDGET_HEIGHT: u16 = 5;
const PAGE_SWITCH_SECONDS: u64 = 30;

/// Data to display in the info widget
#[derive(Debug, Default, Clone)]
pub struct InfoWidgetData {
    pub todos: Vec<TodoItem>,
    pub context_info: Option<ContextInfo>,
    pub queue_mode: Option<bool>,
    pub context_limit: Option<usize>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub session_count: Option<usize>,
    pub client_count: Option<usize>,
    /// Memory system statistics
    pub memory_info: Option<MemoryInfo>,
    // TODO: Add swarm/subagent status summary to the info widget.
}

impl InfoWidgetData {
    pub fn is_empty(&self) -> bool {
        self.todos.is_empty()
            && self.context_info.is_none()
            && self.queue_mode.is_none()
            && self.model.is_none()
            && self.memory_info.is_none()
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
    /// Current page index
    page_index: usize,
    /// Last time the page advanced
    last_page_switch: Option<Instant>,
}

impl Default for WidgetState {
    fn default() -> Self {
        Self {
            visible: false,
            enabled: true,
            rect: Rect::default(),
            page_index: 0,
            last_page_switch: None,
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
    page_index: 0,
    last_page_switch: None,
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

    let best = find_largest_empty_rect(free_widths, MIN_WIDGET_WIDTH, MIN_WIDGET_HEIGHT)?;
    let (top, height, max_width) = best;

    let widget_width = max_width.min(MAX_WIDGET_WIDTH);
    let inner_width = widget_width.saturating_sub(2) as usize;
    let available_inner_height = height.saturating_sub(2);
    let layout = compute_page_layout(data, inner_width, available_inner_height);
    if layout.pages.is_empty() {
        state.visible = false;
        return None;
    }

    let widget_height = layout
        .max_page_height
        .saturating_add(2)
        .max(MIN_WIDGET_HEIGHT)
        .min(height);

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
    state.page_index = state.page_index.min(layout.pages.len().saturating_sub(1));

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
/// Render the widget to the frame
pub fn render(frame: &mut Frame, rect: Rect, data: &InfoWidgetData) {
    // Semi-transparent looking border (using dim colors)
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(70, 70, 80)).dim());

    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let layout = compute_page_layout(data, inner.width as usize, inner.height);
    if layout.pages.is_empty() {
        return;
    }

    let mut state = match WIDGET_STATE.lock() {
        Ok(s) => s,
        Err(_) => return,
    };
    if state.page_index >= layout.pages.len() {
        state.page_index = 0;
    }

    if layout.pages.len() > 1 {
        let now = Instant::now();
        let switch = match state.last_page_switch {
            Some(last) => now.duration_since(last) >= Duration::from_secs(PAGE_SWITCH_SECONDS),
            None => true,
        };
        if switch {
            state.page_index = (state.page_index + 1) % layout.pages.len();
            state.last_page_switch = Some(now);
        }
    } else {
        state.last_page_switch = None;
    }

    let page = layout.pages[state.page_index];
    let mut lines = render_page(page.kind, data, inner);

    if layout.show_dots {
        let content_height = inner.height.saturating_sub(1) as usize;
        if lines.len() > content_height {
            lines.truncate(content_height);
        } else if lines.len() < content_height {
            lines.extend(std::iter::repeat(Line::from("")).take(content_height - lines.len()));
        }
        lines.push(render_pagination_dots(
            layout.pages.len(),
            state.page_index,
            inner.width,
        ));
    }

    let para = Paragraph::new(lines);
    frame.render_widget(para, inner);
}

const MAX_CONTEXT_LINES: usize = 5;
const MAX_TODO_LINES: usize = 8;

#[derive(Clone, Copy, Debug)]
enum InfoPageKind {
    CompactOnly,
    TodosExpanded,
    ContextExpanded,
}

#[derive(Clone, Copy, Debug)]
struct InfoPage {
    kind: InfoPageKind,
    height: u16,
}

struct PageLayout {
    pages: Vec<InfoPage>,
    max_page_height: u16,
    show_dots: bool,
}

fn compute_page_layout(
    data: &InfoWidgetData,
    _inner_width: usize,
    inner_height: u16,
) -> PageLayout {
    let compact_height = compact_overview_height(data);
    if compact_height == 0 {
        return PageLayout {
            pages: Vec::new(),
            max_page_height: 0,
            show_dots: false,
        };
    }

    let mut candidates: Vec<InfoPage> = Vec::new();
    let context_compact = compact_context_height(data);
    let todos_compact = compact_todos_height(data);

    let context_expanded = expanded_context_height(data);
    if context_expanded > 0 {
        candidates.push(InfoPage {
            kind: InfoPageKind::ContextExpanded,
            height: compact_height - context_compact + context_expanded,
        });
    }

    let todos_expanded = expanded_todos_height(data);
    if todos_expanded > 0 {
        candidates.push(InfoPage {
            kind: InfoPageKind::TodosExpanded,
            height: compact_height - todos_compact + todos_expanded,
        });
    }

    let mut pages: Vec<InfoPage> = candidates
        .into_iter()
        .filter(|p| p.height <= inner_height)
        .collect();

    if pages.is_empty() {
        if compact_height <= inner_height {
            pages.push(InfoPage {
                kind: InfoPageKind::CompactOnly,
                height: compact_height,
            });
        } else {
            return PageLayout {
                pages,
                max_page_height: 0,
                show_dots: false,
            };
        }
    }

    let mut show_dots = false;
    if pages.len() > 1 {
        let filtered: Vec<InfoPage> = pages
            .iter()
            .copied()
            .filter(|p| p.height + 1 <= inner_height)
            .collect();
        if filtered.len() > 1 {
            pages = filtered;
            show_dots = true;
        } else if filtered.len() == 1 {
            pages = filtered;
        }
    }
    let max_page_height = pages
        .iter()
        .map(|p| p.height + if show_dots { 1 } else { 0 })
        .max()
        .unwrap_or(0);

    PageLayout {
        pages,
        max_page_height,
        show_dots,
    }
}

fn render_page(kind: InfoPageKind, data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    match kind {
        InfoPageKind::CompactOnly => render_sections(data, inner, None),
        InfoPageKind::TodosExpanded => render_sections(data, inner, Some(InfoPageKind::TodosExpanded)),
        InfoPageKind::ContextExpanded => render_sections(data, inner, Some(InfoPageKind::ContextExpanded)),
    }
}

fn compact_context_height(data: &InfoWidgetData) -> u16 {
    if let Some(info) = &data.context_info {
        if info.total_chars > 0 {
            return 1;
        }
    }
    0
}

fn compact_todos_height(data: &InfoWidgetData) -> u16 {
    if data.todos.is_empty() {
        0
    } else {
        2
    }
}

fn compact_queue_height(data: &InfoWidgetData) -> u16 {
    if data.queue_mode.is_some() {
        1
    } else {
        0
    }
}

fn compact_memory_height(data: &InfoWidgetData) -> u16 {
    if let Some(info) = &data.memory_info {
        if info.total_count > 0 {
            return 1;
        }
    }
    0
}

fn compact_model_height(data: &InfoWidgetData) -> u16 {
    if data.model.is_some() {
        // 1 line for model, +1 if we have session info
        if data.session_count.is_some() {
            2
        } else {
            1
        }
    } else {
        0
    }
}

fn compact_overview_height(data: &InfoWidgetData) -> u16 {
    compact_model_height(data)
        + compact_context_height(data)
        + compact_todos_height(data)
        + compact_queue_height(data)
        + compact_memory_height(data)
}

fn expanded_context_height(data: &InfoWidgetData) -> u16 {
    if let Some(info) = &data.context_info {
        if info.total_chars > 0 {
            return 3 + context_entries(info).len().min(MAX_CONTEXT_LINES) as u16;
        }
    }
    0
}

fn expanded_todos_height(data: &InfoWidgetData) -> u16 {
    if data.todos.is_empty() {
        return 0;
    }
    let todo_lines = data.todos.len().min(MAX_TODO_LINES);
    let mut height = 1 + todo_lines as u16;
    if data.todos.len() > MAX_TODO_LINES {
        height += 1;
    }
    height
}


fn render_sections(
    data: &InfoWidgetData,
    inner: Rect,
    focus: Option<InfoPageKind>,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Model info at the top
    if data.model.is_some() {
        lines.extend(render_model_info(data, inner));
    }

    if let Some(info) = &data.context_info {
        if info.total_chars > 0 {
            if matches!(focus, Some(InfoPageKind::ContextExpanded)) {
                lines.extend(render_context_expanded(data, inner));
            } else {
                lines.extend(render_context_compact(data, inner));
            }
        }
    }

    if !data.todos.is_empty() {
        if matches!(focus, Some(InfoPageKind::TodosExpanded)) {
            lines.extend(render_todos_expanded(data, inner));
        } else {
            lines.extend(render_todos_compact(data, inner));
        }
    }

    if data.queue_mode.is_some() {
        lines.extend(render_queue_compact(data, inner));
    }

    // Memory info at the bottom
    if let Some(info) = &data.memory_info {
        if info.total_count > 0 {
            lines.extend(render_memory_compact(info));
        }
    }

    lines
}

fn render_todos_expanded(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();
    if data.todos.is_empty() {
        return lines;
    }

    lines.push(Line::from(vec![Span::styled(
        "Todos",
        Style::default().fg(Color::Rgb(180, 180, 190)).bold(),
    )]));

    for todo in data.todos.iter().take(MAX_TODO_LINES) {
        let (icon, color) = match todo.status.as_str() {
            "completed" => ("", Color::Rgb(100, 180, 100)),
            "in_progress" => ("", Color::Rgb(255, 200, 100)),
            _ => ("", Color::Rgb(120, 120, 130)),
        };

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

    if data.todos.len() > MAX_TODO_LINES {
        lines.push(Line::from(vec![Span::styled(
            format!("  +{} more", data.todos.len() - MAX_TODO_LINES),
            Style::default().fg(Color::Rgb(100, 100, 110)),
        )]));
    }

    lines
}

fn render_todos_compact(data: &InfoWidgetData, _inner: Rect) -> Vec<Line<'static>> {
    if data.todos.is_empty() {
        return Vec::new();
    }
    let total = data.todos.len();
    let mut completed = 0usize;
    let mut in_progress = 0usize;
    for todo in &data.todos {
        match todo.status.as_str() {
            "completed" => completed += 1,
            "in_progress" => in_progress += 1,
            _ => {}
        }
    }
    let pending = total.saturating_sub(completed);
    vec![
        Line::from(vec![Span::styled(
            "Todos",
            Style::default().fg(Color::Rgb(180, 180, 190)).bold(),
        )]),
        Line::from(vec![
            Span::styled(
                format!("{} total", total),
                Style::default().fg(Color::Rgb(160, 160, 170)),
            ),
            Span::styled(" · ", Style::default().fg(Color::Rgb(100, 100, 110))),
            Span::styled(
                format!("{} active", in_progress),
                Style::default().fg(Color::Rgb(255, 200, 100)),
            ),
            Span::styled(" · ", Style::default().fg(Color::Rgb(100, 100, 110))),
            Span::styled(
                format!("{} open", pending),
                Style::default().fg(Color::Rgb(140, 140, 150)),
            ),
        ]),
    ]
}

fn render_queue_compact(data: &InfoWidgetData, _inner: Rect) -> Vec<Line<'static>> {
    let Some(queue_mode) = data.queue_mode else {
        return Vec::new();
    };

    let (mode_text, mode_color) = if queue_mode {
        ("Wait", Color::Rgb(255, 200, 100))
    } else {
        ("ASAP", Color::Rgb(120, 200, 120))
    };

    vec![Line::from(vec![
        Span::styled("Queue: ", Style::default().fg(Color::Rgb(140, 140, 150))),
        Span::styled(mode_text, Style::default().fg(mode_color)),
    ])]
}

fn render_memory_compact(info: &MemoryInfo) -> Vec<Line<'static>> {
    let mut spans = vec![
        Span::styled("🧠 ", Style::default().fg(Color::Rgb(200, 150, 255))),
        Span::styled(
            format!("{}", info.total_count),
            Style::default().fg(Color::Rgb(180, 180, 190)),
        ),
        Span::styled(" mem", Style::default().fg(Color::Rgb(140, 140, 150))),
    ];

    // Show project/global breakdown if both exist
    if info.project_count > 0 && info.global_count > 0 {
        spans.push(Span::styled(
            format!(" ({}p/{}g)", info.project_count, info.global_count),
            Style::default().fg(Color::Rgb(100, 100, 110)),
        ));
    }

    vec![Line::from(spans)]
}

fn render_model_info(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let Some(model) = &data.model else {
        return Vec::new();
    };

    // Extract short model name (e.g., "claude-opus-4-5-20251101" -> "opus-4.5")
    let short_name = shorten_model_name(model);
    let max_len = inner.width.saturating_sub(2) as usize;

    let mut spans = vec![
        Span::styled("⚡ ", Style::default().fg(Color::Rgb(140, 180, 255))),
        Span::styled(
            if short_name.len() > max_len.saturating_sub(2) {
                format!("{}...", &short_name[..max_len.saturating_sub(5)])
            } else {
                short_name
            },
            Style::default().fg(Color::Rgb(180, 180, 190)).bold(),
        ),
    ];

    // Add reasoning effort if present
    if let Some(effort) = &data.reasoning_effort {
        let effort_short = match effort.as_str() {
            "xhigh" => "xhi",
            "high" => "hi",
            "medium" => "med",
            "low" => "lo",
            "none" => "∅",
            other => other,
        };
        spans.push(Span::styled(" ", Style::default()));
        spans.push(Span::styled(
            format!("({})", effort_short),
            Style::default().fg(Color::Rgb(255, 200, 100)),
        ));
    }

    let mut lines = vec![Line::from(spans)];

    // Add session info line if we have a session count
    if data.session_count.is_some() {
        let mut server_spans: Vec<Span> = Vec::new();

        if let Some(sessions) = data.session_count {
            server_spans.push(Span::styled(
                format!("{} session{}", sessions, if sessions == 1 { "" } else { "s" }),
                Style::default().fg(Color::Rgb(140, 140, 150)),
            ));
        }

        if !server_spans.is_empty() {
            lines.push(Line::from(server_spans));
        }
    }

    lines
}

fn shorten_model_name(model: &str) -> String {
    // Handle common model name patterns
    if model.contains("claude") {
        if model.contains("opus-4-5") || model.contains("opus-4.5") {
            return "opus-4.5".to_string();
        }
        if model.contains("sonnet-4") {
            return "sonnet-4".to_string();
        }
        if model.contains("sonnet-3-5") || model.contains("sonnet-3.5") {
            return "sonnet-3.5".to_string();
        }
        if model.contains("haiku") {
            return "haiku".to_string();
        }
        // Fallback: extract the model family
        if let Some(idx) = model.find("claude-") {
            let rest = &model[idx + 7..];
            if let Some(end) = rest.find('-') {
                return rest[..end].to_string();
            }
        }
    }

    if model.contains("gpt") {
        // e.g., "gpt-5.2-codex" -> "gpt-5.2"
        if let Some(start) = model.find("gpt-") {
            let rest = &model[start..];
            // Find second dash after version number
            let parts: Vec<&str> = rest.splitn(3, '-').collect();
            if parts.len() >= 2 {
                return format!("{}-{}", parts[0], parts[1]);
            }
        }
    }

    // Fallback: truncate long names
    if model.len() > 15 {
        format!("{}…", &model[..14])
    } else {
        model.to_string()
    }
}

fn render_context_expanded(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let Some(info) = &data.context_info else {
        return Vec::new();
    };
    if info.total_chars == 0 {
        return Vec::new();
    }

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![Span::styled(
        "Context",
        Style::default().fg(Color::Rgb(180, 180, 190)).bold(),
    )]));

    let used_tokens = info.estimated_tokens();
    let limit_tokens = data.context_limit.unwrap_or(200_000).max(1);
    let used_str = format_token_k(used_tokens);
    let limit_str = format_token_k(limit_tokens);
    let pct = ((used_tokens as f64 / limit_tokens as f64) * 100.0)
        .round()
        .min(100.0) as usize;
    lines.push(Line::from(vec![
        Span::styled("Usage ", Style::default().fg(Color::Rgb(160, 160, 170))),
        Span::styled(
            format!("{}/{} ({}%)", used_str, limit_str, pct),
            Style::default().fg(Color::Rgb(140, 140, 150)),
        ),
    ]));
    lines.push(render_usage_bar(used_tokens, limit_tokens, inner.width));

    let max_items = MAX_CONTEXT_LINES;
    let max_len = inner.width.saturating_sub(2) as usize;
    let total_tokens = used_tokens.max(1);
    for (icon, label, tokens) in context_entries(info).into_iter().take(max_items) {
        let pct = ((tokens as f64 / total_tokens as f64) * 100.0)
            .round()
            .min(100.0) as usize;
        let mut content = format!(
            "{} {} {} {}%",
            icon,
            label,
            format_token_k(tokens),
            pct
        );
        if content.len() > max_len && max_len > 3 {
            content.truncate(max_len.saturating_sub(3));
            content.push_str("...");
        }
        lines.push(Line::from(Span::styled(
            content,
            Style::default().fg(Color::Rgb(140, 140, 150)),
        )));
    }

    lines
}

fn render_context_compact(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let Some(info) = &data.context_info else {
        return Vec::new();
    };
    if info.total_chars == 0 {
        return Vec::new();
    }

    let used_tokens = info.estimated_tokens();
    let limit_tokens = data.context_limit.unwrap_or(200_000).max(1);
    vec![render_usage_line(used_tokens, limit_tokens, inner.width as usize)]
}

fn render_usage_line(used_tokens: usize, limit_tokens: usize, max_width: usize) -> Line<'static> {
    let used_str = format_token_k(used_tokens);
    let limit_str = format_token_k(limit_tokens);
    let pct = ((used_tokens as f64 / limit_tokens as f64) * 100.0)
        .round()
        .min(100.0) as usize;
    let mut text = format!("Ctx {}/{}", used_str, limit_str);
    if max_width >= text.len() + 5 {
        text.push(' ');
        text.push_str(&format!("{}%", pct));
    }
    Line::from(Span::styled(
        text,
        Style::default().fg(Color::Rgb(160, 160, 170)),
    ))
}

fn render_usage_bar(used_tokens: usize, limit_tokens: usize, width: u16) -> Line<'static> {
    let bar_width = width.saturating_sub(2).min(24).max(8) as usize;
    let mut used_cells = ((used_tokens as f64 / limit_tokens as f64) * bar_width as f64)
        .round()
        .max(0.0) as usize;
    if used_cells > bar_width {
        used_cells = bar_width;
    }
    let empty_cells = bar_width.saturating_sub(used_cells);
    let mut spans = Vec::new();
    spans.push(Span::styled("[", Style::default().fg(Color::Rgb(90, 90, 100))));
    spans.push(Span::styled(
        "█".repeat(used_cells),
        Style::default().fg(Color::Rgb(120, 200, 180)),
    ));
    if empty_cells > 0 {
        spans.push(Span::styled(
            "░".repeat(empty_cells),
            Style::default().fg(Color::Rgb(50, 50, 60)),
        ));
    }
    spans.push(Span::styled("]", Style::default().fg(Color::Rgb(90, 90, 100))));
    Line::from(spans)
}

fn format_token_k(tokens: usize) -> String {
    if tokens >= 1000 {
        format!("{}k", tokens / 1000)
    } else {
        format!("{}", tokens)
    }
}

fn render_pagination_dots(count: usize, current: usize, width: u16) -> Line<'static> {
    if count == 0 {
        return Line::from("");
    }
    let mut dots = String::new();
    for i in 0..count {
        dots.push(if i == current { '•' } else { '·' });
        if i + 1 < count {
            dots.push(' ');
        }
    }
    let pad = width
        .saturating_sub(dots.chars().count() as u16)
        .saturating_div(2);
    Line::from(vec![
        Span::raw(" ".repeat(pad as usize)),
        Span::styled(dots, Style::default().fg(Color::Rgb(140, 140, 150))),
    ])
}

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
