use super::{Tool, ToolContext, ToolOutput};
use crate::bus::{Bus, BusEvent, TodoEvent};
use crate::todo::{TodoItem, load_todos, save_todos};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

pub struct TodoTool;

impl TodoTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct TodoInput {
    todos: Option<Vec<TodoItem>>,
}

#[async_trait]
impl Tool for TodoTool {
    fn name(&self) -> &str {
        "todo"
    }

    fn description(&self) -> &str {
        "Read or update the todo list."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "description": "Todo list to save.",
                    "items": {
                        "type": "object",
                        "required": ["content", "status", "priority", "id"],
                        "properties": {
                            "content": {
                                "type": "string",
                                "description": "Task."
                            },
                            "status": {
                                "type": "string",
                                "description": "Status."
                            },
                            "priority": {
                                "type": "string",
                                "description": "Priority."
                            },
                            "id": {
                                "type": "string",
                                "description": "ID."
                            }
                        }
                    }
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: TodoInput = serde_json::from_value(input)?;
        match params.todos {
            Some(todos) => {
                save_todos(&ctx.session_id, &todos)?;

                Bus::global().publish(BusEvent::TodoUpdated(TodoEvent {
                    session_id: ctx.session_id.clone(),
                    todos: todos.clone(),
                }));

                let remaining = todos.iter().filter(|t| t.status != "completed").count();
                Ok(ToolOutput::new(serde_json::to_string_pretty(&todos)?)
                    .with_title(format!("{} todos", remaining))
                    .with_metadata(json!({"todos": todos})))
            }
            None => {
                let todos = load_todos(&ctx.session_id)?;
                let remaining = todos.iter().filter(|t| t.status != "completed").count();
                Ok(ToolOutput::new(serde_json::to_string_pretty(&todos)?)
                    .with_title(format!("{} todos", remaining))
                    .with_metadata(json!({"todos": todos})))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_is_named_todo() {
        assert_eq!(TodoTool::new().name(), "todo");
    }

    #[test]
    fn schema_only_advertises_todos() {
        let schema = TodoTool::new().parameters_schema();
        let props = schema
            .get("properties")
            .and_then(|v| v.as_object())
            .expect("todo schema should have properties");
        assert_eq!(props.len(), 1);
        assert!(props.contains_key("todos"));
    }
}
