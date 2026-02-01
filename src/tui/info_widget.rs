//! InfoWidget - Floating information panels that appear in empty screen space
//!
//! Supports multiple widget types with priority ordering and side preferences.
//! In centered mode, widgets can appear on both left and right margins.
//! In left-aligned mode, widgets only appear on the right margin.

use crate::prompt::ContextInfo;
use crate::provider::DEFAULT_CONTEXT_LIMIT;
use crate::todo::TodoItem;
use ratatui::{
    prelude::*,
    widgets::{Block, BorderType, Borders, Paragraph},
};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

/// Types of info widgets that can be displayed
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WidgetKind {
    /// Todo list with progress
    Todos,
    /// Token/context usage bar
    ContextUsage,
    /// Memory sidecar activity
    MemoryActivity,
    /// Subagents/sessions status
    SwarmStatus,
    /// Background work indicator
    BackgroundTasks,
    /// 5-hour/weekly subscription bars
    UsageLimits,
    /// Current model name
    ModelInfo,
}

impl WidgetKind {
    /// Priority for display (lower = higher priority)
    pub fn priority(self) -> u8 {
        match self {
            WidgetKind::Todos => 1,
            WidgetKind::ContextUsage => 2,
            WidgetKind::MemoryActivity => 3,
            WidgetKind::SwarmStatus => 4,
            WidgetKind::BackgroundTasks => 5,
            WidgetKind::UsageLimits => 6,
            WidgetKind::ModelInfo => 7,
        }
    }

    /// Preferred side for this widget
    pub fn preferred_side(self) -> Side {
        match self {
            WidgetKind::Todos => Side::Right,
            WidgetKind::ContextUsage => Side::Right,
            WidgetKind::MemoryActivity => Side::Right,
            WidgetKind::SwarmStatus => Side::Left,
            WidgetKind::BackgroundTasks => Side::Left,
            WidgetKind::UsageLimits => Side::Left,
            WidgetKind::ModelInfo => Side::Left,
        }
    }

    /// Minimum height needed for this widget
    pub fn min_height(self) -> u16 {
        match self {
            WidgetKind::Todos => 3,
            WidgetKind::ContextUsage => 2,
            WidgetKind::MemoryActivity => 3,
            WidgetKind::SwarmStatus => 3,
            WidgetKind::BackgroundTasks => 2,
            WidgetKind::UsageLimits => 3,
            WidgetKind::ModelInfo => 3, // Model + usage bars
        }
    }

    /// All widget kinds in priority order
    pub fn all_by_priority() -> &'static [WidgetKind] {
        &[
            WidgetKind::Todos,
            WidgetKind::ContextUsage,
            WidgetKind::MemoryActivity,
            WidgetKind::SwarmStatus,
            WidgetKind::BackgroundTasks,
            WidgetKind::UsageLimits,
            WidgetKind::ModelInfo,
        ]
    }
}

/// Which side of the screen a widget is on
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// A placed widget with its location and type
#[derive(Debug, Clone)]
pub struct WidgetPlacement {
    pub kind: WidgetKind,
    pub rect: Rect,
    pub side: Side,
}

/// Available margin space on one side
#[derive(Debug, Clone)]
pub struct MarginSpace {
    pub side: Side,
    /// Free width for each row (index = row from top of messages area)
    pub widths: Vec<u16>,
    /// X offset where this margin starts
    pub x_offset: u16,
}

/// Swarm/subagent status for the info widget
#[derive(Debug, Default, Clone)]
pub struct SwarmInfo {
    /// Number of sessions in the same swarm (same working directory)
    pub session_count: usize,
    /// Current subagent status (from Task tool execution)
    pub subagent_status: Option<String>,
    /// Number of connected clients (server mode)
    pub client_count: Option<usize>,
    /// List of session names in the swarm
    pub session_names: Vec<String>,
}

/// Background task status for the info widget
#[derive(Debug, Default, Clone)]
pub struct BackgroundInfo {
    /// Number of running background tasks
    pub running_count: usize,
    /// Names of running tasks (e.g., "bash", "task")
    pub running_tasks: Vec<String>,
    /// Memory agent status
    pub memory_agent_active: bool,
    /// Memory agent turn count
    pub memory_agent_turns: usize,
}

/// Which provider the usage info is for
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UsageProvider {
    #[default]
    None,
    /// Anthropic/Claude OAuth (shows subscription usage)
    Anthropic,
    /// OpenAI/Codex OAuth (shows subscription usage)
    OpenAI,
    /// OpenRouter/API-key providers (shows token costs)
    CostBased,
}

/// Authentication method used to access the model
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AuthMethod {
    #[default]
    Unknown,
    /// Anthropic OAuth (Claude Code CLI style)
    AnthropicOAuth,
    /// Anthropic API key
    AnthropicApiKey,
    /// OpenAI OAuth (Codex style)
    OpenAIOAuth,
    /// OpenAI API key
    OpenAIApiKey,
    /// OpenRouter API key
    OpenRouterApiKey,
}

/// Subscription usage info for the info widget
#[derive(Debug, Default, Clone)]
pub struct UsageInfo {
    /// Which provider this usage is for
    pub provider: UsageProvider,
    /// Five-hour window utilization (0.0-1.0) - for OAuth providers
    pub five_hour: f32,
    /// Seven-day window utilization (0.0-1.0) - for OAuth providers
    pub seven_day: f32,
    /// Total cost in USD - for API-key providers (OpenRouter, direct API key)
    pub total_cost: f32,
    /// Input tokens used - for cost calculation
    pub input_tokens: u64,
    /// Output tokens used - for cost calculation
    pub output_tokens: u64,
    /// Whether data was successfully fetched / available to show
    pub available: bool,
}

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
    /// Current memory activity
    pub activity: Option<MemoryActivity>,
}

/// Represents current memory system activity
#[derive(Debug, Clone)]
pub struct MemoryActivity {
    /// Current state of the memory system
    pub state: MemoryState,
    /// Recent events (most recent first)
    pub recent_events: Vec<MemoryEvent>,
}

/// State of the memory sidecar
#[derive(Debug, Clone, PartialEq)]
pub enum MemoryState {
    /// Idle, no activity
    Idle,
    /// Running embedding search
    Embedding,
    /// Sidecar checking relevance
    SidecarChecking { count: usize },
    /// Found relevant memories
    FoundRelevant { count: usize },
    /// Extracting memories from conversation
    Extracting { reason: String },
}

impl Default for MemoryState {
    fn default() -> Self {
        MemoryState::Idle
    }
}

/// A memory system event
#[derive(Debug, Clone)]
pub struct MemoryEvent {
    /// Type of event
    pub kind: MemoryEventKind,
    /// When it happened
    pub timestamp: Instant,
    /// Optional details
    pub detail: Option<String>,
}

#[derive(Debug, Clone)]
pub enum MemoryEventKind {
    /// Embedding search started
    EmbeddingStarted,
    /// Embedding search completed
    EmbeddingComplete { latency_ms: u64, hits: usize },
    /// Sidecar started checking
    SidecarStarted,
    /// Sidecar found memory relevant
    SidecarRelevant { memory_preview: String },
    /// Sidecar found memory not relevant
    SidecarNotRelevant,
    /// Sidecar call completed with latency
    SidecarComplete { latency_ms: u64 },
    /// Memory was surfaced to main agent
    MemorySurfaced { memory_preview: String },
    /// Extraction started
    ExtractionStarted { reason: String },
    /// Extraction completed
    ExtractionComplete { count: usize },
    /// Error occurred
    Error { message: String },
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
    /// Swarm/subagent status
    pub swarm_info: Option<SwarmInfo>,
    /// Background tasks status
    pub background_info: Option<BackgroundInfo>,
    /// Subscription usage info
    pub usage_info: Option<UsageInfo>,
    /// Authentication method used to access the model
    pub auth_method: AuthMethod,
}

impl InfoWidgetData {
    pub fn is_empty(&self) -> bool {
        self.todos.is_empty()
            && self.context_info.is_none()
            && self.queue_mode.is_none()
            && self.model.is_none()
            && self.memory_info.is_none()
            && self.swarm_info.is_none()
            && self.background_info.is_none()
    }

