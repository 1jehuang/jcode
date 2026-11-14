//! Tests for the `write` tool (file creation, overwrite, diff output, schema validation).

use jcode::tool::{Tool, ToolContext, ToolExecutionMode};
use jcode::tool::write::WriteTool;
use serde_json::json;

fn make_ctx(dir: &std::path::Path) -> ToolContext {
    ToolContext {
        session_id: "test-write".into(),
        message_id: "msg".into(),
        tool_call_id: "call-write".into(),
        working_dir: Some(dir.to_path_buf()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    }
}

#[test]
fn write_tool_name_and_description() {
    let tool = WriteTool::new();
    assert_eq!(tool.name(), "write");
    assert!(!tool.description().is_empty());
}

#[test]
fn write_schema_requires_file_path_and_content() {
    let schema = WriteTool::new().parameters_schema();
    let required = schema["required"].as_array().expect("required array");
    assert!(required.iter().any(|v| v.as_str() == Some("file_path")));
    assert!(required.iter().any(|v| v.as_str() == Some("content")));
}

#[tokio::test]
async fn write_creates_new_file() {
    let tmp = tempfile::tempdir().unwrap();
    let tool = WriteTool::new();
    let input = json!({
        "file_path": "new.txt",
        "content": "hello world",
        "intent": "test"
    });
    let output = tool.execute(input, make_ctx(tmp.path())).await.unwrap();
    assert!(output.output.contains("Created"));
    let written = std::fs::read_to_string(tmp.path().join("new.txt")).unwrap();
    assert_eq!(written, "hello world");
}

#[tokio::test]
async fn write_overwrites_existing_file() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("existing.txt");
    std::fs::write(&path, "old content").unwrap();

    let tool = WriteTool::new();
    let input = json!({
        "file_path": "existing.txt",
        "content": "new content",
        "intent": "test"
    });
    let output = tool.execute(input, make_ctx(tmp.path())).await.unwrap();
    assert!(output.output.contains("Updated"));
    let written = std::fs::read_to_string(&path).unwrap();
    assert_eq!(written, "new content");
}

#[tokio::test]
async fn write_creates_parent_directories() {
    let tmp = tempfile::tempdir().unwrap();
    let tool = WriteTool::new();
    let input = json!({
        "file_path": "deep/nested/dir/file.txt",
        "content": "nested",
        "intent": "test"
    });
    tool.execute(input, make_ctx(tmp.path())).await.unwrap();
    let written = std::fs::read_to_string(tmp.path().join("deep/nested/dir/file.txt")).unwrap();
    assert_eq!(written, "nested");
}
