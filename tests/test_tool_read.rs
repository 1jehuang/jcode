//! Tests for the `read` tool (file reading, range parsing, schema validation).

use jcode::tool::{Tool, ToolContext, ToolExecutionMode};
use jcode::tool::read::ReadTool;
use serde_json::json;

fn make_ctx(dir: &std::path::Path) -> ToolContext {
    ToolContext {
        session_id: "test-read".into(),
        message_id: "msg".into(),
        tool_call_id: "call-read".into(),
        working_dir: Some(dir.to_path_buf()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    }
}

#[test]
fn read_tool_name_and_description() {
    let tool = ReadTool::new();
    assert_eq!(tool.name(), "read");
    assert!(!tool.description().is_empty());
}

#[test]
fn read_schema_has_required_file_path() {
    let schema = ReadTool::new().parameters_schema();
    let required = schema["required"]
        .as_array()
        .expect("schema should have required array");
    assert!(
        required.iter().any(|v| v.as_str() == Some("file_path")),
        "file_path must be required"
    );
}

#[tokio::test]
async fn read_file_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let tool = ReadTool::new();
    let input = json!({
        "file_path": "nonexistent.txt",
        "intent": "test"
    });
    let result = tool.execute(input, make_ctx(tmp.path())).await;
    assert!(result.is_err(), "should error for missing file");
}

#[tokio::test]
async fn read_text_file_returns_content() {
    let tmp = tempfile::tempdir().unwrap();
    let file_path = tmp.path().join("hello.txt");
    std::fs::write(&file_path, "line1\nline2\nline3\n").unwrap();

    let tool = ReadTool::new();
    let input = json!({
        "file_path": "hello.txt",
        "intent": "test"
    });
    let output = tool.execute(input, make_ctx(tmp.path())).await.unwrap();
    assert!(output.output.contains("line1"), "should contain file content");
    assert!(output.output.contains("line3"));
}

#[tokio::test]
async fn read_with_start_line_and_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let file_path = tmp.path().join("numbered.txt");
    let content: String = (1..=20).map(|i| format!("line {}\n", i)).collect();
    std::fs::write(&file_path, &content).unwrap();

    let tool = ReadTool::new();
    // Read lines 5..=8 (4 lines) using start_line/end_line
    let input = json!({
        "file_path": "numbered.txt",
        "start_line": 5,
        "end_line": 8,
        "intent": "test"
    });
    let output = tool.execute(input, make_ctx(tmp.path())).await.unwrap();
    assert!(output.output.contains("line 5"));
    assert!(output.output.contains("line 8"));
    assert!(!output.output.contains("line 4"), "should not contain line 4");
    assert!(!output.output.contains("line 9"), "should not contain line 9");
}

#[tokio::test]
async fn read_empty_file_returns_empty_marker() {
    let tmp = tempfile::tempdir().unwrap();
    let file_path = tmp.path().join("empty.txt");
    std::fs::write(&file_path, "").unwrap();

    let tool = ReadTool::new();
    let input = json!({
        "file_path": "empty.txt",
        "intent": "test"
    });
    let output = tool.execute(input, make_ctx(tmp.path())).await.unwrap();
    assert!(
        output.output.contains("empty"),
        "empty file should produce an 'empty' marker: {}",
        output.output
    );
}