    /// Check if a specific widget kind has data to display
    pub fn has_data_for(&self, kind: WidgetKind) -> bool {
        match kind {
            WidgetKind::Todos => !self.todos.is_empty(),
            WidgetKind::ContextUsage => self
                .context_info
                .as_ref()
                .map(|c| c.total_chars > 0)
                .unwrap_or(false),
            WidgetKind::MemoryActivity => self
                .memory_info
                .as_ref()
                .map(|m| m.total_count > 0 || m.activity.is_some())
                .unwrap_or(false),
            WidgetKind::SwarmStatus => self
                .swarm_info
                .as_ref()
                .map(|s| {
                    s.subagent_status.is_some() || s.session_count > 1 || s.client_count.is_some()
                })
                .unwrap_or(false),
            WidgetKind::BackgroundTasks => self
                .background_info
                .as_ref()
                .map(|b| b.running_count > 0 || b.memory_agent_active)
                .unwrap_or(false),
            WidgetKind::UsageLimits => false, // Combined into ModelInfo
            WidgetKind::ModelInfo => self.model.is_some(),
        }
    }

    /// Get list of widget kinds that have data, in priority order
    pub fn available_widgets(&self) -> Vec<WidgetKind> {
        WidgetKind::all_by_priority()
            .iter()
            .copied()
            .filter(|&kind| self.has_data_for(kind))
            .collect()
    }
}

/// State for a single widget instance
#[derive(Debug, Clone)]
struct SingleWidgetState {
    /// Current page index (for widgets with multiple pages)
    page_index: usize,
    /// Last time the page advanced
    last_page_switch: Option<Instant>,
}

impl Default for SingleWidgetState {
    fn default() -> Self {
        Self {
            page_index: 0,
            last_page_switch: None,
        }
    }
}

/// Global state for all widgets
#[derive(Debug, Clone)]
struct WidgetsState {
    /// Whether the user has disabled widgets
    enabled: bool,
    /// Per-widget state (keyed by WidgetKind)
    widget_states: HashMap<WidgetKind, SingleWidgetState>,
    /// Current placements (updated each frame)
    placements: Vec<WidgetPlacement>,
}

impl Default for WidgetsState {
    fn default() -> Self {
        Self {
            enabled: true,
            widget_states: HashMap::new(),
            placements: Vec::new(),
        }
    }
}

/// Global widget state (for polling across frames)
static WIDGETS_STATE: Mutex<Option<WidgetsState>> = Mutex::new(None);

fn get_or_init_state() -> std::sync::MutexGuard<'static, Option<WidgetsState>> {
    let mut guard = WIDGETS_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Some(WidgetsState::default());
    }
    guard
}

/// Toggle widget visibility (user preference)
pub fn toggle_enabled() {
    let mut guard = get_or_init_state();
    if let Some(state) = guard.as_mut() {
        state.enabled = !state.enabled;
    }
}

/// Check if widget is enabled by user
pub fn is_enabled() -> bool {
    get_or_init_state()
        .as_ref()
        .map(|s| s.enabled)
        .unwrap_or(true)
}

/// Margin information for layout calculation
#[derive(Debug, Clone)]
pub struct Margins {
    /// Free widths on the right side for each row
    pub right_widths: Vec<u16>,
    /// Free widths on the left side for each row (only populated in centered mode)
    pub left_widths: Vec<u16>,
    /// Whether we're in centered mode
    pub centered: bool,
}

/// Calculate widget placements for multiple widgets
/// Returns a list of placements for widgets that fit
pub fn calculate_placements(
    messages_area: Rect,
    margins: &Margins,
    data: &InfoWidgetData,
) -> Vec<WidgetPlacement> {
    let mut guard = get_or_init_state();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return Vec::new(),
    };

    // User disabled
    if !state.enabled {
        state.placements.clear();
        return Vec::new();
    }

    if messages_area.height == 0 || messages_area.width == 0 {
        state.placements.clear();
        return Vec::new();
    }

    // Get available widgets in priority order
    let available = data.available_widgets();
    if available.is_empty() {
        state.placements.clear();
        return Vec::new();
    }

    // Build margin spaces
    let mut margin_spaces: Vec<MarginSpace> = Vec::new();

    // Right margin is always available
    if !margins.right_widths.is_empty() {
        margin_spaces.push(MarginSpace {
            side: Side::Right,
            widths: margins.right_widths.clone(),
            x_offset: messages_area.x + messages_area.width, // Will subtract widget width
        });
    }

    // Left margin only in centered mode
    if margins.centered && !margins.left_widths.is_empty() {
        margin_spaces.push(MarginSpace {
            side: Side::Left,
            widths: margins.left_widths.clone(),
            x_offset: messages_area.x,
        });
    }

    // Find rectangles in each margin
    // Format: (side, top, height, width, x_offset, margin_index)
    // We store margin_index to recalculate width when shrinking rects
    let mut all_rects: Vec<(Side, u16, u16, u16, u16, usize)> = Vec::new();

    for (margin_idx, margin) in margin_spaces.iter().enumerate() {
        let rects = find_all_empty_rects(&margin.widths, MIN_WIDGET_WIDTH, MIN_WIDGET_HEIGHT);
        for (top, height, width) in rects {
            let clamped_width = width.min(MAX_WIDGET_WIDTH);
            // Position the widget at the start of the margin (where content ends),
            // not at the right edge minus widget width. This prevents overlap.
            let x = match margin.side {
                Side::Right => margin.x_offset.saturating_sub(width),
                Side::Left => margin.x_offset,
            };
            all_rects.push((margin.side, top, height, clamped_width, x, margin_idx));
        }
    }

    // Place widgets greedily by priority
    // Rects are modified in-place: after placing a widget at top, we shrink the rect
    let mut placements: Vec<WidgetPlacement> = Vec::new();

    for kind in available {
        let min_h = kind.min_height() + 2; // Add border
        let preferred = kind.preferred_side();

        // Find best rectangle for this widget
        // Prefer: 1) correct side, 2) smallest rect that fits (reduces waste)
        let mut best_idx: Option<usize> = None;
        let mut best_score: i32 = i32::MIN;

        for (idx, &(side, _top, height, width, _x, _margin_idx)) in all_rects.iter().enumerate() {
            if height < min_h || width < MIN_WIDGET_WIDTH {
                continue;
            }

            // Score: prefer correct side (+1000), then prefer smaller rects (less waste)
            // Negative area so smaller = higher score
            let mut score = -((height as i32 * width as i32) / 10);
            if side == preferred {
                score += 1000;
            }

            if score > best_score {
                best_score = score;
                best_idx = Some(idx);
            }
        }

        if let Some(idx) = best_idx {
            let (side, top, height, width, x, margin_idx) = all_rects[idx];

            // Calculate actual widget height based on content
            let widget_height = calculate_widget_height(kind, data, width, height);

            // Place widget at top of rect
            let y = messages_area.y + top;

            placements.push(WidgetPlacement {
                kind,
                rect: Rect::new(x, y, width, widget_height),
                side,
            });

            // Shrink the rect: move top down, reduce height, recalculate width
            let remaining_height = height.saturating_sub(widget_height);
            if remaining_height >= MIN_WIDGET_HEIGHT {
                let new_top = top + widget_height;
                all_rects[idx].1 = new_top; // new top
                all_rects[idx].2 = remaining_height; // new height

                // Recalculate width for the new row range to avoid overlapping text
                // The new rows might have wider text than the original rows
                let margin = &margin_spaces[margin_idx];
                let new_end = (new_top as usize + remaining_height as usize).min(margin.widths.len());
                if (new_top as usize) < new_end {
                    // Get actual minimum margin width (unclamped) for positioning
                    let actual_min_width = margin.widths[new_top as usize..new_end]
                        .iter()
                        .copied()
                        .min()
                        .unwrap_or(0);
                    // Widget width is clamped to MAX_WIDGET_WIDTH
                    let new_min_width = actual_min_width.min(MAX_WIDGET_WIDTH);
                    all_rects[idx].3 = new_min_width; // new widget width (clamped)
                    // Position x at the start of the margin (use actual margin width)
                    all_rects[idx].4 = match side {
                        Side::Right => margin.x_offset.saturating_sub(actual_min_width),
                        Side::Left => margin.x_offset,
                    };
                } else {
                    // Invalid range - mark as empty
                    all_rects[idx].2 = 0;
                }
            } else {
                // Too small to reuse - mark as empty
                all_rects[idx].2 = 0;
            }
        }
    }

    state.placements = placements.clone();
    placements
}

