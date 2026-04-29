#![cfg_attr(test, allow(clippy::items_after_test_module))]

//!
//! Supports multiple widget types with priority ordering and side preferences.
//! In centered mode, widgets can appear on both left and right margins.
//! In left-aligned mode, widgets only appear on the right margin.

use super::color_support::rgb;
#[path = "info_widget_git.rs"]
mod git;
#[path = "info_widget_graph.rs"]
mod graph;
#[path = "info_widget_memory_render.rs"]
mod memory_render;
#[path = "info_widget_memory_utils.rs"]
mod memory_utils;
#[path = "info_widget_model.rs"]
mod model;
#[path = "info_widget_swarm_background.rs"]
mod swarm_background;
#[path = "info_widget_text.rs"]
mod text;
#[path = "info_widget_tips.rs"]
mod tips;
#[path = "info_widget_todos.rs"]
mod todos_render;
#[path = "info_widget_usage.rs"]
mod usage_render;
use super::info_widget_overview::{InfoPageKind, MAX_TODO_LINES, compute_page_layout};
use super::workspace_map::VisibleWorkspaceRow;
use crate::ambient::AmbientStatus;
pub use crate::memory_types::{
    InjectedMemoryItem, MemoryActivity, MemoryEvent, MemoryEventKind, MemoryState, PipelineState,
    StepResult, StepStatus,
};
use crate::prompt::ContextInfo;
use crate::protocol::SwarmMemberStatus;
use crate::provider::DEFAULT_CONTEXT_LIMIT;
use crate::todo::TodoItem;
use memory_render::{render_memory_compact, render_memory_expanded, render_memory_widget};
use ratatui::{
    prelude::*,
    widgets::{Block, BorderType, Borders, Paragraph},
};
use std::collections::HashMap;
#[cfg(test)]
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthStr;

use git::{render_git_compact, render_git_widget};
pub use graph::build_graph_topology;
pub(crate) use memory_utils::is_traceworthy_memory_event;
use memory_utils::{
    compact_memory_model_label, memory_active_summary, memory_last_trace_summary,
    memory_state_detail,
};
use model::{render_model_info, render_model_widget};
use swarm_background::{render_background_compact, render_background_widget, render_swarm_widget};
use text::{truncate_smart, truncate_with_ellipsis};
pub(crate) use tips::occasional_status_tip;
use tips::{render_tips_widget, tips_widget_height};
use todos_render::{render_todos_compact, render_todos_expanded, render_todos_widget};
#[cfg(test)]
use usage_render::render_usage_bar;
use usage_render::{render_context_usage_line, render_usage_compact, render_usage_widget};

/// Types of info widgets that can be displayed
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WidgetKind {
    /// Combined overview to reduce scattered widgets
    Overview,
    /// Niri-style workspace map preview
    WorkspaceMap,
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
    /// Session-level KV cache hit ratio
    KvCache,
    /// Current model name
    ModelInfo,
    /// Mermaid diagrams
    Diagrams,
    /// Ambient mode status
    AmbientMode,
    /// Rotating tips/shortcuts
    Tips,
    /// Git status
    GitStatus,
}

impl WidgetKind {
    /// Priority for display (lower = higher priority)
    pub fn priority(self) -> u8 {
        match self {
            WidgetKind::Diagrams => 0, // Highest priority - user explicitly wants to see it
            WidgetKind::WorkspaceMap => 1,
            WidgetKind::Overview => 2,
            WidgetKind::Todos => 3,
            WidgetKind::ContextUsage => 4,
            WidgetKind::UsageLimits => 5, // Bumped up - important when near limits
            WidgetKind::KvCache => 6,
            WidgetKind::MemoryActivity => 7,
            WidgetKind::ModelInfo => 8,
            WidgetKind::BackgroundTasks => 9,
            WidgetKind::GitStatus => 10,
            WidgetKind::SwarmStatus => 11, // Session list - lower priority
            WidgetKind::AmbientMode => 12, // Scheduled agent - lower priority
            WidgetKind::Tips => 13,        // Did you know - lowest
        }
    }

    /// Preferred side for this widget
    pub fn preferred_side(self) -> Side {
        match self {
            WidgetKind::Diagrams => Side::Right, // Diagrams on right
            WidgetKind::WorkspaceMap => Side::Right,
            WidgetKind::Overview => Side::Right,
            WidgetKind::Todos => Side::Right,
            WidgetKind::ContextUsage => Side::Right,
            WidgetKind::MemoryActivity => Side::Right,
            WidgetKind::SwarmStatus => Side::Left,
            WidgetKind::BackgroundTasks => Side::Left,
            WidgetKind::AmbientMode => Side::Left,
            WidgetKind::UsageLimits => Side::Left,
            WidgetKind::KvCache => Side::Left,
            WidgetKind::ModelInfo => Side::Left,
            WidgetKind::Tips => Side::Left,
            WidgetKind::GitStatus => Side::Left,
        }
    }

    /// Minimum height needed for this widget
    pub fn min_height(self) -> u16 {
        match self {
            WidgetKind::Diagrams => 10, // Diagrams need more space
            WidgetKind::WorkspaceMap => 1,
            WidgetKind::Overview => 8,
            WidgetKind::Todos => 3,
            WidgetKind::ContextUsage => 2,
            WidgetKind::MemoryActivity => 3,
            WidgetKind::SwarmStatus => 3,
            WidgetKind::BackgroundTasks => 2,
            WidgetKind::AmbientMode => 3,
            WidgetKind::UsageLimits => 3,
            WidgetKind::KvCache => 3,
            WidgetKind::ModelInfo => 3, // Model + usage bars
            WidgetKind::Tips => 3,
            WidgetKind::GitStatus => 3,
        }
    }

    /// All widget kinds in priority order
    pub fn all_by_priority() -> &'static [WidgetKind] {
        &[
            WidgetKind::Diagrams,
            WidgetKind::WorkspaceMap,
            WidgetKind::Overview,
            WidgetKind::Todos,
            WidgetKind::ContextUsage,
            WidgetKind::UsageLimits,
            WidgetKind::KvCache,
            WidgetKind::MemoryActivity,
            WidgetKind::ModelInfo,
            WidgetKind::BackgroundTasks,
            WidgetKind::GitStatus,
            WidgetKind::SwarmStatus,
            WidgetKind::AmbientMode,
            WidgetKind::Tips,
        ]
    }

    pub fn as_str(self) -> &'static str {
        match self {
            WidgetKind::Diagrams => "diagrams",
            WidgetKind::WorkspaceMap => "workspace",
            WidgetKind::Overview => "overview",
            WidgetKind::Todos => "todos",
            WidgetKind::ContextUsage => "context",
            WidgetKind::MemoryActivity => "memory",
            WidgetKind::SwarmStatus => "swarm",
            WidgetKind::BackgroundTasks => "background",
            WidgetKind::AmbientMode => "ambient",
            WidgetKind::UsageLimits => "usage",
            WidgetKind::KvCache => "kv-cache",
            WidgetKind::ModelInfo => "model",
            WidgetKind::Tips => "tips",
            WidgetKind::GitStatus => "git",
        }
    }
}

