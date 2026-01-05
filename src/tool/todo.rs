use super::Tool;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub content: String,
    pub status: String,
    pub priority: String,
    pub id: String,
}

fn todo_store() -> &'static Mutex<Vec<TodoItem>> {
    static STORE: OnceLock<Mutex<Vec<TodoItem>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(Vec::new()))
}

pub struct TodoWriteTool;
pub struct TodoReadTool;

impl TodoWriteTool {
    pub fn new() -> Self {
        Self
    }
}

impl TodoReadTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct TodoWriteInput {
    todos: Vec<TodoItem>,
}

#[async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &str {
        "todowrite"
    }

    fn description(&self) -> &str {
        "Update the current todo list. Provide the full list of todos."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["todos"],
            "properties": {
                "todos": {
                    "type": "array",
                    "description": "The updated todo list",
                    "items": {
                        "type": "object",
                        "required": ["content", "status", "priority", "id"],
                        "properties": {
                            "content": {
                                "type": "string",
                                "description": "Brief description of the task"
                            },
                            "status": {
                                "type": "string",
                                "description": "pending, in_progress, completed, cancelled"
                            },
                            "priority": {
                                "type": "string",
                                "description": "high, medium, low"
                            },
                            "id": {
                                "type": "string",
                                "description": "Unique identifier for the todo item"
                            }
                        }
                    }
                }
            }
        })
    }

    async fn execute(&self, input: Value) -> Result<String> {
        let params: TodoWriteInput = serde_json::from_value(input)?;
        let mut store = todo_store()
            .lock()
            .map_err(|_| anyhow::anyhow!("Todo store lock poisoned"))?;
        *store = params.todos.clone();

        let remaining = params
            .todos
            .iter()
            .filter(|t| t.status != "completed")
            .count();
        Ok(format!(
            "{} todos\n{}",
            remaining,
            serde_json::to_string_pretty(&params.todos)?
        ))
    }
}

#[async_trait]
impl Tool for TodoReadTool {
    fn name(&self) -> &str {
        "todoread"
    }

    fn description(&self) -> &str {
        "Read the current todo list."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn execute(&self, _input: Value) -> Result<String> {
        let store = todo_store()
            .lock()
            .map_err(|_| anyhow::anyhow!("Todo store lock poisoned"))?;
        let remaining = store.iter().filter(|t| t.status != "completed").count();
        Ok(format!(
            "{} todos\n{}",
            remaining,
            serde_json::to_string_pretty(&*store)?
        ))
    }
}