/// Calculate the height needed for a specific widget type
fn calculate_widget_height(kind: WidgetKind, data: &InfoWidgetData, width: u16, max_height: u16) -> u16 {
    let inner_width = width.saturating_sub(2) as usize;
    let border_height = 2u16;

    let content_height = match kind {
        WidgetKind::Todos => {
            if data.todos.is_empty() {
                return 0;
            }
            // Header + progress bar + up to 5 items
            let items = data.todos.len().min(5) as u16;
            2 + items + if data.todos.len() > 5 { 1 } else { 0 }
        }
        WidgetKind::ContextUsage => {
            if data.context_info.is_none() {
                return 0;
            }
            1 // Just the bar
        }
        WidgetKind::MemoryActivity => {
            let Some(info) = &data.memory_info else {
                return 0;
            };
            let mut h = 1u16; // Title
            if info.activity.is_some() {
                h += 1; // State line
                h += info
                    .activity
                    .as_ref()
                    .map(|a| a.recent_events.len().min(3) as u16)
                    .unwrap_or(0);
            }
            h
        }
        WidgetKind::SwarmStatus => {
            let Some(info) = &data.swarm_info else {
                return 0;
            };
            let mut h = 1u16; // Stats line
            if info.subagent_status.is_some() {
                h += 1;
            }
            h += info.session_names.len().min(3) as u16;
            h
        }
        WidgetKind::BackgroundTasks => {
            if data.background_info.is_none() {
                return 0;
            }
            1 // Single line
        }
        WidgetKind::UsageLimits => {
            if data.usage_info.as_ref().map(|u| u.available).unwrap_or(false) {
                2 // Two bars
            } else {
                0
            }
        }
        WidgetKind::ModelInfo => {
            if data.model.is_none() {
                return 0;
            }
            let mut h = 1u16; // Model name
            if data.auth_method != AuthMethod::Unknown {
                h += 1; // Auth method line
            }
            h
        }
    };

    let total = content_height + border_height;
    total.min(max_height).max(kind.min_height() + border_height)
}

/// Legacy API for backwards compatibility - will be removed
/// Calculate the widget layout based on available space
/// Returns the Rect where the widget should be drawn, or None if it shouldn't show
#[deprecated(note = "Use calculate_placements instead")]
pub fn calculate_layout(
    messages_area: Rect,
    free_widths: &[u16],
    data: &InfoWidgetData,
) -> Option<Rect> {
    let margins = Margins {
        right_widths: free_widths.to_vec(),
        left_widths: Vec::new(),
        centered: false,
    };
    let placements = calculate_placements(messages_area, &margins, data);
    placements.first().map(|p| p.rect)
}

fn find_largest_empty_rect(
    free_widths: &[u16],
    min_width: u16,
    min_height: u16,
) -> Option<(u16, u16, u16)> {
    find_all_empty_rects(free_widths, min_width, min_height)
        .into_iter()
        .max_by_key(|&(_, h, w)| h as u32 * w as u32)
}

/// Find all valid empty rectangles in the margin
/// Returns list of (top_row, height, width)
fn find_all_empty_rects(
    free_widths: &[u16],
    min_width: u16,
    min_height: u16,
) -> Vec<(u16, u16, u16)> {
    let mut rects: Vec<(u16, u16, u16)> = Vec::new();

    if free_widths.is_empty() {
        return rects;
    }

    // Find contiguous regions where width >= min_width
    let mut region_start: Option<usize> = None;

    for (i, &width) in free_widths.iter().enumerate() {
        if width >= min_width {
            if region_start.is_none() {
                region_start = Some(i);
            }
        } else {
            // End of region
            if let Some(start) = region_start {
                add_region_rects(&mut rects, free_widths, start, i, min_width, min_height);
                region_start = None;
            }
        }
    }

    // Handle region extending to end
    if let Some(start) = region_start {
        add_region_rects(
            &mut rects,
            free_widths,
            start,
            free_widths.len(),
            min_width,
            min_height,
        );
    }

    rects
}

/// Add rectangles from a contiguous region
fn add_region_rects(
    rects: &mut Vec<(u16, u16, u16)>,
    free_widths: &[u16],
    start: usize,
    end: usize,
    min_width: u16,
    min_height: u16,
) {
    let region_height = end - start;
    if region_height < min_height as usize {
        return;
    }

    // Find the minimum width in this region
    let min_w = free_widths[start..end]
        .iter()
        .copied()
        .min()
        .unwrap_or(0)
        .min(MAX_WIDGET_WIDTH);

    if min_w >= min_width {
        // Add the full region as one rectangle
        rects.push((start as u16, region_height as u16, min_w));

        // If the region is tall enough, we could split it to place multiple widgets
        // For now, we'll let the placement algorithm handle stacking
    }
}

/// Render all placed widgets
pub fn render_all(frame: &mut Frame, placements: &[WidgetPlacement], data: &InfoWidgetData) {
    for placement in placements {
        render_single_widget(frame, placement, data);
    }
}

/// Render a single widget at its placement
fn render_single_widget(frame: &mut Frame, placement: &WidgetPlacement, data: &InfoWidgetData) {
    let rect = placement.rect;

    // Semi-transparent looking border (using dim colors)
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(70, 70, 80)).dim());

    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let lines = render_widget_content(placement.kind, data, inner);
    let para = Paragraph::new(lines);
    frame.render_widget(para, inner);
}

/// Render content for a specific widget type
fn render_widget_content(kind: WidgetKind, data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    match kind {
        WidgetKind::Todos => render_todos_widget(data, inner),
        WidgetKind::ContextUsage => render_context_widget(data, inner),
        WidgetKind::MemoryActivity => render_memory_widget(data, inner),
        WidgetKind::SwarmStatus => render_swarm_widget(data, inner),
        WidgetKind::BackgroundTasks => render_background_widget(data, inner),
        WidgetKind::UsageLimits => render_usage_widget(data, inner),
        WidgetKind::ModelInfo => render_model_widget(data, inner),
    }
}

/// Render todos widget content
fn render_todos_widget(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    if data.todos.is_empty() {
        return Vec::new();
    }

    let mut lines: Vec<Line> = Vec::new();
    let total = data.todos.len();
    let completed: usize = data.todos.iter().filter(|t| t.status == "completed").count();
    let in_progress: usize = data.todos.iter().filter(|t| t.status == "in_progress").count();

    // Header with progress
    lines.push(Line::from(vec![
        Span::styled(
            "Todos ",
            Style::default().fg(Color::Rgb(180, 180, 190)).bold(),
        ),
        Span::styled(
            format!("{}/{}", completed, total),
            Style::default().fg(Color::Rgb(140, 140, 150)),
        ),
    ]));

    // Mini progress bar
    let bar_width = inner.width.saturating_sub(2).min(20) as usize;
    if bar_width >= 4 && total > 0 {
        let filled = ((completed as f64 / total as f64) * bar_width as f64).round() as usize;
        let empty = bar_width.saturating_sub(filled);
        lines.push(Line::from(vec![
            Span::styled("[", Style::default().fg(Color::Rgb(90, 90, 100))),
            Span::styled(
                "█".repeat(filled),
                Style::default().fg(Color::Rgb(100, 180, 100)),
            ),
            Span::styled(
                "░".repeat(empty),
                Style::default().fg(Color::Rgb(50, 50, 60)),
            ),
            Span::styled("]", Style::default().fg(Color::Rgb(90, 90, 100))),
        ]));
    }

    // Sort todos: in_progress first, then pending, then completed
    let mut sorted_todos: Vec<&crate::todo::TodoItem> = data.todos.iter().collect();
    sorted_todos.sort_by(|a, b| {
        let order = |s: &str| match s {
            "in_progress" => 0,
            "pending" => 1,
            "completed" => 2,
            "cancelled" => 3,
            _ => 4,
        };
        order(&a.status).cmp(&order(&b.status))
    });

    // Render todos (limit based on available height)
    let available_lines = inner.height.saturating_sub(2) as usize; // Account for header + bar
    for todo in sorted_todos.iter().take(available_lines.min(5)) {
        let (icon, status_color) = match todo.status.as_str() {
            "completed" => ("✓", Color::Rgb(100, 180, 100)),
            "in_progress" => ("▶", Color::Rgb(255, 200, 100)),
            "cancelled" => ("✗", Color::Rgb(120, 80, 80)),
            _ => ("○", Color::Rgb(120, 120, 130)),
        };

        let max_len = inner.width.saturating_sub(3) as usize;
        let content = truncate_smart(&todo.content, max_len);

        let text_color = if todo.status == "completed" {
            Color::Rgb(100, 100, 110)
        } else if todo.status == "in_progress" {
            Color::Rgb(200, 200, 210)
        } else {
            Color::Rgb(160, 160, 170)
        };

        lines.push(Line::from(vec![
            Span::styled(format!("{} ", icon), Style::default().fg(status_color)),
            Span::styled(content, Style::default().fg(text_color)),
        ]));
    }

    // Show count of remaining items
    let shown = available_lines.min(5).min(sorted_todos.len());
    if data.todos.len() > shown {
        let remaining = data.todos.len() - shown;
        lines.push(Line::from(vec![Span::styled(
            format!("  +{} more", remaining),
            Style::default().fg(Color::Rgb(100, 100, 110)),
        )]));
    }

    lines
}

