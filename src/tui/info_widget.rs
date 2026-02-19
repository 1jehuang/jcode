//! InfoWidget - Floating information panels that appear in empty screen space
//!
//! Supports multiple widget types with priority ordering and side preferences.
//! In centered mode, widgets can appear on both left and right margins.
//! In left-aligned mode, widgets only appear on the right margin.

use crate::ambient::AmbientStatus;
use crate::memory_graph::EdgeKind;
use crate::prompt::ContextInfo;
use crate::protocol::SwarmMemberStatus;
use crate::provider::DEFAULT_CONTEXT_LIMIT;
use crate::todo::TodoItem;
use ratatui::{
    prelude::*,
    widgets::{Block, BorderType, Borders, Paragraph},
};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Instant;

/// Build graph topology (nodes + edges) from a MemoryGraph for visualization.
/// Combines project and global graphs, sampling nodes if there are too many.
pub fn build_graph_topology(
    project: Option<&crate::memory_graph::MemoryGraph>,
    global: Option<&crate::memory_graph::MemoryGraph>,
) -> (Vec<GraphNode>, Vec<GraphEdge>) {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut id_to_idx: HashMap<String, usize> = HashMap::new();

    // Collect all memory nodes from both graphs
    // Sort keys for deterministic iteration order (HashMap order is random,
    // which causes the graph layout to jitter on every frame redraw)
    let graphs: Vec<&crate::memory_graph::MemoryGraph> =
        [project, global].into_iter().flatten().collect();

    for graph in &graphs {
        let mut memory_ids: Vec<&String> = graph.memories.keys().collect();
        memory_ids.sort();
        for id in memory_ids {
            let entry = &graph.memories[id];
            if !id_to_idx.contains_key(id) {
                let idx = nodes.len();
                id_to_idx.insert(id.clone(), idx);
                nodes.push(GraphNode {
                    id: id.clone(),
                    label: truncate_smart(&entry.content, 30),
                    kind: entry.category.to_string(),
                    is_memory: true,
                    is_active: entry.active,
                    confidence: entry.effective_confidence(),
                    degree: 0,
                });
            }
        }

        let mut tag_ids: Vec<&String> = graph.tags.keys().collect();
        tag_ids.sort();
        for id in tag_ids {
            if !id_to_idx.contains_key(id) {
                let idx = nodes.len();
                let label = graph
                    .tags
                    .get(id)
                    .map(|tag| truncate_smart(&tag.name, 22))
                    .unwrap_or_else(|| id.trim_start_matches("tag:").to_string());
                id_to_idx.insert(id.clone(), idx);
                nodes.push(GraphNode {
                    id: id.clone(),
                    label,
                    kind: "tag".to_string(),
                    is_memory: false,
                    is_active: true,
                    confidence: 1.0,
                    degree: 0,
                });
            }
        }

        let mut cluster_ids: Vec<&String> = graph.clusters.keys().collect();
        cluster_ids.sort();
        for id in cluster_ids {
            if !id_to_idx.contains_key(id) {
                let idx = nodes.len();
                let label = graph
                    .clusters
                    .get(id)
                    .and_then(|cluster| cluster.name.clone())
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| id.trim_start_matches("cluster:").to_string());
                id_to_idx.insert(id.clone(), idx);
                nodes.push(GraphNode {
                    id: id.clone(),
                    label: truncate_smart(&label, 22),
                    kind: "cluster".to_string(),
                    is_memory: false,
                    is_active: true,
                    confidence: 1.0,
                    degree: 0,
                });
            }
        }
    }

    // Collect edges (sort for deterministic order)
    let mut edge_seen: HashSet<(usize, usize, String)> = HashSet::new();
    for graph in &graphs {
        let mut edge_src_ids: Vec<&String> = graph.edges.keys().collect();
        edge_src_ids.sort();
        for src_id in edge_src_ids {
            let edge_list = &graph.edges[src_id];
            let Some(&src_idx) = id_to_idx.get(src_id) else {
                continue;
            };
            let mut sorted_edges = edge_list.clone();
            sorted_edges.sort_by(|a, b| {
                a.target
                    .cmp(&b.target)
                    .then_with(|| edge_kind_name(&a.kind).cmp(edge_kind_name(&b.kind)))
            });
            for edge in sorted_edges {
                let Some(&tgt_idx) = id_to_idx.get(&edge.target) else {
                    continue;
                };
                if src_idx != tgt_idx {
                    let kind = edge_kind_name(&edge.kind).to_string();
                    if !edge_seen.insert((src_idx, tgt_idx, kind.clone())) {
                        continue;
                    }
                    edges.push(GraphEdge {
                        source: src_idx,
                        target: tgt_idx,
                        kind,
                    });
                    if src_idx < nodes.len() {
                        nodes[src_idx].degree += 1;
                    }
                    if tgt_idx < nodes.len() {
                        nodes[tgt_idx].degree += 1;
                    }
                }
            }
        }
    }

    // Bound topology size for stable redraw cost while preserving enough
    // neighborhood signal for contextual subgraph selection.
    let max_nodes = 96;
    if nodes.len() > max_nodes {
        let mut indices: Vec<usize> = (0..nodes.len()).collect();
        indices.sort_by(|&a, &b| {
            graph_node_score(&nodes[b])
                .partial_cmp(&graph_node_score(&nodes[a]))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.cmp(&a))
        });

        let keep: HashSet<usize> = indices.into_iter().take(max_nodes).collect();

        let mut new_nodes = Vec::new();
        let mut old_to_new: HashMap<usize, usize> = HashMap::new();
        for old_idx in 0..nodes.len() {
            if keep.contains(&old_idx) {
                let new_idx = new_nodes.len();
                old_to_new.insert(old_idx, new_idx);
                new_nodes.push(nodes[old_idx].clone());
            }
        }

        let new_edges: Vec<GraphEdge> = edges
            .iter()
            .filter_map(|edge| {
                let na = old_to_new.get(&edge.source)?;
                let nb = old_to_new.get(&edge.target)?;
                Some(GraphEdge {
                    source: *na,
                    target: *nb,
                    kind: edge.kind.clone(),
                })
            })
            .collect();

        return (new_nodes, new_edges);
    }

    (nodes, edges)
}

fn edge_kind_name(kind: &EdgeKind) -> &'static str {
    match kind {
        EdgeKind::HasTag => "has_tag",
        EdgeKind::InCluster => "in_cluster",
        EdgeKind::RelatesTo { .. } => "relates_to",
        EdgeKind::Supersedes => "supersedes",
        EdgeKind::Contradicts => "contradicts",
        EdgeKind::DerivedFrom => "derived_from",
    }
}

fn graph_node_score(node: &GraphNode) -> f32 {
    let memory_bias = if node.is_memory { 2.0 } else { 0.0 };
    let active_bias = if node.is_active { 1.0 } else { 0.0 };
    node.degree as f32 + memory_bias + active_bias + node.confidence * 2.0
}

/// Types of info widgets that can be displayed
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WidgetKind {
    /// Combined overview to reduce scattered widgets
    Overview,
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
}

impl WidgetKind {
    /// Priority for display (lower = higher priority)
    pub fn priority(self) -> u8 {
        match self {
            WidgetKind::Diagrams => 0, // Highest priority - user explicitly wants to see it
            WidgetKind::Overview => 1,
            WidgetKind::Todos => 2,
            WidgetKind::ContextUsage => 3,
            WidgetKind::MemoryActivity => 4,
            WidgetKind::SwarmStatus => 5,
            WidgetKind::BackgroundTasks => 6,
            WidgetKind::AmbientMode => 7,
            WidgetKind::UsageLimits => 8,
            WidgetKind::ModelInfo => 9,
            WidgetKind::Tips => 10,
        }
    }

    /// Preferred side for this widget
    pub fn preferred_side(self) -> Side {
        match self {
            WidgetKind::Diagrams => Side::Right, // Diagrams on right
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
        }
    }

    /// Minimum height needed for this widget
    pub fn min_height(self) -> u16 {
        match self {
            WidgetKind::Diagrams => 10, // Diagrams need more space
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
        }
    }

