use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

const OPERATIONS: &[&str] = &[
    "goToDefinition",
    "findReferences",
    "hover",
    "documentSymbol",
    "workspaceSymbol",
    "goToImplementation",
    "prepareCallHierarchy",
    "incomingCalls",
    "outgoingCalls",
];

pub struct LspTool;

impl LspTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct LspInput {
    operation: String,
    #[serde(rename = "filePath")]
    file_path: String,
    line: u32,
    character: u32,
}

#[async_trait]
impl Tool for LspTool {
    fn name(&self) -> &str {
        "lsp"
    }

    fn description(&self) -> &str {
        "Run a language-server (LSP) operation such as go-to-definition or hover. \
         Currently returns a fallback message because LSP is not integrated in jcode."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["operation", "filePath", "line", "character"],
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": OPERATIONS,
                    "description": "The LSP operation to perform"
                },
                "filePath": {
                    "type": "string",
                    "description": "Absolute or relative path to the file"
                },
                "line": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "1-based line number"
                },
                "character": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "1-based character offset"
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: LspInput = serde_json::from_value(input)?;
        if !OPERATIONS.contains(&params.operation.as_str()) {
            return Err(anyhow::anyhow!(
                "Unsupported LSP operation: {}",
                params.operation
            ));
        }

        let path = ctx.resolve_path(Path::new(&params.file_path));
        if !path.exists() {
            return Err(anyhow::anyhow!("File not found: {}", params.file_path));
        }

        Ok(ToolOutput::new(format!(
            "LSP is not integrated in jcode yet. Requested: {} at {}:{}:{}.\nUse grep or read to inspect symbols.",
            params.operation,
            params.file_path,
            params.line,
            params.character
        )))
    }
}