/// Render context usage widget
fn render_context_widget(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let Some(info) = &data.context_info else {
        return Vec::new();
    };
    if info.total_chars == 0 {
        return Vec::new();
    }

    let used_tokens = info.estimated_tokens();
    let limit_tokens = data.context_limit.unwrap_or(DEFAULT_CONTEXT_LIMIT).max(1);
    let used_pct = ((used_tokens as f64 / limit_tokens as f64) * 100.0)
        .round()
        .clamp(0.0, 100.0) as u8;
    let left_pct = 100u8.saturating_sub(used_pct);

    vec![render_labeled_bar("Context", used_pct, left_pct, None, inner.width)]
}

/// Render memory activity widget
fn render_memory_widget(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let Some(info) = &data.memory_info else {
        return Vec::new();
    };
    if info.total_count == 0 && info.activity.is_none() {
        return Vec::new();
    }

    let mut lines: Vec<Line> = Vec::new();

    // Title with count
    lines.push(Line::from(vec![
        Span::styled("🧠 ", Style::default().fg(Color::Rgb(200, 150, 255))),
        Span::styled(
            format!("{} memories", info.total_count),
            Style::default().fg(Color::Rgb(180, 180, 190)),
        ),
    ]));

    // Activity state if active
    if let Some(activity) = &info.activity {
        let state_line = match &activity.state {
            MemoryState::Idle => Line::from(vec![
                Span::styled("○ ", Style::default().fg(Color::Rgb(100, 100, 110))),
                Span::styled("Idle", Style::default().fg(Color::Rgb(120, 120, 130))),
            ]),
            MemoryState::Embedding => Line::from(vec![
                Span::styled("🔍 ", Style::default().fg(Color::Rgb(255, 200, 100))),
                Span::styled("Searching...", Style::default().fg(Color::Rgb(180, 180, 190))),
            ]),
            MemoryState::SidecarChecking { count } => Line::from(vec![
                Span::styled("⚡ ", Style::default().fg(Color::Rgb(255, 200, 100))),
                Span::styled(
                    format!("Checking {}", count),
                    Style::default().fg(Color::Rgb(180, 180, 190)),
                ),
            ]),
            MemoryState::FoundRelevant { count } => Line::from(vec![
                Span::styled("✓ ", Style::default().fg(Color::Rgb(100, 200, 100))),
                Span::styled(
                    format!("{} relevant", count),
                    Style::default().fg(Color::Rgb(180, 180, 190)),
                ),
            ]),
            MemoryState::Extracting { reason } => Line::from(vec![
                Span::styled("🧠 ", Style::default().fg(Color::Rgb(200, 150, 255))),
                Span::styled(
                    format!("Extracting ({})", reason),
                    Style::default().fg(Color::Rgb(180, 180, 190)),
                ),
            ]),
        };
        lines.push(state_line);

        // Recent events (limit to 3)
        let max_events = (inner.height.saturating_sub(2) as usize).min(3);
        for event in activity.recent_events.iter().take(max_events) {
            let (icon, text, color) = format_memory_event(event, inner.width.saturating_sub(4) as usize);
            lines.push(Line::from(vec![
                Span::styled(format!("  {} ", icon), Style::default().fg(color)),
                Span::styled(text, Style::default().fg(Color::Rgb(140, 140, 150))),
            ]));
        }
    }

    lines
}

fn format_memory_event(event: &MemoryEvent, max_width: usize) -> (&'static str, String, Color) {
    match &event.kind {
        MemoryEventKind::EmbeddingStarted => ("🔍", "Embedding...".to_string(), Color::Rgb(140, 180, 255)),
        MemoryEventKind::EmbeddingComplete { latency_ms, hits } => (
            "→",
            format!("{} hits ({}ms)", hits, latency_ms),
            Color::Rgb(140, 180, 255),
        ),
        MemoryEventKind::SidecarStarted => ("⚡", "Verifying".to_string(), Color::Rgb(255, 200, 100)),
        MemoryEventKind::SidecarRelevant { memory_preview } => {
            let preview = truncate_smart(memory_preview, max_width.saturating_sub(2));
            ("✓", preview, Color::Rgb(100, 200, 100))
        }
        MemoryEventKind::SidecarNotRelevant => ("✗", "Not relevant".to_string(), Color::Rgb(150, 150, 160)),
        MemoryEventKind::SidecarComplete { latency_ms } => ("⏱", format!("{}ms", latency_ms), Color::Rgb(140, 140, 150)),
        MemoryEventKind::MemorySurfaced { memory_preview } => {
            let preview = truncate_smart(memory_preview, max_width.saturating_sub(2));
            ("★", preview, Color::Rgb(255, 220, 100))
        }
        MemoryEventKind::ExtractionStarted { reason } => {
            let msg = truncate_smart(reason, max_width.saturating_sub(2));
            ("🧠", format!("Extracting: {}", msg), Color::Rgb(200, 150, 255))
        }
        MemoryEventKind::ExtractionComplete { count } => {
            ("✓", format!("Saved {} memories", count), Color::Rgb(100, 200, 100))
        }
        MemoryEventKind::Error { message } => {
            let msg = truncate_smart(message, max_width.saturating_sub(2));
            ("!", msg, Color::Rgb(255, 100, 100))
        }
    }
}

/// Render swarm status widget
fn render_swarm_widget(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let Some(info) = &data.swarm_info else {
        return Vec::new();
    };

    let mut lines: Vec<Line> = Vec::new();

    // Stats line
    let mut stats_parts: Vec<Span> = vec![Span::styled(
        "🐝 ",
        Style::default().fg(Color::Rgb(255, 200, 100)),
    )];

    if info.session_count > 0 {
        stats_parts.push(Span::styled(
            format!("{}s", info.session_count),
            Style::default().fg(Color::Rgb(160, 160, 170)),
        ));
    }
    if let Some(clients) = info.client_count {
        if info.session_count > 0 {
            stats_parts.push(Span::styled(" · ", Style::default().fg(Color::Rgb(100, 100, 110))));
        }
        stats_parts.push(Span::styled(
            format!("{}c", clients),
            Style::default().fg(Color::Rgb(160, 160, 170)),
        ));
    }
    lines.push(Line::from(stats_parts));

    // Active subagent status
    if let Some(status) = &info.subagent_status {
        lines.push(Line::from(vec![
            Span::styled("▶ ", Style::default().fg(Color::Rgb(255, 200, 100))),
            Span::styled(
                truncate_smart(status, inner.width.saturating_sub(4) as usize),
                Style::default().fg(Color::Rgb(200, 200, 210)),
            ),
        ]));
    }

    // Session names (limit based on height)
    let max_names = inner.height.saturating_sub(lines.len() as u16) as usize;
    let max_name_len = inner.width.saturating_sub(4) as usize;
    for name in info.session_names.iter().take(max_names.min(3)) {
        lines.push(Line::from(vec![
            Span::styled("  · ", Style::default().fg(Color::Rgb(100, 100, 110))),
            Span::styled(
                truncate_smart(name, max_name_len),
                Style::default().fg(Color::Rgb(140, 140, 150)),
            ),
        ]));
    }

    lines
}

