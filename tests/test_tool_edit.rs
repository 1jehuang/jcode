//! Tests for the `edit` tool (text replacement, diff output, schema validation).

use jcode::tool::{Tool, ToolContext, ToolExecutionMode};
use jcode::tool::edit::EditTool;
use serde_json::json;

fn make_ctx(dir: &std::path::Path) -> ToolContext {
    ToolContext {
        session_id: "test-edit".into(),
        message_id: "msg".into(),
        tool_call_id: "call-edit".into(),
        working_dir: Some(dir.to_path_buf()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    }
}

#[test]
fn edit_tool_name_and_description() {
    let tool = EditTool::new();
    assert_eq!(tool.name(), "edit");
    assert!(!tool.description().is_empty());
}

#[test]
fn edit_schema_requires_file_path_old_string_new_string() {
    let schema = EditTool::new().parameters_schema();
    let required = schema["required"].as_array().expect("required array");
    assert!(required.iter().any(|v| v.as_str() == Some("file_path")));
    assert!(required.iter().any(|v| v.as_str() == Some("old_string")));
    assert!(required.iter().any(|v| v.as_str() == Some("new_string")));
}

#[tokio::test]
async fn edit_replaces_matching_text() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("code.rs");
    std::fs::write(&path, "fn main() {\n    println!(\"hello\");\n}").unwrap();

    let tool = EditTool::new();
    let input = json!({
        "file_path": "code.rs",
        "old_string": "hello",
        "new_string": "world",
        "intent": "test"
    });
    let output = tool.execute(input, make_ctx(tmp.path())).await.unwrap();
    assert!(output.output.contains("1 occurrence"));
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("world"));
    assert!(!content.contains("hello"));
}

#[tokio::test]
async fn edit_same_old_new_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("file.txt");
    std::fs::write(&path, "unchanged").unwrap();

    let tool = EditTool::new();
    let input = json!({
        "file_path": "file.txt",
        "old_string": "same",
        "new_string": "same",
        "intent": "test"
    });
    let result = tool.execute(input, make_ctx(tmp.path())).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn edit_missing_file_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let tool = EditTool::new();
    let input = json!({
        "file_path": "nope.txt",
        "old_string": "a",
        "new_string": "b",
        "intent": "test"
    });
    let result = tool.execute(input, make_ctx(tmp.path())).await;
    assert!(result.is_err());
}