/// Which side of the screen a widget is on
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
        }
    }
}

pub(crate) fn is_overview_mergeable(kind: WidgetKind) -> bool {
    matches!(
        kind,
        WidgetKind::Todos
            | WidgetKind::ContextUsage
            | WidgetKind::SwarmStatus
            | WidgetKind::BackgroundTasks
            | WidgetKind::ModelInfo
            | WidgetKind::UsageLimits
            | WidgetKind::KvCache
            | WidgetKind::GitStatus
    )
}

/// A placed widget with its location and type
#[derive(Debug, Clone)]
pub struct WidgetPlacement {
    pub kind: WidgetKind,
    pub rect: Rect,
    pub side: Side,
}

pub use super::info_widget_layout::Margins;

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
    /// Swarm member lifecycle status updates
    pub members: Vec<SwarmMemberStatus>,
}

/// Background task status for the info widget
#[derive(Debug, Default, Clone)]
pub struct BackgroundInfo {
    /// Number of running background tasks
    pub running_count: usize,
    /// Names of running tasks (e.g., "bash", "task")
    pub running_tasks: Vec<String>,
    /// Compact summary of the most recent task progress
    pub progress_summary: Option<String>,
    /// Detailed display for the most recent task progress
    pub progress_detail: Option<String>,
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
    /// GitHub Copilot (shows session token counts, no cost)
    Copilot,
}

impl UsageProvider {
    pub fn label(&self) -> &'static str {
        match self {
            UsageProvider::None => "",
            UsageProvider::Anthropic => "Anthropic",
            UsageProvider::OpenAI => "OpenAI",
            UsageProvider::CostBased => "",
            UsageProvider::Copilot => "Copilot",
        }
    }
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
    /// GitHub Copilot OAuth
    CopilotOAuth,
    /// Google Gemini OAuth
    GeminiOAuth,
}

/// Subscription usage info for the info widget
#[derive(Debug, Default, Clone)]
pub struct UsageInfo {
    /// Which provider this usage is for
    pub provider: UsageProvider,
    /// Five-hour window utilization (0.0-1.0) - for OAuth providers
    pub five_hour: f32,
    /// Five-hour reset timestamp (RFC3339), if known
    pub five_hour_resets_at: Option<String>,
    /// Seven-day window utilization (0.0-1.0) - for OAuth providers
    pub seven_day: f32,
    /// Seven-day reset timestamp (RFC3339), if known
    pub seven_day_resets_at: Option<String>,
    /// Codex Spark window utilization (0.0-1.0), if available
    pub spark: Option<f32>,
    /// Codex Spark reset timestamp (RFC3339), if known
    pub spark_resets_at: Option<String>,
    /// Total cost in USD - for API-key providers (OpenRouter, direct API key)
    pub total_cost: f32,
    /// Input tokens used - for cost calculation
    pub input_tokens: u64,
    /// Output tokens used - for cost calculation
    pub output_tokens: u64,
    /// Cache read tokens (from cache, cheaper) - for API-key providers
    pub cache_read_tokens: Option<u64>,
    /// Cache write tokens (creating cache, more expensive) - for API-key providers
    pub cache_write_tokens: Option<u64>,
    /// Output tokens per second (live streaming)
    pub output_tps: Option<f32>,
    /// Whether data was successfully fetched / available to show
    pub available: bool,
}

/// Session-level KV cache telemetry for providers that report cache usage.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CacheHitInfo {
    /// Input tokens from completed API requests that included explicit cache telemetry.
    pub reported_input_tokens: u64,
    /// Tokens read from provider KV/prefix cache across this session.
    pub read_tokens: u64,
    /// Tokens written/created in provider cache across this session, when reported.
    pub creation_tokens: u64,
    /// Approximate reusable prefix tokens expected to be cache-readable.
    pub optimal_input_tokens: u64,
    /// Input tokens from the latest completed request with cache telemetry.
    pub last_reported_input_tokens: Option<u64>,
    /// Cached input tokens read on the latest completed request with cache telemetry.
    pub last_read_tokens: Option<u64>,
    /// Approximate reusable prefix tokens expected on the latest completed request.
    pub last_optimal_input_tokens: Option<u64>,
    /// Recent attributed misses with estimated cacheable tokens not read.
    pub miss_attributions: Vec<CacheMissAttribution>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheMissAttribution {
    pub turn_number: usize,
    pub call_index: u16,
    pub missed_tokens: u64,
    pub reason: String,
}

impl CacheHitInfo {
    pub fn hit_ratio(&self) -> Option<f32> {
        if self.reported_input_tokens == 0 {
            None
        } else {
            Some((self.read_tokens as f32 / self.reported_input_tokens as f32).clamp(0.0, 1.0))
        }
    }

    pub fn optimal_ratio(&self) -> Option<f32> {
        if self.optimal_input_tokens == 0 {
            None
        } else {
            Some((self.read_tokens as f32 / self.optimal_input_tokens as f32).clamp(0.0, 1.0))
        }
    }

    pub fn last_ratio(&self) -> Option<f32> {
        let input = self.last_reported_input_tokens?;
        if input == 0 {
            None
        } else {
            Some((self.last_read_tokens.unwrap_or(0) as f32 / input as f32).clamp(0.0, 1.0))
        }
    }

    pub fn last_optimal_ratio(&self) -> Option<f32> {
        let optimal = self.last_optimal_input_tokens?;
        if optimal == 0 {
            None
        } else {
            Some((self.last_read_tokens.unwrap_or(0) as f32 / optimal as f32).clamp(0.0, 1.0))
        }
    }
}

impl UsageInfo {
    /// Return the highest usage percentage across all limit windows (0-100).
    pub fn max_usage_pct(&self) -> u8 {
        let five_hr = (self.five_hour * 100.0).round().clamp(0.0, 100.0) as u8;
        let seven_day = (self.seven_day * 100.0).round().clamp(0.0, 100.0) as u8;
        let spark = self
            .spark
            .map(|v| (v * 100.0).round().clamp(0.0, 100.0) as u8)
            .unwrap_or(0);
        five_hr.max(seven_day).max(spark)
    }
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
    /// Selected sidecar model/backend label for memory work
    pub sidecar_model: Option<String>,
    /// Current memory activity
    pub activity: Option<MemoryActivity>,
    /// Graph topology for visualization (node positions + edges)
    pub graph_nodes: Vec<GraphNode>,
    /// Directed edges into graph_nodes
    pub graph_edges: Vec<GraphEdge>,
}

