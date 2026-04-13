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
#[path = "info_widget_model.rs"]
mod model;
#[path = "info_widget_text.rs"]
mod text;
#[path = "info_widget_tips.rs"]
mod tips;
#[path = "info_widget_usage.rs"]
mod usage_render;
use super::info_widget_overview::{InfoPageKind, MAX_TODO_LINES, compute_page_layout};
use super::workspace_map::VisibleWorkspaceRow;
use crate::ambient::AmbientStatus;
use crate::prompt::ContextInfo;
use crate::protocol::SwarmMemberStatus;
use crate::provider::DEFAULT_CONTEXT_LIMIT;
use crate::todo::TodoItem;
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
use model::{render_model_info, render_model_widget, shorten_model_name};
use text::{truncate_smart, truncate_with_ellipsis};
pub(crate) use tips::occasional_status_tip;
use tips::{render_tips_widget, tips_widget_height};
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
            WidgetKind::MemoryActivity => 6,
            WidgetKind::ModelInfo => 7,
            WidgetKind::BackgroundTasks => 8,
            WidgetKind::GitStatus => 9,
            WidgetKind::SwarmStatus => 10, // Session list - lower priority
            WidgetKind::AmbientMode => 11, // Scheduled agent - lower priority
            WidgetKind::Tips => 12,        // Did you know - lowest
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

/// Represents current memory system activity
#[derive(Debug, Clone)]
pub struct MemoryActivity {
    /// Current state of the memory system
    pub state: MemoryState,
    /// When the current state was entered (for elapsed time display + staleness detection)
    pub state_since: Instant,
    /// Pipeline progress for the per-turn search/verify/inject/maintain flow
    pub pipeline: Option<PipelineState>,
    /// Recent events (most recent first)
    pub recent_events: Vec<MemoryEvent>,
}

impl MemoryActivity {
    pub fn is_processing(&self) -> bool {
        !matches!(self.state, MemoryState::Idle)
            || self
                .pipeline
                .as_ref()
                .map(PipelineState::has_running_step)
                .unwrap_or(false)
    }
}

/// Status of a single pipeline step
#[derive(Debug, Clone, PartialEq)]
pub enum StepStatus {
    Pending,
    Running,
    Done,
    Error,
    Skipped,
}

/// Result data for a completed pipeline step
#[derive(Debug, Clone)]
pub struct StepResult {
    pub summary: String,
    pub latency_ms: u64,
}

/// Tracks the 4-step per-turn memory pipeline: search -> verify -> inject -> maintain
#[derive(Debug, Clone)]
pub struct PipelineState {
    pub search: StepStatus,
    pub search_result: Option<StepResult>,
    pub verify: StepStatus,
    pub verify_result: Option<StepResult>,
    pub verify_progress: Option<(usize, usize)>,
    pub inject: StepStatus,
    pub inject_result: Option<StepResult>,
    pub maintain: StepStatus,
    pub maintain_result: Option<StepResult>,
    pub started_at: Instant,
}

impl PipelineState {
    pub fn new() -> Self {
        Self {
            search: StepStatus::Pending,
            search_result: None,
            verify: StepStatus::Pending,
            verify_result: None,
            verify_progress: None,
            inject: StepStatus::Pending,
            inject_result: None,
            maintain: StepStatus::Pending,
            maintain_result: None,
            started_at: Instant::now(),
        }
    }

    pub fn is_complete(&self) -> bool {
        matches!(
            (&self.search, &self.verify, &self.inject, &self.maintain),
            (
                StepStatus::Done | StepStatus::Error | StepStatus::Skipped,
                StepStatus::Done | StepStatus::Error | StepStatus::Skipped,
                StepStatus::Done | StepStatus::Error | StepStatus::Skipped,
                StepStatus::Done | StepStatus::Error | StepStatus::Skipped,
            )
        )
    }

    pub fn has_running_step(&self) -> bool {
        matches!(self.search, StepStatus::Running)
            || matches!(self.verify, StepStatus::Running)
            || matches!(self.inject, StepStatus::Running)
            || matches!(self.maintain, StepStatus::Running)
    }
}

impl Default for PipelineState {
    fn default() -> Self {
        Self::new()
    }
}

