use super::*;
use serde_json::json;

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

fn test_ctx() -> ToolContext {
    ToolContext {
        session_id: "ses_ask_user_question_tool".to_string(),
        message_id: "msg1".to_string(),
        tool_call_id: "tool1".to_string(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: crate::tool::ToolExecutionMode::AgentTurn,
    }
}

#[tokio::test]
async fn ask_user_question_writes_recommended_quiz_page() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let tool = AskUserQuestionTool::new();
    let output = tool
        .execute(
            json!({
                "question": "Set diagram rendering?",
                "context": "We are tuning Jcode config.",
                "page_id": "config-question",
                "title": "Config Question",
                "options": [
                    {"id": "keep", "label": "Keep current", "value": "none"},
                    {
                        "id": "rec",
                        "label": "Use inline diagrams",
                        "value": "inline",
                        "recommended": true,
                        "recommendation_reason": "Mermaid diagrams render directly in the chat/side panel."
                    }
                ]
            }),
            test_ctx(),
        )
        .await
        .expect("tool execute");

    assert!(output.output.contains("config-question"));
    assert!(output.output.contains("Recommended: rec"));
    assert_eq!(output.title.as_deref(), Some("askUserQuestion"));

    let snapshot =
        crate::side_panel::snapshot_for_session("ses_ask_user_question_tool").expect("snapshot");
    assert_eq!(snapshot.focused_page_id.as_deref(), Some("config-question"));
    let page = snapshot
        .pages
        .iter()
        .find(|page| page.id == "config-question")
        .expect("question page");
    assert_eq!(page.title, "Config Question");
    assert!(page.content.contains("# Question"));
    assert!(page.content.contains("Set diagram rendering?"));
    assert!(
        page.content
            .contains("### ✅ rec. Use inline diagrams **(recommended)**")
    );
    assert!(page.content.contains("Why recommended"));
    assert!(page.content.contains("Reply in chat with one option ID"));
}

#[tokio::test]
async fn ask_user_question_generates_option_ids_and_rejects_empty_options() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let tool = AskUserQuestionTool::new();
    let output = tool
        .execute(
            json!({
                "question": "Pick one",
                "options": [
                    {"label": "Alpha"},
                    {"label": "Beta", "recommended": true}
                ],
                "allow_multiple": true
            }),
            test_ctx(),
        )
        .await
        .expect("tool execute");

    assert!(output.output.contains("Recommended: B"));
    let snapshot =
        crate::side_panel::snapshot_for_session("ses_ask_user_question_tool").expect("snapshot");
    let page = snapshot
        .pages
        .iter()
        .find(|page| page.id == "ask-user-question")
        .expect("default page");
    assert!(page.content.contains("### A. Alpha"));
    assert!(page.content.contains("### ✅ B. Beta **(recommended)**"));
    assert!(page.content.contains("one or more option IDs"));

    let err = tool
        .execute(
            json!({
                "question": "Empty?",
                "options": []
            }),
            test_ctx(),
        )
        .await
        .expect_err("empty options should fail");
    assert!(err.to_string().contains("at least one option"));
}
