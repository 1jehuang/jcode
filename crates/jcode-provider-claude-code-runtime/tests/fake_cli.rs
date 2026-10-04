//! End-to-end tests of the Claude Code runtime against a fake `claude`
//! (`tests/fake_claude.py`) that speaks the Agent SDK stream-json protocol.

use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::StreamExt;
use jcode_message_types::{Message, StreamEvent, ToolDefinition};
use jcode_provider_claude_code_runtime::ClaudeCodeProvider;
use jcode_provider_claude_code_runtime::settings::{ClaudeCodeInstance, ClaudeCodeSettings};
use jcode_provider_core::{AccountPin, AccountProviderKind, NativeToolResult, Provider};
use serde_json::{Value, json};

struct Fake {
    _dir: tempfile::TempDir,
    root: PathBuf,
    log: PathBuf,
}

impl Fake {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let log = root.join("argv.log");
        // A tiny wrapper so each test has its own state dir and argv log.
        let wrapper = root.join("claude");
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake_claude.py");
        std::fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nFAKE_CLAUDE_STATE='{}' FAKE_CLAUDE_LOG='{}' exec python3 '{}' \"$@\"\n",
                root.display(),
                log.display(),
                script.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self {
            _dir: dir,
            root,
            log,
        }
    }

    fn settings(&self) -> ClaudeCodeSettings {
        ClaudeCodeSettings {
            binary: self.root.join("claude").display().to_string(),
            ..ClaudeCodeSettings::default()
        }
    }

    fn provider(&self) -> ClaudeCodeProvider {
        ClaudeCodeProvider::with_settings(self.settings())
    }

    fn launches(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }
}

fn tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition::new("bash", "Run a shell command", json!({"type": "object"})),
        ToolDefinition::new(
            "echo",
            "Echo text back",
            json!({"type": "object", "properties": {"text": {"type": "string"}}}),
        ),
    ]
}

const SYSTEM: &str = "You are jcode.";

/// Answers a jcode tool call the way the agent loop would.
type ToolAnswer<'a> = &'a dyn Fn(&str, &Value) -> NativeToolResult;

/// Run one turn and collect every event. Answers NativeToolCalls the way the
/// jcode agent loop does when `answer` is set.
async fn run_turn(
    provider: &ClaudeCodeProvider,
    messages: &[Message],
    answer: Option<ToolAnswer<'_>>,
) -> Vec<StreamEvent> {
    let mut stream = provider
        .complete(messages, &tools(), SYSTEM, None)
        .await
        .unwrap();
    let mut events = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while let Ok(Some(item)) = tokio::time::timeout_at(deadline, stream.next()).await {
        let event = item.unwrap();
        if let (
            Some(answer),
            StreamEvent::NativeToolCall {
                request_id,
                tool_name,
                input,
            },
        ) = (answer, &event)
        {
            let mut result = answer(tool_name, input);
            result.request_id = request_id.clone();
            provider
                .native_result_sender()
                .unwrap()
                .send(result)
                .await
                .unwrap();
        }
        let done = matches!(event, StreamEvent::MessageEnd { .. });
        events.push(event);
        if done {
            break;
        }
    }
    events
}

fn text(events: &[StreamEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::TextDelta(t) => Some(t.as_str()),
            _ => None,
        })
        .collect()
}

fn errors(events: &[StreamEvent]) -> Vec<(String, Option<u64>)> {
    events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::Error {
                message,
                retry_after_secs,
            } => Some((message.clone(), *retry_after_secs)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn text_turn_streams_once_and_reuses_the_child() {
    let fake = Fake::new();
    let provider = fake.provider();
    let events = run_turn(&provider, &[Message::user("text please")], None).await;
    assert_eq!(text(&events), "Hello from fake claude");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::SessionId(_)))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::TokenUsage { .. }))
    );
    assert!(errors(&events).is_empty(), "{events:?}");

    let limits = provider.rate_limit_snapshot().expect("rate limits stored");
    assert_eq!(limits.five_hour.unwrap().utilization, Some(0.25));

    // Second turn goes to the same child: still one launch.
    let events = run_turn(
        &provider,
        &[
            Message::user("text please"),
            Message::assistant_text("Hello from fake claude"),
            Message::user("text again"),
        ],
        None,
    )
    .await;
    assert_eq!(text(&events), "Hello from fake claude");
    assert_eq!(fake.launches().len(), 1);

    // The CLI's model list replaces the fallback catalog.
    assert_eq!(
        provider.available_models_display(),
        vec!["claude-opus-5-5".to_string(), "claude-sonnet-5".to_string()]
    );
    provider.shutdown().await;
}

