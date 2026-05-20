use super::ui_actions::*;
use std::collections::HashMap;

pub struct ActionSystem {
    action_registry: ActionRegistry,
    action_handlers: HashMap<ActionType, Box<dyn Fn(&str) -> Result<String, String> + Send + Sync>>,
}

impl ActionSystem {
    pub fn new() -> Self {
        let mut registry = ActionRegistry::new();
        Self::register_default_actions(&mut registry);
        
        Self {
            action_registry: registry,
            action_handlers: HashMap::new(),
        }
    }

    fn register_default_actions(registry: &mut ActionRegistry) {
        registry.register_action(
            ActionDefinition::new(
                ActionType::Copy,
                '📋',
                "Copy content",
                "Copy selected content to clipboard",
            )
            .with_shortcut(KeyBinding::new('c').with_ctrl()),
        );

        registry.register_action(
            ActionDefinition::new(
                ActionType::Retry,
                '🔄',
                "Retry action",
                "Retry the last action or tool call",
            )
            .with_shortcut(KeyBinding::new('r').with_ctrl()),
        );

        registry.register_action(
            ActionDefinition::new(
                ActionType::Search,
                '🔍',
                "Search",
                "Search for content in the current context",
            )
            .with_shortcut(KeyBinding::new('f').with_ctrl()),
        );

        registry.register_action(
            ActionDefinition::new(
                ActionType::Expand,
                '📖',
                "Expand",
                "Expand collapsed content",
            )
            .with_shortcut(KeyBinding::new('e').with_ctrl()),
        );

        registry.register_action(
            ActionDefinition::new(
                ActionType::Collapse,
                '📚',
                "Collapse",
                "Collapse expanded content",
            )
            .with_shortcut(KeyBinding::new('c').with_ctrl().with_alt()),
        );

        registry.register_action(
            ActionDefinition::new(
                ActionType::Delete,
                '✕',
                "Delete",
                "Delete the selected item",
            )
            .with_shortcut(KeyBinding::new('d').with_ctrl()),
        );

        registry.register_action(
            ActionDefinition::new(
                ActionType::Edit,
                'E',
                "Edit",
                "Edit the selected content",
            )
            .with_shortcut(KeyBinding::new('e').with_ctrl().with_alt()),
        );

        registry.register_action(
            ActionDefinition::new(
                ActionType::RunCommand,
                'R',
                "Run",
                "Run the selected code block",
            )
            .with_shortcut(KeyBinding::new('r').with_ctrl().with_alt()),
        );
    }

    pub fn get_actions_for_block(&self, block_type: &BlockType) -> Vec<&ActionDefinition> {
        self.action_registry.actions_for_block(block_type.clone())
    }

    pub fn find_action_by_shortcut(&self, binding: &KeyBinding) -> Option<&ActionDefinition> {
        self.action_registry.find_by_shortcut(binding)
    }

    pub fn register_handler(
        &mut self,
        action_type: ActionType,
        handler: impl Fn(&str) -> Result<String, String> + Send + Sync + 'static,
    ) {
        self.action_handlers.insert(action_type, Box::new(handler));
    }

    pub fn execute_action(&self, action_type: &ActionType, context: &str) -> Result<String, String> {
        if let Some(handler) = self.action_handlers.get(action_type) {
            handler(context)
        } else {
            Err(format!("No handler registered for action {:?}", action_type))
        }
    }

    pub fn suggest_actions_for_context(&self, content: &str) -> Vec<SuggestedAction> {
        let mut suggestions = Vec::new();
        
        if content.contains("error") || content.contains("failed") || content.contains("bug") {
            suggestions.push(SuggestedAction {
                label: "Search for similar issues".to_string(),
                icon: '🔍',
                action: ActionType::Search,
                confidence: 0.9,
                reason: "Content mentions errors or bugs that may need investigation".to_string(),
            });
        }
        
        if content.contains("run") || content.contains("execute") || content.contains("test") {
            suggestions.push(SuggestedAction {
                label: "Run the code".to_string(),
                icon: 'R',
                action: ActionType::RunCommand,
                confidence: 0.8,
                reason: "Content suggests executable code or tests".to_string(),
            });
        }
        
        if content.contains("edit") || content.contains("modify") || content.contains("change") {
            suggestions.push(SuggestedAction {
                label: "Edit the content".to_string(),
                icon: 'E',
                action: ActionType::Edit,
                confidence: 0.7,
                reason: "Content indicates editing intent".to_string(),
            });
        }
        
        suggestions.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));
        suggestions
    }
}

impl Default for ActionSystem {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_system_creation() {
        let system = ActionSystem::new();
        let actions = system.action_registry.all_actions();
        assert!(!actions.is_empty());
    }

    #[test]
    fn test_register_and_find_action() {
        let mut system = ActionSystem::new();
        let test_binding = KeyBinding::new('x').with_ctrl();
        
        let action = ActionDefinition::new(
            ActionType::Search,
            '🔍',
            "Test search action",
            "A test action for searching",
        )
        .with_shortcut(test_binding.clone());
        
        system.action_registry.register_action(action);
        
        let found = system.find_action_by_shortcut(&test_binding);
        assert!(found.is_some());
        assert_eq!(found.unwrap().action_type, ActionType::Search);
    }

    #[test]
    fn test_get_actions_for_block_type() {
        let system = ActionSystem::new();
        let block_type = BlockType::Command;
        
        let actions = system.get_actions_for_block(&block_type);
        assert!(!actions.is_empty());
    }

    #[test]
    fn test_action_handler_registration() {
        let mut system = ActionSystem::new();
        
        system.register_handler(ActionType::Copy, |content| {
            Ok(format!("Copied: {}", content.len()))
        });
        
        let result = system.execute_action(&ActionType::Copy, "test content");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Copied: 12");
    }

    #[test]
    fn test_suggest_actions_for_context() {
        let system = ActionSystem::new();
        let suggestions = system.suggest_actions_for_context("I need to fix a bug in my code");
        
        assert!(!suggestions.is_empty());
        assert!(suggestions[0].action_type == ActionType::Search);
    }
}