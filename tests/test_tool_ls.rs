//! Tests for the `ls` tool (directory listing, schema validation).

use jcode::tool::{Tool, ToolContext, ToolExecutionMode};
use jcode::tool::ls::LsTool;
use serde_json::json;

fn make_ctx(dir: &std::path::Path) -> ToolContext {
    ToolContext {
        session_id: "test-ls".into(),
        message_id: "msg".into(),
        tool_call_id: "call-ls".into(),
        working_dir: Some(dir.to_path_buf()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    }
}

#[test]
fn ls_tool_name_and_description() {
    let tool = LsTool::new();
    assert_eq!(tool.name(), "ls");
    assert!(!tool.description().is_empty());
}

#[test]
fn ls_schema_has_optional_path_and_ignore() {
    let schema = LsTool::new().parameters_schema();
    let properties = schema["properties"].as_object().unwrap();
    assert!(properties.contains_key("path"));
    assert!(properties.contains_key("ignore"));
    // path is optional (no required array)
    assert!(schema.get("required").is_none());
}

#[tokio::test]
async fn ls_lists_directory_contents() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("a.txt"), "a").unwrap();
    std::fs::write(tmp.path().join("b.txt"), "b").unwrap();
    std::fs::create_dir(tmp.path().join("subdir")).unwrap();

    let tool = LsTool::new();
    let input = json!({"intent": "test"});
    let output = tool.execute(input, make_ctx(tmp.path())).await.unwrap();
    assert!(output.output.contains("a.txt"));
    assert!(output.output.contains("b.txt"));
    assert!(output.output.contains("subdir/"));
    assert!(output.output.contains("2 files"));
}

#[tokio::test]
async fn ls_nonexistent_directory_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let tool = LsTool::new();
    let input = json!({"path": "no-such-dir", "intent": "test"});
    let result = tool.execute(input, make_ctx(tmp.path())).await;
    assert!(result.is_err());
}