#[tokio::test]
async fn spawn_argv_matches_the_sdk_protocol() {
    let fake = Fake::new();
    let provider = fake.provider();
    provider.set_reasoning_effort("high").unwrap();
    run_turn(&provider, &[Message::user("text")], None).await;
    let launch = &fake.launches()[0];
    let argv: Vec<&str> = launch["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    for flag in [
        "--output-format",
        "stream-json",
        "--input-format",
        "--permission-prompt-tool",
        "stdio",
        "--include-partial-messages",
        "--mcp-config",
    ] {
        assert!(argv.contains(&flag), "missing {flag} in {argv:?}");
    }
    assert!(argv.iter().any(|a| a.starts_with("--session-id=")));
    assert!(argv.windows(2).any(|w| w == ["--effort", "high"]));
    assert!(argv.windows(2).any(|w| w == ["--model", "claude-opus-5-5"]));
    assert!(
        launch["config_dir"].is_null(),
        "default instance has no CLAUDE_CONFIG_DIR"
    );
    provider.shutdown().await;
}

#[tokio::test]
async fn builtin_tool_turn_renders_tool_use_and_result() {
    let fake = Fake::new();
    let provider = fake.provider();
    let events = run_turn(&provider, &[Message::user("tool")], None).await;
    let start = events
        .iter()
        .position(|e| matches!(e, StreamEvent::ToolUseStart { name, .. } if name == "Read"));
    let result = events.iter().position(|e| {
        matches!(e, StreamEvent::ToolResult { content, is_error: false, .. } if content.contains("hello"))
    });
    assert!(start.is_some() && result.is_some(), "{events:?}");
    assert!(start < result);
    assert!(text(&events).contains("The file says hello"));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, StreamEvent::NativeToolCall { .. })),
        "built-in tools never round-trip through jcode"
    );
    provider.shutdown().await;
}

#[tokio::test]
async fn permission_request_is_answered_by_mode() {
    let fake = Fake::new();
    let provider = fake.provider();
    let events = run_turn(&provider, &[Message::user("perm")], None).await;
    assert_eq!(text(&events), "permission:allow");
    provider.shutdown().await;

    let fake = Fake::new();
    let provider = ClaudeCodeProvider::with_settings(ClaudeCodeSettings {
        permission_mode: "dontAsk".into(),
        ..fake.settings()
    });
    let events = run_turn(&provider, &[Message::user("perm")], None).await;
    assert_eq!(text(&events), "permission:deny");
    provider.shutdown().await;
}

#[tokio::test]
async fn jcode_tool_is_bridged_over_sdk_mcp() {
    let fake = Fake::new();
    let provider = fake.provider();
    let answer = |name: &str, input: &Value| {
        assert_eq!(name, "echo");
        NativeToolResult::success(
            String::new(),
            format!("echo:{}", input["text"].as_str().unwrap()),
        )
    };
    let events = run_turn(&provider, &[Message::user("mcp echo")], Some(&answer)).await;
    assert!(
        events.iter().any(|e| matches!(
            e,
            StreamEvent::NativeToolCall { tool_name, input, .. }
                if tool_name == "echo" && input["text"] == "MCPOK"
        )),
        "{events:?}"
    );
    // Rendered under the bare jcode tool name.
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::ToolUseStart { name, .. } if name == "echo"))
    );
    assert!(text(&events).contains("mcp:echo:MCPOK"), "{events:?}");
    provider.shutdown().await;
}

#[tokio::test]
async fn native_duplicate_tools_are_not_exposed() {
    let fake = Fake::new();
    let provider = fake.provider();
    let events = run_turn(&provider, &[Message::user("mcp bash")], None).await;
    // `bash` is not served over MCP, so the call is refused without reaching jcode.
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, StreamEvent::NativeToolCall { .. }))
    );
    assert!(text(&events).contains("Unknown jcode tool"), "{events:?}");
    provider.shutdown().await;
}

