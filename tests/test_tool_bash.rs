//! Tests for the `bash` tool (shell execution, schema validation, stderr).

use jcode::tool::{Tool, ToolContext, ToolExecutionMode};
use jcode::tool::bash::BashTool;
use serde_json::json;

fn make_ctx(dir: &std::path::Path) -> ToolContext {
    ToolContext {
        session_id: "test-bash".into(),
        message_id: "msg".into(),
        tool_call_id: "call-bash".into(),
        working_dir: Some(dir.to_path_buf()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    }
}

#[test]
fn bash_tool_name_and_description() {
    let tool = BashTool::new();
    assert_eq!(tool.name(), "bash");
    assert!(!tool.description().is_empty());
}

#[test]
fn bash_schema_requires_command() {
    let schema = BashTool::new().parameters_schema();
    let required = schema["required"].as_array().expect("required array");
    assert!(required.iter().any(|v| v.as_str() == Some("command")));
}

#[tokio::test]
async fn bash_echo_command() {
    let tmp = tempfile::tempdir().unwrap();
    let tool = BashTool::new();
    let input = json!({"command": "echo integration_test_marker", "timeout": 5000});
    let output = tool.execute(input, make_ctx(tmp.path())).await.unwrap();
    assert!(output.output.contains("integration_test_marker"));
}

#[tokio::test]
async fn bash_failing_command_returns_error() {
    let tmp = tempfile::tempdir().unwrap();
    let tool = BashTool::new();
    let input = json!({"command": "exit 1", "timeout": 5000});
    let result = tool.execute(input, make_ctx(tmp.path())).await;
    assert!(result.is_err(), "exit 1 should produce an error");
}

#[tokio::test]
async fn bash_command_with_stderr() {
    let tmp = tempfile::tempdir().unwrap();
    let tool = BashTool::new();
    let input = json!({"command": "echo err_msg >&2", "timeout": 5000});
    let output = tool.execute(input, make_ctx(tmp.path())).await.unwrap();
    assert!(
        output.output.contains("err_msg"),
        "stderr should appear in output: {}",
        output.output
    );
}