/// Render background tasks widget
fn render_background_widget(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let Some(info) = &data.background_info else {
        return Vec::new();
    };
    if info.running_count == 0 && !info.memory_agent_active {
        return Vec::new();
    }

    let mut spans: Vec<Span> = vec![Span::styled(
        "⏳ ",
        Style::default().fg(Color::Rgb(180, 140, 255)),
    )];

    let mut parts: Vec<String> = Vec::new();
    if info.memory_agent_active {
        parts.push(format!("mem:{}", info.memory_agent_turns));
    }
    if info.running_count > 0 {
        if info.running_tasks.is_empty() {
            parts.push(format!("bg:{}", info.running_count));
        } else {
            let task_str = info.running_tasks.join(",");
            if task_str.len() > 15 {
                parts.push(format!("bg:{}+", info.running_count));
            } else {
                parts.push(format!("bg:{}", task_str));
            }
        }
    }

    spans.push(Span::styled(
        parts.join(" "),
        Style::default().fg(Color::Rgb(160, 160, 170)),
    ));

    vec![Line::from(spans)]
}

/// Render usage limits widget
fn render_usage_widget(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let Some(info) = &data.usage_info else {
        return Vec::new();
    };
    if !info.available {
        return Vec::new();
    }

    match info.provider {
        UsageProvider::CostBased => {
            // Show token costs for API-key providers (OpenRouter, direct API)
            vec![
                Line::from(vec![
                    Span::styled("💰 ", Style::default().fg(Color::Rgb(140, 180, 255))),
                    Span::styled(
                        format!("${:.4}", info.total_cost),
                        Style::default().fg(Color::Rgb(180, 180, 190)).bold(),
                    ),
                ]),
                Line::from(vec![
                    Span::styled(
                        format!("{} in + {} out", format_tokens(info.input_tokens), format_tokens(info.output_tokens)),
                        Style::default().fg(Color::Rgb(140, 140, 150)),
                    ),
                ]),
            ]
        }
        _ => {
            // Show subscription usage for OAuth providers (Anthropic, OpenAI)
            let five_hr_used = (info.five_hour * 100.0).round().clamp(0.0, 100.0) as u8;
            let seven_day_used = (info.seven_day * 100.0).round().clamp(0.0, 100.0) as u8;
            let five_hr_left = 100u8.saturating_sub(five_hr_used);
            let seven_day_left = 100u8.saturating_sub(seven_day_used);

            vec![
                render_labeled_bar("5-hour", five_hr_used, five_hr_left, None, inner.width),
                render_labeled_bar("Weekly", seven_day_used, seven_day_left, None, inner.width),
            ]
        }
    }
}

/// Format token count for display
fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        format!("{}", tokens)
    }
}

/// Format cost for display
fn format_cost(cost: f32) -> String {
    if cost >= 10.0 {
        format!("{:.2}", cost)
    } else if cost >= 1.0 {
        format!("{:.3}", cost)
    } else {
        format!("{:.4}", cost)
    }
}

/// Render model info widget (combined with usage info)
fn render_model_widget(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let Some(model) = &data.model else {
        return Vec::new();
    };

    let mut lines: Vec<Line> = Vec::new();

    let short_name = shorten_model_name(model);
    let max_len = inner.width.saturating_sub(2) as usize;

    let mut spans = vec![
        Span::styled("⚡ ", Style::default().fg(Color::Rgb(140, 180, 255))),
        Span::styled(
            truncate_smart(&short_name, max_len.saturating_sub(2)),
            Style::default().fg(Color::Rgb(180, 180, 190)).bold(),
        ),
    ];

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

    lines.push(Line::from(spans));

    // Auth method line
    if data.auth_method != AuthMethod::Unknown {
        let (icon, label, color) = match data.auth_method {
            AuthMethod::AnthropicOAuth => ("🔐", "OAuth", Color::Rgb(255, 160, 100)),
            AuthMethod::AnthropicApiKey => ("🔑", "API Key", Color::Rgb(180, 180, 190)),
            AuthMethod::OpenAIOAuth => ("🔐", "OAuth", Color::Rgb(100, 200, 180)),
            AuthMethod::OpenAIApiKey => ("🔑", "API Key", Color::Rgb(180, 180, 190)),
            AuthMethod::OpenRouterApiKey => ("🔑", "API Key", Color::Rgb(140, 180, 255)),
            AuthMethod::Unknown => unreachable!(),
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", icon), Style::default().fg(color)),
            Span::styled(label, Style::default().fg(Color::Rgb(140, 140, 150))),
        ]));
    }

    // Usage info (combined from UsageLimits widget)
    if let Some(info) = &data.usage_info {
        if info.available {
            match info.provider {
                UsageProvider::CostBased => {
                    // Cost + tokens for API-key providers
                    lines.push(Line::from(vec![
                        Span::styled("💰 ", Style::default().fg(Color::Rgb(140, 180, 255))),
                        Span::styled(
                            format!("${:.4}", info.total_cost),
                            Style::default().fg(Color::Rgb(180, 180, 190)),
                        ),
                        Span::styled(
                            format!(" ({}↑ {}↓)", format_tokens(info.input_tokens), format_tokens(info.output_tokens)),
                            Style::default().fg(Color::Rgb(120, 120, 130)),
                        ),
                    ]));
                }
                _ => {
                    // Subscription usage bars
                    let five_hr_used = (info.five_hour * 100.0).round().clamp(0.0, 100.0) as u8;
                    let seven_day_used = (info.seven_day * 100.0).round().clamp(0.0, 100.0) as u8;
                    let five_hr_left = 100u8.saturating_sub(five_hr_used);
                    let seven_day_left = 100u8.saturating_sub(seven_day_used);

                    lines.push(render_labeled_bar("5hr", five_hr_used, five_hr_left, None, inner.width));
                    lines.push(render_labeled_bar("7d", seven_day_used, seven_day_left, None, inner.width));
                }
            }
        }
    }

    lines
}

/// Legacy render function - kept for backwards compatibility
/// Renders the first available widget at the given rect
#[deprecated(note = "Use render_all instead")]
#[allow(deprecated)]
pub fn render(frame: &mut Frame, rect: Rect, data: &InfoWidgetData) {
    // Just render as the first available widget type
    let available = data.available_widgets();
    if available.is_empty() {
        return;
    }

    // Create a temporary placement for the first widget
    let placement = WidgetPlacement {
        kind: available[0],
        rect,
        side: Side::Right,
    };
    render_single_widget(frame, &placement, data);
}

const MAX_CONTEXT_LINES: usize = 5;
const MAX_TODO_LINES: usize = 12;
const MAX_MEMORY_EVENTS: usize = 4;

#[derive(Clone, Copy, Debug)]
enum InfoPageKind {
    CompactOnly,
    TodosExpanded,
    ContextExpanded,
    MemoryExpanded,
    SwarmExpanded,
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

    let memory_compact = compact_memory_height(data);
    let memory_expanded = expanded_memory_height(data);
    if memory_expanded > 0 {
        candidates.push(InfoPage {
            kind: InfoPageKind::MemoryExpanded,
            height: compact_height - memory_compact + memory_expanded,
        });
    }