/// A node in the mini graph visualization
#[derive(Debug, Clone)]
pub struct GraphNode {
    /// Stable node ID from memory graph (mem:*, tag:*, cluster:*)
    pub id: String,
    /// Human-readable display label
    pub label: String,
    /// Category: "fact", "preference", "correction", "tag"
    pub kind: String,
    /// Whether this node is a memory (vs tag/cluster)
    pub is_memory: bool,
    /// Whether this node is active (superseded memories are inactive)
    pub is_active: bool,
    /// Effective confidence score (0.0-1.0)
    pub confidence: f32,
    /// Number of connections (degree)
    pub degree: usize,
}

/// A directed edge in the memory graph visualization
#[derive(Debug, Clone)]
pub struct GraphEdge {
    /// Source index into MemoryInfo::graph_nodes
    pub source: usize,
    /// Target index into MemoryInfo::graph_nodes
    pub target: usize,
    /// Edge kind (has_tag, supersedes, contradicts, ...)
    pub kind: String,
}

pub use jcode_tui_mermaid::DiagramInfo;

/// Git repository status for the info widget
#[derive(Debug, Clone)]
pub struct GitInfo {
    pub branch: String,
    pub modified: usize,
    pub staged: usize,
    pub untracked: usize,
    pub ahead: usize,
    pub behind: usize,
    pub dirty_files: Vec<String>,
}

impl GitInfo {
    pub fn is_interesting(&self) -> bool {
        self.modified > 0
            || self.staged > 0
            || self.untracked > 0
            || self.ahead > 0
            || self.behind > 0
    }
}

/// Ambient mode status data for the info widget
#[derive(Debug, Clone)]
pub struct AmbientWidgetData {
    pub show_widget: bool,
    pub status: AmbientStatus,
    pub queue_count: usize,
    pub next_queue_preview: Option<String>,
    pub reminder_count: usize,
    pub next_reminder_preview: Option<String>,
    pub last_run_ago: Option<String>,
    pub last_summary: Option<String>,
    pub next_wake: Option<String>,
    pub next_reminder_wake: Option<String>,
    pub budget_percent: Option<f32>,
}

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
    pub service_tier: Option<String>,
    pub native_compaction_mode: Option<String>,
    pub native_compaction_threshold_tokens: Option<usize>,
    pub session_count: Option<usize>,
    pub session_name: Option<String>,
    pub client_count: Option<usize>,
    /// Memory system statistics
    pub memory_info: Option<MemoryInfo>,
    /// Swarm/subagent status
    pub swarm_info: Option<SwarmInfo>,
    /// Background tasks status
    pub background_info: Option<BackgroundInfo>,
    /// Subscription usage info
    pub usage_info: Option<UsageInfo>,
    /// Streaming output tokens per second (approximate)
    pub tokens_per_second: Option<f32>,
    /// Active provider name (openrouter/openai/anthropic/...)
    pub provider_name: Option<String>,
    /// Authentication method used to access the model
    pub auth_method: AuthMethod,
    /// Upstream provider (e.g., which OpenRouter provider served the request: fireworks, etc.)
    pub upstream_provider: Option<String>,
    /// Active connection type (websocket/https/etc.)
    pub connection_type: Option<String>,
    /// Mermaid diagrams to display
    pub diagrams: Vec<DiagramInfo>,
    /// Visible Niri-style workspace rows
    pub workspace_rows: Vec<VisibleWorkspaceRow>,
    /// Lightweight animation tick for workspace map rendering
    pub workspace_animation_tick: u64,
    /// Ambient mode status
    pub ambient_info: Option<AmbientWidgetData>,
    /// Actual API-reported context tokens (from last streaming response)
    /// When available, this is more accurate than the char-based estimate in context_info
    pub observed_context_tokens: Option<u64>,
    /// Session-level cache read ratio, when the active provider reports cache telemetry.
    pub cache_hit_info: Option<CacheHitInfo>,
    /// Whether background compaction is currently in progress
    pub is_compacting: bool,
    /// Git repository status
    pub git_info: Option<GitInfo>,
}

impl InfoWidgetData {
    fn widget_disabled(kind: WidgetKind) -> bool {
        matches!(
            kind,
            WidgetKind::SwarmStatus | WidgetKind::AmbientMode | WidgetKind::Tips
        )
    }

    pub fn is_empty(&self) -> bool {
        self.todos.is_empty()
            && self.context_info.is_none()
            && self.queue_mode.is_none()
            && self.model.is_none()
            && self.memory_info.is_none()
            && self.swarm_info.is_none()
            && self.background_info.is_none()
            && self.diagrams.is_empty()
            && self.workspace_rows.is_empty()
    }

    /// Check if a specific widget kind has data to display
    pub fn has_data_for(&self, kind: WidgetKind) -> bool {
        if Self::widget_disabled(kind) {
            return false;
        }

        match kind {
            WidgetKind::Diagrams => !self.diagrams.is_empty(),
            WidgetKind::WorkspaceMap => !self.workspace_rows.is_empty(),
            WidgetKind::Overview => {
                let mut sections = 0usize;
                if self.model.is_some() {
                    sections += 1;
                }
                if self
                    .context_info
                    .as_ref()
                    .map(|c| c.total_chars > 0)
                    .unwrap_or(false)
                {
                    sections += 1;
                }
                if !self.todos.is_empty() {
                    sections += 1;
                }
                if self
                    .background_info
                    .as_ref()
                    .map(|b| b.running_count > 0)
                    .unwrap_or(false)
                {
                    sections += 1;
                }
                if self.queue_mode.is_some() {
                    sections += 1;
                }
                if self
                    .usage_info
                    .as_ref()
                    .map(|u| u.available)
                    .unwrap_or(false)
                {
                    sections += 1;
                }
                if self.cache_hit_info.is_some() {
                    sections += 1;
                }
                if self
                    .git_info
                    .as_ref()
                    .map(|g| g.is_interesting())
                    .unwrap_or(false)
                {
                    sections += 1;
                }
                // Only useful as a "join" mode when there are multiple sections.
                sections >= 2
            }
            WidgetKind::Todos => !self.todos.is_empty(),
            WidgetKind::ContextUsage => self
                .context_info
                .as_ref()
                .map(|c| c.total_chars > 0)
                .unwrap_or(false),
            WidgetKind::MemoryActivity => self
                .memory_info
                .as_ref()
                .map(|m| m.total_count > 0 || m.activity.is_some() || m.sidecar_model.is_some())
                .unwrap_or(false),
            WidgetKind::SwarmStatus => false,
            WidgetKind::BackgroundTasks => self
                .background_info
                .as_ref()
                .map(|b| b.running_count > 0)
                .unwrap_or(false),
            WidgetKind::AmbientMode => false,
            WidgetKind::UsageLimits => self
                .usage_info
                .as_ref()
                .map(|u| u.available)
                .unwrap_or(false),
            WidgetKind::KvCache => self.cache_hit_info.is_some(),
            WidgetKind::ModelInfo => self.model.is_some(),
            WidgetKind::Tips => false,
            WidgetKind::GitStatus => self
                .git_info
                .as_ref()
                .map(|g| g.is_interesting())
                .unwrap_or(false),
        }
    }