#[tokio::test]
async fn rejected_rate_limit_stops_the_turn_with_retry_after() {
    let fake = Fake::new();
    let provider = fake.provider();
    let mut stream = provider
        .complete(&[Message::user("ratelimit")], &tools(), SYSTEM, None)
        .await
        .unwrap();
    let mut events = Vec::new();
    while let Ok(Some(item)) = tokio::time::timeout(Duration::from_secs(20), stream.next()).await {
        events.push(item.unwrap());
    }
    let errs = errors(&events);
    assert_eq!(errs.len(), 1, "{events:?}");
    assert!(errs[0].0.contains("5-hour limit"), "{errs:?}");
    assert!(errs[0].1.unwrap() > 3600);
    // The child survived the interrupt and serves the next turn.
    let events = run_turn(&provider, &[Message::user("text")], None).await;
    assert_eq!(text(&events), "Hello from fake claude");
    assert_eq!(fake.launches().len(), 1);
    provider.shutdown().await;
}

#[tokio::test]
async fn auth_failure_points_at_the_instance_login() {
    let fake = Fake::new();
    let home = fake.root.join("work-home");
    let provider = ClaudeCodeProvider::with_settings(ClaudeCodeSettings {
        default_instance: "work".into(),
        instances: vec![ClaudeCodeInstance {
            id: "work".into(),
            home: Some(home.display().to_string()),
            ..ClaudeCodeInstance::default()
        }],
        ..fake.settings()
    });
    let events = run_turn(&provider, &[Message::user("authfail")], None).await;
    let errs = errors(&events);
    assert_eq!(errs.len(), 1, "{events:?}");
    assert!(
        errs[0]
            .0
            .contains(&format!("CLAUDE_CONFIG_DIR={}", home.display())),
        "{errs:?}"
    );
    assert!(errs[0].0.contains("auth login"));
    assert_eq!(
        fake.launches()[0]["config_dir"],
        json!(home.display().to_string())
    );
    provider.shutdown().await;
}

#[tokio::test]
async fn crash_restarts_with_resume_once() {
    let fake = Fake::new();
    let provider = fake.provider();
    let first = run_turn(&provider, &[Message::user("session")], None).await;
    let session = text(&first);
    assert!(session.starts_with("session:new:"), "{session}");
    let session_id = session.trim_start_matches("session:new:").to_string();

    let events = run_turn(&provider, &[Message::user("crash")], None).await;
    assert_eq!(text(&events), "recovered after crash", "{events:?}");
    let launches = fake.launches();
    assert_eq!(launches.len(), 2);
    let argv = launches[1]["argv"].as_array().unwrap();
    assert!(
        argv.iter()
            .any(|a| a.as_str() == Some(&format!("--resume={session_id}"))),
        "{argv:?}"
    );
    provider.shutdown().await;
}

#[tokio::test]
async fn unknown_resume_id_falls_back_to_a_new_session() {
    let fake = Fake::new();
    let provider = fake.provider();
    let mut stream = provider
        .complete(
            &[Message::user("session")],
            &tools(),
            SYSTEM,
            Some("6f1d2c3b-0000-4000-8000-000000000000"),
        )
        .await
        .unwrap();
    let mut events = Vec::new();
    while let Ok(Some(item)) = tokio::time::timeout(Duration::from_secs(20), stream.next()).await {
        let event = item.unwrap();
        let done = matches!(event, StreamEvent::MessageEnd { .. });
        events.push(event);
        if done {
            break;
        }
    }
    assert!(text(&events).starts_with("session:new:"), "{events:?}");
    assert_eq!(
        fake.launches().len(),
        2,
        "failed resume, then a new session"
    );
    provider.shutdown().await;
}

#[tokio::test]
async fn dropping_the_stream_interrupts_and_keeps_the_child() {
    let fake = Fake::new();
    let provider = fake.provider();
    let mut stream = provider
        .complete(&[Message::user("slow")], &tools(), SYSTEM, None)
        .await
        .unwrap();
    // Wait for the turn to start, then cancel like Esc does.
    let first = tokio::time::timeout(Duration::from_secs(20), stream.next())
        .await
        .unwrap();
    assert!(first.is_some());
    drop(stream);
    let events = run_turn(&provider, &[Message::user("text")], None).await;
    assert_eq!(text(&events), "Hello from fake claude");
    assert_eq!(fake.launches().len(), 1, "interrupt keeps the child alive");
    provider.shutdown().await;
}