    let swarm_compact = compact_swarm_height(data);
    let swarm_expanded = expanded_swarm_height(data);
    if swarm_expanded > 0 {
        candidates.push(InfoPage {
            kind: InfoPageKind::SwarmExpanded,
            height: compact_height - swarm_compact + swarm_expanded,
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
        InfoPageKind::TodosExpanded => {
            render_sections(data, inner, Some(InfoPageKind::TodosExpanded))
        }
        InfoPageKind::ContextExpanded => {
            render_sections(data, inner, Some(InfoPageKind::ContextExpanded))
        }
        InfoPageKind::MemoryExpanded => {
            render_sections(data, inner, Some(InfoPageKind::MemoryExpanded))
        }
        InfoPageKind::SwarmExpanded => {
            render_sections(data, inner, Some(InfoPageKind::SwarmExpanded))
        }
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

fn compact_background_height(data: &InfoWidgetData) -> u16 {
    if let Some(info) = &data.background_info {
        if info.running_count > 0 || info.memory_agent_active {
            return 1;
        }
    }
    0
}

fn compact_usage_height(data: &InfoWidgetData) -> u16 {
    if let Some(info) = &data.usage_info {
        if info.available {
            return 2; // Two lines: 5-hour and Weekly bars
        }
    }
    0
}

fn compact_overview_height(data: &InfoWidgetData) -> u16 {
    compact_model_height(data)
        + compact_context_height(data)
        + compact_todos_height(data)
        + compact_queue_height(data)
        + compact_memory_height(data)
        + compact_swarm_height(data)
        + compact_background_height(data)
        + compact_usage_height(data)
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
    // Header (1) + progress bar (1) + todo items + possible "+N more" line
    let available_lines = MAX_TODO_LINES.saturating_sub(2); // Same as in render
    let todo_lines = data.todos.len().min(available_lines);
    let mut height = 2 + todo_lines as u16; // Header + progress bar + items
    if data.todos.len() > available_lines {
        height += 1; // "+N more" line
    }
    height
}

fn expanded_memory_height(data: &InfoWidgetData) -> u16 {
    if let Some(info) = &data.memory_info {
        if info.total_count > 0 || info.activity.is_some() {
            // Title line + stats line + activity lines
            let mut height = 2u16;

            // Add lines for activity
            if let Some(activity) = &info.activity {
                // State line
                height += 1;
                // Recent events (up to MAX_MEMORY_EVENTS)
                let event_count = activity.recent_events.len().min(MAX_MEMORY_EVENTS);
                height += event_count as u16;
            }

            // Category breakdown if we have memories
            if !info.by_category.is_empty() {
                height += 1; // One line for categories
            }

            return height;
        }
    }
    0
}

fn compact_swarm_height(data: &InfoWidgetData) -> u16 {
    if let Some(info) = &data.swarm_info {
        // Show if we have active subagent or multiple sessions
        if info.subagent_status.is_some() || info.session_count > 1 || info.client_count.is_some() {
            return 1;
        }
    }
    0
}

fn expanded_swarm_height(data: &InfoWidgetData) -> u16 {
    if let Some(info) = &data.swarm_info {
        if info.subagent_status.is_some() || info.session_count > 1 || info.client_count.is_some() {
            // Title (1) + status line (1) + session list (up to 4)
            let mut height = 2u16;
            if info.subagent_status.is_some() {
                height += 1; // Active subagent line
            }
            // Show session names (up to 4)
            height += info.session_names.len().min(4) as u16;
            return height;
        }
    }
    0
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

    // Memory info
    if let Some(info) = &data.memory_info {
        if info.total_count > 0 || info.activity.is_some() {
            if matches!(focus, Some(InfoPageKind::MemoryExpanded)) {
                lines.extend(render_memory_expanded(info, inner));
            } else {
                lines.extend(render_memory_compact(info));
            }
        }
    }

    // Swarm/subagent info at the bottom
    if let Some(info) = &data.swarm_info {
        if info.subagent_status.is_some() || info.session_count > 1 || info.client_count.is_some() {
            if matches!(focus, Some(InfoPageKind::SwarmExpanded)) {
                lines.extend(render_swarm_expanded(info, inner));
            } else {
                lines.extend(render_swarm_compact(info));
            }
        }
    }

    // Background tasks info
    if let Some(info) = &data.background_info {
        if info.running_count > 0 || info.memory_agent_active {
            lines.extend(render_background_compact(info));
        }
    }

    // Usage info (subscription limits)
    if let Some(info) = &data.usage_info {
        if info.available {
            lines.extend(render_usage_compact(info, inner.width));
        }
    }

    lines
}

fn render_todos_expanded(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();
    if data.todos.is_empty() {
        return lines;
    }

    // Calculate stats
    let total = data.todos.len();
    let completed: usize = data
        .todos
        .iter()
        .filter(|t| t.status == "completed")
        .count();
    let in_progress: usize = data
        .todos
        .iter()
        .filter(|t| t.status == "in_progress")
        .count();

    // Header with progress
    lines.push(Line::from(vec![
        Span::styled(
            "Todos ",
            Style::default().fg(Color::Rgb(180, 180, 190)).bold(),
        ),
        Span::styled(
            format!("{}/{}", completed, total),
            Style::default().fg(Color::Rgb(140, 140, 150)),
        ),
    ]));

    // Mini progress bar
    let bar_width = inner.width.saturating_sub(2).min(20) as usize;
    if bar_width >= 4 && total > 0 {
        let filled = ((completed as f64 / total as f64) * bar_width as f64).round() as usize;
        let empty = bar_width.saturating_sub(filled);
        lines.push(Line::from(vec![
            Span::styled("[", Style::default().fg(Color::Rgb(90, 90, 100))),
            Span::styled(
                "█".repeat(filled),
                Style::default().fg(Color::Rgb(100, 180, 100)),
            ),
            Span::styled(
                "░".repeat(empty),
                Style::default().fg(Color::Rgb(50, 50, 60)),
            ),
            Span::styled("]", Style::default().fg(Color::Rgb(90, 90, 100))),
        ]));
    }

    // Sort todos: in_progress first, then pending, then completed
    let mut sorted_todos: Vec<&crate::todo::TodoItem> = data.todos.iter().collect();
    sorted_todos.sort_by(|a, b| {
        let order = |s: &str| match s {
            "in_progress" => 0,
            "pending" => 1,
            "completed" => 2,
            "cancelled" => 3,
            _ => 4,
        };
        order(&a.status).cmp(&order(&b.status))
    });

    // Render todos with priority colors
    let available_lines = MAX_TODO_LINES.saturating_sub(2); // Account for header + bar
    for todo in sorted_todos.iter().take(available_lines) {
        let (icon, status_color) = match todo.status.as_str() {
            "completed" => ("✓", Color::Rgb(100, 180, 100)),
            "in_progress" => ("▶", Color::Rgb(255, 200, 100)),
            "cancelled" => ("✗", Color::Rgb(120, 80, 80)),
            _ => ("○", Color::Rgb(120, 120, 130)),
        };

        // Priority indicator
        let priority_marker = match todo.priority.as_str() {
            "high" => ("!", Color::Rgb(255, 120, 100)),
            "medium" => ("", Color::Rgb(200, 180, 100)),
            _ => ("", Color::Rgb(120, 120, 130)),
        };

        let max_len = inner.width.saturating_sub(4) as usize;
        let content = truncate_smart(&todo.content, max_len);

        // Dim completed items
        let text_color = if todo.status == "completed" {
            Color::Rgb(100, 100, 110)
        } else if todo.status == "in_progress" {
            Color::Rgb(200, 200, 210)
        } else {
            Color::Rgb(160, 160, 170)
        };

        let mut spans = vec![Span::styled(
            format!("{} ", icon),
            Style::default().fg(status_color),
        )];

        if !priority_marker.0.is_empty() {
            spans.push(Span::styled(
                format!("{}", priority_marker.0),
                Style::default().fg(priority_marker.1),
            ));
        }

        spans.push(Span::styled(content, Style::default().fg(text_color)));

        lines.push(Line::from(spans));
    }

    // Show count of remaining items
    let shown = available_lines.min(sorted_todos.len());
    if data.todos.len() > shown {
        let remaining = data.todos.len() - shown;
        let remaining_completed = sorted_todos
            .iter()
            .skip(shown)
            .filter(|t| t.status == "completed")
            .count();
        let desc = if remaining_completed == remaining {
            format!("  +{} done", remaining)
        } else if remaining_completed > 0 {
            format!("  +{} more ({} done)", remaining, remaining_completed)
        } else {
            format!("  +{} more", remaining)
        };
        lines.push(Line::from(vec![Span::styled(
            desc,
            Style::default().fg(Color::Rgb(100, 100, 110)),
        )]));
    }

    lines
}

/// Truncate string smartly, trying to break at word boundaries
fn truncate_smart(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    if max_len <= 3 {
        return "...".to_string();
    }

    let target = max_len - 3;
    // Try to find a word boundary
    if let Some(pos) = s[..target].rfind(' ') {
        if pos > target / 2 {
            return format!("{}...", &s[..pos]);
        }
    }
    format!("{}...", &s[..target])
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

    // Show activity indicator if active
    if let Some(activity) = &info.activity {
        match &activity.state {
            MemoryState::Embedding => {
                spans.push(Span::styled(
                    " · ",
                    Style::default().fg(Color::Rgb(100, 100, 110)),
                ));
                spans.push(Span::styled(
                    "🔍",
                    Style::default().fg(Color::Rgb(255, 200, 100)),
                ));
            }
            MemoryState::SidecarChecking { count } => {
                spans.push(Span::styled(
                    " · ",
                    Style::default().fg(Color::Rgb(100, 100, 110)),
                ));
                spans.push(Span::styled(
                    format!("⚡{}", count),
                    Style::default().fg(Color::Rgb(255, 200, 100)),
                ));
            }
            MemoryState::FoundRelevant { count } => {
                spans.push(Span::styled(
                    " · ",
                    Style::default().fg(Color::Rgb(100, 100, 110)),
                ));
                spans.push(Span::styled(
                    format!("✓{}", count),
                    Style::default().fg(Color::Rgb(100, 200, 100)),
                ));
            }
            MemoryState::Extracting { .. } => {
                spans.push(Span::styled(
                    " · ",
                    Style::default().fg(Color::Rgb(100, 100, 110)),
                ));
                spans.push(Span::styled(
                    "🧠",
                    Style::default().fg(Color::Rgb(200, 150, 255)),
                ));
            }
            MemoryState::Idle => {}
        }
    }

    vec![Line::from(spans)]
}

fn render_memory_expanded(info: &MemoryInfo, inner: Rect) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();

    // Title
    lines.push(Line::from(vec![Span::styled(
        "Memory",
        Style::default().fg(Color::Rgb(180, 180, 190)).bold(),
    )]));

    // Stats line
    let mut stats_spans = vec![Span::styled(
        format!("{} total", info.total_count),
        Style::default().fg(Color::Rgb(160, 160, 170)),
    )];
    if info.project_count > 0 || info.global_count > 0 {
        stats_spans.push(Span::styled(
            format!(" ({}p/{}g)", info.project_count, info.global_count),
            Style::default().fg(Color::Rgb(120, 120, 130)),
        ));
    }
    lines.push(Line::from(stats_spans));

    // Category breakdown
    if !info.by_category.is_empty() {
        let max_width = inner.width.saturating_sub(2) as usize;
        let mut cat_parts: Vec<String> = info
            .by_category
            .iter()
            .map(|(cat, count)| format!("{}:{}", &cat[..3.min(cat.len())], count))
            .collect();
        cat_parts.sort();
        let cat_str = cat_parts.join(" ");
        let cat_display = if cat_str.len() > max_width {
            format!("{}…", &cat_str[..max_width.saturating_sub(1)])
        } else {
            cat_str
        };
        lines.push(Line::from(vec![Span::styled(
            cat_display,
            Style::default().fg(Color::Rgb(100, 100, 110)),
        )]));
    }

    // Activity section
    if let Some(activity) = &info.activity {
        // Current state
        let state_line = match &activity.state {
            MemoryState::Idle => Line::from(vec![
                Span::styled("○ ", Style::default().fg(Color::Rgb(100, 100, 110))),
                Span::styled("Idle", Style::default().fg(Color::Rgb(120, 120, 130))),
            ]),
            MemoryState::Embedding => Line::from(vec![
                Span::styled("🔍 ", Style::default().fg(Color::Rgb(255, 200, 100))),
                Span::styled(
                    "Searching...",
                    Style::default().fg(Color::Rgb(180, 180, 190)),
                ),
            ]),
            MemoryState::SidecarChecking { count } => Line::from(vec![
                Span::styled("⚡ ", Style::default().fg(Color::Rgb(255, 200, 100))),
                Span::styled(
                    format!("Checking {} memories", count),
                    Style::default().fg(Color::Rgb(180, 180, 190)),
                ),
            ]),
            MemoryState::FoundRelevant { count } => Line::from(vec![
                Span::styled("✓ ", Style::default().fg(Color::Rgb(100, 200, 100))),
                Span::styled(
                    format!("{} relevant", count),
                    Style::default().fg(Color::Rgb(180, 180, 190)),
                ),
            ]),
            MemoryState::Extracting { reason } => Line::from(vec![
                Span::styled("🧠 ", Style::default().fg(Color::Rgb(200, 150, 255))),
                Span::styled(
                    format!("Extracting ({})", reason),
                    Style::default().fg(Color::Rgb(180, 180, 190)),
                ),
            ]),
        };
        lines.push(state_line);

        // Recent events
        let max_width = inner.width.saturating_sub(4) as usize;
        for event in activity.recent_events.iter().take(MAX_MEMORY_EVENTS) {
            let (icon, text, color) = match &event.kind {
                MemoryEventKind::EmbeddingStarted => {
                    ("🔍", "Embedding...".to_string(), Color::Rgb(140, 180, 255))
                }
                MemoryEventKind::EmbeddingComplete { latency_ms, hits } => (
                    "→",
                    format!("{} hits ({}ms)", hits, latency_ms),
                    Color::Rgb(140, 180, 255),
                ),
                MemoryEventKind::SidecarStarted => (
                    "⚡",
                    "Sidecar verifying".to_string(),
                    Color::Rgb(255, 200, 100),
                ),
                MemoryEventKind::SidecarRelevant { memory_preview } => {
                    let preview = if memory_preview.len() > max_width.saturating_sub(4) {
                        format!("{}…", &memory_preview[..max_width.saturating_sub(5)])
                    } else {
                        memory_preview.clone()
                    };
                    ("✓", preview, Color::Rgb(100, 200, 100))
                }
                MemoryEventKind::SidecarNotRelevant => {
                    ("✗", "Not relevant".to_string(), Color::Rgb(150, 150, 160))
                }
                MemoryEventKind::SidecarComplete { latency_ms } => {
                    ("⏱", format!("{}ms", latency_ms), Color::Rgb(140, 140, 150))
                }
                MemoryEventKind::MemorySurfaced { memory_preview } => {
                    let preview = if memory_preview.len() > max_width.saturating_sub(4) {
                        format!("{}…", &memory_preview[..max_width.saturating_sub(5)])
                    } else {
                        memory_preview.clone()
                    };
                    ("★", preview, Color::Rgb(255, 220, 100))
                }
                MemoryEventKind::ExtractionStarted { reason } => {
                    let msg = if reason.len() > max_width.saturating_sub(4) {
                        format!("{}…", &reason[..max_width.saturating_sub(5)])
                    } else {
                        reason.clone()
                    };
                    ("🧠", format!("Extracting: {}", msg), Color::Rgb(200, 150, 255))
                }
                MemoryEventKind::ExtractionComplete { count } => {
                    ("✓", format!("Saved {} memories", count), Color::Rgb(100, 200, 100))
                }
                MemoryEventKind::Error { message } => {
                    let msg = if message.len() > max_width.saturating_sub(4) {
                        format!("{}…", &message[..max_width.saturating_sub(5)])
                    } else {
                        message.clone()
                    };
                    ("!", msg, Color::Rgb(255, 100, 100))
                }
            };

            lines.push(Line::from(vec![
                Span::styled(format!("  {} ", icon), Style::default().fg(color)),
                Span::styled(text, Style::default().fg(Color::Rgb(140, 140, 150))),
            ]));
        }
    }

    lines
}

fn render_swarm_compact(info: &SwarmInfo) -> Vec<Line<'static>> {
    let mut spans: Vec<Span> = Vec::new();

    // Show active subagent status first (most important)
    if let Some(status) = &info.subagent_status {
        spans.push(Span::styled(
            "▶ ",
            Style::default().fg(Color::Rgb(255, 200, 100)),
        ));
        spans.push(Span::styled(
            truncate_smart(status, 20),
            Style::default().fg(Color::Rgb(180, 180, 190)),
        ));
    } else {
        // Show swarm icon (bee for "swarm")
        spans.push(Span::styled(
            "🐝 ",
            Style::default().fg(Color::Rgb(255, 200, 100)),
        ));
    }

    // Session count if > 1
    if info.session_count > 1 {
        if !spans.is_empty() && info.subagent_status.is_none() {
            // Already have icon
        } else if info.subagent_status.is_some() {
            spans.push(Span::styled(
                " · ",
                Style::default().fg(Color::Rgb(100, 100, 110)),
            ));
        }
        spans.push(Span::styled(
            format!("{}s", info.session_count),
            Style::default().fg(Color::Rgb(140, 140, 150)),
        ));
    }

    // Client count if present
    if let Some(clients) = info.client_count {
        if !spans.is_empty() {
            spans.push(Span::styled(
                " · ",
                Style::default().fg(Color::Rgb(100, 100, 110)),
            ));
        }
        spans.push(Span::styled(
            format!("{}c", clients),
            Style::default().fg(Color::Rgb(140, 140, 150)),
        ));
    }

    if spans.is_empty() {
        return Vec::new();
    }

    vec![Line::from(spans)]
}

fn render_swarm_expanded(info: &SwarmInfo, inner: Rect) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();

    // Title
    lines.push(Line::from(vec![Span::styled(
        "Swarm",
        Style::default().fg(Color::Rgb(180, 180, 190)).bold(),
    )]));

    // Stats line
    let mut stats_parts: Vec<Span> = Vec::new();
    if info.session_count > 0 {
        stats_parts.push(Span::styled(
            format!(
                "{} session{}",
                info.session_count,
                if info.session_count == 1 { "" } else { "s" }
            ),
            Style::default().fg(Color::Rgb(160, 160, 170)),
        ));
    }
    if let Some(clients) = info.client_count {
        if !stats_parts.is_empty() {
            stats_parts.push(Span::styled(
                " · ",
                Style::default().fg(Color::Rgb(100, 100, 110)),
            ));
        }
        stats_parts.push(Span::styled(
            format!("{} client{}", clients, if clients == 1 { "" } else { "s" }),
            Style::default().fg(Color::Rgb(160, 160, 170)),
        ));
    }
    if !stats_parts.is_empty() {
        lines.push(Line::from(stats_parts));
    }

    // Active subagent status
    if let Some(status) = &info.subagent_status {
        lines.push(Line::from(vec![
            Span::styled("▶ ", Style::default().fg(Color::Rgb(255, 200, 100))),
            Span::styled(
                truncate_smart(status, inner.width.saturating_sub(4) as usize),
                Style::default().fg(Color::Rgb(200, 200, 210)),
            ),
        ]));
    }

    // Session names (up to 4)
    let max_name_len = inner.width.saturating_sub(4) as usize;
    for name in info.session_names.iter().take(4) {
        lines.push(Line::from(vec![
            Span::styled("  · ", Style::default().fg(Color::Rgb(100, 100, 110))),
            Span::styled(
                truncate_smart(name, max_name_len),
                Style::default().fg(Color::Rgb(140, 140, 150)),
            ),
        ]));
    }

    // Show count of remaining sessions
    if info.session_names.len() > 4 {
        let remaining = info.session_names.len() - 4;
        lines.push(Line::from(vec![Span::styled(
            format!("  +{} more", remaining),
            Style::default().fg(Color::Rgb(100, 100, 110)),
        )]));
    }

    lines
}

fn render_background_compact(info: &BackgroundInfo) -> Vec<Line<'static>> {
    let mut spans: Vec<Span> = Vec::new();