    /// Get list of widget kinds that have data, in priority order
    /// Get effective priority for a widget, accounting for dynamic state.
    /// UsageLimits gets bumped up when usage is high.
    /// MemoryActivity gets bumped up while memory work is actively processing.
    pub fn effective_priority(&self, kind: WidgetKind) -> u8 {
        match kind {
            WidgetKind::MemoryActivity => {
                if self
                    .memory_info
                    .as_ref()
                    .and_then(|info| info.activity.as_ref())
                    .map(MemoryActivity::is_processing)
                    .unwrap_or(false)
                {
                    0
                } else {
                    kind.priority()
                }
            }
            WidgetKind::UsageLimits => {
                let max_pct = self
                    .usage_info
                    .as_ref()
                    .map(|u| u.max_usage_pct())
                    .unwrap_or(0);
                if max_pct >= 80 {
                    1 // Very high - right after diagrams
                } else if max_pct >= 50 {
                    3 // Elevated - after overview and todos
                } else {
                    kind.priority()
                }
            }
            _ => kind.priority(),
        }
    }

    pub fn available_widgets(&self) -> Vec<WidgetKind> {
        let mut widgets: Vec<WidgetKind> = WidgetKind::all_by_priority()
            .iter()
            .copied()
            .filter(|&kind| self.has_data_for(kind))
            .collect();
        widgets.sort_by_key(|&kind| self.effective_priority(kind));
        widgets
    }
}

/// State for a single widget instance
#[derive(Debug, Clone, Default)]
struct SingleWidgetState {
    /// Current page index (for widgets with multiple pages)
    page_index: usize,
    /// Last time the page advanced
    last_page_switch: Option<Instant>,
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

    let placements = super::info_widget_layout::calculate_placements(
        messages_area,
        margins,
        data,
        state.enabled,
        &state.placements,
    );
    state.placements = placements.clone();
    placements
}

/// Calculate the height needed for a specific widget type
pub(crate) fn calculate_widget_height(
    kind: WidgetKind,
    data: &InfoWidgetData,
    width: u16,
    max_height: u16,
) -> u16 {
    let inner_width = width.saturating_sub(2) as usize;
    let border_height = 2u16;

    let content_height = match kind {
        WidgetKind::WorkspaceMap => {
            if data.workspace_rows.is_empty() {
                return 0;
            }
            let (_preferred_w, preferred_h) =
                super::workspace_map_widget::preferred_size(&data.workspace_rows);
            preferred_h.min(max_height.saturating_sub(border_height))
        }
        WidgetKind::Overview => {
            let mut overview = data.clone();
            // Keep memory in its own widget so graph rendering stays focused.
            overview.memory_info = None;
            let inner_h = max_height.saturating_sub(border_height);
            let layout = compute_page_layout(&overview, inner_width, inner_h);
            if layout.max_page_height == 0 {
                return 0;
            }
            layout.max_page_height
        }
        WidgetKind::Diagrams => {
            if data.diagrams.is_empty() {
                return 0;
            }
            // Use the full available height so the image fills the panel
            max_height.saturating_sub(border_height)
        }
        WidgetKind::Todos => {
            if data.todos.is_empty() {
                return 0;
            }
            // Header + progress bar + up to 5 items
            let items = data.todos.len().min(5) as u16;
            2 + items + if data.todos.len() > 5 { 1 } else { 0 }
        }
        WidgetKind::ContextUsage => {
            if data
                .context_info
                .as_ref()
                .map(|c| c.total_chars == 0)
                .unwrap_or(true)
            {
                return 0;
            }
            1 // Just the bar
        }
        WidgetKind::MemoryActivity => {
            if data.memory_info.is_none() {
                return 0;
            };
            let lines =
                render_memory_widget(data, Rect::new(0, 0, width.saturating_sub(2), max_height));
            if lines.is_empty() {
                return 0;
            }
            lines.len() as u16
        }
        WidgetKind::SwarmStatus => {
            let Some(info) = &data.swarm_info else {
                return 0;
            };
            if info.subagent_status.is_none()
                && info.session_count <= 1
                && info.client_count.is_none()
                && info.members.is_empty()
            {
                return 0;
            }
            let mut h = 1u16; // Stats line
            if info.subagent_status.is_some() {
                h += 1;
            }
            h += info.session_names.len().min(3) as u16;
            h
        }
        WidgetKind::BackgroundTasks => {
            if data
                .background_info
                .as_ref()
                .map(|b| b.running_count == 0)
                .unwrap_or(true)
            {
                return 0;
            }
            data.background_info
                .as_ref()
                .map(|b| {
                    let task_lines = b.running_tasks.len().min(3) as u16;
                    let overflow_line = u16::from(b.running_tasks.len() > 3);
                    1 + task_lines + overflow_line
                })
                .unwrap_or(1)
        }
        WidgetKind::AmbientMode => {
            let Some(info) = &data.ambient_info else {
                return 0;
            };
            if !info.show_widget {
                return 0;
            }
            let mut h = 1u16; // Status line
            if info.queue_count > 0 || info.reminder_count > 0 {
                h += 1; // Queue line
            }
            if info.last_run_ago.is_some() {
                h += 1; // Last run line
            }
            if info.next_wake.is_some() || info.next_reminder_wake.is_some() {
                h += 1; // Next wake line
            }
            if info.budget_percent.is_some() {
                h += 1; // Budget bar
            }
            h
        }
        WidgetKind::UsageLimits => {
            if let Some(info) = data.usage_info.as_ref() {
                if info.available {
                    2 + if info.spark.is_some() { 1 } else { 0 }
                } else {
                    0
                }
            } else {
                0
            }
        }
        WidgetKind::KvCache => {
            let Some(cache) = data.cache_hit_info.as_ref() else {
                return 0;
            };
            let attribution_lines = if cache.miss_attributions.is_empty() {
                2
            } else {
                let visible = cache.miss_attributions.len().min(5) as u16;
                2 + visible + u16::from(cache.miss_attributions.len() > 5)
            };
            1 + attribution_lines
        }
        WidgetKind::ModelInfo => {
            if data.model.is_none() {
                return 0;
            }
            let mut h = 1u16; // Model name
            if data
                .provider_name
                .as_deref()
                .map(str::trim)
                .is_some_and(|s| !s.is_empty())
            {
                h += 1; // Provider line
            }
            if data
                .connection_type
                .as_deref()
                .map(str::trim)
                .is_some_and(|s| !s.is_empty())
            {
                h += 1; // Connection line
            }
            if data.auth_method != AuthMethod::Unknown {
                h += 1; // Auth method line
            }
            if data.session_count.is_some() || data.session_name.is_some() {
                h += 1; // Session/name line
            }
            if let Some(info) = &data.usage_info
                && info.available
            {
                match info.provider {
                    UsageProvider::CostBased | UsageProvider::Copilot => {
                        h += 1; // Cost/tokens line
                        if info.cache_read_tokens.is_some() || info.cache_write_tokens.is_some() {
                            h += 1; // Cache line
                        }
                        if info.output_tps.is_some() {
                            h += 1; // TPS line
                        }
                    }
                    _ => {
                        h += 2; // Base subscription bars
                        if info.spark.is_some() {
                            h += 1; // Optional Spark bar
                        }
                    }
                }
            }
            h
        }
        WidgetKind::Tips => tips_widget_height(inner_width),
        WidgetKind::GitStatus => {
            let Some(info) = &data.git_info else {
                return 0;
            };
            if !info.is_interesting() {
                return 0;
            }
            let mut h = 1u16; // Branch + stats on one line
            h += info.dirty_files.len().min(5) as u16;
            if info.dirty_files.len() > 5 {
                h += 1;
            }
            h
        }
    };

    let total = content_height + border_height;
    total.min(max_height)
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
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(rgb(70, 70, 80)).dim());

    if placement.kind == WidgetKind::WorkspaceMap {
        block = block.title(Span::styled(
            " Workspace ",
            Style::default().fg(rgb(120, 120, 130)).dim(),
        ));
    }

    let inner = block.inner(rect);

    // Diagrams need special handling - render image instead of text
    if placement.kind == WidgetKind::Diagrams {
        frame.render_widget(block, rect);
        render_diagrams_widget(frame, inner, data);
        return;
    }
    if placement.kind == WidgetKind::Overview {
        // Check if overview would actually render content before drawing the border
        let mut overview = data.clone();
        overview.memory_info = None;
        overview.diagrams.clear();
        let layout = compute_page_layout(&overview, inner.width as usize, inner.height);
        if layout.pages.is_empty() || layout.max_page_height == 0 {
            return;
        }
        frame.render_widget(block, rect);
        render_overview_widget(frame, inner, data);
        return;
    }
    if placement.kind == WidgetKind::WorkspaceMap {
        if data.workspace_rows.is_empty() || inner.width == 0 || inner.height == 0 {
            return;
        }
        frame.render_widget(block, rect);
        super::workspace_map_widget::render_workspace_map(
            frame.buffer_mut(),
            inner,
            &data.workspace_rows,
            data.workspace_animation_tick,
        );
        return;
    }
    let lines = render_widget_content(placement.kind, data, inner);
    if lines.is_empty() {
        return;
    }
    frame.render_widget(block, rect);
    let para = Paragraph::new(lines);
    frame.render_widget(para, inner);
}