/// State of the memory sidecar
#[derive(Debug, Clone, PartialEq, Default)]
pub enum MemoryState {
    /// Idle, no activity
    #[default]
    Idle,
    /// Running embedding search
    Embedding,
    /// Sidecar checking relevance
    SidecarChecking { count: usize },
    /// Found relevant memories
    FoundRelevant { count: usize },
    /// Extracting memories from conversation
    Extracting { reason: String },
    /// Background maintenance/gardening of the memory graph
    Maintaining { phase: String },
    /// Agent is actively using a memory tool
    ToolAction { action: String, detail: String },
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
pub struct InjectedMemoryItem {
    pub section: String,
    pub content: String,
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
    /// Memory payload was injected into model context
    MemoryInjected {
        count: usize,
        prompt_chars: usize,
        age_ms: u64,
        preview: String,
        items: Vec<InjectedMemoryItem>,
    },
    /// Background maintenance started
    MaintenanceStarted { verified: usize, rejected: usize },
    /// Background maintenance discovered/strengthened links
    MaintenanceLinked { links: usize },
    /// Background maintenance adjusted confidence
    MaintenanceConfidence { boosted: usize, decayed: usize },
    /// Background maintenance refined clusters
    MaintenanceCluster { clusters: usize, members: usize },
    /// Background maintenance inferred/applied a shared tag
    MaintenanceTagInferred { tag: String, applied: usize },
    /// Background maintenance detected a gap
    MaintenanceGap { candidates: usize },
    /// Background maintenance completed
    MaintenanceComplete { latency_ms: u64 },
    /// Extraction started
    ExtractionStarted { reason: String },
    /// Extraction completed
    ExtractionComplete { count: usize },
    /// Error occurred
    Error { message: String },
    /// Agent stored a memory via tool
    ToolRemembered {
        content: String,
        scope: String,
        category: String,
    },
    /// Agent recalled/searched memories via tool
    ToolRecalled { query: String, count: usize },
    /// Agent forgot a memory via tool
    ToolForgot { id: String },
    /// Agent tagged a memory via tool
    ToolTagged { id: String, tags: String },
    /// Agent linked memories via tool
    ToolLinked { from: String, to: String },
    /// Agent listed memories via tool
    ToolListed { count: usize },
}

/// Info about a mermaid diagram for display in the info widget
#[derive(Debug, Clone)]
pub struct DiagramInfo {
    /// Hash for mermaid cache lookup
    pub hash: u64,
    /// Original PNG width
    pub width: u32,
    /// Original PNG height
    pub height: u32,
    /// Optional label/title
    pub label: Option<String>,
}

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
                    .map(|b| b.running_count > 0 || b.memory_agent_active)
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
                .map(|b| b.running_count > 0 || b.memory_agent_active)
                .unwrap_or(false),
            WidgetKind::AmbientMode => false,
            WidgetKind::UsageLimits => self
                .usage_info
                .as_ref()
                .map(|u| u.available)
                .unwrap_or(false),
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
                .map(|b| b.running_count == 0 && !b.memory_agent_active)
                .unwrap_or(true)
            {
                return 0;
            }
            1 // Single line
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
#[allow(dead_code)]
const MEMORY_TEXT_SUBGRAPH_MAX_NODES: usize = 8;
#[cfg(test)]
#[allow(dead_code)]
const MEMORY_TEXT_SUBGRAPH_MAX_EDGES: usize = 10;
#[cfg(test)]
#[derive(Debug, Clone)]
struct MemorySubgraph {
    nodes: Vec<GraphNode>,
    #[allow(dead_code)]
    edges: Vec<GraphEdge>,
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
        edges: sub_edges,
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
        WidgetKind::ModelInfo => render_model_widget(data, inner),
        WidgetKind::Tips => render_tips_widget(inner),
        WidgetKind::GitStatus => render_git_widget(data, inner),
    }
}