    /// All widget kinds in priority order
    pub fn all_by_priority() -> &'static [WidgetKind] {
        &[
            WidgetKind::Diagrams,
            WidgetKind::Overview,
            WidgetKind::Todos,
            WidgetKind::ContextUsage,
            WidgetKind::MemoryActivity,
            WidgetKind::SwarmStatus,
            WidgetKind::BackgroundTasks,
            WidgetKind::AmbientMode,
            WidgetKind::UsageLimits,
            WidgetKind::ModelInfo,
            WidgetKind::Tips,
        ]
    }

    pub fn as_str(self) -> &'static str {
        match self {
            WidgetKind::Diagrams => "diagrams",
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

fn is_overview_mergeable(kind: WidgetKind) -> bool {
    matches!(
        kind,
        WidgetKind::Todos
            | WidgetKind::ContextUsage
            | WidgetKind::SwarmStatus
            | WidgetKind::BackgroundTasks
            | WidgetKind::UsageLimits
            | WidgetKind::ModelInfo
    )
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
    /// Cache read tokens (from cache, cheaper) - for API-key providers
    pub cache_read_tokens: Option<u64>,
    /// Cache write tokens (creating cache, more expensive) - for API-key providers
    pub cache_write_tokens: Option<u64>,
    /// Output tokens per second (live streaming)
    pub output_tps: Option<f32>,
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
    /// Background maintenance/gardening of the memory graph
    Maintaining { phase: String },
    /// Agent is actively using a memory tool
    ToolAction { action: String, detail: String },
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

/// Ambient mode status data for the info widget
#[derive(Debug, Clone)]
pub struct AmbientWidgetData {
    pub status: AmbientStatus,
    pub queue_count: usize,
    pub next_queue_preview: Option<String>,
    pub last_run_ago: Option<String>,
    pub last_summary: Option<String>,
    pub next_wake: Option<String>,
    pub budget_percent: Option<f32>,
}

/// Minimum width needed to show the widget
const MIN_WIDGET_WIDTH: u16 = 24;
/// Maximum width the widget can take
const MAX_WIDGET_WIDTH: u16 = 40;
/// Minimum height needed to show the widget
const MIN_WIDGET_HEIGHT: u16 = 5;
/// How much width shrinkage to tolerate before forcing a widget to reposition.
/// Higher values = stickier widgets during scroll (less jitter).
const STICKY_WIDTH_TOLERANCE: u16 = 4;
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
    /// Mermaid diagrams to display
    pub diagrams: Vec<DiagramInfo>,
    /// Ambient mode status
    pub ambient_info: Option<AmbientWidgetData>,
    /// Actual API-reported context tokens (from last streaming response)
    /// When available, this is more accurate than the char-based estimate in context_info
    pub observed_context_tokens: Option<u64>,
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
            && self.diagrams.is_empty()
    }

    /// Check if a specific widget kind has data to display
    pub fn has_data_for(&self, kind: WidgetKind) -> bool {
        match kind {
            WidgetKind::Diagrams => !self.diagrams.is_empty(),
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
                    .swarm_info
                    .as_ref()
                    .map(|s| {
                        s.subagent_status.is_some()
                            || s.session_count > 1
                            || s.client_count.is_some()
                            || !s.members.is_empty()
                    })
                    .unwrap_or(false)
                {
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
                if self
                    .usage_info
                    .as_ref()
                    .map(|u| u.available)
                    .unwrap_or(false)
                {
                    sections += 1;
                }
                if self.queue_mode.is_some() {
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
                .map(|m| m.total_count > 0 || m.activity.is_some())
                .unwrap_or(false),
            WidgetKind::SwarmStatus => self
                .swarm_info
                .as_ref()
                .map(|s| {
                    s.subagent_status.is_some()
                        || s.session_count > 1
                        || s.client_count.is_some()
                        || !s.members.is_empty()
                })
                .unwrap_or(false),
            WidgetKind::BackgroundTasks => self
                .background_info
                .as_ref()
                .map(|b| b.running_count > 0 || b.memory_agent_active)
                .unwrap_or(false),
            WidgetKind::AmbientMode => self.ambient_info.is_some(),
            WidgetKind::UsageLimits => false, // Combined into ModelInfo
            WidgetKind::ModelInfo => self.model.is_some(),
            WidgetKind::Tips => true, // Always available
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
    let overview_requested = available.contains(&WidgetKind::Overview);

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
            // Anchor widget flush against the edge — right edge stays at x_offset,
            // left edge stays at x_offset. Only the widget width varies.
            let x = match margin.side {
                Side::Right => margin.x_offset.saturating_sub(clamped_width),
                Side::Left => margin.x_offset,
            };
            all_rects.push((margin.side, top, height, clamped_width, x, margin_idx));
        }
    }

    // Phase 1: Sticky positioning — try to keep previous widgets in place.
    // This prevents jittery repositioning during scroll when margins change slightly.
    let prev_placements = state.placements.clone();
    let mut placements: Vec<WidgetPlacement> = Vec::new();
    let mut kept: std::collections::HashSet<WidgetKind> = std::collections::HashSet::new();

    for prev in &prev_placements {
        if !available.contains(&prev.kind) {
            continue;
        }
        // Never keep border-only placements from older frames.
        if prev.rect.height <= 2 {
            continue;
        }
        // When overview is available, prefer reflowing mergeable widgets into one panel
        // rather than sticking to old scattered placements.
        if overview_requested && is_overview_mergeable(prev.kind) {
            continue;
        }

        // Convert widget rect to row-relative coordinates
        let row_start = prev.rect.y.saturating_sub(messages_area.y) as usize;
        let row_end = row_start + prev.rect.height as usize;

        // Check if the old position still has enough margin space (with tolerance)
        let widths = match prev.side {
            Side::Right => &margins.right_widths,
            Side::Left => &margins.left_widths,
        };

        // All rows must still exist and have enough width
        let still_fits = row_end <= widths.len()
            && (row_start..row_end)
                .all(|row| widths[row] + STICKY_WIDTH_TOLERANCE >= prev.rect.width);

        if still_fits {
            // Keep the same rows/side, but clamp width to the current actual margin.
            // This preserves sticky positioning without allowing text overlap.
            let actual_fit_width = widths[row_start..row_end]
                .iter()
                .copied()
                .min()
                .unwrap_or(0)
                .min(MAX_WIDGET_WIDTH);
            if actual_fit_width < MIN_WIDGET_WIDTH {
                continue;
            }
            let kept_width = prev.rect.width.min(actual_fit_width);
            let kept_x = match prev.side {
                Side::Right => messages_area
                    .x
                    .saturating_add(messages_area.width)
                    .saturating_sub(kept_width),
                Side::Left => messages_area.x,
            };
            placements.push(WidgetPlacement {
                kind: prev.kind,
                rect: Rect::new(kept_x, prev.rect.y, kept_width, prev.rect.height),
                side: prev.side,
            });
            kept.insert(prev.kind);

            // Remove the kept widget's rows from available rects so greedy placement
            // doesn't overlap. Shrink or split any rect that overlaps these rows.
            for rect in all_rects.iter_mut() {
                if rect.2 == 0 || rect.0 != prev.side {
                    continue;
                }
                let r_start = rect.1 as usize;
                let r_end = r_start + rect.2 as usize;
                // Check overlap
                if row_start < r_end && row_end > r_start {
                    if row_start <= r_start && row_end >= r_end {
                        // Fully consumed
                        rect.2 = 0;
                    } else if row_start <= r_start {
                        // Trim from top
                        let trim = (row_end - r_start) as u16;
                        rect.1 += trim;
                        rect.2 = rect.2.saturating_sub(trim);
                    } else {
                        // Trim from bottom (keep top portion only)
                        rect.2 = (row_start - r_start) as u16;
                    }
                }
            }
        }
    }

    // Phase 2: Greedy placement for widgets that couldn't keep their position
    let mut overview_placed = placements.iter().any(|p| p.kind == WidgetKind::Overview);
    for kind in available {
        if kept.contains(&kind) {
            continue;
        }
        if overview_placed && is_overview_mergeable(kind) {
            continue;
        }

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
            // Skip widgets that would render as an empty border.
            if widget_height <= 2 {
                continue;
            }

            // Place widget at top of rect
            let y = messages_area.y + top;

            placements.push(WidgetPlacement {
                kind,
                rect: Rect::new(x, y, width, widget_height),
                side,
            });
            if kind == WidgetKind::Overview {
                overview_placed = true;
            }

            // Shrink the rect: move top down, reduce height, recalculate width
            let remaining_height = height.saturating_sub(widget_height);
            if remaining_height >= MIN_WIDGET_HEIGHT {
                let new_top = top + widget_height;
                all_rects[idx].1 = new_top; // new top
                all_rects[idx].2 = remaining_height; // new height

                // Recalculate width for the new row range to avoid overlapping text
                // The new rows might have wider text than the original rows
                let margin = &margin_spaces[margin_idx];
                let new_end =
                    (new_top as usize + remaining_height as usize).min(margin.widths.len());
                if (new_top as usize) < new_end {
                    // Get actual minimum margin width (unclamped) for positioning
                    let actual_min_width = margin.widths[new_top as usize..new_end]
                        .iter()
                        .copied()
                        .min()
                        .unwrap_or(0);
                    // Widget width is clamped to MAX_WIDGET_WIDTH
                    let new_min_width = actual_min_width.min(MAX_WIDGET_WIDTH);
                    all_rects[idx].3 = new_min_width;
                    // Anchor flush against the edge.
                    all_rects[idx].4 = match side {
                        Side::Right => margin.x_offset.saturating_sub(new_min_width),
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
fn calculate_widget_height(
    kind: WidgetKind,
    data: &InfoWidgetData,
    width: u16,
    max_height: u16,
) -> u16 {
    let inner_width = width.saturating_sub(2) as usize;
    let border_height = 2u16;

    let content_height = match kind {
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
            // Diagrams need significant height to be useful
            15
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
            let Some(info) = &data.memory_info else {
                return 0;
            };
            let mut h = 1u16; // Title
            if info.total_count > 0 {
                h += 1; // Project/global + topology summary
                if !info.by_category.is_empty() {
                    h += 1; // Category summary
                }
            }
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
            let mut h = 1u16; // Status line
            if info.queue_count > 0 {
                h += 1; // Queue line
            }
            if info.last_run_ago.is_some() {
                h += 1; // Last run line
            }
            if info.next_wake.is_some() {
                h += 1; // Next wake line
            }
            if info.budget_percent.is_some() {
                h += 1; // Budget bar
            }
            h
        }
        WidgetKind::UsageLimits => {
            if data
                .usage_info
                .as_ref()
                .map(|u| u.available)
                .unwrap_or(false)
            {
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
            if data.session_count.is_some() || data.session_name.is_some() {
                h += 1; // Session/name line
            }
            if let Some(info) = &data.usage_info {
                if info.available {
                    match info.provider {
                        UsageProvider::CostBased => {
                            h += 1; // Cost + tokens
                            if info.cache_read_tokens.is_some() || info.cache_write_tokens.is_some()
                            {
                                h += 1; // Cache line
                            }
                            if info.output_tps.is_some() {
                                h += 1; // TPS line
                            }
                        }
                        _ => {
                            h += 2; // Subscription bars
                        }
                    }
                }
            }
            h
        }
        WidgetKind::Tips => {
            let effective_w = inner_width.saturating_sub(2); // 2-char indent on tip text
            let tip = current_tip(effective_w);
            let lines = wrap_tip_text(&tip.text, effective_w);
            1 + lines.len() as u16 // header + wrapped text
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
                dots.push(Span::styled(
                    "● ",
                    Style::default().fg(Color::Rgb(170, 170, 180)),
                ));
            } else {
                dots.push(Span::styled(
                    "○ ",
                    Style::default().fg(Color::Rgb(100, 100, 110)),
                ));
            }
        }
        if !dots.is_empty() {
            lines.push(Line::from(dots));
        }
    }

    lines.truncate(inner.height as usize);
    frame.render_widget(Paragraph::new(lines), inner);
}

const MEMORY_TEXT_SUBGRAPH_MAX_NODES: usize = 8;
const MEMORY_TEXT_SUBGRAPH_MAX_EDGES: usize = 10;

#[derive(Debug, Clone)]
struct MemorySubgraph {
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
}

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
                    graph_node_score(&info.graph_nodes[*b_idx])
                        .partial_cmp(&graph_node_score(&info.graph_nodes[*a_idx]))
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
            graph_node_score(&info.graph_nodes[*b])
                .partial_cmp(&graph_node_score(&info.graph_nodes[*a]))
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

fn pick_subgraph_center(info: &MemoryInfo) -> Option<usize> {
    let mut best_idx: Option<usize> = None;
    let mut best_score: f32 = -1.0;

    for (idx, node) in info.graph_nodes.iter().enumerate() {
        let mut score = graph_node_score(node);
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

    // Render todos (limit based on available height)
    let available_lines = inner.height.saturating_sub(2) as usize; // Account for header + bar
    for todo in sorted_todos.iter().take(available_lines.min(5)) {
        let is_blocked = !todo.blocked_by.is_empty();
        let (icon, status_color) = if is_blocked && todo.status != "completed" {
            ("⊳", Color::Rgb(180, 140, 100))
        } else {
            match todo.status.as_str() {
                "completed" => ("✓", Color::Rgb(100, 180, 100)),
                "in_progress" => ("▶", Color::Rgb(255, 200, 100)),
                "cancelled" => ("✗", Color::Rgb(120, 80, 80)),
                _ => ("○", Color::Rgb(120, 120, 130)),
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
            Color::Rgb(100, 100, 110)
        } else if is_blocked {
            Color::Rgb(120, 120, 130)
        } else if todo.status == "in_progress" {
            Color::Rgb(200, 200, 210)
        } else {
            Color::Rgb(160, 160, 170)
        };

        let mut spans = vec![
            Span::styled(format!("{} ", icon), Style::default().fg(status_color)),
            Span::styled(content, Style::default().fg(text_color)),
        ];
        if !suffix.is_empty() {
            spans.push(Span::styled(
                suffix.to_string(),
                Style::default().fg(Color::Rgb(100, 100, 110)),
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
    if info.total_chars == 0 && data.observed_context_tokens.is_none() {
        return Vec::new();
    }

    let used_tokens = data
        .observed_context_tokens
        .map(|t| t as usize)
        .unwrap_or_else(|| info.estimated_tokens());
    let limit_tokens = data.context_limit.unwrap_or(DEFAULT_CONTEXT_LIMIT).max(1);
    let used_pct = ((used_tokens as f64 / limit_tokens as f64) * 100.0)
        .round()
        .clamp(0.0, 100.0) as u8;
    let left_pct = 100u8.saturating_sub(used_pct);

    vec![render_labeled_bar(
        "Context",
        used_pct,
        left_pct,
        None,
        inner.width,
    )]
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

    if info.total_count > 0 {
        let summary = format!(
            "{}p/{}g · {}n {}e",
            info.project_count,
            info.global_count,
            info.graph_nodes.len(),
            info.graph_edges.len()
        );
        lines.push(Line::from(vec![Span::styled(
            summary,
            Style::default().fg(Color::Rgb(130, 130, 140)),
        )]));

        if !info.by_category.is_empty() {
            let mut categories: Vec<(&String, &usize)> = info.by_category.iter().collect();
            categories.sort_by(|(name_a, count_a), (name_b, count_b)| {
                count_b.cmp(count_a).then_with(|| name_a.cmp(name_b))
            });

            let cat_text = categories
                .into_iter()
                .take(4)
                .map(|(name, count)| format!("{}:{}", truncate_chars(name, 3), count))
                .collect::<Vec<_>>()
                .join(" ");
            let cat_text = truncate_smart(&cat_text, inner.width.saturating_sub(2) as usize);

            lines.push(Line::from(vec![Span::styled(
                cat_text,
                Style::default().fg(Color::Rgb(105, 105, 115)),
            )]));
        }
    }

    let remaining = inner.height.saturating_sub(lines.len() as u16);
    if remaining > 0 {
        lines.extend(render_memory_topology_lines(
            info,
            Rect::new(0, 0, inner.width, remaining),
        ));
    }

    // Activity state if active
    if let Some(activity) = &info.activity {
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
            MemoryState::Maintaining { phase } => Line::from(vec![
                Span::styled("🌿 ", Style::default().fg(Color::Rgb(120, 220, 180))),
                Span::styled(
                    truncate_smart(
                        &format!("Maintaining ({})", phase),
                        inner.width.saturating_sub(4) as usize,
                    ),
                    Style::default().fg(Color::Rgb(180, 180, 190)),
                ),
            ]),
            MemoryState::ToolAction { action, detail } => {
                let icon = match action.as_str() {
                    "remember" | "store" => "💾",
                    "recall" | "search" => "🔍",
                    "forget" => "🗑\u{fe0f}",
                    "tag" => "🏷\u{fe0f}",
                    "link" => "🔗",
                    "list" | "related" => "📋",
                    _ => "🧠",
                };
                let text = if detail.is_empty() {
                    action.clone()
                } else {
                    format!("{}: {}", action, detail)
                };
                Line::from(vec![
                    Span::styled(
                        format!("{} ", icon),
                        Style::default().fg(Color::Rgb(200, 150, 255)),
                    ),
                    Span::styled(
                        truncate_smart(&text, inner.width.saturating_sub(4) as usize),
                        Style::default().fg(Color::Rgb(200, 200, 210)),
                    ),
                ])
            }
        };
        lines.push(state_line);

        // Recent events (limit to 3)
        let max_events = (inner.height.saturating_sub(2) as usize).min(3);
        for event in activity.recent_events.iter().take(max_events) {
            let (icon, text, color) =
                format_memory_event(event, inner.width.saturating_sub(4) as usize);
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
        MemoryEventKind::EmbeddingStarted => {
            ("🔍", "Embedding...".to_string(), Color::Rgb(140, 180, 255))
        }
        MemoryEventKind::EmbeddingComplete { latency_ms, hits } => (
            "→",
            format!("{} hits ({}ms)", hits, latency_ms),
            Color::Rgb(140, 180, 255),
        ),
        MemoryEventKind::SidecarStarted => {
            ("⚡", "Verifying".to_string(), Color::Rgb(255, 200, 100))
        }
        MemoryEventKind::SidecarRelevant { memory_preview } => {
            let preview = truncate_smart(memory_preview, max_width.saturating_sub(2));
            ("✓", preview, Color::Rgb(100, 200, 100))
        }
        MemoryEventKind::SidecarNotRelevant => {
            ("✗", "Not relevant".to_string(), Color::Rgb(150, 150, 160))
        }
        MemoryEventKind::SidecarComplete { latency_ms } => {
            ("⏱", format!("{}ms", latency_ms), Color::Rgb(140, 140, 150))
        }
        MemoryEventKind::MemorySurfaced { memory_preview } => {
            let preview = truncate_smart(memory_preview, max_width.saturating_sub(2));
            ("★", preview, Color::Rgb(255, 220, 100))
        }
        MemoryEventKind::MemoryInjected {
            count,
            prompt_chars,
            age_ms,
            preview,
            items,
        } => {
            let snippet = items
                .first()
                .map(|item| format!("[{}] {}", item.section, item.content))
                .unwrap_or_else(|| preview.clone());
            let msg = truncate_smart(
                &format!(
                    "{} mem ({}c, {}ms) {}",
                    count, prompt_chars, age_ms, snippet
                ),
                max_width.saturating_sub(2),
            );
            ("↳", msg, Color::Rgb(140, 210, 255))
        }
        MemoryEventKind::MaintenanceStarted { verified, rejected } => (
            "🌿",
            format!("maintain v{} r{}", verified, rejected),
            Color::Rgb(120, 220, 180),
        ),
        MemoryEventKind::MaintenanceLinked { links } => {
            ("⇄", format!("{} links", links), Color::Rgb(120, 220, 180))
        }
        MemoryEventKind::MaintenanceConfidence { boosted, decayed } => (
            "±",
            format!("conf +{} -{}", boosted, decayed),
            Color::Rgb(120, 220, 180),
        ),
        MemoryEventKind::MaintenanceCluster { clusters, members } => (
            "◈",
            format!("clusters {} ({} mem)", clusters, members),
            Color::Rgb(120, 220, 180),
        ),
        MemoryEventKind::MaintenanceTagInferred { tag, applied } => (
            "#",
            format!("tag {} (+{})", truncate_smart(tag, 20), applied),
            Color::Rgb(120, 220, 180),
        ),
        MemoryEventKind::MaintenanceGap { candidates } => (
            "?",
            format!("gap after {} candidates", candidates),
            Color::Rgb(220, 180, 120),
        ),
        MemoryEventKind::MaintenanceComplete { latency_ms } => (
            "✓",
            format!("maint {}ms", latency_ms),
            Color::Rgb(120, 220, 180),
        ),
        MemoryEventKind::ExtractionStarted { reason } => {
            let msg = truncate_smart(reason, max_width.saturating_sub(2));
            (
                "🧠",
                format!("Extracting: {}", msg),
                Color::Rgb(200, 150, 255),
            )
        }
        MemoryEventKind::ExtractionComplete { count } => (
            "✓",
            format!("Saved {} memories", count),
            Color::Rgb(100, 200, 100),
        ),
        MemoryEventKind::Error { message } => {
            let msg = truncate_smart(message, max_width.saturating_sub(2));
            ("!", msg, Color::Rgb(255, 100, 100))
        }
        MemoryEventKind::ToolRemembered {
            content,
            scope,
            category,
        } => {
            let msg = truncate_smart(
                &format!("[{}] {} ({})", category, content, scope),
                max_width.saturating_sub(2),
            );
            ("💾", msg, Color::Rgb(100, 200, 100))
        }
        MemoryEventKind::ToolRecalled { query, count } => {
            let msg = truncate_smart(
                &format!("{} found for '{}'", count, query),
                max_width.saturating_sub(2),
            );
            ("🔍", msg, Color::Rgb(140, 180, 255))
        }
        MemoryEventKind::ToolForgot { id } => {
            let msg = truncate_smart(id, max_width.saturating_sub(2));
            ("🗑\u{fe0f}", msg, Color::Rgb(255, 170, 100))
        }
        MemoryEventKind::ToolTagged { id, tags } => {
            let msg = truncate_smart(&format!("{} +{}", id, tags), max_width.saturating_sub(2));
            ("🏷\u{fe0f}", msg, Color::Rgb(140, 200, 255))
        }
        MemoryEventKind::ToolLinked { from, to } => {
            let msg = truncate_smart(&format!("{} → {}", from, to), max_width.saturating_sub(2));
            ("🔗", msg, Color::Rgb(200, 180, 255))
        }
        MemoryEventKind::ToolListed { count } => (
            "📋",
            format!("{} memories", count),
            Color::Rgb(140, 140, 150),
        ),
    }
}

fn render_memory_topology_lines(info: &MemoryInfo, inner: Rect) -> Vec<Line<'static>> {
    if info.graph_nodes.is_empty() || inner.width < 8 || inner.height == 0 {
        return Vec::new();
    }

    let max_lines = inner.height.min(3) as usize;
    let Some(subgraph) = select_contextual_subgraph(
        info,
        MEMORY_TEXT_SUBGRAPH_MAX_NODES,
        MEMORY_TEXT_SUBGRAPH_MAX_EDGES,
    ) else {
        return Vec::new();
    };
    if subgraph.nodes.is_empty() {
        return Vec::new();
    }

    let mut lines: Vec<Line> = Vec::new();
    let hub = &subgraph.nodes[0];
    let hub_label = truncate_smart(&hub.label, inner.width.saturating_sub(8) as usize);
    let hub_kind = if hub.kind == "tag" { "tag" } else { "mem" };
    lines.push(Line::from(vec![
        Span::styled("• ", Style::default().fg(Color::Rgb(140, 180, 220))),
        Span::styled(
            format!("hub {}: {}", hub_kind, hub_label),
            Style::default().fg(Color::Rgb(145, 145, 155)),
        ),
    ]));

    let mut edges = subgraph.edges;
    edges.sort_by(|a, b| {
        let a_hub = a.source == 0 || a.target == 0;
        let b_hub = b.source == 0 || b.target == 0;
        b_hub
            .cmp(&a_hub)
            .then_with(|| edge_kind_priority(&b.kind).cmp(&edge_kind_priority(&a.kind)))
            .then_with(|| a.source.cmp(&b.source))
            .then_with(|| a.target.cmp(&b.target))
    });

    for edge in edges.into_iter().take(max_lines.saturating_sub(1)) {
        let other_idx = if edge.source == 0 {
            edge.target
        } else {
            edge.source
        };
        let Some(other) = subgraph.nodes.get(other_idx) else {
            continue;
        };
        let relation = memory_edge_label(&edge.kind);
        let text = format!("↳ {} {}", relation, other.label);
        let text = truncate_smart(&text, inner.width.saturating_sub(2) as usize);
        lines.push(Line::from(vec![Span::styled(
            text,
            Style::default().fg(Color::Rgb(110, 110, 122)),
        )]));
        if lines.len() >= max_lines {
            break;
        }
    }

    lines
}

fn memory_edge_label(kind: &str) -> &'static str {
    match kind {
        "has_tag" => "tag",
        "in_cluster" => "cluster",
        "supersedes" => "sup",
        "contradicts" => "contra",
        "derived_from" => "derived",
        "relates_to" => "rel",
        _ => "rel",
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
        "spawned" => (Color::Rgb(140, 140, 150), "○"),
        "ready" => (Color::Rgb(120, 180, 120), "●"),
        "running" => (Color::Rgb(255, 200, 100), "▶"),
        "blocked" => (Color::Rgb(255, 170, 80), "⏸"),
        "failed" => (Color::Rgb(255, 100, 100), "✗"),
        "completed" => (Color::Rgb(100, 200, 100), "✓"),
        "stopped" => (Color::Rgb(140, 140, 150), "■"),
        "crashed" => (Color::Rgb(255, 80, 80), "!"),
        _ => (Color::Rgb(140, 140, 150), "·"),
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
            Style::default().fg(Color::Rgb(255, 200, 100)),
        ),
        Span::styled(format!("{} ", icon), Style::default().fg(color)),
        Span::styled(line_text, Style::default().fg(Color::Rgb(140, 140, 150))),
    ])
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
            stats_parts.push(Span::styled(
                " · ",
                Style::default().fg(Color::Rgb(100, 100, 110)),
            ));
        }
        stats_parts.push(Span::styled(
            format!("{}c", clients),
            Style::default().fg(Color::Rgb(160, 160, 170)),
        ));
    }
    lines.push(Line::from(stats_parts));

    // Active subagent status (only when we don't have member status lines)
    if info.members.is_empty() {
        if let Some(status) = &info.subagent_status {
            lines.push(Line::from(vec![
                Span::styled("▶ ", Style::default().fg(Color::Rgb(255, 200, 100))),
                Span::styled(
                    truncate_smart(status, inner.width.saturating_sub(4) as usize),
                    Style::default().fg(Color::Rgb(200, 200, 210)),
                ),
            ]));
        }
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
                Span::styled("  · ", Style::default().fg(Color::Rgb(100, 100, 110))),
                Span::styled(
                    truncate_smart(name, max_name_len),
                    Style::default().fg(Color::Rgb(140, 140, 150)),
                ),
            ]));
        }
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

/// Render ambient mode status widget
fn render_ambient_widget(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let Some(info) = &data.ambient_info else {
        return Vec::new();
    };

    let mut lines: Vec<Line> = Vec::new();

    // Status line
    let (icon, status_text, status_color) = match &info.status {
        AmbientStatus::Idle => ("○", "Idle".to_string(), Color::Rgb(120, 120, 130)),
        AmbientStatus::Running { detail } => (
            "●",
            format!("Running ({})", detail),
            Color::Rgb(100, 200, 100),
        ),
        AmbientStatus::Scheduled { .. } => {
            ("◐", "Scheduled".to_string(), Color::Rgb(140, 180, 255))
        }
        AmbientStatus::Paused { reason } => (
            "⏸",
            format!(
                "Paused ({})",
                truncate_smart(reason, inner.width.saturating_sub(12) as usize)
            ),
            Color::Rgb(255, 200, 100),
        ),
        AmbientStatus::Disabled => ("■", "Disabled".to_string(), Color::Rgb(100, 100, 110)),
    };

    lines.push(Line::from(vec![
        Span::styled(format!("{} ", icon), Style::default().fg(status_color)),
        Span::styled(
            truncate_smart(&status_text, inner.width.saturating_sub(3) as usize),
            Style::default().fg(Color::Rgb(180, 180, 190)),
        ),
    ]));

    // Queue line
    if info.queue_count > 0 {
        let queue_text = if let Some(ref preview) = info.next_queue_preview {
            format!(
                "Queue: {} (next: {})",
                info.queue_count,
                truncate_smart(preview, inner.width.saturating_sub(16) as usize)
            )
        } else {
            format!("Queue: {} items", info.queue_count)
        };
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                truncate_smart(&queue_text, inner.width.saturating_sub(2) as usize),
                Style::default().fg(Color::Rgb(140, 140, 150)),
            ),
        ]));
    }

    // Last run line
    if let Some(ref ago) = info.last_run_ago {
        let last_text = if let Some(ref summary) = info.last_summary {
            format!(
                "Last: {} — {}",
                ago,
                truncate_smart(
                    summary,
                    inner.width.saturating_sub(10 + ago.len() as u16) as usize
                )
            )
        } else {
            format!("Last: {}", ago)
        };
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                truncate_smart(&last_text, inner.width.saturating_sub(2) as usize),
                Style::default().fg(Color::Rgb(140, 140, 150)),
            ),
        ]));
    }

    // Next wake line
    if let Some(ref next) = info.next_wake {
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                format!("Next: ~{} (adaptive)", next),
                Style::default().fg(Color::Rgb(140, 140, 150)),
            ),
        ]));
    }

    // Budget bar
    if let Some(budget) = info.budget_percent {
        let pct = (budget * 100.0).round().clamp(0.0, 100.0) as u8;
        let bar_width = inner.width.saturating_sub(12).min(10).max(4) as usize;
        let filled = ((budget * bar_width as f32).round() as usize).min(bar_width);
        let empty = bar_width.saturating_sub(filled);

        let bar_color = if pct < 20 {
            Color::Rgb(255, 100, 100)
        } else if pct <= 50 {
            Color::Rgb(255, 200, 100)
        } else {
            Color::Rgb(100, 200, 100)
        };

        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled("█".repeat(filled), Style::default().fg(bar_color)),
            Span::styled(
                "░".repeat(empty),
                Style::default().fg(Color::Rgb(50, 50, 60)),
            ),
            Span::styled(format!(" {}%", pct), Style::default().fg(bar_color)),
        ]));
    }

    lines
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
                Line::from(vec![Span::styled(
                    format!(
                        "{} in + {} out",
                        format_tokens(info.input_tokens),
                        format_tokens(info.output_tokens)
                    ),
                    Style::default().fg(Color::Rgb(140, 140, 150)),
                )]),
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

    // Add session info line if we have session count/name.
    if data.session_count.is_some() || data.session_name.is_some() {
        let mut parts = Vec::new();

        if let Some(sessions) = data.session_count {
            parts.push(format!(
                "{} session{}",
                sessions,
                if sessions == 1 { "" } else { "s" }
            ));
        }

        if let Some(name) = data.session_name.as_deref() {
            if !name.trim().is_empty() {
                parts.push(name.to_string());
            }
        }

        if !parts.is_empty() {
            let detail = truncate_smart(&parts.join(" · "), max_len.saturating_sub(2));
            lines.push(Line::from(vec![Span::styled(
                detail,
                Style::default().fg(Color::Rgb(140, 140, 150)),
            )]));
        }
    }

    if let Some(provider) = data
        .provider_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let mut provider_spans = vec![
            Span::styled("☁ ", Style::default().fg(Color::Rgb(140, 180, 255))),
            Span::styled(
                provider.to_lowercase(),
                Style::default().fg(Color::Rgb(140, 180, 255)),
            ),
        ];
        if let Some(upstream) = data.upstream_provider.as_deref().map(str::trim) {
            if !upstream.is_empty() {
                provider_spans.push(Span::styled(
                    " -> ",
                    Style::default().fg(Color::Rgb(100, 100, 110)),
                ));
                provider_spans.push(Span::styled(
                    upstream.to_string(),
                    Style::default().fg(Color::Rgb(220, 190, 120)),
                ));
            }
        }
        lines.push(Line::from(provider_spans));
    }

    // Auth method line (with upstream provider if available)
    if data.auth_method != AuthMethod::Unknown {
        let (icon, label, color) = match data.auth_method {
            AuthMethod::AnthropicOAuth => ("🔐", "OAuth", Color::Rgb(255, 160, 100)),
            AuthMethod::AnthropicApiKey => ("🔑", "API Key", Color::Rgb(180, 180, 190)),
            AuthMethod::OpenAIOAuth => ("🔐", "OAuth", Color::Rgb(100, 200, 180)),
            AuthMethod::OpenAIApiKey => ("🔑", "API Key", Color::Rgb(180, 180, 190)),
            AuthMethod::OpenRouterApiKey => ("🔑", "API Key", Color::Rgb(140, 180, 255)),
            AuthMethod::Unknown => unreachable!(),
        };

        // Show auth method with upstream provider if available
        if let Some(ref upstream) = data.upstream_provider {
            lines.push(Line::from(vec![
                Span::styled(format!("{} ", icon), Style::default().fg(color)),
                Span::styled(label, Style::default().fg(Color::Rgb(140, 140, 150))),
                Span::styled(" via ", Style::default().fg(Color::Rgb(100, 100, 110))),
                Span::styled(
                    upstream.clone(),
                    Style::default().fg(Color::Rgb(200, 180, 100)),
                ),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled(format!("{} ", icon), Style::default().fg(color)),
                Span::styled(label, Style::default().fg(Color::Rgb(140, 140, 150))),
            ]));
        }
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
                            format!(
                                " ({}↑ {}↓)",
                                format_tokens(info.input_tokens),
                                format_tokens(info.output_tokens)
                            ),
                            Style::default().fg(Color::Rgb(120, 120, 130)),
                        ),
                    ]));

                    // Cache info if available
                    if info.cache_read_tokens.is_some() || info.cache_write_tokens.is_some() {
                        let cache_read = info.cache_read_tokens.unwrap_or(0);
                        let cache_write = info.cache_write_tokens.unwrap_or(0);
                        lines.push(Line::from(vec![
                            Span::styled("🗄 ", Style::default().fg(Color::Rgb(100, 180, 100))),
                            Span::styled(
                                format!("hit: {}", format_tokens(cache_read)),
                                Style::default().fg(Color::Rgb(100, 180, 100)),
                            ),
                            Span::styled(
                                format!(" write: {}", format_tokens(cache_write)),
                                Style::default().fg(Color::Rgb(255, 160, 80)),
                            ),
                        ]));
                    }

                    if let Some(tps) = info.output_tps {
                        lines.push(Line::from(vec![
                            Span::styled("⏱ ", Style::default().fg(Color::Rgb(120, 170, 220))),
                            Span::styled(
                                format!("{:.1} tps", tps),
                                Style::default().fg(Color::Rgb(140, 140, 150)),
                            ),
                        ]));
                    }
                }
                _ => {
                    // Subscription usage bars
                    let five_hr_used = (info.five_hour * 100.0).round().clamp(0.0, 100.0) as u8;
                    let seven_day_used = (info.seven_day * 100.0).round().clamp(0.0, 100.0) as u8;
                    let five_hr_left = 100u8.saturating_sub(five_hr_used);
                    let seven_day_left = 100u8.saturating_sub(seven_day_used);

                    lines.push(render_labeled_bar(
                        "5hr",
                        five_hr_used,
                        five_hr_left,
                        None,
                        inner.width,
                    ));
                    lines.push(render_labeled_bar(
                        "7d",
                        seven_day_used,
                        seven_day_left,
                        None,
                        inner.width,
                    ));

                    if let Some(tps) = info.output_tps {
                        lines.push(Line::from(vec![
                            Span::styled("⏱ ", Style::default().fg(Color::Rgb(120, 170, 220))),
                            Span::styled(
                                format!("{:.1} tps", tps),
                                Style::default().fg(Color::Rgb(140, 140, 150)),
                            ),
                        ]));
                    }
                }
            }
        }
    }

    if let Some(tps) = data.tokens_per_second {
        if tps.is_finite() && tps > 0.1 {
            lines.push(Line::from(vec![
                Span::styled("⏱ ", Style::default().fg(Color::Rgb(140, 180, 255))),
                Span::styled(
                    format!("{:.1} t/s", tps),
                    Style::default().fg(Color::Rgb(140, 140, 150)),
                ),
            ]));
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
        let mut lines = 1;
        if data.session_count.is_some() || data.session_name.is_some() {
            lines += 1;
        }
        lines
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
        if info.subagent_status.is_some()
            || info.session_count > 1
            || info.client_count.is_some()
            || !info.members.is_empty()
        {
            return 1;
        }
    }
    0
}