    // Show spinner icon for active background work
    spans.push(Span::styled(
        "⏳ ",
        Style::default().fg(Color::Rgb(180, 140, 255)),
    ));

    let mut parts: Vec<String> = Vec::new();

    // Memory agent status
    if info.memory_agent_active {
        parts.push(format!("mem:{}", info.memory_agent_turns));
    }

    // Running background tasks
    if info.running_count > 0 {
        if info.running_tasks.is_empty() {
            parts.push(format!("bg:{}", info.running_count));
        } else {
            // Show task names
            let task_str = info.running_tasks.join(",");
            if task_str.len() > 15 {
                parts.push(format!("bg:{}+", info.running_count));
            } else {
                parts.push(format!("bg:{}", task_str));
            }
        }
    }

    spans.push(Span::styled(
        parts.join(" "),
        Style::default().fg(Color::Rgb(160, 160, 170)),
    ));

    if spans.len() <= 1 {
        return Vec::new();
    }

    vec![Line::from(spans)]
}

fn render_usage_compact(info: &UsageInfo, width: u16) -> Vec<Line<'static>> {
    if !info.available {
        return Vec::new();
    }

    let five_hr_used = (info.five_hour * 100.0).round().clamp(0.0, 100.0) as u8;
    let seven_day_used = (info.seven_day * 100.0).round().clamp(0.0, 100.0) as u8;
    let five_hr_left = 100u8.saturating_sub(five_hr_used);
    let seven_day_left = 100u8.saturating_sub(seven_day_used);

    vec![
        render_labeled_bar("5-hour", five_hr_used, five_hr_left, None, width),
        render_labeled_bar("Weekly", seven_day_used, seven_day_left, None, width),
    ]
}

