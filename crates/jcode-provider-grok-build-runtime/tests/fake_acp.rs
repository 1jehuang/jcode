use futures::StreamExt;
use jcode_message_types::{Message, StreamEvent};
use jcode_provider_core::Provider;
use jcode_provider_grok_build_runtime::{GrokBuildProcess, GrokBuildProvider};
use std::collections::BTreeMap;
use std::path::Path;

fn fake_process(log: &Path) -> GrokBuildProcess {
    let mut env = BTreeMap::new();
    env.insert(
        "JCODE_FAKE_GROK_ACP_LOG".to_string(),
        log.display().to_string(),
    );
    env.insert("JCODE_GROK_ACP_DISABLE_MCP".to_string(), "1".to_string());
    GrokBuildProcess {
        command: env!("CARGO_BIN_EXE_jcode-fake-grok-acp").into(),
        args: Vec::new(),
        env,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn provider_surfaces_payment_failure_written_to_subprocess_stderr() {
    let temp = tempfile::tempdir().unwrap();
    let mut process = fake_process(&temp.path().join("requests.jsonl"));
    process
        .env
        .insert("JCODE_FAKE_GROK_ACP_PAYMENT_REQUIRED".into(), "1".into());
    let provider = GrokBuildProvider::with_process(process);

    let error = provider
        .complete_simple("Reply exactly AUTH_TEST_OK", "")
        .await
        .unwrap_err();
    let detail = format!("{error:#}");
    assert!(detail.contains("402 Payment Required"), "{detail}");
    assert!(detail.contains("usage balance exhausted"), "{detail}");
}

#[tokio::test(flavor = "current_thread")]
async fn fake_subprocess_covers_handshake_models_new_prompt_and_auth_isolation() {
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("acp.jsonl");
    let provider = GrokBuildProvider::with_process(fake_process(&log));

    provider.prefetch_models().await.unwrap();
    assert_eq!(
        provider.available_models_display(),
        ["grok-4.5", "grok-code-fast-1"]
    );
    provider.set_model("grok-code-fast-1").unwrap();

    let mut stream = provider
        .complete(
            &[Message::user("Reply exactly AUTH_TEST_OK")],
            &[],
            "outer-system",
            None,
        )
        .await
        .unwrap();
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event.unwrap());
    }
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::SessionId(id) if id == "fake-session-new"))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::ThinkingDelta(text) if text == "thinking"))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::TextDelta(text) if text == "AUTH_TEST_OK"))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::MessageEnd { .. }))
    );

    let requests = std::fs::read_to_string(&log).unwrap();
    assert!(requests.contains("\"method\":\"initialize\""));
    assert!(requests.contains("\"method\":\"authenticate\""));
    assert!(requests.contains("\"methodId\":\"cached_token\""));
    assert!(!requests.contains("\"methodId\":\"xai.api_key\""));
    assert!(requests.contains("\"method\":\"session/new\""));
    assert!(requests.contains("\"method\":\"session/set_model\""));
    assert!(requests.contains("\"rules\":\"outer-system\""));
    assert!(requests.contains("\"yoloMode\":true"));
    let prompt_line = requests
        .lines()
        .find(|line| line.contains("\"method\":\"session/prompt\""))
        .expect("session/prompt was logged");
    assert!(prompt_line.contains("AUTH_TEST_OK"), "{prompt_line}");
    assert!(
        !prompt_line.contains("outer-system"),
        "Jcode system prompt must not be wrapped into session/prompt: {prompt_line}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn fake_subprocess_surfaces_acp_file_diffs_as_edit_tools() {
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("edit.jsonl");
    let mut process = fake_process(&log);
    process
        .env
        .insert("JCODE_FAKE_GROK_ACP_EDIT".into(), "1".into());
    let provider = GrokBuildProvider::with_process(process);
    provider.prefetch_models().await.unwrap();

    let mut stream = provider
        .complete(&[Message::user("edit the file")], &[], "", None)
        .await
        .unwrap();
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event.unwrap());
    }

    assert!(
        events.iter().any(|event| {
            matches!(event, StreamEvent::ToolUseStart { id, name } if id == "edit-1" && name == "edit")
        }),
        "missing ToolUseStart: {events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            StreamEvent::ToolInputDelta(delta) if delta.contains("src/lib.rs") && delta.contains("fn new() {}")
        )),
        "missing edit input: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::ToolUseEnd)),
        "missing ToolUseEnd: {events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            StreamEvent::ToolResult { tool_use_id, content, is_error }
                if tool_use_id == "edit-1" && !*is_error && content.contains("-fn old() {}") && content.contains("+fn new() {}")
        )),
        "missing ToolResult diff: {events:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn fake_subprocess_resumes_without_history_replay_or_model_reset() {
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("resume.jsonl");
    let provider = GrokBuildProvider::with_process(fake_process(&log));
    provider.prefetch_models().await.unwrap();
    provider.set_model("grok-code-fast-1").unwrap();

    let mut stream = provider
        .complete(
            &[
                Message::user("old prompt"),
                Message::assistant_text("old answer"),
                Message::user("new prompt"),
            ],
            &[],
            "outer-system",
            Some("existing-session"),
        )
        .await
        .unwrap();
    while stream.next().await.is_some() {}

    let requests = std::fs::read_to_string(&log).unwrap();
    assert!(requests.contains("\"method\":\"session/resume\""));
    assert!(requests.contains("\"sessionId\":\"existing-session\""));
    assert!(!requests.contains("old answer"));
    assert!(requests.contains("new prompt"));
    assert!(!requests.contains("\"method\":\"session/set_model\""));
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_stream_cancels_prompt_and_terminates_subprocess() {
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("cancel.jsonl");
    let mut process = fake_process(&log);
    process
        .env
        .insert("JCODE_FAKE_GROK_ACP_HANG".to_string(), "1".to_string());
    let provider = GrokBuildProvider::with_process(process);

    let mut stream = provider
        .complete(&[Message::user("wait")], &[], "", None)
        .await
        .unwrap();
    let session = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
        .await
        .expect("session setup timed out")
        .expect("stream closed before session setup")
        .unwrap();
    assert!(matches!(session, StreamEvent::SessionId(_)));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    drop(stream);

    let mut cancelled = false;
    for _ in 0..40 {
        let requests = std::fs::read_to_string(&log).unwrap_or_default();
        if requests.contains("\"method\":\"session/cancel\"") {
            cancelled = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(cancelled, "stream drop did not send session/cancel");
}
