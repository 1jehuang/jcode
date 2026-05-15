use super::{Tool, ToolContext, ToolOutput};
use crate::bus::{Bus, BusEvent, TodoEvent};
use crate::todo::{TodoItem, load_todos, save_todos};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

pub struct TodoTool;

impl TodoTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct TodoInput {
    #[serde(default, deserialize_with = "deserialize_todos_flexible")]
    todos: Option<Vec<TodoItem>>,
}

/// Accept `todos` as any of:
///   - a real JSON array (the schema-correct form)
///   - a JSON-string-encoded array (some LLMs emit `"todos": "[...]"`)
///   - a single object (auto-wrapped into a one-element vec)
///   - `null` / missing (returns `None` → triggers a read)
///
/// This makes the todo tool robust to common tool-call wire-format quirks
/// across providers without changing the advertised schema.
fn deserialize_todos_flexible<'de, D>(deserializer: D) -> Result<Option<Vec<TodoItem>>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;

    let value = Value::deserialize(deserializer)?;
    parse_todos_value(value).map_err(D::Error::custom)
}

fn parse_todos_value(value: Value) -> Result<Option<Vec<TodoItem>>, String> {
    match value {
        Value::Null => Ok(None),
        Value::Array(_) => serde_json::from_value::<Vec<TodoItem>>(value)
            .map(Some)
            .map_err(|e| format!("invalid todos array: {}", e)),
        Value::Object(_) => serde_json::from_value::<TodoItem>(value)
            .map(|item| Some(vec![item]))
            .map_err(|e| format!("invalid single-todo object: {}", e)),
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            let inner: Value = serde_json::from_str(trimmed)
                .map_err(|e| format!("todos string was not valid JSON: {}", e))?;
            // Recurse to handle array / object / null inside the string.
            parse_todos_value(inner)
        }
        other => Err(format!(
            "todos must be an array, object, or JSON-string; got {}",
            match other {
                Value::Bool(_) => "bool",
                Value::Number(_) => "number",
                _ => "unknown",
            }
        )),
    }
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
                "intent": super::intent_schema_property(),
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
        let operation = if params.todos.is_some() {
            "write"
        } else {
            "read"
        };
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
        .map_err(|err| {
            crate::logging::warn(&format!(
                "[tool:todo] operation failed operation={} session_id={} error={}",
                operation, ctx.session_id, err
            ));
            err
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_todo_json() -> Value {
        json!({"id":"a","content":"do thing","status":"pending","priority":"high"})
    }

    #[test]
    fn tool_is_named_todo() {
        assert_eq!(TodoTool::new().name(), "todo");
    }

    #[test]
    fn schema_advertises_intent_and_todos() {
        let schema = TodoTool::new().parameters_schema();
        let props = schema
            .get("properties")
            .and_then(|v| v.as_object())
            .expect("todo schema should have properties");
        assert_eq!(props.len(), 2);
        assert!(props.contains_key("intent"));
        assert!(props.contains_key("todos"));
    }

    #[test]
    fn accepts_native_array() {
        let input = json!({"todos": [sample_todo_json()]});
        let parsed: TodoInput = serde_json::from_value(input).expect("native array");
        let todos = parsed.todos.expect("todos present");
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].id, "a");
    }

    #[test]
    fn accepts_json_string_encoded_array() {
        // Some LLMs emit `"todos": "[...]"` instead of `"todos": [...]`.
        let encoded = serde_json::to_string(&vec![sample_todo_json()]).unwrap();
        let input = json!({"todos": encoded});
        let parsed: TodoInput =
            serde_json::from_value(input).expect("string-encoded array should parse");
        let todos = parsed.todos.expect("todos present");
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].id, "a");
    }

    #[test]
    fn accepts_single_object_and_wraps_in_vec() {
        let input = json!({"todos": sample_todo_json()});
        let parsed: TodoInput = serde_json::from_value(input).expect("single object");
        let todos = parsed.todos.expect("todos present");
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].id, "a");
    }

    #[test]
    fn missing_todos_field_is_read_operation() {
        let input = json!({});
        let parsed: TodoInput = serde_json::from_value(input).expect("empty");
        assert!(parsed.todos.is_none());
    }

    #[test]
    fn null_todos_field_is_read_operation() {
        let input = json!({"todos": null});
        let parsed: TodoInput = serde_json::from_value(input).expect("null");
        assert!(parsed.todos.is_none());
    }

    #[test]
    fn empty_string_todos_is_read_operation() {
        let input = json!({"todos": ""});
        let parsed: TodoInput = serde_json::from_value(input).expect("empty string");
        assert!(parsed.todos.is_none());
    }

    #[test]
    fn rejects_non_json_string() {
        let input = json!({"todos": "not json at all"});
        let result: Result<TodoInput, _> = serde_json::from_value(input);
        assert!(result.is_err(), "non-JSON string should be rejected");
    }

    #[test]
    fn rejects_number_todos() {
        let input = json!({"todos": 42});
        let result: Result<TodoInput, _> = serde_json::from_value(input);
        assert!(result.is_err(), "scalar todos should be rejected");
    }
}
