use super::*;
use crate::ask_user::{AskUserAnswer, AskUserAnswerKind, submit_answer};
use crate::bus::{Bus, BusEvent};
use serde_json::json;

fn unique_session_id(label: &str) -> String {
    format!(
        "ses_aq_{label}_{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    )
}

fn test_ctx_with_session(session_id: String) -> ToolContext {
    ToolContext {
        session_id,
        message_id: "msg1".to_string(),
        tool_call_id: format!(
            "tool_aq_{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: crate::tool::ToolExecutionMode::AgentTurn,
    }
}

/// Wait for an `AskUserQuestionOpened` event matching `session_id`. Other
/// concurrent tests share the global bus, so we must filter by session to
/// avoid grabbing an unrelated request_id.
async fn wait_for_question(
    rx: &mut tokio::sync::broadcast::Receiver<BusEvent>,
    session_id: &str,
) -> String {
    loop {
        match rx.recv().await {
            Ok(BusEvent::AskUserQuestionOpened(q)) if q.session_id == session_id => {
                return q.request_id;
            }
            Ok(_) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(e) => panic!("bus dropped before opening event: {e}"),
        }
    }
}

#[tokio::test]
async fn rejects_empty_options() {
    let tool = AskUserQuestionTool::new();
    let err = tool
        .execute(
            json!({
                "question": "Empty?",
                "options": []
            }),
            test_ctx_with_session(unique_session_id("reject")),
        )
        .await
        .expect_err("empty options should fail");
    assert!(err.to_string().contains("at least one option"));
}

#[tokio::test]
async fn publishes_question_and_resolves_with_options_answer() {
    let mut rx = Bus::global().subscribe();
    let tool = AskUserQuestionTool::new();
    let session_id = unique_session_id("opts");
    let session_id_for_exec = session_id.clone();

    let exec = tokio::spawn(async move {
        tool.execute(
            json!({
                "question": "Pick one",
                "options": [
                    {"label": "Alpha"},
                    {"label": "Beta", "recommended": true, "value": "beta-val"}
                ],
            }),
            test_ctx_with_session(session_id_for_exec),
        )
        .await
    });

    let request_id = wait_for_question(&mut rx, &session_id).await;

    let ok = submit_answer(AskUserAnswer {
        request_id: request_id.clone(),
        kind: AskUserAnswerKind::Options {
            ids: vec!["B".into()],
            labels: vec!["Beta".into()],
            values: vec![Some("beta-val".into())],
        },
    });
    assert!(ok, "submit_answer should succeed for known request");

    let output = exec.await.expect("join").expect("tool execute");
    assert!(
        output.output.contains("User chose: B (Beta)"),
        "tool output did not include selection summary: {}",
        output.output
    );
    let metadata = output.metadata.expect("metadata");
    assert_eq!(metadata["outcome"], "selected");
    assert_eq!(metadata["selected_ids"][0], "B");
    assert_eq!(metadata["selected_values"][0], "beta-val");
}

#[tokio::test]
async fn custom_answer_reaches_tool_output() {
    let mut rx = Bus::global().subscribe();
    let tool = AskUserQuestionTool::new();
    let session_id = unique_session_id("custom");
    let session_id_for_exec = session_id.clone();
    let exec = tokio::spawn(async move {
        tool.execute(
            json!({
                "question": "What now?",
                "options": [{"label": "Whatever"}]
            }),
            test_ctx_with_session(session_id_for_exec),
        )
        .await
    });

    let request_id = wait_for_question(&mut rx, &session_id).await;
    let ok = submit_answer(AskUserAnswer {
        request_id,
        kind: AskUserAnswerKind::Custom {
            text: "do the thing".into(),
        },
    });
    assert!(ok);

    let output = exec.await.expect("join").expect("execute");
    assert!(output.output.contains("do the thing"));
    let metadata = output.metadata.expect("metadata");
    assert_eq!(metadata["outcome"], "custom");
    assert_eq!(metadata["custom_text"], "do the thing");
}

#[tokio::test]
async fn canceled_answer_is_surfaced() {
    let mut rx = Bus::global().subscribe();
    let tool = AskUserQuestionTool::new();
    let session_id = unique_session_id("cancel");
    let session_id_for_exec = session_id.clone();
    let exec = tokio::spawn(async move {
        tool.execute(
            json!({
                "question": "Anything?",
                "options": [{"label": "Whatever"}]
            }),
            test_ctx_with_session(session_id_for_exec),
        )
        .await
    });

    let request_id = wait_for_question(&mut rx, &session_id).await;
    submit_answer(AskUserAnswer {
        request_id,
        kind: AskUserAnswerKind::Canceled,
    });
    let output = exec.await.expect("join").expect("execute");
    assert!(output.output.contains("canceled"));
    let metadata = output.metadata.expect("metadata");
    assert_eq!(metadata["outcome"], "canceled");
}
