//! TUI Application state — Pure rendering, no business logic

use crate::agent_bridge::AgentBridge;
use crate::config::CliConfig;
use crate::tui::widgets::file_tree::FileTree;

/// UI message types (display only)
#[derive(Debug, Clone)]
pub enum UIMessage {
    User(String),
    Assistant(String),
    ToolCall { name: String, params: serde_json::Value },
    ToolResult { name: String, result: String },
    System(String),
    Error(String),
}

/// TUI application state
pub struct App {
    pub messages: Vec<UIMessage>,
    pub input: String,
    pub input_mode: InputMode,
    pub bridge: AgentBridge,
    pub config: CliConfig,
    pub should_quit: bool,
    /// File tree for workspace navigation (Dashboard feature)
    pub file_tree: FileTree,
    /// Help overlay visibility
    pub show_help: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Insert,
}

impl App {
    pub fn new(config: CliConfig, bridge: AgentBridge) -> Self {
        let working_dir = std::env::current_dir().unwrap_or_default();
        let mut file_tree = FileTree::new();
        if let Err(e) = file_tree.scan_directory(&working_dir) {
            tracing::warn!(error = %e, "Failed to scan workspace for file tree");
        }
        Self {
            messages: vec![],
            input: String::new(),
            input_mode: InputMode::Normal,
            bridge,
            config,
            should_quit: false,
            file_tree,
            show_help: false,
        }
    }

    /// Handle user input — delegates to bridge
    pub async fn handle_input(&mut self, input: String) {
        if input.is_empty() { return; }
        self.messages.push(UIMessage::User(input.clone()));
        match self.bridge.execute_turn(&input).await {
            Ok(output) => {
                self.messages.push(UIMessage::Assistant(output.text));
                for tc in &output.tool_calls {
                    self.messages.push(UIMessage::ToolCall { name: tc.name.clone(), params: tc.params.clone() });
                    if let Some(ref result) = tc.result {
                        self.messages.push(UIMessage::ToolResult { name: tc.name.clone(), result: result.to_string() });
                    }
                }
            }
            Err(e) => {
                self.messages.push(UIMessage::Error(e.to_string()));
            }
        }
        self.input.clear();
    }

    /// Toggle file tree visibility
    pub fn toggle_file_tree(&mut self) {
        self.file_tree.toggle();
    }

    /// Toggle help overlay
    pub fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CliConfig;

    fn create_test_app() -> App {
        let config = CliConfig::default();
        let ctx = carpai_core::build_local_agent_context(&config.core);
        let bridge = AgentBridge::new_local(ctx);
        App::new(config, bridge)
    }

    #[test]
    fn test_app_initial_state() {
        let app = create_test_app();
        assert_eq!(app.input_mode, InputMode::Normal);
        assert!(!app.should_quit);
        assert!(app.messages.is_empty());
        assert!(app.input.is_empty());
        assert!(!app.show_help);
        assert!(!app.file_tree.visible);
    }

    #[test]
    fn test_ui_message_display() {
        if let UIMessage::User(t) = UIMessage::User("hello".into()) {
            assert_eq!(t, "hello");
        }
    }

    #[test]
    fn test_toggle_file_tree() {
        let mut app = create_test_app();
        assert!(!app.file_tree.visible);
        app.toggle_file_tree();
        assert!(app.file_tree.visible);
        app.toggle_file_tree();
        assert!(!app.file_tree.visible);
    }

    #[test]
    fn test_toggle_help() {
        let mut app = create_test_app();
        assert!(!app.show_help);
        app.toggle_help();
        assert!(app.show_help);
        app.toggle_help();
        assert!(!app.show_help);
    }
}
