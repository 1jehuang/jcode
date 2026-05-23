//! Cross-File Repair Engine Integration for Agent Workflow
//!
//! Integrates `jcode-cross-file-repair` crate into the Agent's plan execution pipeline.
//! Provides automatic type-checking and self-correction for multi-file edits.

use std::sync::Arc;
use tracing::{info, warn, error};

use jcode_cross_file_repair::{
    CrossFileRepairEngine, TreeSitterAstAdapter, TypeChecker, EditBridge,
    AstEdit, AstEditOp, LanguageKind,
};

/// Wrapper for cross-file repair engine in Agent context
pub struct AgentCrossFileRepair {
    engine: Arc<CrossFileRepairEngine<TreeSitterAstAdapter>>,
}

impl AgentCrossFileRepair {
    /// Create a new cross-file repair instance
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let ast_adapter = Arc::new(TreeSitterAstAdapter::new(LanguageKind::Rust));
        let type_checker = TypeChecker::new();
        
        let engine = Arc::new(CrossFileRepairEngine::new(ast_adapter, type_checker));
        
        info!("Cross-file repair engine initialized");
        
        Ok(Self { engine })
    }

    /// Validate and repair edits before applying to workspace
    pub async fn validate_and_repair(
        &self,
        edits: Vec<AgentEdit>,
        workspace_root: &str,
    ) -> Result<Vec<AgentEdit>, Box<dyn std::error::Error>> {
        info!(
            "Validating {} edits with cross-file repair engine",
            edits.len()
        );

        // Convert AgentEdit to AstEdit
        let ast_edits = self.convert_to_ast_edits(edits)?;

        // Run validation and repair
        let repaired_edits = self.engine
            .validate_and_repair(ast_edits, workspace_root)
            .await?;

        // Convert back to AgentEdit
        let agent_edits = self.convert_from_ast_edits(repaired_edits)?;

        info!(
            "Cross-file repair completed: {} edits validated",
            agent_edits.len()
        );

        Ok(agent_edits)
    }

    /// Convert AgentEdit format to AstEdit
    fn convert_to_ast_edits(&self, edits: Vec<AgentEdit>) -> Result<Vec<AstEdit>, Box<dyn std::error::Error>> {
        let mut ast_edits = Vec::new();

        for edit in edits {
            let op = match edit.operation.as_str() {
                "insert" => AstEditOp::Insert {
                    content: edit.content,
                    line: edit.start_line,
                    column: 0,
                },
                "delete" => AstEditOp::Delete {
                    start_line: edit.start_line,
                    end_line: edit.end_line,
                },
                "replace" => AstEditOp::Replace {
                    start_line: edit.start_line,
                    end_line: edit.end_line,
                    content: edit.content,
                },
                _ => return Err(format!("Unknown operation: {}", edit.operation).into()),
            };

            ast_edits.push(AstEdit {
                file_path: edit.file_path,
                language: LanguageKind::Generic,
                operations: vec![op],
            });
        }

        Ok(ast_edits)
    }

    /// Convert AstEdit back to AgentEdit
    fn convert_from_ast_edits(&self, edits: Vec<AstEdit>) -> Result<Vec<AgentEdit>, Box<dyn std::error::Error>> {
        let mut agent_edits = Vec::new();

        for edit in edits {
            for op in edit.operations {
                let (operation, start_line, end_line, content) = match op {
                    AstEditOp::Insert { content, line, .. } => {
                        ("insert".to_string(), line, line, content)
                    }
                    AstEditOp::Delete { start_line, end_line } => {
                        ("delete".to_string(), start_line, end_line, String::new())
                    }
                    AstEditOp::Replace { start_line, end_line, content } => {
                        ("replace".to_string(), start_line, end_line, content)
                    }
                    AstEditOp::ReplaceFunction { name: _, new_body } => {
                        ("replace".to_string(), 0, 0, new_body)
                    }
                    AstEditOp::AddImport { import } => {
                        ("insert".to_string(), 0, 0, import)
                    }
                    _ => continue,
                };

                agent_edits.push(AgentEdit {
                    file_path: edit.file_path.clone(),
                    operation,
                    start_line,
                    end_line,
                    content,
                });
            }
        }

        Ok(agent_edits)
    }
}

/// Agent's internal edit representation
#[derive(Debug, Clone)]
pub struct AgentEdit {
    pub file_path: String,
    pub operation: String,
    pub start_line: usize,
    pub end_line: usize,
    pub content: String,
}

/// Integration helper for Agent workflow
pub async fn apply_cross_file_repair(
    edits: Vec<AgentEdit>,
    workspace_root: &str,
) -> Result<Vec<AgentEdit>, Box<dyn std::error::Error>> {
    let repair_engine = AgentCrossFileRepair::new()?;
    repair_engine.validate_and_repair(edits, workspace_root).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_repair_engine_creation() {
        let result = AgentCrossFileRepair::new();
        assert!(result.is_ok(), "Failed to create repair engine: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_edit_conversion() {
        let engine = AgentCrossFileRepair::new().unwrap();
        
        let agent_edits = vec![
            AgentEdit {
                file_path: "src/main.rs".to_string(),
                operation: "insert".to_string(),
                start_line: 10,
                end_line: 10,
                content: "fn test() {}".to_string(),
            }
        ];

        let ast_edits = engine.convert_to_ast_edits(agent_edits.clone()).unwrap();
        assert_eq!(ast_edits.len(), 1);
        
        let converted_back = engine.convert_from_ast_edits(ast_edits).unwrap();
        assert_eq!(converted_back.len(), 1);
        assert_eq!(converted_back[0].file_path, agent_edits[0].file_path);
    }
}