/// Render todos widget content
fn render_todos_widget(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    if data.todos.is_empty() {
        return Vec::new();
    }

    let mut lines: Vec<Line> = Vec::new();
    let total = data.todos.len();
    let completed: usize = data
        .todos
        .iter()
        .filter(|t| t.status == "completed")
        .count();
    let _in_progress: usize = data
        .todos
        .iter()
        .filter(|t| t.status == "in_progress")
        .count();

    // Header with progress
    lines.push(Line::from(vec![
        Span::styled("Todos ", Style::default().fg(rgb(180, 180, 190)).bold()),
        Span::styled(
            format!("{}/{}", completed, total),
            Style::default().fg(rgb(140, 140, 150)),
        ),
    ]));

    // Mini progress bar
    let bar_width = inner.width.saturating_sub(2).min(20) as usize;
    if bar_width >= 4 && total > 0 {
        let filled = ((completed as f64 / total as f64) * bar_width as f64).round() as usize;
        let empty = bar_width.saturating_sub(filled);
        lines.push(Line::from(vec![
            Span::styled("[", Style::default().fg(rgb(90, 90, 100))),
            Span::styled("█".repeat(filled), Style::default().fg(rgb(100, 180, 100))),
            Span::styled("░".repeat(empty), Style::default().fg(rgb(50, 50, 60))),
            Span::styled("]", Style::default().fg(rgb(90, 90, 100))),
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
        let is_blocked = !todo.blocked_by.is_empty();
        let (icon, status_color) = if is_blocked && todo.status != "completed" {
            ("⊳", rgb(180, 140, 100))
        } else {
            match todo.status.as_str() {
                "completed" => ("✓", rgb(100, 180, 100)),
                "in_progress" => ("▶", rgb(255, 200, 100)),
                "cancelled" => ("✗", rgb(120, 80, 80)),
                _ => ("○", rgb(120, 120, 130)),
            }
        };

        let suffix = if is_blocked && todo.status != "completed" {
            " (blocked)"
        } else {
            ""
        };
        let max_len = inner.width.saturating_sub(3 + suffix.len() as u16) as usize;
        let content = truncate_smart(&todo.content, max_len);

        let text_color = if todo.status == "completed" {
            rgb(100, 100, 110)
        } else if is_blocked {
            rgb(120, 120, 130)
        } else if todo.status == "in_progress" {
            rgb(200, 200, 210)
        } else {
            rgb(160, 160, 170)
        };

        let mut spans = vec![
            Span::styled(format!("{} ", icon), Style::default().fg(status_color)),
            Span::styled(content, Style::default().fg(text_color)),
        ];
        if !suffix.is_empty() {
            spans.push(Span::styled(
                suffix.to_string(),
                Style::default().fg(rgb(100, 100, 110)),
            ));
        }
        lines.push(Line::from(spans));
    }

    // Show count of remaining items
    let shown = available_lines.min(5).min(sorted_todos.len());
    if data.todos.len() > shown {
        let remaining = data.todos.len() - shown;
        lines.push(Line::from(vec![Span::styled(
            format!("  +{} more", remaining),
            Style::default().fg(rgb(100, 100, 110)),
        )]));
    }

    lines
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

/// Render memory activity widget
fn render_memory_widget(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let Some(info) = &data.memory_info else {
        return Vec::new();
    };
    if inner.width == 0 || inner.height == 0 {
        return Vec::new();
    }
    if info.total_count == 0 && info.activity.is_none() && info.sidecar_model.is_none() {
        return Vec::new();
    }

    let mut lines: Vec<Line> = Vec::new();
    let max_width = inner.width as usize;
    let activity = info.activity.as_ref();

    lines.push(render_memory_header_line(info, activity, max_width));

    if lines.len() < inner.height as usize
        && let Some(count_line) = render_memory_count_line(info, max_width)
    {
        lines.push(count_line);
    }

    if let Some(activity) = activity {
        if lines.len() < inner.height as usize {
            lines.push(render_memory_status_line(activity, max_width));
        }

        if lines.len() < inner.height as usize
            && let Some(model_line) = render_memory_model_line(info, max_width)
        {
            lines.push(model_line);
        }

        if memory_should_render_pipeline(activity) {
            for line in render_memory_pipeline_display_lines(activity, max_width) {
                if lines.len() >= inner.height as usize {
                    break;
                }
                lines.push(line);
            }
        }

        if lines.len() < inner.height as usize
            && let Some(trace_line) = render_memory_last_trace_line(activity, max_width)
        {
            lines.push(trace_line);
        }
    } else if lines.len() < inner.height as usize
        && let Some(model_line) = render_memory_model_line(info, max_width)
    {
        lines.push(model_line);
    }

    lines.truncate(inner.height as usize);
    lines
}

fn render_memory_header_line(
    _info: &MemoryInfo,
    activity: Option<&MemoryActivity>,
    max_width: usize,
) -> Line<'static> {
    let title = "Memory".to_string();
    let (badge, badge_color) = memory_status_badge(activity);
    let badge_text = format!(" {} ", badge);
    let title_width = UnicodeWidthStr::width(title.as_str());
    let badge_width = UnicodeWidthStr::width(badge_text.as_str());
    let available_title = if max_width > badge_width + 2 {
        max_width.saturating_sub(badge_width + 2)
    } else {
        max_width
    };

    let mut spans = vec![
        Span::styled("🧠 ", Style::default().fg(rgb(200, 150, 255))),
        Span::styled(
            truncate_with_ellipsis(&title, available_title.max(6)),
            Style::default().fg(rgb(210, 210, 220)).bold(),
        ),
    ];

    if max_width >= title_width + badge_width + 2 {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            badge_text,
            Style::default().fg(badge_color).bg(rgb(32, 32, 40)).bold(),
        ));
    }

    Line::from(spans)
}

fn render_memory_count_line(info: &MemoryInfo, max_width: usize) -> Option<Line<'static>> {
    if info.total_count == 0 {
        return None;
    }

    Some(Line::from(vec![Span::styled(
        truncate_with_ellipsis(&memory_count_label(info.total_count), max_width.max(8)),
        Style::default().fg(rgb(160, 160, 170)).bold(),
    )]))
}

fn memory_count_label(total_count: usize) -> String {
    if total_count == 1 {
        "1 memory".to_string()
    } else {
        format!("{total_count} memories")
    }
}

fn memory_recent_done(activity: &MemoryActivity) -> bool {
    matches!(activity.state, MemoryState::Idle)
        && activity
            .pipeline
            .as_ref()
            .map(PipelineState::is_complete)
            .unwrap_or(false)
        && activity.state_since.elapsed() <= Duration::from_secs(5)
}

fn memory_should_render_pipeline(activity: &MemoryActivity) -> bool {
    activity.pipeline.is_some()
        && (!matches!(activity.state, MemoryState::Idle) || memory_recent_done(activity))
}

fn memory_compact_summary(info: &MemoryInfo) -> String {
    if let Some(activity) = info.activity.as_ref() {
        if activity.is_processing() {
            return memory_active_summary(&activity.state)
                .or_else(|| {
                    activity
                        .pipeline
                        .as_ref()
                        .map(memory_pipeline_progress_summary)
                })
                .or_else(|| memory_last_trace_summary(activity))
                .unwrap_or_else(|| "working".to_string());
        }

        if memory_recent_done(activity) {
            return "done".to_string();
        }

        return "idle".to_string();
    }

    if info.total_count > 0 {
        "idle".to_string()
    } else {
        info.sidecar_model
            .as_deref()
            .map(compact_memory_model_label)
            .unwrap_or_else(|| "idle".to_string())
    }
}