/// Render mermaid diagrams widget (renders images, not text)
fn render_diagrams_widget(frame: &mut Frame, inner: Rect, data: &InfoWidgetData) {
    if data.diagrams.is_empty() {
        return;
    }

    // For now, just render the first/most recent diagram
    // Could add pagination later for multiple diagrams
    let diagram = &data.diagrams[0];

    // Render the image using mermaid module
    super::mermaid::render_image_widget(diagram.hash, inner, frame.buffer_mut(), false, false);
}

fn render_overview_widget(frame: &mut Frame, inner: Rect, data: &InfoWidgetData) {
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let mut overview = data.clone();
    // Keep memory graph and diagram visuals in dedicated widgets.
    overview.memory_info = None;
    overview.diagrams.clear();

    let layout = compute_page_layout(&overview, inner.width as usize, inner.height);
    if layout.pages.is_empty() {
        return;
    }

    let mut guard = get_or_init_state();
    let state = match guard.as_mut() {
        Some(state) => state,
        None => return,
    };
    let widget_state = state.widget_states.entry(WidgetKind::Overview).or_default();

    if layout.pages.len() > 1 {
        let now = Instant::now();
        let should_advance = widget_state
            .last_page_switch
            .map(|last| now.duration_since(last).as_secs() >= PAGE_SWITCH_SECONDS)
            .unwrap_or(true);
        if should_advance {
            widget_state.page_index = (widget_state.page_index + 1) % layout.pages.len();
            widget_state.last_page_switch = Some(now);
        }
    } else {
        widget_state.page_index = 0;
        widget_state.last_page_switch = None;
    }

    let page_index = widget_state.page_index.min(layout.pages.len() - 1);
    let page = layout.pages[page_index];
    let mut lines = render_page(page.kind, &overview, inner);

    // If the page rendered no content, bail out to avoid an empty box
    if lines.is_empty() {
        return;
    }

    if layout.show_dots && inner.height > 0 {
        let mut dots: Vec<Span<'static>> = Vec::new();
        for i in 0..layout.pages.len() {
            if i == page_index {
                dots.push(Span::styled("● ", Style::default().fg(rgb(170, 170, 180))));
            } else {
                dots.push(Span::styled("○ ", Style::default().fg(rgb(100, 100, 110))));
            }
        }
        if !dots.is_empty() {
            lines.push(Line::from(dots));
        }
    }

    lines.truncate(inner.height as usize);
    frame.render_widget(Paragraph::new(lines), inner);
}
#[cfg(test)]
#[derive(Debug, Clone)]
struct MemorySubgraph {
    nodes: Vec<GraphNode>,
    _edges: Vec<GraphEdge>,
}
#[cfg(test)]
fn select_contextual_subgraph(
    info: &MemoryInfo,
    max_nodes: usize,
    max_edges: usize,
) -> Option<MemorySubgraph> {
    if info.graph_nodes.is_empty() || max_nodes == 0 {
        return None;
    }
    let node_count = info.graph_nodes.len();
    let center_idx = pick_subgraph_center(info)?;
    let mut neighbors: Vec<Vec<(usize, usize)>> = vec![Vec::new(); node_count];
    for (edge_idx, edge) in info.graph_edges.iter().enumerate() {
        if edge.source >= node_count || edge.target >= node_count {
            continue;
        }
        neighbors[edge.source].push((edge.target, edge_idx));
        neighbors[edge.target].push((edge.source, edge_idx));
    }
    let mut selected = Vec::with_capacity(max_nodes.min(node_count));
    let mut selected_set: HashSet<usize> = HashSet::new();
    let mut queue = std::collections::VecDeque::new();
    selected.push(center_idx);
    selected_set.insert(center_idx);
    queue.push_back(center_idx);
    while let Some(current) = queue.pop_front() {
        if selected.len() >= max_nodes {
            break;
        }
        let mut ranked = neighbors[current].clone();
        ranked.sort_by(|(a_idx, a_edge), (b_idx, b_edge)| {
            edge_kind_priority(&info.graph_edges[*b_edge].kind)
                .cmp(&edge_kind_priority(&info.graph_edges[*a_edge].kind))
                .then_with(|| {
                    graph::graph_node_score(&info.graph_nodes[*b_idx])
                        .partial_cmp(&graph::graph_node_score(&info.graph_nodes[*a_idx]))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a_idx.cmp(b_idx))
        });
        for (next_idx, _) in ranked {
            if selected.len() >= max_nodes {
                break;
            }
            if selected_set.insert(next_idx) {
                selected.push(next_idx);
                queue.push_back(next_idx);
            }
        }
    }

    if selected.len() < max_nodes {
        let mut remaining: Vec<usize> = (0..node_count)
            .filter(|idx| !selected_set.contains(idx))
            .collect();
        remaining.sort_by(|a, b| {
            graph::graph_node_score(&info.graph_nodes[*b])
                .partial_cmp(&graph::graph_node_score(&info.graph_nodes[*a]))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.cmp(b))
        });
        for idx in remaining {
            if selected.len() >= max_nodes {
                break;
            }
            selected_set.insert(idx);
            selected.push(idx);
        }
    }

    let mut old_to_new = HashMap::new();
    let mut sub_nodes = Vec::with_capacity(selected.len());
    for (new_idx, old_idx) in selected.iter().copied().enumerate() {
        old_to_new.insert(old_idx, new_idx);
        sub_nodes.push(info.graph_nodes[old_idx].clone());
    }

    let center_new = old_to_new.get(&center_idx).copied().unwrap_or(0);
    let mut dedup: HashSet<(usize, usize, String)> = HashSet::new();
    let mut sub_edges: Vec<GraphEdge> = info
        .graph_edges
        .iter()
        .filter_map(|edge| {
            let source = *old_to_new.get(&edge.source)?;
            let target = *old_to_new.get(&edge.target)?;
            if source == target {
                return None;
            }
            if !dedup.insert((source, target, edge.kind.clone())) {
                return None;
            }
            Some(GraphEdge {
                source,
                target,
                kind: edge.kind.clone(),
            })
        })
        .collect();

    sub_edges.sort_by(|a, b| {
        let a_center = a.source == center_new || a.target == center_new;
        let b_center = b.source == center_new || b.target == center_new;
        b_center
            .cmp(&a_center)
            .then_with(|| edge_kind_priority(&b.kind).cmp(&edge_kind_priority(&a.kind)))
            .then_with(|| a.source.cmp(&b.source))
            .then_with(|| a.target.cmp(&b.target))
    });
    if sub_edges.len() > max_edges {
        sub_edges.truncate(max_edges);
    }

    Some(MemorySubgraph {
        nodes: sub_nodes,
        _edges: sub_edges,
    })
}

