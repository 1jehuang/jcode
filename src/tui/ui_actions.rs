use ratatui::{
    style::{Color, Style, Modifier},
    text::{Line, Span},
    layout::Rect,
    buffer::Buffer,
};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionType {
    Copy,
    Edit,
    Delete,
    Pin,
    Retry,
    Cancel,
    Expand,
    Collapse,
    OpenFile,
    RunCommand,
    Diff,
    Search,
    Filter,
    Export,
    Share,
    Bookmark,
    #[doc(hidden)]
    __Nonexhaustive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockType {
    Command,
    Output,
    Error,
    Info,
    Image,
    Diff,
    Markdown,
    Table,
    Code,
    Log,
    Mermaid,
}

#[derive(Debug, Clone)]
pub struct CommandBlock {
    pub id: Uuid,
    pub block_type: BlockType,
    pub content: String,
    pub title: Option<String>,
}

impl CommandBlock {
    pub fn new(block_type: BlockType, content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            block_type,
            content: content.into(),
            title: None,
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockTypeFilter {
    All,
    Only(&'static [BlockType]),
    Exclude(&'static [BlockType]),
}

impl BlockTypeFilter {
    pub fn matches(&self, block_type: BlockType) -> bool {
        match self {
            Self::All => true,
            Self::Only(types) => types.contains(&block_type),
            Self::Exclude(types) => !types.contains(&block_type),
        }
    }
}

#[derive(Debug, Clone)]
pub struct KeyBinding {
    pub key: char,
    pub modifiers: Vec<KeyModifier>,
}

impl KeyBinding {
    pub fn new(key: char) -> Self {
        Self { key, modifiers: Vec::new() }
    }

    pub fn with_ctrl(mut self) -> Self {
        self.modifiers.push(KeyModifier::Ctrl);
        self
    }

    pub fn with_alt(mut self) -> Self {
        self.modifiers.push(KeyModifier::Alt);
        self
    }

    pub fn display_label(&self) -> String {
        let mut parts = Vec::new();
        for m in &self.modifiers {
            parts.push(match m { KeyModifier::Ctrl => "Ctrl".to_string(), KeyModifier::Alt => "Alt".to_string(), KeyModifier::Shift => "Shift".to_string() });
        }
        parts.push(self.key.to_string());
        parts.join("+")
    }

    pub fn matches_key(&self, key: char, modifiers: &[KeyModifier]) -> bool {
        if self.key != key { return false; }
        if self.modifiers.len() != modifiers.len() { return false; }
        self.modifiers.iter().all(|m| modifiers.contains(m))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyModifier { Ctrl, Alt, Shift }

#[derive(Debug, Clone)]
pub struct ActionDefinition {
    pub action_type: ActionType,
    pub icon: char,
    pub label: String,
    pub default_shortcut: Option<KeyBinding>,
    pub applicable_block_types: Vec<BlockTypeFilter>,
    pub description: String,
}

impl ActionDefinition {
    pub fn new(
        action_type: ActionType,
        icon: char,
        label: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            action_type,
            icon,
            label: label.into(),
            default_shortcut: None,
            applicable_block_types: vec![BlockTypeFilter::All],
            description: description.into(),
        }
    }

    pub fn with_shortcut(mut self, binding: KeyBinding) -> Self {
        self.default_shortcut = Some(binding);
        self
    }

    pub fn for_block_types(mut self, filters: Vec<BlockTypeFilter>) -> Self {
        self.applicable_block_types = filters;
        self
    }

    pub fn applies_to(&self, block_type: BlockType) -> bool {
        self.applicable_block_types.is_empty() || self.applicable_block_types.iter().any(|f| f.matches(block_type))
    }
}

#[derive(Debug, Clone, Default)]
pub struct ActionRegistry {
    global_actions: Vec<ActionDefinition>,
}

impl ActionRegistry {
    pub fn new() -> Self {
        let mut registry = Self { global_actions: Vec::new() };
        registry.register_defaults();
        registry
    }

    fn register_defaults(&mut self) {
        let defaults = vec![
            ActionDefinition::new(ActionType::Copy, '⎘', "Copy", "Copy block content to clipboard")
                .with_shortcut(KeyBinding::new('c').with_ctrl()),
            ActionDefinition::new(ActionType::Edit, '✎', "Edit", "Edit block content inline")
                .with_shortcut(KeyBinding::new('e').with_ctrl())
                .for_block_types(vec![BlockTypeFilter::Only(&[BlockType::Command, BlockType::Code])]),
            ActionDefinition::new(ActionType::Delete, '✕', "Delete", "Remove this block")
                .with_shortcut(KeyBinding::new('d').with_ctrl()),
            ActionDefinition::new(ActionType::Pin, '📌', "Pin", "Pin block to sidebar"),
            ActionDefinition::new(ActionType::Retry, '↻', "Retry", "Re-execute this command")
                .for_block_types(vec![BlockTypeFilter::Only(&[BlockType::Command, BlockType::Error])]),
            ActionDefinition::new(ActionType::Cancel, '✗', "Cancel", "Cancel running operation")
                .for_block_types(vec![BlockTypeFilter::Only(&[BlockType::Command])]),
            ActionDefinition::new(ActionType::Expand, '▼', "Expand", "Show full content"),
            ActionDefinition::new(ActionType::Collapse, '▶', "Collapse", "Minimize display"),
            ActionDefinition::new(ActionType::OpenFile, '📂', "Open", "Open file in editor")
                .for_block_types(vec![BlockTypeFilter::Only(&[BlockType::Code, BlockType::Diff])]),
            ActionDefinition::new(ActionType::Diff, 'Δ', "Diff", "Show diff view")
                .for_block_types(vec![BlockTypeFilter::Only(&[BlockType::Code, BlockType::Command])]),
            ActionDefinition::new(ActionType::Search, '🔍', "Search", "Search within block"),
            ActionDefinition::new(ActionType::Export, '📤', "Export", "Export block to file"),
            ActionDefinition::new(ActionType::Bookmark, '★', "Bookmark", "Bookmark for quick access"),
        ];
        self.global_actions.extend(defaults);
    }

    pub fn actions_for_block(&self, block_type: BlockType) -> Vec<&ActionDefinition> {
        self.global_actions.iter().filter(|a| a.applies_to(block_type)).collect()
    }

    pub fn find_by_action_type(&self, action_type: &ActionType) -> Option<&ActionDefinition> {
        self.global_actions.iter().find(|a| &a.action_type == action_type)
    }

    pub fn find_by_shortcut(&self, binding: &KeyBinding) -> Option<&ActionDefinition> {
        self.global_actions.iter().find(|a| a.default_shortcut.as_ref().map_or(false, |s| s.key == binding.key && s.modifiers == binding.modifiers))
    }

    pub fn all_actions(&self) -> &[ActionDefinition] {
        &self.global_actions
    }

    pub fn register_action(&mut self, definition: ActionDefinition) {
        self.global_actions.push(definition);
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProjectContext {
    pub git_root: Option<std::path::PathBuf>,
    pub current_branch: Option<String>,
    pub open_files: Vec<std::path::PathBuf>,
    pub recent_commands: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SuggestedAction {
    pub label: String,
    pub icon: char,
    pub action: ActionType,
    pub confidence: f64,
    pub reason: String,
}

#[derive(Debug, Clone, Default)]
pub struct ContextActionsFactory;

impl ContextActionsFactory {
    pub fn generate_context_actions(
        &self,
        _block: &CommandBlock,
        _project_context: &ProjectContext,
    ) -> Vec<SuggestedAction> {
        Vec::new()
    }
}

#[derive(Debug, Clone, Default)]
pub struct AnalyzedContext {
    pub file_paths: Vec<std::path::PathBuf>,
    pub urls: Vec<String>,
    pub git_refs: Vec<String>,
    pub errors: Vec<(String, ErrorFixSuggestion)>,
    pub code_symbols: Vec<CodeSymbolRef>,
}

#[derive(Debug, Clone)]
pub struct ErrorFixSuggestion {
    pub pattern: String,
    pub fix_command: String,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct CodeSymbolRef {
    pub name: String,
    pub kind: SymbolKind,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind { Function, Class, Variable, Module, Type, Method }

fn extract_file_paths(_content: &str) -> Vec<std::path::PathBuf> { Vec::new() }
fn extract_urls(_content: &str) -> Vec<String> { Vec::new() }
fn extract_git_refs(_content: &str) -> Vec<String> { Vec::new() }
fn extract_error_patterns(_content: &str) -> Vec<(String, ErrorFixSuggestion)> { Vec::new() }
fn extract_code_symbols(_content: &str) -> Vec<CodeSymbolRef> { Vec::new() }

#[derive(Debug, Clone, Default)]
pub struct ContextAnalyzer;

impl ContextAnalyzer {
    pub fn analyze(block_content: &str) -> AnalyzedContext {
        AnalyzedContext {
            file_paths: extract_file_paths(block_content),
            urls: extract_urls(block_content),
            git_refs: extract_git_refs(block_content),
            errors: extract_error_patterns(block_content),
            code_symbols: extract_code_symbols(block_content),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct HoverState {
    hovered_action: Option<usize>,
    tooltip: Option<String>,
}

impl HoverState {
    pub fn set_hovered(&mut self, idx: Option<usize>) {
        self.hovered_action = idx;
    }

    pub fn hovered_index(&self) -> Option<usize> {
        self.hovered_action
    }

    pub fn set_tooltip(&mut self, tooltip: Option<String>) {
        self.tooltip = tooltip;
    }

    pub fn tooltip(&self) -> Option<&String> {
        self.tooltip.as_ref()
    }

    pub fn clear(&mut self) {
        self.hovered_action = None;
        self.tooltip = None;
    }
}

#[derive(Debug, Clone)]
pub struct RenderableAction {
    pub definition: ActionDefinition,
    pub is_hovered: bool,
    pub is_enabled: bool,
    pub position: Rect,
}

#[derive(Debug, Clone)]
pub enum ActionResult {
    Triggered { action: ActionType, block_id: Uuid },
    ShowTooltip { text: String, position: (u16, u16) },
    None,
}

pub struct ActionBarManager {
    actions_registry: ActionRegistry,
    context_analyzer: ContextAnalyzer,
    hover_state: HoverState,
}

impl Default for ActionBarManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ActionBarManager {
    pub fn new() -> Self {
        Self {
            actions_registry: ActionRegistry::new(),
            context_analyzer: ContextAnalyzer,
            hover_state: HoverState::default(),
        }
    }

    pub fn get_actions_for_block(&self, block: &CommandBlock) -> Vec<RenderableAction> {
        let definitions = self.actions_registry.actions_for_block(block.block_type);
        definitions.into_iter().enumerate().map(|(idx, def)| {
            RenderableAction {
                definition: def.clone(),
                is_hovered: self.hover_state.hovered_index() == Some(idx),
                is_enabled: true,
                position: Rect::default(),
            }
        }).collect()
    }

    pub fn handle_click(&mut self, action_idx: usize, block_id: Uuid) -> ActionResult {
        let actions: Vec<RenderableAction> = {
            let dummy = CommandBlock::new(BlockType::Command, "");
            self.get_actions_for_block(&dummy)
        };
        match actions.get(action_idx) {
            Some(renderable) => ActionResult::Triggered { action: renderable.definition.action_type.clone(), block_id },
            None => ActionResult::None,
        }
    }

    pub fn handle_shortcut(&mut self, binding: &KeyBinding, block_id: Uuid) -> Option<ActionResult> {
        self.actions_registry.find_by_shortcut(binding).map(|def| {
            ActionResult::Triggered { action: def.action_type.clone(), block_id }
        })
    }

    pub fn update_hover(&mut self, mouse_x: u16, mouse_y: u16, area: Rect) {
        if !area.contains(ratatui::layout::Position::new(mouse_x, mouse_y)) {
            self.hover_state.clear();
            return;
        }
        let action_width = 3u16;
        let max_actions = ((area.width) / action_width) as usize;
        if area.x <= mouse_x && mouse_x < area.x + action_width * max_actions as u16 {
            let idx = ((mouse_x - area.x) / action_width) as usize;
            self.hover_state.set_hovered(Some(idx));
            let actions = self.actions_registry.all_actions();
            if let Some(def) = actions.get(idx) {
                self.hover_state.set_tooltip(Some(def.description.clone()));
            }
        } else {
            self.hover_state.clear();
        }
    }

    pub fn render_action_bar(&self, actions: &[RenderableAction], area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 { return; }
        let mut x = area.x;
        let spans: Vec<Span<'_>> = actions.iter().flat_map(|action| {
            let style = if action.is_hovered {
                Style::default().fg(Color::White).bg(Color::Blue).add_modifier(Modifier::BOLD)
            } else if action.is_enabled {
                Style::default().fg(Color::Gray)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let icon_span = Span::styled(format!(" {} ", action.definition.icon), style);
            let sep = Span::styled(" ", Style::default());
            vec![icon_span, sep]
        }).collect();
        let width = spans.iter().map(|s| s.width() as u16).sum::<u16>();
        buf.set_line(area.x, area.y, &Line::from(spans), area.width.min(x - area.x + width));
    }

    pub fn registry(&self) -> &ActionRegistry {
        &self.actions_registry
    }

    pub fn registry_mut(&mut self) -> &mut ActionRegistry {
        &mut self.actions_registry
    }

    pub fn hover_state(&self) -> &HoverState {
        &self.hover_state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_command_block() -> CommandBlock {
        CommandBlock::new(BlockType::Command, "cargo test").with_title("Run tests")
    }

    #[test]
    fn command_block_new_sets_id_and_type() {
        let block = CommandBlock::new(BlockType::Output, "hello");
        assert_eq!(block.block_type, BlockType::Output);
        assert_eq!(block.content, "hello");
        assert!(block.title.is_none());
    }

    #[test]
    fn command_block_with_title() {
        let block = CommandBlock::new(BlockType::Info, "info msg").with_title("My Title");
        assert_eq!(block.title.as_deref(), Some("My Title"));
    }

    #[test]
    fn block_type_filter_all_matches_everything() {
        assert!(BlockTypeFilter::All.matches(BlockType::Command));
        assert!(BlockTypeFilter::All.matches(BlockType::Error));
    }

    #[test]
    fn block_type_filter_only_matches_listed() {
        let filter = BlockTypeFilter::Only(&[BlockType::Command, BlockType::Output]);
        assert!(filter.matches(BlockType::Command));
        assert!(!filter.matches(BlockType::Image));
    }

    #[test]
    fn block_type_filter_exclude_omits_listed() {
        let filter = BlockTypeFilter::Exclude(&[BlockType::Error]);
        assert!(filter.matches(BlockType::Command));
        assert!(!filter.matches(BlockType::Error));
    }

    #[test]
    fn key_binding_display_label() {
        let kb = KeyBinding::new('c').with_ctrl();
        assert_eq!(kb.display_label(), "Ctrl+c");
        let kb2 = KeyBinding::new('a').with_alt().with_ctrl();
        assert!(kb2.display_label().contains("Ctrl"));
        assert!(kb2.display_label().contains("Alt"));
    }

    #[test]
    fn key_binding_matches_key() {
        let kb = KeyBinding::new('c').with_ctrl();
        assert!(kb.matches_key('c', &[KeyModifier::Ctrl]));
        assert!(!kb.matches_key('c', &[]));
        assert!(!kb.matches_key('x', &[KeyModifier::Ctrl]));
    }

    #[test]
    fn action_definition_applies_to_correct_blocks() {
        let def = ActionDefinition::new(ActionType::Copy, '⎘', "Copy", "")
            .for_block_types(vec![BlockTypeFilter::Only(&[BlockType::Code, BlockType::Diff])]);
        assert!(def.applies_to(BlockType::Code));
        assert!(!def.applies_to(BlockType::Image));
    }

    #[test]
    fn action_registry_has_default_actions() {
        let reg = ActionRegistry::new();
        assert!(!reg.all_actions().is_empty());
        assert!(reg.find_by_action_type(&ActionType::Copy).is_some());
    }

    #[test]
    fn action_registry_filters_by_block_type() {
        let reg = ActionRegistry::new();
        let cmd_actions = reg.actions_for_block(BlockType::Command);
        let img_actions = reg.actions_for_block(BlockType::Image);
        assert!(cmd_actions.len() >= img_actions.len());
    }

    #[test]
    fn action_registry_find_by_shortcut() {
        let reg = ActionRegistry::new();
        let found = reg.find_by_shortcut(&KeyBinding::new('c').with_ctrl());
        assert!(found.is_some());
        assert_eq!(found.unwrap().action_type, ActionType::Copy);
    }

    #[test]
    fn action_bar_manager_get_actions_for_block() {
        let mgr = ActionBarManager::new();
        let block = make_command_block();
        let actions = mgr.get_actions_for_block(&block);
        assert!(!actions.is_empty());
        assert!(actions.iter().all(|a| a.is_enabled));
    }

    #[test]
    fn action_bar_manager_handle_click_returns_triggered() {
        let mut mgr = ActionBarManager::new();
        let result = mgr.handle_click(0, Uuid::nil());
        match result {
            ActionResult::Triggered { action, .. } => assert_eq!(action, ActionType::Copy),
            _ => panic!("Expected Triggered result"),
        }
    }

    #[test]
    fn action_bar_manager_handle_click_out_of_bounds() {
        let mut mgr = ActionBarManager::new();
        let result = mgr.handle_click(999, Uuid::nil());
        assert!(matches!(result, ActionResult::None));
    }

    #[test]
    fn action_bar_manager_handle_shortcut() {
        let mut mgr = ActionBarManager::new();
        let result = mgr.handle_shortcut(&KeyBinding::new('c').with_ctrl(), Uuid::nil());
        assert!(result.is_some());
    }

    #[test]
    fn context_analyzer_analyze_empty_content() {
        let ctx = ContextAnalyzer::analyze("");
        assert!(ctx.file_paths.is_empty());
        assert!(ctx.urls.is_empty());
        assert!(ctx.errors.is_empty());
        assert!(ctx.code_symbols.is_empty());
    }

    #[test]
    fn hover_state_set_and_clear() {
        let mut state = HoverState::default();
        assert!(state.hovered_index().is_none());
        state.set_hovered(Some(3));
        assert_eq!(state.hovered_index(), Some(3));
        state.clear();
        assert!(state.hovered_index().is_none());
        assert!(state.tooltip().is_none());
    }

    #[test]
    fn action_bar_manager_update_hover_inside_area() {
        let mut mgr = ActionBarManager::new();
        let area = Rect::new(0, 0, 30, 1);
        mgr.update_hover(5, 0, area);
        assert!(mgr.hover_state().hovered_index().is_some());
    }

    #[test]
    fn action_bar_manager_update_hover_outside_area_clears() {
        let mut mgr = ActionBarManager::new();
        let area = Rect::new(0, 0, 10, 1);
        mgr.update_hover(50, 5, area);
        assert!(mgr.hover_state().hovered_index().is_none());
    }

    #[test]
    fn action_bar_manager_render_produces_output() {
        let mgr = ActionBarManager::new();
        let block = make_command_block();
        let actions = mgr.get_actions_for_block(&block);
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 1));
        mgr.render_action_bar(&actions, Rect::new(0, 0, 40, 1), &mut buf);
        let cell = buf.cell((0, 0)).unwrap();
        assert!(cell.symbol().len() > 0);
    }
}
