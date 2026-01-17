use super::{Tool, ToolContext, ToolOutput};
use crate::bus::{Bus, BusEvent, TodoEvent};
use crate::todo::{load_todos, save_todos, TodoItem};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

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

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: TodoWriteInput = serde_json::from_value(input)?;
        save_todos(&ctx.session_id, &params.todos)?;

        Bus::global().publish(BusEvent::TodoUpdated(TodoEvent {
            session_id: ctx.session_id.clone(),
            todos: params.todos.clone(),
        }));

        let remaining = params
            .todos
            .iter()
            .filter(|t| t.status != "completed")
            .count();
        Ok(
            ToolOutput::new(serde_json::to_string_pretty(&params.todos)?)
                .with_title(format!("{} todos", remaining))
                .with_metadata(json!({"todos": params.todos})),
        )
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

    async fn execute(&self, _input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let todos = load_todos(&ctx.session_id)?;
        let remaining = todos.iter().filter(|t| t.status != "completed").count();
        Ok(ToolOutput::new(serde_json::to_string_pretty(&todos)?)
            .with_title(format!("{} todos", remaining))
            .with_metadata(json!({"todos": todos})))
    }
}