#[cfg(test)]
fn pick_subgraph_center(info: &MemoryInfo) -> Option<usize> {
    let mut best_idx: Option<usize> = None;
    let mut best_score: f32 = -1.0;

    for (idx, node) in info.graph_nodes.iter().enumerate() {
        let mut score = graph::graph_node_score(node);
        if node.kind == "tag" || node.kind == "cluster" {
            score -= 0.75;
        }
        if !node.is_active {
            score -= 1.0;
        }
        if score > best_score {
            best_score = score;
            best_idx = Some(idx);
        }
    }

    best_idx
}

#[cfg(test)]
fn edge_kind_priority(kind: &str) -> u8 {
    match kind {
        "contradicts" => 6,
        "supersedes" => 5,
        "derived_from" => 4,
        "relates_to" => 3,
        "in_cluster" => 2,
        "has_tag" => 1,
        _ => 1,
    }
}

/// Render content for a specific widget type
fn render_widget_content(
    kind: WidgetKind,
    data: &InfoWidgetData,
    inner: Rect,
) -> Vec<Line<'static>> {
    match kind {
        WidgetKind::Diagrams => Vec::new(), // Handled specially in render_single_widget
        WidgetKind::WorkspaceMap => Vec::new(), // Handled specially in render_single_widget
        WidgetKind::Overview => Vec::new(), // Handled specially in render_single_widget
        WidgetKind::Todos => render_todos_widget(data, inner),
        WidgetKind::ContextUsage => render_context_widget(data, inner),
        WidgetKind::MemoryActivity => render_memory_widget(data, inner),
        WidgetKind::SwarmStatus => render_swarm_widget(data, inner),
        WidgetKind::BackgroundTasks => render_background_widget(data, inner),
        WidgetKind::AmbientMode => render_ambient_widget(data, inner),
        WidgetKind::UsageLimits => render_usage_widget(data, inner),
        WidgetKind::KvCache => render_kv_cache_widget(data, inner),
        WidgetKind::ModelInfo => render_model_widget(data, inner),
        WidgetKind::Tips => render_tips_widget(inner),
        WidgetKind::GitStatus => render_git_widget(data, inner),
    }
}

fn render_kv_cache_widget(data: &InfoWidgetData, _inner: Rect) -> Vec<Line<'static>> {
    let Some(cache) = data.cache_hit_info.as_ref() else {
        return Vec::new();
    };
    let mut lines = vec![render_kv_cache_summary_line(cache)];

    lines.push(Line::from(vec![Span::styled(
        "miss attribution",
        Style::default().fg(rgb(140, 140, 150)).bold(),
    )]));

    if cache.miss_attributions.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            "none",
            Style::default().fg(rgb(110, 210, 140)),
        )]));
        return lines;
    }

    let total_missed: u64 = cache
        .miss_attributions
        .iter()
        .map(|sample| sample.missed_tokens)
        .sum();
    lines.push(Line::from(vec![Span::styled(
        format!("{} missed total", compact_token_count(total_missed)),
        Style::default().fg(rgb(180, 180, 190)),
    )]));

    for sample in cache.miss_attributions.iter().take(5) {
        lines.push(Line::from(vec![
            Span::styled(
                format_cache_turn_label(sample.turn_number, sample.call_index),
                Style::default().fg(rgb(140, 180, 255)).bold(),
            ),
            Span::styled(
                format!(" {} miss ", compact_token_count(sample.missed_tokens)),
                Style::default().fg(rgb(255, 200, 100)),
            ),
            Span::styled(
                format!("({})", sample.reason),
                Style::default().fg(rgb(140, 140, 150)),
            ),
        ]));
    }

    if cache.miss_attributions.len() > 5 {
        lines.push(Line::from(vec![Span::styled(
            format!("… {} more", cache.miss_attributions.len() - 5),
            Style::default().fg(rgb(100, 100, 110)),
        )]));
    }

    lines
}

