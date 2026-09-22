use futures::StreamExt;
use jcode_message_types::{ContentBlock, Message, Role, StreamEvent};
use jcode_provider_core::Provider;
use jcode_provider_grok_build_runtime::{GrokBuildProcess, GrokBuildProvider};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct EnvVarGuard {
    key: &'static str,
    prev: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let prev = std::env::var_os(key);
        // SAFETY: tests that mutate process env hold `env_lock`.
        unsafe { std::env::set_var(key, value) };
        Self { key, prev }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(value) => unsafe { std::env::set_var(self.key, value) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

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
    assert!(requests.contains("\"yoloMode\":false"));
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

fn tool_result(content: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "edit-1".to_string(),
            content: content.to_string(),
            is_error: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }
}

async fn drain(stream: &mut jcode_provider_core::EventStream) -> (Vec<StreamEvent>, Vec<String>) {
    let mut events = Vec::new();
    let mut errors = Vec::new();
    while let Some(event) = stream.next().await {
        match event {
            Ok(event) => events.push(event),
            Err(error) => errors.push(format!("{error:#}")),
        }
    }
    (events, errors)
}

#[tokio::test(flavor = "current_thread")]
async fn tool_completion_does_not_start_another_user_task() {
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("loop.jsonl");
    let provider = GrokBuildProvider::with_process(fake_process(&log));
    let system = "Jcode coordinator guidance. Do not wrap this as a user query.";

    let mut first = provider
        .complete(&[Message::user("print degrees")], &[], system, None)
        .await
        .unwrap();
    let (first_events, first_errors) = drain(&mut first).await;
    assert!(first_errors.is_empty(), "{first_errors:?}");
    assert!(
        first_events
            .iter()
            .any(|event| matches!(event, StreamEvent::TextDelta(text) if text == "AUTH_TEST_OK"))
    );

    let after_first = std::fs::read_to_string(&log).unwrap();
    let prompt_line = after_first
        .lines()
        .find(|line| line.contains("\"method\":\"session/prompt\""))
        .expect("first prompt");
    assert!(prompt_line.contains("print degrees"), "{prompt_line}");
    assert!(
        !prompt_line.contains("Jcode coordinator"),
        "system guidance must stay out of the user prompt: {prompt_line}"
    );
    assert!(after_first.contains("\"rules\":\"Jcode coordinator guidance"));

    let mut follow_up = provider
        .complete(
            &[
                Message::user("print degrees"),
                Message::assistant_text("printed"),
                tool_result("Print degrees from local BE /api/v2/basic"),
            ],
            &[],
            system,
            Some("fake-session-new"),
        )
        .await
        .unwrap();
    let (follow_events, follow_errors) = drain(&mut follow_up).await;
    assert!(
        follow_events.is_empty() && follow_errors.is_empty(),
        "tool completion must not start another turn: {follow_events:?} {follow_errors:?}"
    );
    let after_follow = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        after_first.matches("\"method\":\"session/prompt\"").count(),
        1
    );
    assert_eq!(
        after_follow, after_first,
        "no ACP prompt after a tool result"
    );
    assert!(!after_follow.contains("/api/v2/basic"));

    let mut real = provider
        .complete(
            &[
                Message::user("print degrees"),
                tool_result("Print degrees from local BE /api/v2/basic"),
                Message::user("what is the temperature now"),
            ],
            &[],
            system,
            Some("fake-session-new"),
        )
        .await
        .unwrap();
    let (_, real_errors) = drain(&mut real).await;
    assert!(real_errors.is_empty(), "{real_errors:?}");
    let requests = std::fs::read_to_string(&log).unwrap();
    let prompts: Vec<_> = requests
        .lines()
        .filter(|line| line.contains("\"method\":\"session/prompt\""))
        .collect();
    assert_eq!(prompts.len(), 2, "{requests}");
    let second = prompts[1];
    assert!(second.contains("what is the temperature now"), "{second}");
    assert!(!second.contains("print degrees"), "{second}");
    assert!(!second.contains("Jcode coordinator"), "{second}");
    assert!(!second.contains("/api/v2/basic"), "{second}");
}