fn expanded_swarm_height(data: &InfoWidgetData) -> u16 {
    if let Some(info) = &data.swarm_info {
        if info.subagent_status.is_some()
            || info.session_count > 1
            || info.client_count.is_some()
            || !info.members.is_empty()
        {
            // Title (1) + status line (1) + session list (up to 4)
            let mut height = 2u16;
            if info.subagent_status.is_some() {
                height += 1; // Active subagent line
            }
            // Show session names (up to 4)
            let member_len = if info.members.is_empty() {
                info.session_names.len()
            } else {
                info.members.len()
            };
            height += member_len.min(4) as u16;
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
        if info.subagent_status.is_some()
            || info.session_count > 1
            || info.client_count.is_some()
            || !info.members.is_empty()
        {
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
        let is_blocked = !todo.blocked_by.is_empty();
        let (icon, status_color) = if is_blocked && todo.status != "completed" {
            ("⊳", Color::Rgb(180, 140, 100))
        } else {
            match todo.status.as_str() {
                "completed" => ("✓", Color::Rgb(100, 180, 100)),
                "in_progress" => ("▶", Color::Rgb(255, 200, 100)),
                "cancelled" => ("✗", Color::Rgb(120, 80, 80)),
                _ => ("○", Color::Rgb(120, 120, 130)),
            }
        };

        // Priority indicator
        let priority_marker = match todo.priority.as_str() {
            "high" => ("!", Color::Rgb(255, 120, 100)),
            "medium" => ("", Color::Rgb(200, 180, 100)),
            _ => ("", Color::Rgb(120, 120, 130)),
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
            Color::Rgb(100, 100, 110)
        } else if is_blocked {
            Color::Rgb(120, 120, 130)
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

        if !suffix.is_empty() {
            spans.push(Span::styled(
                suffix.to_string(),
                Style::default().fg(Color::Rgb(100, 100, 110)),
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
            Style::default().fg(Color::Rgb(100, 100, 110)),
        )]));
    }

    lines
}

/// Truncate string smartly, trying to break at word boundaries
fn truncate_smart(s: &str, max_len: usize) -> String {
    let char_len = s.chars().count();
    if char_len <= max_len {
        return s.to_string();
    }
    if max_len <= 3 {
        return "...".to_string();
    }

    let target = max_len - 3;
    let prefix = truncate_chars(s, target);

    // Try to find a word boundary
    if let Some(pos) = prefix.rfind(' ') {
        let before = &prefix[..pos];
        let pos_chars = before.chars().count();
        if pos_chars > target / 2 {
            return format!("{}...", before);
        }
    }
    format!("{}...", prefix)
}

/// Truncate to a maximum character count without splitting UTF-8 codepoints.
fn truncate_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// Truncate to a maximum character count and append an ellipsis if needed.
fn truncate_with_ellipsis(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    if max_chars == 1 {
        return "…".to_string();
    }
    let truncated = truncate_chars(s, max_chars.saturating_sub(1));
    format!("{}…", truncated)
}

// ---------------------------------------------------------------------------
// Tips widget — rotating helpful tips and keyboard shortcuts
// ---------------------------------------------------------------------------

const TIP_CYCLE_SECONDS: u64 = 15;

struct Tip {
    text: String,
}

fn all_tips() -> Vec<Tip> {
    [
        "Ctrl+J / Ctrl+K to scroll chat up and down",
        "Ctrl+U / Ctrl+D to scroll half a page at a time",
        "```mermaid code blocks render as diagrams",
        "Swarms form automatically when multiple sessions share a repo — they coordinate plans, share context, and track file conflicts",
        "Memories are stored in a graph with semantic embeddings — recall finds related facts even if you use different words",
        "Ambient mode runs background cycles while you're away — maintaining memories, compacting context, and doing proactive work",
        "Ambient cycles can email you a summary and you can reply with directives for the next run",
        "Pro setup: Arch Linux + Kitty terminal with 'copy on select' enabled — highlight text to copy instantly, no Ctrl+C needed",
    ]
    .iter()
    .map(|t| Tip {
        text: t.to_string(),
    })
    .collect()
}

static TIP_STATE: Mutex<Option<(usize, Instant)>> = Mutex::new(None);

fn current_tip(_max_width: usize) -> Tip {
    let tips = all_tips();
    let mut guard = TIP_STATE.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    let (idx, _last) = guard.get_or_insert_with(|| (0, now));

    let should_advance = now.duration_since(*_last).as_secs() >= TIP_CYCLE_SECONDS;
    if should_advance {
        *idx = (*idx + 1) % tips.len();
        *_last = now;
    }

    let i = *idx % tips.len();
    drop(guard);
    Tip {
        text: tips[i].text.clone(),
    }
}

fn wrap_tip_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let mut remaining = text;
    while !remaining.is_empty() {
        if remaining.len() <= width {
            lines.push(remaining.to_string());
            break;
        }
        let mut boundary = width.min(remaining.len());
        while boundary > 0 && !remaining.is_char_boundary(boundary) {
            boundary -= 1;
        }
        let split = remaining[..boundary].rfind(' ').unwrap_or(boundary);
        let (line, rest) = remaining.split_at(split);
        lines.push(line.to_string());
        remaining = rest.trim_start();
    }
    lines
}

fn render_tips_widget(inner: Rect) -> Vec<Line<'static>> {
    let w = inner.width.saturating_sub(2) as usize; // 2-char indent on tip lines
    let tip = current_tip(w);
    let wrapped = wrap_tip_text(&tip.text, w);

    let mut lines: Vec<Line<'static>> = Vec::new();

    // Header line: icon + "Did you know?"
    lines.push(Line::from(vec![
        Span::styled("💡 ", Style::default().fg(Color::Rgb(255, 210, 80))),
        Span::styled(
            "Did you know?",
            Style::default()
                .fg(Color::Rgb(200, 200, 210))
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    // Tip text lines
    for line_text in wrapped {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(line_text, Style::default().fg(Color::Rgb(160, 160, 175))),
        ]));
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::{
        calculate_placements, render_memory_topology_lines, render_memory_widget, truncate_smart,
        BackgroundInfo, GraphEdge, GraphNode, InfoWidgetData, Margins, MemoryInfo, SwarmInfo,
        UsageInfo, UsageProvider, WidgetKind,
    };
    use ratatui::layout::Rect;

    #[test]
    fn truncate_smart_handles_unicode() {
        let s = "eagle running — keep going";
        let out = truncate_smart(s, 15);
        assert_eq!(out, "eagle runnin...");
    }

    fn node(kind: &str, label: &str, degree: usize) -> GraphNode {
        GraphNode {
            id: format!("{}:{}", kind, label.replace(' ', "_")),
            label: label.to_string(),
            kind: kind.to_string(),
            is_memory: kind != "tag" && kind != "cluster",
            is_active: true,
            confidence: 0.9,
            degree,
        }
    }

    fn edge(source: usize, target: usize, kind: &str) -> GraphEdge {
        GraphEdge {
            source,
            target,
            kind: kind.to_string(),
        }
    }

    #[test]
    fn topology_lines_render_hub_and_edges() {
        let info = MemoryInfo {
            total_count: 4,
            graph_nodes: vec![
                node("fact", "Rust project uses cargo", 2),
                node("preference", "User likes concise answers", 2),
                node("correction", "Use oauth flow", 1),
                node("tag", "rust", 1),
            ],
            graph_edges: vec![
                edge(0, 1, "relates_to"),
                edge(1, 2, "contradicts"),
                edge(1, 3, "has_tag"),
            ],
            ..Default::default()
        };

        let lines = render_memory_topology_lines(&info, Rect::new(0, 0, 30, 3));
        assert!(!lines.is_empty());

        let text = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("hub"));
        assert!(text.contains("↳"));
    }

    #[test]
    fn memory_widget_uses_full_graph_height_when_idle() {
        let info = MemoryInfo {
            total_count: 3,
            graph_nodes: vec![
                node("fact", "build uses release binary", 1),
                node("preference", "Prefer small commits", 2),
                node("tag", "workflow", 1),
            ],
            graph_edges: vec![edge(0, 1, "relates_to"), edge(1, 2, "has_tag")],
            ..Default::default()
        };
        let data = InfoWidgetData {
            memory_info: Some(info),
            ..Default::default()
        };

        // Memory widget is text-only.
        let lines = render_memory_widget(&data, Rect::new(0, 0, 24, 5));
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn contextual_subgraph_prefers_memory_hub() {
        let mut nodes = vec![
            node("fact", "core build flow", 6),
            node("preference", "use cargo test", 4),
            node("tag", "rust", 5),
            node("tag", "testing", 3),
            node("fact", "docs in readme", 1),
        ];
        nodes[0].is_active = true;
        nodes[0].confidence = 0.95;

        let info = MemoryInfo {
            total_count: 5,
            graph_nodes: nodes,
            graph_edges: vec![
                edge(0, 1, "relates_to"),
                edge(0, 2, "has_tag"),
                edge(1, 3, "has_tag"),
                edge(4, 2, "has_tag"),
            ],
            ..Default::default()
        };

        let subgraph = super::select_contextual_subgraph(&info, 3, 6).expect("subgraph");
        assert_eq!(subgraph.nodes.len(), 3);
        assert!(subgraph
            .nodes
            .iter()
            .any(|n| n.label.contains("core build flow")));
    }

    #[test]
    fn overview_requires_multiple_sections() {
        let one_section = InfoWidgetData {
            model: Some("gpt-test".to_string()),
            ..Default::default()
        };
        assert!(!one_section.has_data_for(WidgetKind::Overview));

        let two_sections = InfoWidgetData {
            model: Some("gpt-test".to_string()),
            queue_mode: Some(true),
            ..Default::default()
        };
        assert!(two_sections.has_data_for(WidgetKind::Overview));
    }

    #[test]
    fn overview_widget_is_placed_when_space_allows() {
        {
            let mut guard = super::get_or_init_state();
            if let Some(state) = guard.as_mut() {
                state.enabled = true;
                state.placements.clear();
                state.widget_states.clear();
            }
        }

        let data = InfoWidgetData {
            model: Some("gpt-test".to_string()),
            queue_mode: Some(true),
            ..Default::default()
        };
        let margins = Margins {
            right_widths: vec![40; 20],
            left_widths: Vec::new(),
            centered: false,
        };
        let placements = calculate_placements(Rect::new(0, 0, 80, 20), &margins, &data);
        assert!(
            placements.iter().any(|p| p.kind == WidgetKind::Overview),
            "expected overview widget placement"
        );
    }

    #[test]
    fn sticky_placement_clamps_width_to_current_margin() {
        {
            let mut guard = super::get_or_init_state();
            if let Some(state) = guard.as_mut() {
                state.enabled = true;
                state.placements.clear();
                state.widget_states.clear();
            }
        }

        let data = InfoWidgetData {
            model: Some("gpt-test".to_string()),
            queue_mode: Some(true),
            ..Default::default()
        };
        let area = Rect::new(0, 0, 100, 10);

        // First frame places a wide widget.
        let first = calculate_placements(
            area,
            &Margins {
                right_widths: vec![30; 10],
                left_widths: Vec::new(),
                centered: false,
            },
            &data,
        );
        assert!(!first.is_empty(), "expected initial placement");
        assert_eq!(first[0].rect.width, 30);

        // Second frame shrinks margin by 4 columns (within sticky tolerance).
        let second_margins = vec![26; 10];
        let second = calculate_placements(
            area,
            &Margins {
                right_widths: second_margins.clone(),
                left_widths: Vec::new(),
                centered: false,
            },
            &data,
        );
        assert!(!second.is_empty(), "expected sticky placement");

        let p = &second[0];
        let row_start = p.rect.y.saturating_sub(area.y) as usize;
        let row_end = row_start + p.rect.height as usize;
        let min_margin = second_margins[row_start..row_end]
            .iter()
            .copied()
            .min()
            .unwrap_or(0);
        assert!(
            p.rect.width <= min_margin,
            "sticky width {} exceeded current margin {}",
            p.rect.width,
            min_margin
        );
    }

    #[test]
    fn placements_never_include_border_only_widgets() {
        {
            let mut guard = super::get_or_init_state();
            if let Some(state) = guard.as_mut() {
                state.enabled = true;
                state.placements.clear();
                state.widget_states.clear();
            }
        }

        let data = InfoWidgetData {
            model: Some("gpt-test".to_string()),
            session_count: Some(2),
            context_info: Some(crate::prompt::ContextInfo {
                system_prompt_chars: 24_000,
                total_chars: 40_000,
                ..Default::default()
            }),
            todos: vec![crate::todo::TodoItem {
                content: "ship patch".to_string(),
                status: "in_progress".to_string(),
                priority: "high".to_string(),
                id: "todo-1".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
            }],
            queue_mode: Some(true),
            memory_info: Some(MemoryInfo {
                total_count: 1,
                ..Default::default()
            }),
            swarm_info: Some(SwarmInfo {
                session_count: 2,
                ..Default::default()
            }),
            background_info: Some(BackgroundInfo {
                running_count: 1,
                running_tasks: vec!["bash".to_string()],
                ..Default::default()
            }),
            usage_info: Some(UsageInfo {
                provider: UsageProvider::Anthropic,
                five_hour: 0.35,
                seven_day: 0.62,
                available: true,
                ..Default::default()
            }),
            ..Default::default()
        };

        let placements = calculate_placements(
            Rect::new(0, 0, 100, 10),
            &Margins {
                right_widths: vec![40; 10],
                left_widths: Vec::new(),
                centered: false,
            },
            &data,
        );

        assert!(
            placements.iter().all(|p| p.rect.height > 2),
            "found border-only widget placement: {:?}",
            placements
        );
    }
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
            MemoryState::Maintaining { .. } => {
                spans.push(Span::styled(
                    " · ",
                    Style::default().fg(Color::Rgb(100, 100, 110)),
                ));
                spans.push(Span::styled(
                    "🌿",
                    Style::default().fg(Color::Rgb(120, 220, 180)),
                ));
            }
            MemoryState::ToolAction { action, .. } => {
                let icon = match action.as_str() {
                    "remember" | "store" => "💾",
                    "recall" | "search" => "🔍",
                    "forget" => "🗑\u{fe0f}",
                    "tag" => "🏷\u{fe0f}",
                    "link" => "🔗",
                    _ => "🧠",
                };
                spans.push(Span::styled(
                    " · ",
                    Style::default().fg(Color::Rgb(100, 100, 110)),
                ));
                spans.push(Span::styled(
                    icon,
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
            .map(|(cat, count)| format!("{}:{}", truncate_chars(cat, 3), count))
            .collect();
        cat_parts.sort();
        let cat_str = cat_parts.join(" ");
        let cat_display = truncate_with_ellipsis(&cat_str, max_width);
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
            MemoryState::Maintaining { phase } => Line::from(vec![
                Span::styled("🌿 ", Style::default().fg(Color::Rgb(120, 220, 180))),
                Span::styled(
                    truncate_smart(
                        &format!("Maintaining ({})", phase),
                        inner.width.saturating_sub(4) as usize,
                    ),
                    Style::default().fg(Color::Rgb(180, 180, 190)),
                ),
            ]),
            MemoryState::ToolAction { action, detail } => {
                let icon = match action.as_str() {
                    "remember" | "store" => "💾",
                    "recall" | "search" => "🔍",
                    "forget" => "🗑\u{fe0f}",
                    "tag" => "🏷\u{fe0f}",
                    "link" => "🔗",
                    "list" | "related" => "📋",
                    _ => "🧠",
                };
                let text = if detail.is_empty() {
                    action.clone()
                } else {
                    format!("{}: {}", action, detail)
                };
                Line::from(vec![
                    Span::styled(
                        format!("{} ", icon),
                        Style::default().fg(Color::Rgb(200, 150, 255)),
                    ),
                    Span::styled(
                        truncate_smart(&text, inner.width.saturating_sub(4) as usize),
                        Style::default().fg(Color::Rgb(200, 200, 210)),
                    ),
                ])
            }
        };
        lines.push(state_line);

        // Recent events
        let max_width = inner.width.saturating_sub(4) as usize;
        for event in activity.recent_events.iter().take(MAX_MEMORY_EVENTS) {
            if let MemoryEventKind::MemoryInjected {
                count,
                prompt_chars,
                age_ms,
                preview,
                items,
            } = &event.kind
            {
                let plural = if *count == 1 { "memory" } else { "memories" };
                let summary = truncate_with_ellipsis(
                    &format!(
                        "Injected {} {} ({}c, {}ms)",
                        count, plural, prompt_chars, age_ms
                    ),
                    max_width,
                );
                lines.push(Line::from(vec![
                    Span::styled("  ↳ ", Style::default().fg(Color::Rgb(140, 210, 255))),
                    Span::styled(summary, Style::default().fg(Color::Rgb(140, 140, 150))),
                ]));

                if items.is_empty() {
                    let fallback = truncate_with_ellipsis(preview, max_width.saturating_sub(4));
                    lines.push(Line::from(vec![
                        Span::styled("    • ", Style::default().fg(Color::Rgb(170, 170, 180))),
                        Span::styled(fallback, Style::default().fg(Color::Rgb(140, 140, 150))),
                    ]));
                } else {
                    let max_items = 5usize;
                    for item in items.iter().take(max_items) {
                        let label = truncate_with_ellipsis(&item.section, 16);
                        let text = truncate_with_ellipsis(
                            &format!("[{}] {}", label, item.content),
                            max_width.saturating_sub(4),
                        );
                        lines.push(Line::from(vec![
                            Span::styled("    • ", Style::default().fg(Color::Rgb(170, 170, 180))),
                            Span::styled(text, Style::default().fg(Color::Rgb(140, 140, 150))),
                        ]));
                    }
                    if items.len() > max_items {
                        lines.push(Line::from(vec![
                            Span::styled("    … ", Style::default().fg(Color::Rgb(120, 120, 130))),
                            Span::styled(
                                format!("+{} more", items.len() - max_items),
                                Style::default().fg(Color::Rgb(120, 120, 130)),
                            ),
                        ]));
                    }
                }
                continue;
            }

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
                    let preview =
                        truncate_with_ellipsis(memory_preview, max_width.saturating_sub(4));
                    ("✓", preview, Color::Rgb(100, 200, 100))
                }
                MemoryEventKind::SidecarNotRelevant => {
                    ("✗", "Not relevant".to_string(), Color::Rgb(150, 150, 160))
                }
                MemoryEventKind::SidecarComplete { latency_ms } => {
                    ("⏱", format!("{}ms", latency_ms), Color::Rgb(140, 140, 150))
                }
                MemoryEventKind::MemorySurfaced { memory_preview } => {
                    let preview =
                        truncate_with_ellipsis(memory_preview, max_width.saturating_sub(4));
                    ("★", preview, Color::Rgb(255, 220, 100))
                }
                MemoryEventKind::MemoryInjected {
                    count,
                    prompt_chars,
                    age_ms,
                    preview,
                    items: _,
                } => {
                    let msg = truncate_with_ellipsis(
                        &format!(
                            "{} mem ({}c, {}ms) {}",
                            count, prompt_chars, age_ms, preview
                        ),
                        max_width.saturating_sub(4),
                    );
                    ("↳", msg, Color::Rgb(140, 210, 255))
                }
                MemoryEventKind::MaintenanceStarted { verified, rejected } => (
                    "🌿",
                    format!("maintain v{} r{}", verified, rejected),
                    Color::Rgb(120, 220, 180),
                ),
                MemoryEventKind::MaintenanceLinked { links } => {
                    ("⇄", format!("{} links", links), Color::Rgb(120, 220, 180))
                }
                MemoryEventKind::MaintenanceConfidence { boosted, decayed } => (
                    "±",
                    format!("conf +{} -{}", boosted, decayed),
                    Color::Rgb(120, 220, 180),
                ),
                MemoryEventKind::MaintenanceCluster { clusters, members } => (
                    "◈",
                    format!("clusters {} ({} mem)", clusters, members),
                    Color::Rgb(120, 220, 180),
                ),
                MemoryEventKind::MaintenanceTagInferred { tag, applied } => (
                    "#",
                    format!("tag {} (+{})", truncate_with_ellipsis(tag, 20), applied),
                    Color::Rgb(120, 220, 180),
                ),
                MemoryEventKind::MaintenanceGap { candidates } => (
                    "?",
                    format!("gap after {} candidates", candidates),
                    Color::Rgb(220, 180, 120),
                ),
                MemoryEventKind::MaintenanceComplete { latency_ms } => (
                    "✓",
                    format!("maint {}ms", latency_ms),
                    Color::Rgb(120, 220, 180),
                ),
                MemoryEventKind::ExtractionStarted { reason } => {
                    let msg = truncate_with_ellipsis(reason, max_width.saturating_sub(4));
                    (
                        "🧠",
                        format!("Extracting: {}", msg),
                        Color::Rgb(200, 150, 255),
                    )
                }
                MemoryEventKind::ExtractionComplete { count } => (
                    "✓",
                    format!("Saved {} memories", count),
                    Color::Rgb(100, 200, 100),
                ),
                MemoryEventKind::Error { message } => {
                    let msg = truncate_with_ellipsis(message, max_width.saturating_sub(4));
                    ("!", msg, Color::Rgb(255, 100, 100))
                }
                MemoryEventKind::ToolRemembered {
                    content,
                    scope,
                    category,
                } => {
                    let msg = truncate_with_ellipsis(
                        &format!("[{}] {} ({})", category, content, scope),
                        max_width.saturating_sub(4),
                    );
                    ("💾", msg, Color::Rgb(100, 200, 100))
                }
                MemoryEventKind::ToolRecalled { query, count } => {
                    let msg = truncate_with_ellipsis(
                        &format!("{} found for '{}'", count, query),
                        max_width.saturating_sub(4),
                    );
                    ("🔍", msg, Color::Rgb(140, 180, 255))
                }
                MemoryEventKind::ToolForgot { id } => {
                    let msg = truncate_with_ellipsis(id, max_width.saturating_sub(4));
                    ("🗑\u{fe0f}", msg, Color::Rgb(255, 170, 100))
                }
                MemoryEventKind::ToolTagged { id, tags } => {
                    let msg = truncate_with_ellipsis(
                        &format!("{} +{}", id, tags),
                        max_width.saturating_sub(4),
                    );
                    ("🏷\u{fe0f}", msg, Color::Rgb(140, 200, 255))
                }
                MemoryEventKind::ToolLinked { from, to } => {
                    let msg = truncate_with_ellipsis(
                        &format!("{} → {}", from, to),
                        max_width.saturating_sub(4),
                    );
                    ("🔗", msg, Color::Rgb(200, 180, 255))
                }
                MemoryEventKind::ToolListed { count } => (
                    "📋",
                    format!("{} memories", count),
                    Color::Rgb(140, 140, 150),
                ),
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

    // Show active member or subagent status first (most important)
    let active_member = info
        .members
        .iter()
        .find(|m| matches!(m.status.as_str(), "running" | "blocked" | "failed"));
    if let Some(member) = active_member {
        let (color, icon) = swarm_status_style(&member.status);
        spans.push(Span::styled(
            format!("{} ", icon),
            Style::default().fg(color),
        ));
        let detail = member.detail.as_deref().unwrap_or(member.status.as_str());
        let label = format!("{} {}", swarm_member_label(member), detail);
        spans.push(Span::styled(
            truncate_smart(&label, 20),
            Style::default().fg(Color::Rgb(180, 180, 190)),
        ));
    } else if let Some(status) = &info.subagent_status {
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
        if !spans.is_empty() {
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

    // Active subagent status (only when we don't have member status lines)
    if info.members.is_empty() {
        if let Some(status) = &info.subagent_status {
            lines.push(Line::from(vec![
                Span::styled("▶ ", Style::default().fg(Color::Rgb(255, 200, 100))),
                Span::styled(
                    truncate_smart(status, inner.width.saturating_sub(4) as usize),
                    Style::default().fg(Color::Rgb(200, 200, 210)),
                ),
            ]));
        }
    }

    let max_name_len = inner.width.saturating_sub(8) as usize;
    if !info.members.is_empty() {
        let remaining_height = inner.height.saturating_sub(lines.len() as u16) as usize;
        let need_graph = remaining_height >= info.members.len() + 3;

        if need_graph {
            // Graph view: coordinator on top, connector, agents below
            let coordinator = info
                .members
                .iter()
                .find(|m| m.role.as_deref() == Some("coordinator"));
            let agents: Vec<_> = info
                .members
                .iter()
                .filter(|m| m.role.as_deref() != Some("coordinator"))
                .collect();

            if let Some(coord) = coordinator {
                let coord_label = swarm_member_label(coord);
                let (color, icon) = swarm_status_style(&coord.status);
                lines.push(Line::from(vec![
                    Span::styled("★ ", Style::default().fg(Color::Rgb(255, 200, 100))),
                    Span::styled(format!("{} ", icon), Style::default().fg(color)),
                    Span::styled(
                        truncate_smart(&coord_label, max_name_len),
                        Style::default().fg(Color::Rgb(200, 200, 210)),
                    ),
                ]));

                // Connector line
                if !agents.is_empty() {
                    let connector_width = inner.width.saturating_sub(4).min(20) as usize;
                    let connector = format!(
                        "  {}",
                        "├".to_string() + &"─".repeat(connector_width.saturating_sub(2)) + "┤"
                    );
                    lines.push(Line::from(vec![Span::styled(
                        connector,
                        Style::default().fg(Color::Rgb(80, 80, 90)),
                    )]));
                }
            }

            for agent in agents.iter().take(4) {
                lines.push(swarm_member_line(agent, max_name_len));
            }
            if agents.len() > 4 {
                let remaining = agents.len() - 4;
                lines.push(Line::from(vec![Span::styled(
                    format!("  +{} more", remaining),
                    Style::default().fg(Color::Rgb(100, 100, 110)),
                )]));
            }
        } else {
            // Flat list when not enough height for graph
            for member in info.members.iter().take(4) {
                lines.push(swarm_member_line(member, max_name_len));
            }
            if info.members.len() > 4 {
                let remaining = info.members.len() - 4;
                lines.push(Line::from(vec![Span::styled(
                    format!("  +{} more", remaining),
                    Style::default().fg(Color::Rgb(100, 100, 110)),
                )]));
            }
        }
    } else {
        // Session names (up to 4)
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
            if short_name.chars().count() > max_len.saturating_sub(2) {
                format!(
                    "{}...",
                    truncate_chars(&short_name, max_len.saturating_sub(5))
                )
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

    // Add session info line if we have session count/name.
    if data.session_count.is_some() || data.session_name.is_some() {
        let mut parts = Vec::new();

        if let Some(sessions) = data.session_count {
            parts.push(format!(
                "{} session{}",
                sessions,
                if sessions == 1 { "" } else { "s" }
            ));
        }

        if let Some(name) = data.session_name.as_deref() {
            if !name.trim().is_empty() {
                parts.push(name.to_string());
            }
        }

        if !parts.is_empty() {
            let detail = truncate_smart(&parts.join(" · "), max_len.saturating_sub(2));
            lines.push(Line::from(vec![Span::styled(
                detail,
                Style::default().fg(Color::Rgb(140, 140, 150)),
            )]));
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
    if info.total_chars == 0 && data.observed_context_tokens.is_none() {
        return Vec::new();
    }

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![Span::styled(
        "Context",
        Style::default().fg(Color::Rgb(180, 180, 190)).bold(),
    )]));

    let used_tokens = data
        .observed_context_tokens
        .map(|t| t as usize)
        .unwrap_or_else(|| info.estimated_tokens());
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
        if content.chars().count() > max_len && max_len > 3 {
            let truncated = truncate_chars(&content, max_len.saturating_sub(3));
            content = format!("{}...", truncated);
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
    if info.total_chars == 0 && data.observed_context_tokens.is_none() {
        return Vec::new();
    }

    let used_tokens = data
        .observed_context_tokens
        .map(|t| t as usize)
        .unwrap_or_else(|| info.estimated_tokens());
    let limit_tokens = data.context_limit.unwrap_or(DEFAULT_CONTEXT_LIMIT).max(1);
    let used_pct = ((used_tokens as f64 / limit_tokens as f64) * 100.0)
        .round()
        .clamp(0.0, 100.0) as u8;
    let left_pct = 100u8.saturating_sub(used_pct);

    vec![render_labeled_bar(
        "Context",
        used_pct,
        left_pct,
        None,
        inner.width,
    )]
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