fn render_kv_cache_summary_line(cache: &CacheHitInfo) -> Line<'static> {
    let Some(lifetime_ratio) = cache.hit_ratio() else {
        return Line::default();
    };

    let lifetime_pct = ratio_pct(lifetime_ratio);
    let warm_pct = cache.optimal_ratio().map(ratio_pct);
    let last_pct = cache.last_ratio().map(ratio_pct);
    let last_optimal_pct = cache.last_optimal_ratio().map(ratio_pct);
    let health_pct = last_optimal_pct
        .or(last_pct)
        .or(warm_pct)
        .unwrap_or(lifetime_pct);
    let color = kv_cache_optimal_color(health_pct);

    let mut spans = vec![Span::styled(
        "KV cache: ",
        Style::default().fg(rgb(180, 180, 190)).bold(),
    )];

    if let Some(warm_pct) = warm_pct {
        spans.push(Span::styled(
            "warm ",
            Style::default().fg(rgb(140, 140, 150)),
        ));
        spans.push(Span::styled(
            format!("{}%", warm_pct),
            Style::default().fg(color).bold(),
        ));
    } else {
        spans.push(Span::styled(
            "warming",
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    }

    if let Some(last_pct) = last_pct {
        spans.push(Span::styled(" · ", Style::default().fg(rgb(80, 80, 90))));
        spans.push(Span::styled(
            "last ",
            Style::default().fg(rgb(140, 140, 150)),
        ));
        spans.push(Span::styled(
            format!("{}%", last_pct),
            Style::default().fg(color).bold(),
        ));
    }

    spans.push(Span::styled(" · ", Style::default().fg(rgb(80, 80, 90))));
    spans.push(Span::styled(
        "all ",
        Style::default().fg(rgb(140, 140, 150)),
    ));
    spans.push(Span::styled(
        format!("{}%", lifetime_pct),
        Style::default().fg(color).bold(),
    ));

    spans.push(Span::styled(
        " lifetime",
        Style::default().fg(rgb(100, 100, 110)),
    ));

    Line::from(spans)
}

fn ratio_pct(ratio: f32) -> u8 {
    (ratio * 100.0).round().clamp(0.0, 100.0) as u8
}

fn kv_cache_optimal_color(pct: u8) -> Color {
    match pct {
        0..=24 => rgb(255, 110, 110),
        25..=59 => rgb(255, 200, 100),
        60..=84 => rgb(140, 180, 255),
        _ => rgb(110, 210, 140),
    }
}

fn format_cache_turn_label(turn_number: usize, call_index: u16) -> String {
    if call_index <= 1 {
        format!("{}>", turn_number)
    } else {
        format!("{}.{}>", turn_number, call_index)
    }
}

fn compact_token_count(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f32 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.0}k", tokens as f32 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

/// Render context usage widget
fn render_context_widget(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let Some(info) = &data.context_info else {
        return Vec::new();
    };
    if info.total_chars == 0 && data.observed_context_tokens.is_none() {
        return Vec::new();
    }

    let used_tokens = data
        .observed_context_tokens
        .map(|t| t as usize)
        .unwrap_or_else(|| info.estimated_tokens());
    let limit_tokens = data.context_limit.unwrap_or(DEFAULT_CONTEXT_LIMIT).max(1);
    vec![render_context_usage_line(
        "Context",
        used_tokens,
        limit_tokens,
        inner.width,
    )]
}

/// Render ambient mode status widget
fn render_ambient_widget(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let Some(info) = &data.ambient_info else {
        return Vec::new();
    };
    if !info.show_widget {
        return Vec::new();
    }

    let mut lines: Vec<Line> = Vec::new();
    let dim = rgb(100, 100, 110);
    let label_color = rgb(140, 140, 150);
    let max_w = inner.width.saturating_sub(2) as usize;

    // Status line with icon
    let (icon, status_text, status_color) = match &info.status {
        AmbientStatus::Idle => ("○", "Idle".to_string(), rgb(120, 120, 130)),
        AmbientStatus::Running { detail } => {
            ("●", format!("Running: {}", detail), rgb(100, 200, 100))
        }
        AmbientStatus::Scheduled { .. } => {
            ("◐", "Waiting for next run".to_string(), rgb(140, 180, 255))
        }
        AmbientStatus::Paused { reason } => (
            "⏸",
            format!(
                "Paused: {}",
                truncate_smart(reason, inner.width.saturating_sub(12) as usize)
            ),
            rgb(255, 200, 100),
        ),
        AmbientStatus::Disabled if info.reminder_count > 0 => (
            "⏰",
            "Scheduled tasks active".to_string(),
            rgb(140, 180, 255),
        ),
        AmbientStatus::Disabled => ("○", "Not running".to_string(), dim),
    };

    lines.push(Line::from(vec![
        Span::styled(format!("{} ", icon), Style::default().fg(status_color)),
        Span::styled(
            truncate_smart(&status_text, inner.width.saturating_sub(3) as usize),
            Style::default().fg(rgb(180, 180, 190)),
        ),
    ]));

    // Scheduled tasks count
    let queue_count = if matches!(info.status, AmbientStatus::Disabled) && info.reminder_count > 0 {
        info.reminder_count
    } else {
        info.queue_count
    };
    let queue_preview = if matches!(info.status, AmbientStatus::Disabled) && info.reminder_count > 0
    {
        info.next_reminder_preview.as_ref()
    } else {
        info.next_queue_preview.as_ref()
    };

    if queue_count > 0 {
        let count_text =
            if matches!(info.status, AmbientStatus::Disabled) && info.reminder_count > 0 {
                if queue_count == 1 {
                    "1 scheduled task".to_string()
                } else {
                    format!("{} scheduled tasks", queue_count)
                }
            } else if queue_count == 1 {
                "1 task queued".to_string()
            } else {
                format!("{} tasks queued", queue_count)
            };
        let mut spans = vec![
            Span::styled("  ", Style::default()),
            Span::styled(count_text, Style::default().fg(label_color)),
        ];
        if let Some(preview) = queue_preview {
            spans.push(Span::styled(
                truncate_smart(&format!(" ({})", preview), max_w.saturating_sub(18)),
                Style::default().fg(dim),
            ));
        }
        lines.push(Line::from(spans));
    }

    // Last run
    if let Some(ref ago) = info.last_run_ago {
        let mut spans = vec![
            Span::styled("  ", Style::default()),
            Span::styled(format!("Ran {}", ago), Style::default().fg(label_color)),
        ];
        if let Some(ref summary) = info.last_summary {
            let remaining = max_w.saturating_sub(6 + ago.len());
            if remaining > 5 {
                spans.push(Span::styled(
                    truncate_smart(&format!(" - {}", summary), remaining),
                    Style::default().fg(dim),
                ));
            }
        }
        lines.push(Line::from(spans));
    }

    // Next scheduled run
    let next_due = if matches!(info.status, AmbientStatus::Disabled) && info.reminder_count > 0 {
        info.next_reminder_wake.as_ref()
    } else {
        info.next_wake.as_ref()
    };

    if let Some(next) = next_due {
        let prefix = if matches!(info.status, AmbientStatus::Disabled) && info.reminder_count > 0 {
            "Next scheduled task"
        } else {
            "Next run"
        };
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                format!("{} {}", prefix, next),
                Style::default().fg(label_color),
            ),
        ]));
    }

    // Budget bar
    if let Some(budget) = info.budget_percent {
        let pct = (budget * 100.0).round().clamp(0.0, 100.0) as u8;
        let bar_width = inner.width.saturating_sub(12).clamp(4, 10) as usize;
        let filled = ((budget * bar_width as f32).round() as usize).min(bar_width);
        let empty = bar_width.saturating_sub(filled);

        let bar_color = if pct < 20 {
            rgb(255, 100, 100)
        } else if pct <= 50 {
            rgb(255, 200, 100)
        } else {
            rgb(100, 200, 100)
        };

        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled("█".repeat(filled), Style::default().fg(bar_color)),
            Span::styled("░".repeat(empty), Style::default().fg(rgb(50, 50, 60))),
            Span::styled(format!(" {}%", pct), Style::default().fg(bar_color)),
        ]));
    }

    lines
}