fn memory_status_badge(activity: Option<&MemoryActivity>) -> (String, Color) {
    let Some(activity) = activity else {
        return ("IDLE".to_string(), rgb(120, 120, 130));
    };

    if let Some(pipeline) = &activity.pipeline {
        let live_step = [
            ("SEARCH", &pipeline.search, rgb(140, 180, 255)),
            ("VERIFY", &pipeline.verify, rgb(255, 200, 100)),
            ("INJECT", &pipeline.inject, rgb(200, 150, 255)),
            ("UPDATE", &pipeline.maintain, rgb(120, 220, 180)),
        ]
        .into_iter()
        .find(|(_, status, _)| matches!(status, StepStatus::Running | StepStatus::Error));

        if let Some((label, status, color)) = live_step {
            return (
                if matches!(status, StepStatus::Error) {
                    "FAILED".to_string()
                } else {
                    label.to_string()
                },
                if matches!(status, StepStatus::Error) {
                    rgb(255, 100, 100)
                } else {
                    color
                },
            );
        }

        if memory_recent_done(activity) {
            return ("DONE".to_string(), rgb(100, 200, 100));
        }
    }

    match &activity.state {
        MemoryState::Idle => ("IDLE".to_string(), rgb(120, 120, 130)),
        MemoryState::Embedding => ("SEARCH".to_string(), rgb(140, 180, 255)),
        MemoryState::SidecarChecking { .. } => ("VERIFY".to_string(), rgb(255, 200, 100)),
        MemoryState::FoundRelevant { .. } => ("READY".to_string(), rgb(100, 200, 100)),
        MemoryState::Extracting { .. } => ("SAVE".to_string(), rgb(200, 150, 255)),
        MemoryState::Maintaining { .. } => ("UPDATE".to_string(), rgb(120, 220, 180)),
        MemoryState::ToolAction { .. } => ("TOOL".to_string(), rgb(140, 200, 255)),
    }
}

fn render_memory_model_line(info: &MemoryInfo, max_width: usize) -> Option<Line<'static>> {
    let model = info.sidecar_model.as_deref()?.trim();
    if model.is_empty() {
        return None;
    }

    let available = max_width.saturating_sub(7);
    Some(Line::from(vec![
        Span::styled("Model: ", Style::default().fg(rgb(120, 120, 130))),
        Span::styled(
            truncate_with_ellipsis(model, available),
            Style::default().fg(rgb(140, 200, 255)).bold(),
        ),
    ]))
}

fn render_memory_status_line(activity: &MemoryActivity, max_width: usize) -> Line<'static> {
    let (_badge, badge_color) = memory_status_badge(Some(activity));
    let summary = memory_state_detail(&activity.state)
        .or_else(|| {
            if memory_should_render_pipeline(activity) {
                activity
                    .pipeline
                    .as_ref()
                    .map(memory_pipeline_progress_summary)
            } else {
                None
            }
        })
        .or_else(|| memory_last_trace_summary(activity))
        .unwrap_or_else(|| "idle".to_string());
    let prefix = if activity.is_processing() {
        "Now: "
    } else {
        "Last: "
    };
    let age = format_age(activity.state_since.elapsed());
    let prefix_width = UnicodeWidthStr::width(prefix);
    let age_width = UnicodeWidthStr::width(age.as_str()) + 3;
    let summary_width = UnicodeWidthStr::width(summary.as_str());
    let show_age = prefix_width + summary_width + age_width <= max_width;
    let available = if show_age {
        max_width.saturating_sub(prefix_width + age_width)
    } else {
        max_width.saturating_sub(prefix_width)
    };

    let mut spans = vec![
        Span::styled(prefix, Style::default().fg(rgb(120, 120, 130))),
        Span::styled(
            truncate_smart(&summary, available),
            Style::default().fg(badge_color).bold(),
        ),
    ];

    if show_age {
        spans.push(Span::styled(" · ", Style::default().fg(rgb(90, 90, 100))));
        spans.push(Span::styled(age, Style::default().fg(rgb(120, 120, 130))));
    }

    Line::from(spans)
}

fn render_memory_pipeline_lines(pipeline: &PipelineState, max_width: usize) -> Vec<Line<'static>> {
    vec![
        render_memory_step_line(
            "╭ ",
            "Find matches",
            &pipeline.search,
            memory_step_detail(
                "search",
                &pipeline.search,
                pipeline.search_result.as_ref(),
                None,
            ),
            max_width,
        ),
        render_memory_step_line(
            "├ ",
            "Check relevance",
            &pipeline.verify,
            memory_step_detail(
                "verify",
                &pipeline.verify,
                pipeline.verify_result.as_ref(),
                pipeline.verify_progress,
            ),
            max_width,
        ),
        render_memory_step_line(
            "├ ",
            "Inject context",
            &pipeline.inject,
            memory_step_detail(
                "inject",
                &pipeline.inject,
                pipeline.inject_result.as_ref(),
                None,
            ),
            max_width,
        ),
        render_memory_step_line(
            "╰ ",
            "Update memory",
            &pipeline.maintain,
            memory_step_detail(
                "maintain",
                &pipeline.maintain,
                pipeline.maintain_result.as_ref(),
                None,
            ),
            max_width,
        ),
    ]
}

