use super::Tool;
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

pub struct WriteTool;

impl WriteTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct WriteInput {
    file_path: String,
    content: String,
}

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "Write content to a file. Creates the file if it doesn't exist, \
         or overwrites if it does. Creates parent directories as needed."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["file_path", "content"],
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The path to the file to write (absolute or relative)"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            }
        })
    }

    async fn execute(&self, input: Value) -> Result<String> {
        let params: WriteInput = serde_json::from_value(input)?;

        let path = Path::new(&params.file_path);

        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                tokio::fs::create_dir_all(parent).await?;
            }
        }

        // Check if file existed before
        let existed = path.exists();
        let old_len = if existed {
            tokio::fs::metadata(path).await.ok().map(|m| m.len())
        } else {
            None
        };

        // Write the file
        tokio::fs::write(path, &params.content).await?;

        let new_len = params.content.len();
        let line_count = params.content.lines().count();

        if existed {
            Ok(format!(
                "Updated {} ({} bytes → {} bytes, {} lines)",
                params.file_path,
                old_len.unwrap_or(0),
                new_len,
                line_count
            ))
        } else {
            Ok(format!(
                "Created {} ({} bytes, {} lines)",
                params.file_path, new_len, line_count
            ))
        }
    }
}