/// Legacy render function - kept for backwards compatibility
/// Renders the first available widget at the given rect
#[deprecated(note = "Use render_all instead")]
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

fn render_page(kind: InfoPageKind, data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    match kind {
        InfoPageKind::CompactOnly => render_sections(data, inner, None),
        InfoPageKind::TodosExpanded => {
            render_sections(data, inner, Some(InfoPageKind::TodosExpanded))
        }
        InfoPageKind::MemoryExpanded => {
            render_sections(data, inner, Some(InfoPageKind::MemoryExpanded))
        }
    }
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

    if let Some(info) = &data.context_info
        && info.total_chars > 0
    {
        lines.extend(render_context_compact(data, inner));
    }

    if !data.todos.is_empty() {
        if matches!(focus, Some(InfoPageKind::TodosExpanded)) {
            lines.extend(render_todos_expanded(data, inner));
        } else {
            lines.extend(render_todos_compact(data, inner));
        }
    }

    // Memory info
    if let Some(info) = &data.memory_info
        && (info.total_count > 0 || info.activity.is_some())
    {
        if matches!(focus, Some(InfoPageKind::MemoryExpanded)) {
            lines.extend(render_memory_expanded(info, inner));
        } else {
            lines.extend(render_memory_compact(info, inner.width));
        }
    }

    // Background tasks info
    if let Some(info) = &data.background_info
        && info.running_count > 0
    {
        lines.extend(render_background_compact(info));
    }

    // Usage info (subscription limits)
    if let Some(info) = &data.usage_info
        && info.available
    {
        lines.extend(render_usage_compact(info, inner.width));
    }

    if let Some(cache) = data.cache_hit_info.as_ref() {
        lines.push(render_kv_cache_summary_line(cache));
    }

    // Git info
    if let Some(info) = &data.git_info
        && info.is_interesting()
    {
        lines.extend(render_git_compact(info, inner.width));
    }

    lines
}

// ---------------------------------------------------------------------------
// Tips widget — rotating helpful tips and keyboard shortcuts
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "info_widget_tests.rs"]
mod tests;

fn format_event_for_expanded(
    event: &MemoryEvent,
    max_width: usize,
) -> (&'static str, String, Color) {
    match &event.kind {
        MemoryEventKind::EmbeddingComplete { latency_ms, hits } => (
            "→",
            truncate_with_ellipsis(&format!("{} hits ({}ms)", hits, latency_ms), max_width),
            rgb(140, 180, 255),
        ),
        MemoryEventKind::SidecarRelevant { memory_preview } => (
            "✓",
            truncate_with_ellipsis(memory_preview, max_width),
            rgb(100, 200, 100),
        ),
        MemoryEventKind::MemorySurfaced { memory_preview } => (
            "★",
            truncate_with_ellipsis(memory_preview, max_width),
            rgb(255, 220, 100),
        ),
        MemoryEventKind::MemoryInjected {
            count,
            prompt_chars,
            items,
            ..
        } => {
            let plural = if *count == 1 { "memory" } else { "memories" };
            let detail = items
                .first()
                .map(|item| format!(" [{}]", item.section))
                .unwrap_or_default();
            (
                "↳",
                truncate_with_ellipsis(
                    &format!("{} {} ({}c){}", count, plural, prompt_chars, detail),
                    max_width,
                ),
                rgb(140, 210, 255),
            )
        }
        MemoryEventKind::MaintenanceComplete { latency_ms } => (
            "🌿",
            truncate_with_ellipsis(&format!("maintained ({}ms)", latency_ms), max_width),
            rgb(120, 220, 180),
        ),
        MemoryEventKind::ExtractionStarted { reason } => (
            "🧠",
            truncate_with_ellipsis(&format!("extracting: {}", reason), max_width),
            rgb(200, 150, 255),
        ),
        MemoryEventKind::ExtractionComplete { count } => (
            "✓",
            truncate_with_ellipsis(&format!("saved {} memories", count), max_width),
            rgb(100, 200, 100),
        ),
        MemoryEventKind::Error { message } => (
            "!",
            truncate_with_ellipsis(message, max_width),
            rgb(255, 100, 100),
        ),
        MemoryEventKind::ToolRemembered {
            content, category, ..
        } => (
            "💾",
            truncate_with_ellipsis(&format!("[{}] {}", category, content), max_width),
            rgb(100, 200, 100),
        ),
        MemoryEventKind::ToolRecalled { query, count } => (
            "🔍",
            truncate_with_ellipsis(&format!("{} found for '{}'", count, query), max_width),
            rgb(140, 180, 255),
        ),
        MemoryEventKind::ToolForgot { id } => (
            "🗑\u{fe0f}",
            truncate_with_ellipsis(id, max_width),
            rgb(255, 170, 100),
        ),
        MemoryEventKind::ToolTagged { id, tags } => (
            "🏷\u{fe0f}",
            truncate_with_ellipsis(&format!("{} +{}", id, tags), max_width),
            rgb(140, 200, 255),
        ),
        MemoryEventKind::ToolLinked { from, to } => (
            "🔗",
            truncate_with_ellipsis(&format!("{} → {}", from, to), max_width),
            rgb(200, 180, 255),
        ),
        MemoryEventKind::ToolListed { count } => {
            ("📋", format!("{} memories", count), rgb(140, 140, 150))
        }
        _ => ("·", String::new(), rgb(100, 100, 110)),
    }
}

fn render_context_compact(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let Some(info) = &data.context_info else {
        return Vec::new();
    };
    if info.total_chars == 0 && data.observed_context_tokens.is_none() {
        return Vec::new();
    }

    let used_tokens = data
        .observed_context_tokens
        .map(|t| t as usize)
        .unwrap_or_else(|| info.estimated_tokens());
    let limit_tokens = data.context_limit.unwrap_or(DEFAULT_CONTEXT_LIMIT).max(1);
    let label = if data.is_compacting {
        "Context📦"
    } else {
        "Context"
    };

    vec![render_context_usage_line(
        label,
        used_tokens,
        limit_tokens,
        inner.width,
    )]
}