#[tokio::test]
async fn history_from_another_provider_is_seeded_once() {
    let fake = Fake::new();
    let provider = fake.provider();
    let history = [
        Message::user("what is 2+2"),
        Message::assistant_text("4"),
        Message::user("echo-prompt"),
    ];
    let events = run_turn(&provider, &history, None).await;
    let sent = text(&events);
    assert!(sent.contains("previous_conversation"), "{sent}");
    assert!(sent.contains("User: what is 2+2"), "{sent}");
    assert!(sent.contains("Assistant: 4"), "{sent}");

    let mut more = history.to_vec();
    more.push(Message::assistant_text(&sent));
    more.push(Message::user("echo-prompt"));
    let events = run_turn(&provider, &more, None).await;
    assert!(!text(&events).contains("previous_conversation"));
    provider.shutdown().await;
}

#[tokio::test]
async fn model_switch_uses_set_model_and_instance_switch_restarts() {
    let fake = Fake::new();
    let other_home = fake.root.join("personal");
    let provider = ClaudeCodeProvider::with_settings(ClaudeCodeSettings {
        instances: vec![
            ClaudeCodeInstance {
                id: "default".into(),
                ..ClaudeCodeInstance::default()
            },
            ClaudeCodeInstance {
                id: "personal".into(),
                home: Some(other_home.display().to_string()),
                ..ClaudeCodeInstance::default()
            },
        ],
        ..fake.settings()
    });
    run_turn(&provider, &[Message::user("text")], None).await;
    provider.set_model("claude-code:claude-sonnet-5").unwrap();
    let events = run_turn(&provider, &[Message::user("model")], None).await;
    assert_eq!(text(&events), "model:claude-sonnet-5");
    assert_eq!(fake.launches().len(), 1, "set_model is a control request");

    provider
        .set_account_pin(
            AccountProviderKind::ClaudeCode,
            Some(AccountPin::new("personal", None)),
        )
        .unwrap();
    assert!(
        provider
            .set_account_pin(
                AccountProviderKind::ClaudeCode,
                Some(AccountPin::new("missing", None)),
            )
            .is_err()
    );
    let events = run_turn(&provider, &[Message::user("session")], None).await;
    assert!(
        text(&events).starts_with("session:new:"),
        "another account starts a new Claude session"
    );
    let launches = fake.launches();
    assert_eq!(launches.len(), 2);
    assert_eq!(
        launches[1]["config_dir"],
        json!(other_home.display().to_string())
    );
    assert_eq!(
        provider.resolved_account_label(AccountProviderKind::ClaudeCode),
        Some("personal".into())
    );
    provider.shutdown().await;
}

#[tokio::test]
async fn complete_simple_uses_a_one_shot_child() {
    let fake = Fake::new();
    let provider = fake.provider();
    let out = provider
        .complete_simple("summarize this", "be brief")
        .await
        .unwrap();
    assert_eq!(out, "oneshot:summarize this");
    let argv = fake.launches()[0]["argv"].clone();
    let argv: Vec<&str> = argv
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(argv.contains(&"-p"));
    assert!(
        argv.windows(2)
            .any(|w| w == ["--system-prompt", "be brief"])
    );
}

#[tokio::test]
async fn probe_reports_identity_without_a_prompt() {
    let fake = Fake::new();
    let provider = fake.provider();
    let identity = provider
        .probe_identity(&ClaudeCodeInstance::implicit_default())
        .await
        .unwrap();
    assert_eq!(identity.email.as_deref(), Some("fake@example.com"));
    assert_eq!(identity.subscription.as_deref(), Some("Claude Max"));
    assert_eq!(identity.models, vec!["claude-opus-5-5", "claude-sonnet-5"]);
    // Cached: a second probe does not spawn again.
    provider
        .probe_identity(&ClaudeCodeInstance::implicit_default())
        .await
        .unwrap();
    assert_eq!(fake.launches().len(), 1);
}

#[tokio::test]
async fn forks_get_their_own_child() {
    let fake = Fake::new();
    let provider = fake.provider();
    run_turn(&provider, &[Message::user("text")], None).await;
    let fork = provider.fork();
    assert!(fork.handles_tools_internally());
    let mut stream = fork
        .complete(&[Message::user("text")], &tools(), SYSTEM, None)
        .await
        .unwrap();
    while let Ok(Some(item)) = tokio::time::timeout(Duration::from_secs(20), stream.next()).await {
        if matches!(item.unwrap(), StreamEvent::MessageEnd { .. }) {
            break;
        }
    }
    assert_eq!(fake.launches().len(), 2);
    provider.shutdown().await;
}