fn render_memory_pipeline_display_lines(
    activity: &MemoryActivity,
    max_width: usize,
) -> Vec<Line<'static>> {
    if let Some(pipeline) = &activity.pipeline {
        return render_memory_pipeline_lines(pipeline, max_width);
    }

    let (search, verify, inject, maintain, verify_progress) =
        fallback_pipeline_statuses(&activity.state);

    vec![
        render_memory_step_line(
            "╭ ",
            "Find matches",
            &search,
            memory_step_detail("search", &search, None, None),
            max_width,
        ),
        render_memory_step_line(
            "├ ",
            "Check relevance",
            &verify,
            memory_step_detail("verify", &verify, None, verify_progress),
            max_width,
        ),
        render_memory_step_line(
            "├ ",
            "Inject context",
            &inject,
            memory_step_detail("inject", &inject, None, None),
            max_width,
        ),
        render_memory_step_line(
            "╰ ",
            "Update memory",
            &maintain,
            memory_step_detail("maintain", &maintain, None, None),
            max_width,
        ),
    ]
}

fn fallback_pipeline_statuses(
    state: &MemoryState,
) -> (
    StepStatus,
    StepStatus,
    StepStatus,
    StepStatus,
    Option<(usize, usize)>,
) {
    match state {
        MemoryState::Idle => (
            StepStatus::Pending,
            StepStatus::Pending,
            StepStatus::Pending,
            StepStatus::Pending,
            None,
        ),
        MemoryState::Embedding => (
            StepStatus::Running,
            StepStatus::Pending,
            StepStatus::Pending,
            StepStatus::Pending,
            None,
        ),
        MemoryState::SidecarChecking { count } => (
            StepStatus::Done,
            StepStatus::Running,
            StepStatus::Pending,
            StepStatus::Pending,
            Some((0, *count)),
        ),
        MemoryState::FoundRelevant { .. } => (
            StepStatus::Done,
            StepStatus::Done,
            StepStatus::Running,
            StepStatus::Pending,
            None,
        ),
        MemoryState::Extracting { .. } | MemoryState::Maintaining { .. } => (
            StepStatus::Done,
            StepStatus::Done,
            StepStatus::Done,
            StepStatus::Running,
            None,
        ),
        MemoryState::ToolAction { .. } => (
            StepStatus::Pending,
            StepStatus::Pending,
            StepStatus::Pending,
            StepStatus::Running,
            None,
        ),
    }
}

fn render_memory_last_trace_line(
    activity: &MemoryActivity,
    max_width: usize,
) -> Option<Line<'static>> {
    let event = activity
        .recent_events
        .iter()
        .find(|event| is_traceworthy_memory_event(event))?;
    let (icon, text, color) = format_event_for_expanded(event, max_width.saturating_sub(8));
    if text.is_empty() {
        return None;
    }

    Some(Line::from(vec![
        Span::styled("Trace: ", Style::default().fg(rgb(120, 120, 130))),
        Span::styled(format!("{} ", icon), Style::default().fg(color)),
        Span::styled(
            truncate_with_ellipsis(&text, max_width.saturating_sub(7)),
            Style::default().fg(rgb(160, 160, 170)),
        ),
    ]))
}

fn render_memory_step_line(
    prefix: &'static str,
    label: &'static str,
    status: &StepStatus,
    detail: Option<String>,
    max_width: usize,
) -> Line<'static> {
    let (marker, marker_color, label_color, detail_color, fallback) = match status {
        StepStatus::Pending => (
            "·",
            rgb(100, 100, 110),
            rgb(140, 140, 150),
            rgb(120, 120, 130),
            Some("waiting"),
        ),
        StepStatus::Running => (
            current_memory_spinner_frame(),
            rgb(255, 200, 100),
            rgb(220, 220, 230),
            rgb(255, 200, 100),
            Some("running"),
        ),
        StepStatus::Done => (
            "✓",
            rgb(100, 200, 100),
            rgb(180, 180, 190),
            rgb(160, 160, 170),
            Some("done"),
        ),
        StepStatus::Error => (
            "!",
            rgb(255, 100, 100),
            rgb(220, 180, 180),
            rgb(255, 140, 140),
            Some("failed"),
        ),
        StepStatus::Skipped => (
            "-",
            rgb(100, 100, 110),
            rgb(120, 120, 130),
            rgb(120, 120, 130),
            Some("skipped"),
        ),
    };

    let detail = detail
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| fallback.unwrap_or("").to_string());
    let available = max_width
        .saturating_sub(UnicodeWidthStr::width(prefix))
        .saturating_sub(UnicodeWidthStr::width(marker))
        .saturating_sub(label.chars().count() + 4);
    let rail_color = if matches!(status, StepStatus::Running) {
        rgb(255, 200, 100)
    } else {
        rgb(80, 80, 92)
    };

    Line::from(vec![
        Span::styled(prefix.to_string(), Style::default().fg(rail_color)),
        Span::styled(format!("{} ", marker), Style::default().fg(marker_color)),
        Span::styled(
            label.to_string(),
            if matches!(status, StepStatus::Running | StepStatus::Done) {
                Style::default().fg(label_color).bold()
            } else {
                Style::default().fg(label_color)
            },
        ),
        Span::styled("  ", Style::default().fg(rgb(100, 100, 110))),
        Span::styled(
            truncate_smart(&detail, available),
            Style::default().fg(detail_color),
        ),
    ])
}

fn current_memory_spinner_frame() -> &'static str {
    if !crate::perf::tui_policy().enable_decorative_animations {
        return "•";
    }

    const FRAMES: [&str; 4] = ["/", "-", "\\", "|"];
    let frame = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_millis() / 120) as usize)
        .unwrap_or(0);
    FRAMES[frame % FRAMES.len()]
}

