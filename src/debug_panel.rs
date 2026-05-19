//! # 可视化调试面板 (Debug Panel)
//!
//! 提供终端内嵌的可视化调试能力：
//! - 决策树 / 记忆图谱 / 性能图表 / 安全日志 等面板类型
//! - ASCII 和 Mermaid 双模式渲染
//! - 实时数据流展示与交互式展开/折叠
//! - 导出为文本或图片格式
//! - 键盘导航支持（上下移动、展开/折叠、搜索）

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fmt::Write as FmtWrite;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PanelId {
    DecisionTree,
    MemoryGraph,
    PerformanceChart,
    SecurityLog,
    TokenUsage,
    SwarmTopology,
}

impl PanelId {
    pub fn all() -> Vec<PanelId> {
        vec![
            PanelId::DecisionTree,
            PanelId::MemoryGraph,
            PanelId::PerformanceChart,
            PanelId::SecurityLog,
            PanelId::TokenUsage,
            PanelId::SwarmTopology,
        ]
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            PanelId::DecisionTree => "Decision Tree",
            PanelId::MemoryGraph => "Memory Graph",
            PanelId::PerformanceChart => "Performance Chart",
            PanelId::SecurityLog => "Security Log",
            PanelId::TokenUsage => "Token Usage",
            PanelId::SwarmTopology => "Swarm Topology",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DebugPanelType {
    DecisionTree,
    MemoryGraph,
    PerformanceChart,
    SecurityLog,
    TokenUsage,
    SwarmTopology,
}

impl From<PanelId> for DebugPanelType {
    fn from(id: PanelId) -> Self {
        match id {
            PanelId::DecisionTree => DebugPanelType::DecisionTree,
            PanelId::MemoryGraph => DebugPanelType::MemoryGraph,
            PanelId::PerformanceChart => DebugPanelType::PerformanceChart,
            PanelId::SecurityLog => DebugPanelType::SecurityLog,
            PanelId::TokenUsage => DebugPanelType::TokenUsage,
            PanelId::SwarmTopology => DebugPanelType::SwarmTopology,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RenderFormat {
    Ascii,
    Mermaid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExportFormat {
    Text,
    Png,
    Svg,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PanelDataPoint {
    pub label: String,
    pub value: f64,
    pub timestamp: u64,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeNode {
    pub id: String,
    pub label: String,
    pub children: Vec<TreeNode>,
    pub expanded: bool,
    pub depth: usize,
    pub metadata: HashMap<String, String>,
}

impl TreeNode {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            children: Vec::new(),
            expanded: false,
            depth: 0,
            metadata: HashMap::new(),
        }
    }

    pub fn add_child(mut self, child: TreeNode) -> Self {
        self.children.push(child);
        self
    }

    pub fn total_nodes(&self) -> usize {
        1 + self.children.iter().map(|c| c.total_nodes()).sum::<usize>()
    }

    /// Recursively set depth for this node and all children
    pub fn set_depth(&mut self, depth: usize) {
        self.depth = depth;
        for child in &mut self.children {
            child.set_depth(depth + 1);
        }
    }

    /// Render tree as ASCII with proper indentation
    pub fn render_ascii(&self, prefix: &str, is_last: bool) -> String {
        let mut output = String::new();
        let connector = if is_last { "└── " } else { "├── " };
        let expansion = if self.expanded && !self.children.is_empty() { "▼ " } else { "▶ " };
        
        writeln!(output, "{}{}{}{}", prefix, connector, expansion, self.label).unwrap();
        
        if self.expanded {
            let new_prefix = format!("{}{}", prefix, if is_last { "    " } else { "│   " });
            for (i, child) in self.children.iter().enumerate() {
                let is_last_child = i == self.children.len() - 1;
                output.push_str(&child.render_ascii(&new_prefix, is_last_child));
            }
        }
        
        output
    }

    /// Toggle expanded state
    pub fn toggle_expanded(&mut self) {
        self.expanded = !self.expanded;
    }

    /// Find node by ID
    pub fn find_node(&self, id: &str) -> Option<&TreeNode> {
        if self.id == id {
            return Some(self);
        }
        for child in &self.children {
            if let Some(found) = child.find_node(id) {
                return Some(found);
            }
        }
        None
    }

    /// Find mutable node by ID
    pub fn find_node_mut(&mut self, id: &str) -> Option<&mut TreeNode> {
        if self.id == id {
            return Some(self);
        }
        for child in &mut self.children {
            if let Some(found) = child.find_node_mut(id) {
                return Some(found);
            }
        }
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityEvent {
    pub level: SecurityLevel,
    pub message: String,
    pub source: String,
    pub timestamp: u64,
    pub details: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecurityLevel {
    Info,
    Warning,
    Critical,
}

impl SecurityLevel {
    pub fn icon(&self) -> &'static str {
        match self {
            SecurityLevel::Info => "[i]",
            SecurityLevel::Warning => "[!]",
            SecurityLevel::Critical => "[X]",
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            SecurityLevel::Info => "INFO",
            SecurityLevel::Warning => "WARN",
            SecurityLevel::Critical => "CRIT",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PanelRenderState {
    pub focused_panel: Option<PanelId>,
    pub scroll_offset: usize,
    pub selected_index: usize,
    pub filter_query: Option<String>,
    pub collapsed_panels: Vec<PanelId>,
}

#[derive(Debug, Clone, Default)]
pub struct MetricsCollector {
    data_points: HashMap<PanelId, VecDeque<PanelDataPoint>>,
    max_points_per_panel: usize,
}

impl MetricsCollector {
    pub fn new(max_points: usize) -> Self {
        Self {
            data_points: HashMap::new(),
            max_points_per_panel: max_points,
        }
    }

    pub fn record(&mut self, panel_id: PanelId, point: PanelDataPoint) {
        let queue = self.data_points.entry(panel_id).or_default();
        if queue.len() >= self.max_points_per_panel {
            queue.pop_front();
        }
        queue.push_back(point);
    }

    pub fn get_latest(&self, panel_id: &PanelId) -> Option<&PanelDataPoint> {
        self.data_points.get(panel_id).and_then(|q| q.back())
    }

    pub fn get_all(&self, panel_id: &PanelId) -> Vec<&PanelDataPoint> {
        self.data_points
            .get(panel_id)
            .map(|q| q.iter().collect())
            .unwrap_or_default()
    }

    pub fn summary(&self, panel_id: &PanelId) -> MetricSummary {
        let points = self.get_all(panel_id);
        if points.is_empty() {
            return MetricSummary { count: 0, min: 0.0, max: 0.0, avg: 0.0 };
        }
        let count = points.len();
        let min = points.iter().map(|p| p.value).fold(f64::INFINITY, f64::min);
        let max = points.iter().map(|p| p.value).fold(f64::NEG_INFINITY, f64::max);
        let sum: f64 = points.iter().map(|p| p.value).sum();
        MetricSummary { count, min, max, avg: sum / count as f64 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricSummary {
    pub count: usize,
    pub min: f64,
    pub max: f64,
    pub avg: f64,
}

#[derive(Debug, Clone)]
pub struct InteractionHandler {
    navigation_stack: Vec<PanelId>,
    search_buffer: String,
    mode: InteractionMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionMode {
    Normal,
    Search,
    Select,
}

impl Default for InteractionMode {
    fn default() -> Self {
        InteractionMode::Normal
    }
}

impl InteractionHandler {
    pub fn new() -> Self {
        Self {
            navigation_stack: Vec::new(),
            search_buffer: String::new(),
            mode: InteractionMode::Normal,
        }
    }

    pub fn push_navigation(&mut self, panel_id: PanelId) {
        if self.navigation_stack.last() != Some(&panel_id) {
            self.navigation_stack.push(panel_id);
        }
    }

    pub fn pop_navigation(&mut self) -> Option<PanelId> {
        self.navigation_stack.pop()
    }

    pub fn current_panel(&self) -> Option<&PanelId> {
        self.navigation_stack.last()
    }

    pub fn start_search(&mut self) {
        self.mode = InteractionMode::Search;
        self.search_buffer.clear();
    }

    pub fn end_search(&mut self) -> String {
        self.mode = InteractionMode::Normal;
        std::mem::take(&mut self.search_buffer)
    }

    pub fn type_char(&mut self, ch: char) {
        if self.mode == InteractionMode::Search {
            self.search_buffer.push(ch);
        }
    }

    pub fn backspace(&mut self) {
        if self.mode == InteractionMode::Search {
            self.search_buffer.pop();
        }
    }

    pub fn search_text(&self) -> &str {
        &self.search_buffer
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugPanel {
    pub id: PanelId,
    pub panel_type: DebugPanelType,
    pub title: String,
    pub tree_root: Option<TreeNode>,
    pub events: Vec<SecurityEvent>,
    pub data_series: Vec<PanelDataPoint>,
    pub is_visible: bool,
    pub is_expanded: bool,
}

impl DebugPanel {
    pub fn new(id: PanelId, panel_type: DebugPanelType) -> Self {
        Self {
            id,
            panel_type,
            title: id.display_name().to_string(),
            tree_root: None,
            events: Vec::new(),
            data_series: Vec::new(),
            is_visible: true,
            is_expanded: false,
        }
    }

    pub fn with_tree(mut self, root: TreeNode) -> Self {
        self.tree_root = Some(root);
        self
    }

    pub fn add_event(&mut self, event: SecurityEvent) {
        self.events.push(event);
    }

    pub fn add_data_point(&mut self, point: PanelDataPoint) {
        self.data_series.push(point);
    }

    pub fn toggle_expand(&mut self) {
        self.is_expanded = !self.is_expanded;
    }

    pub fn toggle_visibility(&mut self) {
        self.is_visible = !self.is_visible;
    }
}

#[derive(Debug, Clone)]
pub struct DebugPanelManager {
    panels: HashMap<PanelId, DebugPanel>,
    render_state: PanelRenderState,
    metrics_collector: MetricsCollector,
    interaction_handler: InteractionHandler,
}

impl DebugPanelManager {
    pub fn new() -> Self {
        let mut panels = HashMap::new();
        for id in PanelId::all() {
            panels.insert(id, DebugPanel::new(id, DebugPanelType::from(id)));
        }
        Self {
            panels,
            render_state: PanelRenderState::default(),
            metrics_collector: MetricsCollector::new(500),
            interaction_handler: InteractionHandler::new(),
        }
    }

    pub fn get_panel(&self, id: &PanelId) -> Option<&DebugPanel> {
        self.panels.get(id)
    }

    pub fn get_panel_mut(&mut self, id: &PanelId) -> Option<&mut DebugPanel> {
        self.panels.get_mut(id)
    }

    pub fn visible_panels(&self) -> Vec<&DebugPanel> {
        self.panels.values().filter(|p| p.is_visible).collect()
    }

    pub fn focus_panel(&mut self, id: PanelId) {
        self.render_state.focused_panel = Some(id);
        self.interaction_handler.push_navigation(id);
    }

    pub fn focused_panel_id(&self) -> Option<PanelId> {
        self.render_state.focused_panel
    }

    pub fn record_metric(&mut self, panel_id: PanelId, point: PanelDataPoint) {
        self.metrics_collector.record(panel_id, point.clone());
        if let Some(panel) = self.panels.get_mut(&panel_id) {
            panel.add_data_point(point);
        }
    }

    pub fn metrics_summary(&self, panel_id: &PanelId) -> MetricSummary {
        self.metrics_collector.summary(panel_id)
    }

    pub fn render_ascii(&self, panel_id: &PanelId) -> Option<String> {
        let panel = self.panels.get(panel_id)?;
        match panel.panel_type {
            DebugPanelType::DecisionTree | DebugPanelType::MemoryGraph | DebugPanelType::SwarmTopology => {
                panel.tree_root.as_ref().map(|root| self.render_tree_ascii(root))
            }
            DebugPanelType::PerformanceChart | DebugPanelType::TokenUsage => {
                Some(self.render_chart_ascii(&panel.data_series))
            }
            DebugPanelType::SecurityLog => Some(self.render_security_log_ascii(&panel.events)),
        }
    }

    pub fn render_mermaid(&self, panel_id: &PanelId) -> Option<String> {
        let panel = self.panels.get(panel_id)?;
        match panel.panel_type {
            DebugPanelType::DecisionTree | DebugPanelType::MemoryGraph | DebugPanelType::SwarmTopology => {
                panel.tree_root.as_ref().map(|root| self.render_tree_mermaid(root))
            }
            _ => None,
        }
    }

    fn render_tree_ascii(&self, node: &TreeNode) -> String {
        let mut output = String::new();
        self.render_node_ascii(node, "", true, &mut output);
        output
    }

    fn render_node_ascii(&self, node: &TreeNode, prefix: &str, is_last: bool, out: &mut String) {
        let connector = if is_last { "+-- " } else { "+-- " };
        let expanded_marker = if node.expanded { "▼" } else { "▶" };
        writeln!(out, "{}{}{} {}", prefix, connector, expanded_marker, node.label).ok();
        let new_prefix = format!("{}{}", prefix, if is_last { "    " } else { "|   " });
        for (i, child) in node.children.iter().enumerate() {
            self.render_node_ascii(child, &new_prefix, i == node.children.len() - 1, out);
        }
    }

    fn render_tree_mermaid(&self, root: &TreeNode) -> String {
        let mut out = String::from("graph TD\n");
        self.collect_mermaid_edges(root, &mut out);
        out
    }

    fn collect_mermaid_edges(&self, node: &TreeNode, out: &mut String) {
        let safe_id: String = node.id.chars().filter(|c| c.is_alphanumeric()).collect();
        for child in &node.children {
            let _safe_child: String = child.id.chars().filter(|c| c.is_alphanumeric()).collect();
            writeln!(out, "{} --> \"{}\"", safe_id, child.label).ok();
            self.collect_mermaid_edges(child, out);
        }
    }

    fn render_chart_ascii(&self, data: &[PanelDataPoint]) -> String {
        if data.is_empty() {
            return "(no data)".to_string();
        }
        let max_val = data.iter().map(|d| d.value).fold(0.0_f64, f64::max).max(1.0);
        let height = 12usize;
        let width = 40usize;
        let mut out = String::new();
        writeln!(out, "{}", "-".repeat(width + 2)).ok();
        for row in (0..height).rev() {
            let threshold = max_val * row as f64 / height as f64;
            let mut line = String::from("|");
            for d in data.iter().take(width) {
                if d.value > threshold {
                    line.push('█');
                } else {
                    line.push(' ');
                }
            }
            line.push('|');
            writeln!(out, "{}", line).ok();
        }
        writeln!(out, "{}", "-".repeat(width + 2)).ok();
        out
    }

    fn render_security_log_ascii(&self, events: &[SecurityEvent]) -> String {
        if events.is_empty() {
            return "(no events)".to_string();
        }
        let mut out = String::new();
        writeln!(out, "{:<6} {:<8} {:<20} {}", "LEVEL", "SOURCE", "MESSAGE", "DETAILS").ok();
        writeln!(out, "{}", "-".repeat(78)).ok();
        for e in events {
            let detail = e.details.as_deref().unwrap_or("-");
            writeln!(out, "{} {:<8} {:<20} {} {}", e.level.icon(), e.level.as_str(), &e.source, &e.message, detail).ok();
        }
        out
    }

    pub fn export(&self, panel_id: &PanelId, format: ExportFormat) -> Result<String, String> {
        match format {
            ExportFormat::Text => {
                self.render_ascii(panel_id).ok_or("No panel data".to_string())
            }
            ExportFormat::Svg | ExportFormat::Png => {
                self.render_mermaid(panel_id)
                    .ok_or_else(|| "Mermaid not supported for this panel type".to_string())
            }
        }
    }

    pub fn handle_key(&mut self, key: KeyAction) -> Option<PanelAction> {
        match key {
            KeyAction::Up => {
                if self.render_state.selected_index > 0 {
                    self.render_state.selected_index -= 1;
                }
                None
            }
            KeyAction::Down => {
                let visible_count = self.visible_panels().len();
                if visible_count > 0 && self.render_state.selected_index < visible_count.saturating_sub(1) {
                    self.render_state.selected_index += 1;
                }
                None
            }
            KeyAction::Enter => {
                let visible = self.visible_panels();
                if let Some(panel) = visible.get(self.render_state.selected_index) {
                    Some(PanelAction::ToggleExpand(panel.id))
                } else {
                    None
                }
            }
            KeyAction::ToggleVisibility => {
                let visible = self.visible_panels();
                if let Some(panel) = visible.get(self.render_state.selected_index) {
                    Some(PanelAction::ToggleVisibility(panel.id))
                } else {
                    None
                }
            }
            KeyAction::SearchForward => {
                self.interaction_handler.start_search();
                None
            }
            KeyAction::Char(ch) => {
                self.interaction_handler.type_char(ch);
                None
            }
            KeyAction::Backspace => {
                self.interaction_handler.backspace();
                None
            }
            KeyAction::Escape => {
                if self.interaction_handler.mode == InteractionMode::Search {
                    let query = self.interaction_handler.end_search();
                    self.render_state.filter_query = if query.is_empty() { None } else { Some(query) };
                }
                None
            }
        }
    }

    pub fn execute_action(&mut self, action: PanelAction) {
        match action {
            PanelAction::ToggleExpand(id) => {
                if let Some(panel) = self.panels.get_mut(&id) {
                    panel.toggle_expand();
                }
            }
            PanelAction::ToggleVisibility(id) => {
                if let Some(panel) = self.panels.get_mut(&id) {
                    panel.toggle_visibility();
                }
            }
        }
    }

    pub fn collapse_all(&mut self) {
        for panel in self.panels.values_mut() {
            panel.is_expanded = false;
        }
    }

    pub fn expand_all(&mut self) {
        for panel in self.panels.values_mut() {
            panel.is_expanded = true;
        }
    }
}

impl Default for DebugPanelManager {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Up,
    Down,
    Enter,
    ToggleVisibility,
    SearchForward,
    Char(char),
    Backspace,
    Escape,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelAction {
    ToggleExpand(PanelId),
    ToggleVisibility(PanelId),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_panel_id_display_names() {
        assert_eq!(PanelId::DecisionTree.display_name(), "Decision Tree");
        assert_eq!(PanelId::SwarmTopology.display_name(), "Swarm Topology");
        assert_eq!(PanelId::TokenUsage.display_name(), "Token Usage");
    }

    #[test]
    fn test_panel_id_all_returns_six_variants() {
        let all = PanelId::all();
        assert_eq!(all.len(), 6);
        assert!(all.contains(&PanelId::SecurityLog));
    }

    #[test]
    fn test_debug_panel_manager_creation() {
        let mgr = DebugPanelManager::new();
        assert_eq!(mgr.panels.len(), 6);
        assert!(mgr.get_panel(&PanelId::DecisionTree).is_some());
        assert_eq!(mgr.visible_panels().len(), 6);
    }

    #[test]
    fn test_tree_node_building_and_counting() {
        let root = TreeNode::new("r", "root")
            .add_child(TreeNode::new("a", "alpha").add_child(TreeNode::new("a1", "a1")))
            .add_child(TreeNode::new("b", "beta"));
        assert_eq!(root.total_nodes(), 4);
        assert_eq!(root.children.len(), 2);
    }

    #[test]
    fn test_metrics_collector_record_and_summary() {
        let mut collector = MetricsCollector::new(10);
        collector.record(
            PanelId::PerformanceChart,
            PanelDataPoint { label: "cpu".into(), value: 80.0, timestamp: 1, metadata: HashMap::new() },
        );
        collector.record(
            PanelId::PerformanceChart,
            PanelDataPoint { label: "cpu".into(), value: 60.0, timestamp: 2, metadata: HashMap::new() },
        );
        let summary = collector.summary(&PanelId::PerformanceChart);
        assert_eq!(summary.count, 2);
        assert!((summary.avg - 70.0).abs() < f64::EPSILON);
        assert_eq!(summary.min, 60.0);
        assert_eq!(summary.max, 80.0);
    }

    #[test]
    fn test_metrics_collector_max_capacity_eviction() {
        let mut collector = MetricsCollector::new(3);
        for i in 0..5u64 {
            collector.record(
                PanelId::TokenUsage,
                PanelDataPoint { label: "tokens".into(), value: i as f64, timestamp: i, metadata: HashMap::new() },
            );
        }
        let summary = collector.summary(&PanelId::TokenUsage);
        assert_eq!(summary.count, 3);
        assert_eq!(summary.min, 2.0);
    }

    #[test]
    fn test_interaction_handler_search_flow() {
        let mut handler = InteractionHandler::new();
        handler.start_search();
        assert_eq!(handler.mode, InteractionMode::Search);
        handler.type_char('f');
        handler.type_char('o');
        handler.type_char('o');
        assert_eq!(handler.search_text(), "foo");
        handler.backspace();
        assert_eq!(handler.search_text(), "fo");
        let query = handler.end_search();
        assert_eq!(query, "fo");
        assert_eq!(handler.mode, InteractionMode::Normal);
    }

    #[test]
    fn test_security_level_display() {
        assert_eq!(SecurityLevel::Info.icon(), "[i]");
        assert_eq!(SecurityLevel::Critical.as_str(), "CRIT");
    }

    #[test]
    fn test_render_tree_ascii_output() {
        let root = TreeNode::new("1", "Root")
            .add_child(TreeNode::new("2", "ChildA"))
            .add_child(TreeNode::new("3", "ChildB"));
        let mgr = DebugPanelManager::new();
        let _rendered = mgr.render_node_ascii(&root, "", true, &mut String::new());
        let mut output = String::new();
        mgr.render_node_ascii(&root, "", true, &mut output);
        assert!(output.contains("Root"));
        assert!(output.contains("ChildA"));
        assert!(output.contains("ChildB"));
        assert!(output.contains("+--"));
        assert!(output.contains("+--"));
    }

    #[test]
    fn test_render_chart_ascii_empty() {
        let mgr = DebugPanelManager::new();
        let result = mgr.render_chart_ascii(&[]);
        assert_eq!(result, "(no data)");
    }

    #[test]
    fn test_export_text_format() {
        let mut mgr = DebugPanelManager::new();
        mgr.get_panel_mut(&PanelId::SecurityLog).unwrap().add_event(SecurityEvent {
            level: SecurityLevel::Info,
            message: "test event".into(),
            source: "unit_test".into(),
            timestamp: 1000,
            details: None,
        });
        let result = mgr.export(&PanelId::SecurityLog, ExportFormat::Text);
        assert!(result.is_ok());
        assert!(result.unwrap().contains("test event"));
    }

    #[test]
    fn test_toggle_expand_and_collapse_all() {
        let mut mgr = DebugPanelManager::new();
        mgr.execute_action(PanelAction::ToggleExpand(PanelId::DecisionTree));
        assert!(mgr.get_panel(&PanelId::DecisionTree).unwrap().is_expanded);
        mgr.collapse_all();
        assert!(!mgr.get_panel(&PanelId::DecisionTree).unwrap().is_expanded);
        mgr.expand_all();
        assert!(mgr.get_panel(&PanelId::DecisionTree).unwrap().is_expanded);
    }

    #[test]
    fn test_handle_key_navigation() {
        let mut mgr = DebugPanelManager::new();
        assert_eq!(mgr.render_state.selected_index, 0);
        mgr.handle_key(KeyAction::Down);
        assert_eq!(mgr.render_state.selected_index, 1);
        mgr.handle_key(KeyAction::Up);
        assert_eq!(mgr.render_state.selected_index, 0);
        mgr.handle_key(KeyAction::Up);
        assert_eq!(mgr.render_state.selected_index, 0);
    }
}