#[tokio::test(flavor = "current_thread")]
async fn resume_uses_stored_workspace_and_only_missing_session_starts_over() {
    let _guard = env_lock();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set("GROK_HOME", temp.path());
    let session_id = "sess-cwd-drift";
    let encoded = temp
        .path()
        .join("sessions")
        .join("%2Fmnt%2Fc%2FUsers%2FHASAKI");
    std::fs::create_dir_all(encoded.join(session_id)).unwrap();

    let log = temp.path().join("cwd.jsonl");
    let provider = GrokBuildProvider::with_process(fake_process(&log));
    let mut stream = provider
        .complete(
            &[Message::user("continue in the original workspace")],
            &[],
            "system guidance",
            Some(session_id),
        )
        .await
        .unwrap();
    let (events, errors) = drain(&mut stream).await;
    assert!(
        errors.is_empty(),
        "stored workspace resume failed: {errors:?} {events:?}"
    );
    let requests = std::fs::read_to_string(&log).unwrap();
    let resume = requests
        .lines()
        .find(|line| line.contains("\"method\":\"session/resume\""))
        .expect("session/resume");
    assert!(resume.contains(session_id), "{resume}");
    assert!(
        resume.contains("/mnt/c/Users/HASAKI"),
        "resume cwd drifted away from the stored workspace: {resume}"
    );
    assert!(!requests.contains("\"method\":\"session/new\""));
    let prompt = requests
        .lines()
        .find(|line| line.contains("\"method\":\"session/prompt\""))
        .expect("prompt");
    assert!(
        !prompt.contains("system guidance"),
        "system guidance leaked into the resumed prompt: {prompt}"
    );

    for (kind, needle) in [
        ("auth", "authentication failed"),
        ("transport", "connection reset"),
    ] {
        let log = temp.path().join(format!("{kind}.jsonl"));
        let mut process = fake_process(&log);
        process
            .env
            .insert("JCODE_FAKE_GROK_ACP_RESUME_ERROR".into(), kind.into());
        let provider = GrokBuildProvider::with_process(process);
        let mut stream = provider
            .complete(
                &[Message::user("again")],
                &[],
                "system guidance",
                Some(session_id),
            )
            .await
            .unwrap();
        let (events, errors) = drain(&mut stream).await;
        let detail = errors.join("\n");
        assert!(
            detail.contains(needle),
            "{kind}: {detail} events={events:?}"
        );
        let requests = std::fs::read_to_string(&log).unwrap();
        assert!(
            requests.contains("\"method\":\"session/resume\""),
            "{requests}"
        );
        assert!(
            !requests.contains("\"method\":\"session/new\""),
            "{kind} error started a new conversation: {requests}"
        );
        assert!(
            !requests.contains("\"method\":\"session/prompt\""),
            "{kind} error still prompted: {requests}"
        );
    }

    let log = temp.path().join("missing.jsonl");
    let mut process = fake_process(&log);
    process
        .env
        .insert("JCODE_FAKE_GROK_ACP_RESUME_ERROR".into(), "missing".into());
    let provider = GrokBuildProvider::with_process(process);
    let mut stream = provider
        .complete(
            &[Message::user("session folder is gone")],
            &[],
            "system guidance",
            Some("missing-session"),
        )
        .await
        .unwrap();
    let (events, errors) = drain(&mut stream).await;
    assert!(
        errors.is_empty(),
        "genuine FS_NOT_FOUND should fall back to session/new: {errors:?} {events:?}"
    );
    let requests = std::fs::read_to_string(&log).unwrap();
    assert!(requests.contains("\"method\":\"session/resume\""));
    assert!(
        requests.contains("\"method\":\"session/new\""),
        "{requests}"
    );
    assert!(requests.contains("session folder is gone"));
}

#[tokio::test(flavor = "current_thread")]
async fn repeated_edit_updates_render_diff_and_failed_update_is_kept() {
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("stages.jsonl");
    let mut process = fake_process(&log);
    process
        .env
        .insert("JCODE_FAKE_GROK_ACP_EDIT_STAGES".into(), "1".into());
    let provider = GrokBuildProvider::with_process(process);
    let mut stream = provider
        .complete(&[Message::user("edit the file")], &[], "", None)
        .await
        .unwrap();
    let (events, errors) = drain(&mut stream).await;
    assert!(errors.is_empty(), "{errors:?}");
    let inputs: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::ToolInputDelta(delta) => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        inputs.iter().any(|delta| delta.contains("fn mid()")),
        "in-progress update dropped: {inputs:?}"
    );
    assert!(
        inputs
            .last()
            .is_some_and(|delta| delta.contains("fn new()")),
        "latest diff was not kept: {inputs:?}"
    );
    let results: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::ToolResult {
                content, is_error, ..
            } => Some((*is_error, content.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        results.len(),
        1,
        "preview must not finalize early: {events:?}"
    );
    assert!(
        !results[0].0 && results[0].1.contains("+fn new()"),
        "{results:?}"
    );

    let log = temp.path().join("fail.jsonl");
    let mut process = fake_process(&log);
    process
        .env
        .insert("JCODE_FAKE_GROK_ACP_EDIT_FAIL".into(), "1".into());
    let provider = GrokBuildProvider::with_process(process);
    let mut stream = provider
        .complete(&[Message::user("edit then fail")], &[], "", None)
        .await
        .unwrap();
    let (events, errors) = drain(&mut stream).await;
    assert!(errors.is_empty(), "{errors:?}");
    assert!(
        events.iter().any(|event| matches!(
            event,
            StreamEvent::ToolResult { is_error: true, content, .. }
                if content.contains("write failed") || content.contains("+fn new()")
        )),
        "failed transition dropped: {events:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, StreamEvent::ToolResult { .. }))
            .count(),
        1,
        "in-progress preview must not finalize before Failed: {events:?}"
    );

    let log = temp.path().join("write.jsonl");
    let mut process = fake_process(&log);
    process
        .env
        .insert("JCODE_FAKE_GROK_ACP_WRITE".into(), "1".into());
    let provider = GrokBuildProvider::with_process(process);
    let mut stream = provider
        .complete(&[Message::user("create a file")], &[], "", None)
        .await
        .unwrap();
    let (events, errors) = drain(&mut stream).await;
    assert!(errors.is_empty(), "{errors:?}");
    assert!(
        events.iter().any(|event| matches!(
            event,
            StreamEvent::ToolUseStart { name, .. } if name == "write"
        )),
        "new file was not a write tool: {events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            StreamEvent::ToolResult { is_error: false, content, .. }
                if content.contains("+fn created()")
        )),
        "write diff was not rendered: {events:?}"
    );
}