fn memory_step_detail(
    step: &str,
    status: &StepStatus,
    result: Option<&StepResult>,
    progress: Option<(usize, usize)>,
) -> Option<String> {
    match status {
        StepStatus::Running => progress.map(|(done, total)| format!("{done}/{total}")),
        StepStatus::Done | StepStatus::Error => result.and_then(|res| {
            let summary = res.summary.trim();
            if summary.is_empty() {
                None
            } else {
                Some(match step {
                    "search" if summary.ends_with("hits") => summary.replace("hits", "found"),
                    _ => summary.to_string(),
                })
            }
        }),
        StepStatus::Pending | StepStatus::Skipped => None,
    }
}

fn memory_pipeline_progress_summary(pipeline: &PipelineState) -> String {
    let completed = [
        &pipeline.search,
        &pipeline.verify,
        &pipeline.inject,
        &pipeline.maintain,
    ]
    .into_iter()
    .filter(|status| matches!(status, StepStatus::Done))
    .count();

    let active = [
        ("search", &pipeline.search, None),
        ("verify", &pipeline.verify, pipeline.verify_progress),
        ("inject", &pipeline.inject, None),
        ("update", &pipeline.maintain, None),
    ]
    .into_iter()
    .find_map(|(name, status, progress)| match status {
        StepStatus::Running => Some(if let Some((done, total)) = progress {
            format!("{} {}/{}", name, done, total)
        } else {
            name.to_string()
        }),
        StepStatus::Error => Some(format!("{} failed", name)),
        _ => None,
    });

    if let Some(active) = active {
        format!("{}/4 done · {}", completed, active)
    } else {
        format!("{}/4 done", completed)
    }
}

fn swarm_member_label(member: &SwarmMemberStatus) -> String {
    member
        .friendly_name
        .clone()
        .unwrap_or_else(|| member.session_id.chars().take(8).collect())
}

fn swarm_status_style(status: &str) -> (Color, &'static str) {
    match status {
        "spawned" => (rgb(140, 140, 150), "○"),
        "ready" => (rgb(120, 180, 120), "●"),
        "running" => (rgb(255, 200, 100), "▶"),
        "blocked" => (rgb(255, 170, 80), "⏸"),
        "failed" => (rgb(255, 100, 100), "✗"),
        "completed" => (rgb(100, 200, 100), "✓"),
        "stopped" => (rgb(140, 140, 150), "■"),
        "crashed" => (rgb(255, 80, 80), "!"),
        _ => (rgb(140, 140, 150), "·"),
    }
}

fn swarm_role_prefix(member: &SwarmMemberStatus) -> &'static str {
    match member.role.as_deref() {
        Some("coordinator") => "★ ",
        Some("worktree_manager") => "◆ ",
        _ => "  ",
    }
}

fn swarm_member_line(member: &SwarmMemberStatus, max_width: usize) -> Line<'static> {
    let name = swarm_member_label(member);
    let mut detail = member.detail.clone().unwrap_or_default();
    if !detail.is_empty() {
        detail = format!(" — {}", detail);
    }
    let role_prefix = swarm_role_prefix(member);
    let line_text = truncate_smart(&format!("{} {}{}", name, member.status, detail), max_width);
    let (color, icon) = swarm_status_style(&member.status);
    Line::from(vec![
        Span::styled(
            role_prefix.to_string(),
            Style::default().fg(rgb(255, 200, 100)),
        ),
        Span::styled(format!("{} ", icon), Style::default().fg(color)),
        Span::styled(line_text, Style::default().fg(rgb(140, 140, 150))),
    ])
}

/// Render swarm status widget
fn render_swarm_widget(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let Some(info) = &data.swarm_info else {
        return Vec::new();
    };

    let mut lines: Vec<Line> = Vec::new();

    // Stats line
    let mut stats_parts: Vec<Span> =
        vec![Span::styled("🐝 ", Style::default().fg(rgb(255, 200, 100)))];

    if info.session_count > 0 {
        stats_parts.push(Span::styled(
            format!("{}s", info.session_count),
            Style::default().fg(rgb(160, 160, 170)),
        ));
    }
    if let Some(clients) = info.client_count {
        if info.session_count > 0 {
            stats_parts.push(Span::styled(" · ", Style::default().fg(rgb(100, 100, 110))));
        }
        stats_parts.push(Span::styled(
            format!("{}c", clients),
            Style::default().fg(rgb(160, 160, 170)),
        ));
    }
    lines.push(Line::from(stats_parts));

    // Active subagent status (only when we don't have member status lines)
    if info.members.is_empty()
        && let Some(status) = &info.subagent_status
    {
        lines.push(Line::from(vec![
            Span::styled("▶ ", Style::default().fg(rgb(255, 200, 100))),
            Span::styled(
                truncate_smart(status, inner.width.saturating_sub(4) as usize),
                Style::default().fg(rgb(200, 200, 210)),
            ),
        ]));
    }

    // Session names or member status lines (limit based on height)
    let max_names = inner.height.saturating_sub(lines.len() as u16) as usize;
    let max_name_len = inner.width.saturating_sub(6) as usize;
    if !info.members.is_empty() {
        for member in info.members.iter().take(max_names.min(3)) {
            lines.push(swarm_member_line(member, max_name_len));
        }
    } else {
        for name in info.session_names.iter().take(max_names.min(3)) {
            lines.push(Line::from(vec![
                Span::styled("  · ", Style::default().fg(rgb(100, 100, 110))),
                Span::styled(
                    truncate_smart(name, max_name_len),
                    Style::default().fg(rgb(140, 140, 150)),
                ),
            ]));
        }
    }

    lines
}

