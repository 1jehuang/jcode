use super::*;

struct EnvVarGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set_path(key: &'static str, value: &std::path::Path) -> Self {
        let previous = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            crate::env::set_var(self.key, previous);
        } else {
            crate::env::remove_var(self.key);
        }
    }
}

#[tokio::test]
async fn side_panel_tool_writes_page() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let tool = SidePanelTool::new();
    let output = tool
        .execute(
            json!({
                "action": "write",
                "page_id": "notes",
                "title": "Notes",
                "content": "# Notes"
            }),
            ToolContext {
                session_id: "ses_side_panel_tool".to_string(),
                message_id: "msg1".to_string(),
                tool_call_id: "tool1".to_string(),
                working_dir: None,
                stdin_request_tx: None,
                graceful_shutdown_signal: None,
                execution_mode: crate::tool::ToolExecutionMode::AgentTurn,
            },
        )
        .await
        .expect("tool execute");

    assert!(output.output.contains("notes"));
}

#[tokio::test]
async fn side_panel_tool_loads_file_with_derived_page_id() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let doc_path = temp.path().join("Project Plan.md");
    std::fs::write(&doc_path, "# Plan\n\nInitial").expect("write source file");

    let tool = SidePanelTool::new();
    let output = tool
        .execute(
            json!({
                "action": "load",
                "file_path": "Project Plan.md"
            }),
            ToolContext {
                session_id: "ses_side_panel_tool_load".to_string(),
                message_id: "msg1".to_string(),
                tool_call_id: "tool1".to_string(),
                working_dir: Some(temp.path().to_path_buf()),
                stdin_request_tx: None,
                graceful_shutdown_signal: None,
                execution_mode: crate::tool::ToolExecutionMode::AgentTurn,
            },
        )
        .await
        .expect("tool execute");

    assert!(output.output.contains("project-plan"));
    let snapshot: crate::side_panel::SidePanelSnapshot =
        serde_json::from_value(output.metadata.expect("snapshot metadata"))
            .expect("parse side panel metadata");
    let page = snapshot
        .pages
        .iter()
        .find(|page| page.id == "project-plan")
        .expect("loaded page");
    assert_eq!(page.title, "Project Plan.md");
    assert_eq!(page.content, "# Plan\n\nInitial");
}