/// Render a labeled progress bar with color-coded status
/// Shows "X% left" or a reset time if depleted
fn render_labeled_bar(
    label: &str,
    used_pct: u8,
    left_pct: u8,
    reset_time: Option<&str>,
    width: u16,
) -> Line<'static> {
    // Color based on remaining percentage
    let color = if left_pct == 0 {
        Color::Rgb(255, 100, 100) // Red - depleted
    } else if left_pct < 20 {
        Color::Rgb(255, 100, 100) // Red - critical
    } else if left_pct <= 50 {
        Color::Rgb(255, 200, 100) // Yellow - getting low
    } else {
        Color::Rgb(100, 200, 100) // Green - plenty left
    };

    // Calculate bar width: total width - label - space - suffix
    // Label is max 7 chars ("Context" or "5-hour " or "Weekly ")
    // Suffix is " XX% left" (10 chars) or " resets Xh" (10 chars)
    let label_width = 7;
    let suffix_width = 10;
    let bar_width = width
        .saturating_sub(label_width + 1 + suffix_width)
        .min(12)
        .max(4) as usize;

    // Build the bar
    let filled = ((used_pct as f32 / 100.0) * bar_width as f32).round() as usize;
    let empty = bar_width.saturating_sub(filled);

    let bar_filled = "█".repeat(filled);
    let bar_empty = "░".repeat(empty);

    // Build suffix
    let suffix = if left_pct == 0 {
        if let Some(reset) = reset_time {
            format!(" resets {}", reset)
        } else {
            " 0% left".to_string()
        }
    } else {
        format!(" {}% left", left_pct)
    };

    // Pad label to fixed width
    let padded_label = format!("{:<7}", label);

    Line::from(vec![
        Span::styled(padded_label, Style::default().fg(Color::Rgb(140, 140, 150))),
        Span::styled(bar_filled, Style::default().fg(color)),
        Span::styled(bar_empty, Style::default().fg(Color::Rgb(50, 50, 60))),
        Span::styled(suffix, Style::default().fg(color)),
    ])
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
                format!(
                    "{} session{}",
                    sessions,
                    if sessions == 1 { "" } else { "s" }
                ),
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
        format!("{}…", crate::util::truncate_str(model, 14))
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
    let limit_tokens = data.context_limit.unwrap_or(DEFAULT_CONTEXT_LIMIT).max(1);
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
        let mut content = format!("{} {} {} {}%", icon, label, format_token_k(tokens), pct);
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
    let limit_tokens = data.context_limit.unwrap_or(DEFAULT_CONTEXT_LIMIT).max(1);
    let used_pct = ((used_tokens as f64 / limit_tokens as f64) * 100.0)
        .round()
        .clamp(0.0, 100.0) as u8;
    let left_pct = 100u8.saturating_sub(used_pct);

    vec![render_labeled_bar("Context", used_pct, left_pct, None, inner.width)]
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
    spans.push(Span::styled(
        "[",
        Style::default().fg(Color::Rgb(90, 90, 100)),
    ));
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
    spans.push(Span::styled(
        "]",
        Style::default().fg(Color::Rgb(90, 90, 100)),
    ));
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
    let memory_chars = info.memory_chars;
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
    if memory_chars > 0 {
        entries.push(("🧠", "mem", memory_chars / 4));
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