/// Render background tasks widget
fn render_background_widget(data: &InfoWidgetData, _inner: Rect) -> Vec<Line<'static>> {
    let Some(info) = &data.background_info else {
        return Vec::new();
    };
    if info.running_count == 0 && !info.memory_agent_active {
        return Vec::new();
    }

    let mut spans: Vec<Span> = vec![Span::styled("⏳ ", Style::default().fg(rgb(180, 140, 255)))];

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
        Style::default().fg(rgb(160, 160, 170)),
    ));

    vec![Line::from(spans)]
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
        && (info.running_count > 0 || info.memory_agent_active)
    {
        lines.extend(render_background_compact(info));
    }

    // Usage info (subscription limits)
    if let Some(info) = &data.usage_info
        && info.available
    {
        lines.extend(render_usage_compact(info, inner.width));
    }

    // Git info
    if let Some(info) = &data.git_info
        && info.is_interesting()
    {
        lines.extend(render_git_compact(info, inner.width));
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
    let _in_progress: usize = data
        .todos
        .iter()
        .filter(|t| t.status == "in_progress")
        .count();

    // Header with progress
    lines.push(Line::from(vec![
        Span::styled("Todos ", Style::default().fg(rgb(180, 180, 190)).bold()),
        Span::styled(
            format!("{}/{}", completed, total),
            Style::default().fg(rgb(140, 140, 150)),
        ),
    ]));

    // Mini progress bar
    let bar_width = inner.width.saturating_sub(2).min(20) as usize;
    if bar_width >= 4 && total > 0 {
        let filled = ((completed as f64 / total as f64) * bar_width as f64).round() as usize;
        let empty = bar_width.saturating_sub(filled);
        lines.push(Line::from(vec![
            Span::styled("[", Style::default().fg(rgb(90, 90, 100))),
            Span::styled("█".repeat(filled), Style::default().fg(rgb(100, 180, 100))),
            Span::styled("░".repeat(empty), Style::default().fg(rgb(50, 50, 60))),
            Span::styled("]", Style::default().fg(rgb(90, 90, 100))),
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
        let is_blocked = !todo.blocked_by.is_empty();
        let (icon, status_color) = if is_blocked && todo.status != "completed" {
            ("⊳", rgb(180, 140, 100))
        } else {
            match todo.status.as_str() {
                "completed" => ("✓", rgb(100, 180, 100)),
                "in_progress" => ("▶", rgb(255, 200, 100)),
                "cancelled" => ("✗", rgb(120, 80, 80)),
                _ => ("○", rgb(120, 120, 130)),
            }
        };

        // Priority indicator
        let priority_marker = match todo.priority.as_str() {
            "high" => ("!", rgb(255, 120, 100)),
            "medium" => ("", rgb(200, 180, 100)),
            _ => ("", rgb(120, 120, 130)),
        };

        let suffix = if is_blocked && todo.status != "completed" {
            " (blocked)"
        } else {
            ""
        };
        let max_len = inner.width.saturating_sub(4 + suffix.len() as u16) as usize;
        let content = truncate_smart(&todo.content, max_len);

        // Dim completed and blocked items
        let text_color = if todo.status == "completed" {
            rgb(100, 100, 110)
        } else if is_blocked {
            rgb(120, 120, 130)
        } else if todo.status == "in_progress" {
            rgb(200, 200, 210)
        } else {
            rgb(160, 160, 170)
        };

        let mut spans = vec![Span::styled(
            format!("{} ", icon),
            Style::default().fg(status_color),
        )];

        if !priority_marker.0.is_empty() {
            spans.push(Span::styled(
                priority_marker.0,
                Style::default().fg(priority_marker.1),
            ));
        }

        spans.push(Span::styled(content, Style::default().fg(text_color)));

        if !suffix.is_empty() {
            spans.push(Span::styled(
                suffix.to_string(),
                Style::default().fg(rgb(100, 100, 110)),
            ));
        }

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
            Style::default().fg(rgb(100, 100, 110)),
        )]));
    }

    lines
}

// ---------------------------------------------------------------------------
// Tips widget — rotating helpful tips and keyboard shortcuts
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "info_widget_tests.rs"]
mod tests;

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
            Style::default().fg(rgb(180, 180, 190)).bold(),
        )]),
        Line::from(vec![
            Span::styled(
                format!("{} total", total),
                Style::default().fg(rgb(160, 160, 170)),
            ),
            Span::styled(" · ", Style::default().fg(rgb(100, 100, 110))),
            Span::styled(
                format!("{} active", in_progress),
                Style::default().fg(rgb(255, 200, 100)),
            ),
            Span::styled(" · ", Style::default().fg(rgb(100, 100, 110))),
            Span::styled(
                format!("{} open", pending),
                Style::default().fg(rgb(140, 140, 150)),
            ),
        ]),
    ]
}

fn render_memory_compact(info: &MemoryInfo, inner_width: u16) -> Vec<Line<'static>> {
    let max_width = inner_width.saturating_sub(2) as usize;
    let title = if info.total_count > 0 {
        memory_count_label(info.total_count)
    } else {
        "Memory".to_string()
    };
    let summary = memory_compact_summary(info);

    let title_width = UnicodeWidthStr::width(title.as_str());
    let summary_width = max_width.saturating_sub(title_width + 5);
    let accent = if let Some(activity) = info.activity.as_ref() {
        memory_status_badge(Some(activity)).1
    } else if info.total_count > 0 {
        rgb(160, 160, 170)
    } else {
        rgb(140, 200, 255)
    };

    vec![Line::from(vec![
        Span::styled("🧠 ", Style::default().fg(rgb(200, 150, 255))),
        Span::styled(title, Style::default().fg(rgb(180, 180, 190)).bold()),
        Span::styled(" · ", Style::default().fg(rgb(100, 100, 110))),
        Span::styled(
            truncate_with_ellipsis(&summary, summary_width.max(8)),
            Style::default().fg(accent),
        ),
    ])]
}

fn compact_memory_model_label(model: &str) -> String {
    let trimmed = model.trim();
    let model_name = trimmed
        .rsplit_once('·')
        .map(|(_, model)| model.trim())
        .filter(|model| !model.is_empty())
        .unwrap_or(trimmed);
    shorten_model_name(model_name)
}

fn memory_active_summary(state: &MemoryState) -> Option<String> {
    match state {
        MemoryState::Idle => None,
        MemoryState::Embedding => Some("searching".to_string()),
        MemoryState::SidecarChecking { count } => Some(format!("verify {count}")),
        MemoryState::FoundRelevant { count } => Some(format!("ready {count}")),
        MemoryState::Extracting { reason } => Some(if reason.trim().is_empty() {
            "extracting".to_string()
        } else {
            format!("extract {}", reason)
        }),
        MemoryState::Maintaining { phase } => Some(if phase.trim().is_empty() {
            "maintaining".to_string()
        } else {
            format!("maintain {}", phase)
        }),
        MemoryState::ToolAction { action, detail } => Some(if detail.trim().is_empty() {
            action.clone()
        } else {
            format!("{} {}", action, detail)
        }),
    }
}

pub(crate) fn is_traceworthy_memory_event(event: &MemoryEvent) -> bool {
    !matches!(
        event.kind,
        MemoryEventKind::EmbeddingStarted
            | MemoryEventKind::SidecarStarted
            | MemoryEventKind::SidecarNotRelevant
            | MemoryEventKind::SidecarComplete { .. }
    )
}

fn memory_last_trace_summary(activity: &MemoryActivity) -> Option<String> {
    let event = activity
        .recent_events
        .iter()
        .find(|event| is_traceworthy_memory_event(event))?;
    let (_, text, _) = format_event_for_expanded(event, 120);
    if text.is_empty() { None } else { Some(text) }
}

fn memory_state_detail(state: &MemoryState) -> Option<String> {
    match state {
        MemoryState::Idle => None,
        MemoryState::Embedding => Some("embedding search".to_string()),
        MemoryState::SidecarChecking { count } => Some(format!("checking {} candidate(s)", count)),
        MemoryState::FoundRelevant { count } => Some(format!("found {} relevant", count)),
        MemoryState::Extracting { reason } => Some(if reason.trim().is_empty() {
            "extracting".to_string()
        } else {
            format!("extracting {}", reason)
        }),
        MemoryState::Maintaining { phase } => Some(if phase.trim().is_empty() {
            "maintaining graph".to_string()
        } else {
            format!("maintaining {}", phase)
        }),
        MemoryState::ToolAction { action, detail } => Some(if detail.trim().is_empty() {
            action.clone()
        } else {
            format!("{} {}", action, detail)
        }),
    }
}

fn render_memory_expanded(info: &MemoryInfo, inner: Rect) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();
    let max_width = inner.width.saturating_sub(2) as usize;

    lines.push(render_memory_header_line(
        info,
        info.activity.as_ref(),
        max_width,
    ));
    if let Some(count_line) = render_memory_count_line(info, max_width) {
        lines.push(count_line);
    }
    if let Some(activity) = &info.activity {
        lines.push(render_memory_status_line(activity, max_width));
    }
    if let Some(model_line) = render_memory_model_line(info, max_width) {
        lines.push(model_line);
    }

    if let Some(activity) = &info.activity {
        if memory_should_render_pipeline(activity) {
            lines.extend(render_memory_pipeline_display_lines(activity, max_width));
        }

        if let Some(last_line) = render_memory_last_trace_line(activity, max_width) {
            lines.push(last_line);
        }
    }

    lines.truncate(inner.height as usize);
    lines
}
fn format_age(duration: std::time::Duration) -> String {
    let secs = duration.as_secs();
    if secs < 2 {
        "now".to_string()
    } else if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}

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

fn render_background_compact(info: &BackgroundInfo) -> Vec<Line<'static>> {
    let mut spans: Vec<Span> = Vec::new();

    // Show spinner icon for active background work
    spans.push(Span::styled("⏳ ", Style::default().fg(rgb(180, 140, 255))));

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
        Style::default().fg(rgb(160, 160, 170)),
    ));

    if spans.len() <= 1 {
        return Vec::new();
    }

    vec![Line::from(spans)]
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
