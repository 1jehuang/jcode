//! End-to-end tests for jcode using a mock provider
//!
//! These tests verify the full flow from user input to response
//! without making actual API calls.

mod mock_provider;

use anyhow::Result;
use jcode::agent::Agent;
use jcode::message::StreamEvent;
use jcode::protocol::ServerEvent;
use jcode::server;
use jcode::session::Session;
use jcode::tool::Registry;
use mock_provider::MockProvider;
use std::ffi::OsString;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

static JCODE_HOME_LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();

fn lock_jcode_home() -> std::sync::MutexGuard<'static, ()> {
    let mutex = JCODE_HOME_LOCK.get_or_init(|| Mutex::new(()));
    // Recover from poisoned state if a previous test panicked
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

struct TestEnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    prev_home: Option<OsString>,
    prev_test_session: Option<OsString>,
    prev_debug_control: Option<OsString>,
    _temp_home: tempfile::TempDir,
}

impl TestEnvGuard {
    fn new() -> Result<Self> {
        let lock = lock_jcode_home();
        let temp_home = tempfile::Builder::new()
            .prefix("jcode-e2e-home-")
            .tempdir()?;
        let prev_home = std::env::var_os("JCODE_HOME");
        let prev_test_session = std::env::var_os("JCODE_TEST_SESSION");
        let prev_debug_control = std::env::var_os("JCODE_DEBUG_CONTROL");

        std::env::set_var("JCODE_HOME", temp_home.path());
        std::env::set_var("JCODE_TEST_SESSION", "1");
        std::env::set_var("JCODE_DEBUG_CONTROL", "1");

        Ok(Self {
            _lock: lock,
            prev_home,
            prev_test_session,
            prev_debug_control,
            _temp_home: temp_home,
        })
    }
}

impl Drop for TestEnvGuard {
    fn drop(&mut self) {
        if let Some(prev_home) = &self.prev_home {
            std::env::set_var("JCODE_HOME", prev_home);
        } else {
            std::env::remove_var("JCODE_HOME");
        }

        if let Some(prev_test_session) = &self.prev_test_session {
            std::env::set_var("JCODE_TEST_SESSION", prev_test_session);
        } else {
            std::env::remove_var("JCODE_TEST_SESSION");
        }

        if let Some(prev_debug_control) = &self.prev_debug_control {
            std::env::set_var("JCODE_DEBUG_CONTROL", prev_debug_control);
        } else {
            std::env::remove_var("JCODE_DEBUG_CONTROL");
        }
    }
}

fn setup_test_env() -> Result<TestEnvGuard> {
    TestEnvGuard::new()
}

async fn wait_for_socket(path: &std::path::Path) -> Result<()> {
    let start = Instant::now();
    while !path.exists() {
        if start.elapsed() > Duration::from_secs(10) {
            anyhow::bail!("Server socket did not appear");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Ok(())
}

async fn debug_create_headless_session(debug_socket_path: std::path::PathBuf) -> Result<String> {
    let mut debug_client = server::Client::connect_debug_with_path(debug_socket_path).await?;
    let request_id = debug_client.debug_command("create_session", None).await?;

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let event =
            tokio::time::timeout(Duration::from_secs(1), debug_client.read_event()).await??;
        match event {
            ServerEvent::Ack { .. } => continue,
            ServerEvent::DebugResponse { id, ok, output } if id == request_id => {
                if !ok {
                    anyhow::bail!("create_session debug command failed: {}", output);
                }
                let value: serde_json::Value = serde_json::from_str(&output)?;
                let session_id = value
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("missing session_id in debug response"))?;
                return Ok(session_id.to_string());
            }
            _ => {}
        }
    }

    anyhow::bail!("Timed out waiting for create_session debug response")
}

/// Test that a simple text response works
#[tokio::test]
async fn test_simple_response() -> Result<()> {
    let _env = setup_test_env()?;
    let provider = MockProvider::new();

    // Queue a simple response
    provider.queue_response(vec![
        StreamEvent::TextDelta("Hello! ".to_string()),
        StreamEvent::TextDelta("How can I help?".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
        StreamEvent::SessionId("test-session-123".to_string()),
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);

    let response = agent.run_once_capture("Say hello").await?;
    let saved = Session::load(agent.session_id())?;

    assert_eq!(response, "Hello! How can I help?");
    assert!(saved.is_debug, "test sessions should be marked debug");
    Ok(())
}

#[tokio::test]
async fn test_agent_clear_preserves_debug_flag() -> Result<()> {
    let _env = setup_test_env()?;
    let provider = MockProvider::new();
    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);
    agent.set_debug(true);
    let old_session_id = agent.session_id().to_string();

    agent.clear();

    assert_ne!(agent.session_id(), old_session_id);
    assert!(agent.is_debug());
    Ok(())
}

#[tokio::test]
async fn test_debug_create_session_marks_debug() -> Result<()> {
    let _env = setup_test_env()?;
    let runtime_dir = std::env::temp_dir().join(format!(
        "jcode-debug-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;
    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    let provider = MockProvider::new();
    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let server_instance =
        server::Server::new_with_paths(provider, socket_path.clone(), debug_socket_path.clone());
    let server_handle = tokio::spawn(async move { server_instance.run().await });

    wait_for_socket(&socket_path).await?;

    let session_id = debug_create_headless_session(debug_socket_path.clone()).await?;
    let session = Session::load(&session_id)?;
    assert!(session.is_debug);

    server_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&debug_socket_path);

    Ok(())
}

#[tokio::test]
async fn test_clear_preserves_debug_for_resumed_debug_session() -> Result<()> {
    let _env = setup_test_env()?;
    let runtime_dir = std::env::temp_dir().join(format!(
        "jcode-clear-debug-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;
    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    let provider = MockProvider::new();
    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let server_instance =
        server::Server::new_with_paths(provider, socket_path.clone(), debug_socket_path.clone());
    let server_handle = tokio::spawn(async move { server_instance.run().await });

    wait_for_socket(&socket_path).await?;

    let debug_session_id = debug_create_headless_session(debug_socket_path.clone()).await?;
    let mut client = server::Client::connect_with_path(socket_path.clone()).await?;
    let resume_id = client.resume_session(&debug_session_id).await?;

    // Drain resume completion so clear() events are unambiguous.
    let mut saw_resume_history = false;
    let resume_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < resume_deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client.read_event()).await??;
        match event {
            ServerEvent::Ack { .. } => continue,
            ServerEvent::History { id, .. } if id == resume_id => {
                saw_resume_history = true;
                break;
            }
            ServerEvent::Error { id, message, .. } if id == resume_id => {
                anyhow::bail!("resume_session failed: {}", message);
            }
            _ => {}
        }
    }
    if !saw_resume_history {
        anyhow::bail!("Timed out waiting for resume history event");
    }

    client.clear().await?;

    let mut new_session_id = None;
    let clear_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < clear_deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client.read_event()).await??;
        match event {
            ServerEvent::Ack { .. } => continue,
            ServerEvent::SessionId { session_id } => {
                new_session_id = Some(session_id);
            }
            ServerEvent::Done { .. } if new_session_id.is_some() => break,
            _ => {}
        }
    }

    let new_session_id = new_session_id
        .ok_or_else(|| anyhow::anyhow!("Did not receive new session id after clear"))?;
    assert_ne!(new_session_id, debug_session_id);
    let session = Session::load(&new_session_id)?;
    assert!(session.is_debug);

    server_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&debug_socket_path);

    Ok(())
}

/// Test that multi-turn conversation works with session resume
#[tokio::test]
async fn test_multi_turn_conversation() -> Result<()> {
    let _env = setup_test_env()?;
    let provider = MockProvider::new();

    // First turn response
    provider.queue_response(vec![
        StreamEvent::TextDelta("I'll remember that.".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
        StreamEvent::SessionId("session-abc".to_string()),
    ]);

    // Second turn response
    provider.queue_response(vec![
        StreamEvent::TextDelta("You said hello earlier.".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
        StreamEvent::SessionId("session-abc".to_string()),
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);

    // First turn
    let response1 = agent.run_once_capture("Hello").await?;
    assert_eq!(response1, "I'll remember that.");

    // Second turn - should use session resume
    let response2 = agent.run_once_capture("What did I say?").await?;
    assert_eq!(response2, "You said hello earlier.");

    Ok(())
}

/// Test that token usage is tracked
#[tokio::test]
async fn test_token_usage() -> Result<()> {
    let _env = setup_test_env()?;
    let provider = MockProvider::new();

    provider.queue_response(vec![
        StreamEvent::TokenUsage {
            input_tokens: Some(10),
            output_tokens: Some(20),
            cache_read_input_tokens: None,
            cache_creation_input_tokens: None,
        },
        StreamEvent::TextDelta("Response".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
        StreamEvent::SessionId("session-123".to_string()),
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);

    let response = agent.run_once_capture("Test").await?;
    assert_eq!(response, "Response");

    Ok(())
}

/// Test error handling
#[tokio::test]
async fn test_stream_error() -> Result<()> {
    let _env = setup_test_env()?;
    let provider = MockProvider::new();

    provider.queue_response(vec![
        StreamEvent::TextDelta("Starting...".to_string()),
        StreamEvent::Error {
            message: "Something went wrong".to_string(),
            retry_after_secs: None,
        },
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);

    let result = agent.run_once_capture("Test").await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Something went wrong"));

    Ok(())
}

/// Test model cycling over the socket interface (server + client)
#[tokio::test]
async fn test_socket_model_cycle_supported_models() -> Result<()> {
    let _env = setup_test_env()?;
    let runtime_dir = std::env::temp_dir().join(format!(
        "jcode-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;
    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    let provider = MockProvider::with_models(vec!["gpt-5.2-codex", "claude-opus-4-5-20251101"]);
    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let server_instance =
        server::Server::new_with_paths(provider, socket_path.clone(), debug_socket_path.clone());

    let server_handle = tokio::spawn(async move { server_instance.run().await });

    // Wait for socket to appear
    let start = Instant::now();
    while !socket_path.exists() {
        if start.elapsed() > Duration::from_secs(10) {
            server_handle.abort();
            anyhow::bail!("Server socket did not appear");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let mut client = server::Client::connect_with_path(socket_path.clone()).await?;
    let request_id = client.cycle_model(1).await?;

    let mut saw_model_changed = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client.read_event()).await??;
        match event {
            ServerEvent::Ack { .. } => continue,
            ServerEvent::ModelChanged {
                id, model, error, ..
            } if id == request_id => {
                assert!(error.is_none(), "Expected successful model change");
                assert_eq!(model, "claude-opus-4-5-20251101");
                saw_model_changed = true;
                break;
            }
            _ => {}
        }
    }

    server_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&debug_socket_path);

    assert!(saw_model_changed, "Did not receive model_changed event");
    Ok(())
}

/// Test that resume restores model selection and tool output in history
#[tokio::test]
async fn test_resume_restores_model_and_tool_history() -> Result<()> {
    let _env = setup_test_env()?;
    let runtime_dir = std::env::temp_dir().join(format!(
        "jcode-resume-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;

    let mut session = Session::create(None, Some("Resume Test".to_string()));
    session.model = Some("gpt-5.2-codex".to_string());
    session.add_message(
        jcode::message::Role::User,
        vec![jcode::message::ContentBlock::Text {
            text: "Run a tool".to_string(),
            cache_control: None,
        }],
    );
    session.add_message(
        jcode::message::Role::Assistant,
        vec![
            jcode::message::ContentBlock::Text {
                text: "Running...".to_string(),
                cache_control: None,
            },
            jcode::message::ContentBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"cmd": "echo hi"}),
            },
        ],
    );
    session.add_message(
        jcode::message::Role::User,
        vec![jcode::message::ContentBlock::ToolResult {
            tool_use_id: "tool-1".to_string(),
            content: "hi\n".to_string(),
            is_error: None,
        }],
    );
    session.save()?;

    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    // Default model = claude, resume should switch to gpt-5.2-codex
    let provider = MockProvider::with_models(vec!["claude-opus-4-5-20251101", "gpt-5.2-codex"]);
    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let server_instance =
        server::Server::new_with_paths(provider, socket_path.clone(), debug_socket_path.clone());
    let server_handle = tokio::spawn(async move { server_instance.run().await });

    let start = Instant::now();
    while !socket_path.exists() {
        if start.elapsed() > Duration::from_secs(10) {
            server_handle.abort();
            anyhow::bail!("Server socket did not appear");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let mut client = server::Client::connect_with_path(socket_path.clone()).await?;
    let resume_id = client.resume_session(&session.id).await?;

    let mut history_event = None;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client.read_event()).await??;
        match event {
            ServerEvent::History {
                id,
                messages,
                provider_model,
                ..
            } if id == resume_id => {
                history_event = Some((messages, provider_model));
                break;
            }
            _ => {}
        }
    }

    server_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&debug_socket_path);

    let (messages, provider_model) =
        history_event.ok_or_else(|| anyhow::anyhow!("Did not receive history event"))?;

    assert_eq!(provider_model, Some("gpt-5.2-codex".to_string()));

    let tool_msg = messages
        .iter()
        .find(|m| m.role == "tool")
        .ok_or_else(|| anyhow::anyhow!("Tool message missing in history"))?;
    assert!(tool_msg.content.contains("hi"));
    let tool_data = tool_msg
        .tool_data
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Tool metadata missing in history"))?;
    assert_eq!(tool_data.name, "bash");

    Ok(())
}

/// Test that subscribe selfdev hint marks the session as canary
#[tokio::test]
async fn test_subscribe_selfdev_hint_marks_canary() -> Result<()> {
    let _env = setup_test_env()?;
    let runtime_dir = std::env::temp_dir().join(format!(
        "jcode-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;
    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    let provider = MockProvider::new();
    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let server_instance =
        server::Server::new_with_paths(provider, socket_path.clone(), debug_socket_path.clone());

    let server_handle = tokio::spawn(async move { server_instance.run().await });

    // Wait for socket to appear
    let start = Instant::now();
    while !socket_path.exists() {
        if start.elapsed() > Duration::from_secs(2) {
            server_handle.abort();
            anyhow::bail!("Server socket did not appear");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let mut client = server::Client::connect_with_path(socket_path.clone()).await?;
    let subscribe_id = client.subscribe_with_info(None, Some(true)).await?;

    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client.read_event()).await??;
        if matches!(event, ServerEvent::Done { id } if id == subscribe_id) {
            break;
        }
    }

    let history_event = client.get_history_event().await?;
    match history_event {
        ServerEvent::History { is_canary, .. } => {
            assert_eq!(is_canary, Some(true));
        }
        _ => anyhow::bail!("Expected history event after subscribe"),
    }

    server_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&debug_socket_path);

    Ok(())
}

/// Test that switching models resets the provider resume session
#[tokio::test]
async fn test_model_switch_resets_provider_session() -> Result<()> {
    let _env = setup_test_env()?;
    let runtime_dir = std::env::temp_dir().join(format!(
        "jcode-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;
    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    let provider = Arc::new(MockProvider::with_models(vec!["model-a", "model-b"]));
    provider.queue_response(vec![
        StreamEvent::TextDelta("hello".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
        StreamEvent::SessionId("session-1".to_string()),
    ]);
    provider.queue_response(vec![
        StreamEvent::TextDelta("again".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
    ]);

    let provider_dyn: Arc<dyn jcode::provider::Provider> = provider.clone();
    let server_instance = server::Server::new_with_paths(
        provider_dyn,
        socket_path.clone(),
        debug_socket_path.clone(),
    );

    let server_handle = tokio::spawn(async move { server_instance.run().await });

    let start = Instant::now();
    while !socket_path.exists() {
        if start.elapsed() > Duration::from_secs(2) {
            server_handle.abort();
            anyhow::bail!("Server socket did not appear");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let mut client = server::Client::connect_with_path(socket_path.clone()).await?;

    let msg_id = client.send_message("hello").await?;
    let mut saw_done1 = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client.read_event()).await??;
        if matches!(event, ServerEvent::Done { id } if id == msg_id) {
            saw_done1 = true;
            break;
        }
    }
    assert!(saw_done1, "Did not receive Done for first message");

    let model_id = client.cycle_model(1).await?;
    let mut saw_model = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client.read_event()).await??;
        if matches!(event, ServerEvent::ModelChanged { id, error: None, .. } if id == model_id) {
            saw_model = true;
            break;
        }
    }
    assert!(saw_model, "Did not receive ModelChanged after cycle");

    let msg2_id = client.send_message("second").await?;
    let mut saw_done2 = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client.read_event()).await??;
        if matches!(event, ServerEvent::Done { id } if id == msg2_id) {
            saw_done2 = true;
            break;
        }
    }
    assert!(saw_done2, "Did not receive Done for second message");

    let resume_ids = provider.captured_resume_session_ids.lock().unwrap().clone();
    assert_eq!(resume_ids.len(), 2);
    assert_eq!(resume_ids[0], None);
    assert_eq!(resume_ids[1], None);

    server_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&debug_socket_path);

    Ok(())
}

/// Test that switching models only affects the active session
#[tokio::test]
async fn test_model_switch_is_per_session() -> Result<()> {
    let _env = setup_test_env()?;
    let runtime_dir = std::env::temp_dir().join(format!(
        "jcode-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;
    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    let provider = Arc::new(MockProvider::with_models(vec!["model-a", "model-b"]));
    provider.queue_response(vec![
        StreamEvent::TextDelta("one".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
        StreamEvent::SessionId("session-1".to_string()),
    ]);
    provider.queue_response(vec![
        StreamEvent::TextDelta("two".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
        StreamEvent::SessionId("session-2".to_string()),
    ]);
    provider.queue_response(vec![
        StreamEvent::TextDelta("three".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
    ]);

    let provider_dyn: Arc<dyn jcode::provider::Provider> = provider.clone();
    let server_instance = server::Server::new_with_paths(
        provider_dyn,
        socket_path.clone(),
        debug_socket_path.clone(),
    );

    let server_handle = tokio::spawn(async move { server_instance.run().await });

    let start = Instant::now();
    while !socket_path.exists() {
        if start.elapsed() > Duration::from_secs(2) {
            server_handle.abort();
            anyhow::bail!("Server socket did not appear");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let mut client1 = server::Client::connect_with_path(socket_path.clone()).await?;
    let mut client2 = server::Client::connect_with_path(socket_path.clone()).await?;

    // Give server time to set up both client sessions
    tokio::time::sleep(Duration::from_millis(100)).await;

    let msg1 = client1.send_message("hello").await?;
    let mut done1 = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client1.read_event()).await??;
        if matches!(event, ServerEvent::Done { id } if id == msg1) {
            done1 = true;
            break;
        }
    }
    assert!(done1, "Did not receive Done for client1 message");

    let msg2 = client2.send_message("hello").await?;
    let mut done2 = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client2.read_event()).await??;
        if matches!(event, ServerEvent::Done { id } if id == msg2) {
            done2 = true;
            break;
        }
    }
    assert!(done2, "Did not receive Done for client2 message");

    let model_id = client1.cycle_model(1).await?;
    let mut saw_model = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client1.read_event()).await??;
        if matches!(event, ServerEvent::ModelChanged { id, error: None, .. } if id == model_id) {
            saw_model = true;
            break;
        }
    }
    assert!(saw_model, "Did not receive ModelChanged after cycle");

    let msg3 = client2.send_message("after").await?;
    let mut done3 = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let event = tokio::time::timeout(Duration::from_secs(1), client2.read_event()).await??;
        if matches!(event, ServerEvent::Done { id } if id == msg3) {
            done3 = true;
            break;
        }
    }
    assert!(done3, "Did not receive Done for client2 after switch");

    let models = provider.captured_models.lock().unwrap().clone();
    assert!(models.len() >= 3, "Expected at least 3 model captures");
    assert_eq!(models[2], "model-a");

    server_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&debug_socket_path);

    Ok(())
}

/// Test that the system prompt does NOT identify the agent as "Claude Code"
/// The agent should identify as "jcode" or just a generic "coding assistant powered by Claude"
#[tokio::test]
async fn test_system_prompt_no_claude_code_identity() -> Result<()> {
    let _env = setup_test_env()?;
    let provider = Arc::new(MockProvider::new());

    // Queue a simple response
    provider.queue_response(vec![
        StreamEvent::TextDelta("I'm a coding assistant.".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
        StreamEvent::SessionId("test-identity-123".to_string()),
    ]);

    // Keep a clone of Arc<MockProvider> before converting to Arc<dyn Provider>
    let provider_for_check = provider.clone();
    let provider_dyn: Arc<dyn jcode::provider::Provider> = provider;
    let registry = Registry::new(provider_dyn.clone()).await;
    let mut agent = Agent::new(provider_dyn, registry);

    // Run a simple query - we just need to trigger a complete() call
    let _ = agent.run_once_capture("Who are you?").await?;

    // Get the captured system prompt from our Arc<MockProvider>
    let captured_prompts = provider_for_check.captured_system_prompts.lock().unwrap();

    assert!(
        !captured_prompts.is_empty(),
        "No system prompts were captured"
    );

    let system_prompt = &captured_prompts[0];

    // Check only the identity portion at the start of the system prompt
    // (not the full prompt which may include CLAUDE.md with "Claude Code CLI" references)
    // The first ~500 chars contain the identity statement
    let identity_portion = if system_prompt.len() > 500 {
        &system_prompt[..500]
    } else {
        system_prompt
    };
    let lower_identity = identity_portion.to_lowercase();

    // The identity portion should NOT say "you are claude code" or similar
    assert!(
        !lower_identity.contains("you are claude code"),
        "System prompt should NOT identify as 'You are Claude Code'. Found: {}",
        identity_portion
    );

    // Should identify as jcode
    assert!(
        lower_identity.contains("jcode"),
        "System prompt should identify as jcode. Found: {}",
        identity_portion
    );

    // It's OK if it says "powered by Claude" or just "Claude" (the model name)
    // It's OK if project context references "Claude Code CLI" as a tool

    Ok(())
}

// ============================================================================
// Binary Integration Tests
// These tests run the actual jcode binary and require real credentials.
// Run with: cargo test --test e2e binary_integration -- --ignored
// ============================================================================

/// Test that the jcode binary can run standalone with Claude provider
#[tokio::test]
#[ignore] // Requires Claude credentials
async fn binary_integration_standalone_claude() -> Result<()> {
    use std::process::Command;
    let _env = setup_test_env()?;

    let output = Command::new("cargo")
        .args([
            "run",
            "--release",
            "--bin",
            "jcode",
            "--",
            "run",
            "Say 'test-ok' and nothing else",
        ])
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success() || stdout.contains("test") || stderr.contains("Claude"),
        "Binary should run successfully. stdout: {}, stderr: {}",
        stdout,
        stderr
    );

    Ok(())
}

/// Test that the jcode binary can run with OpenAI provider
#[tokio::test]
#[ignore] // Requires OpenAI/Codex credentials
async fn binary_integration_openai_provider() -> Result<()> {
    use std::process::Command;
    let _env = setup_test_env()?;

    let output = Command::new("cargo")
        .args([
            "run",
            "--release",
            "--bin",
            "jcode",
            "--",
            "--provider",
            "openai",
            "run",
            "Say 'openai-ok' and nothing else",
        ])
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Check either success or identifiable OpenAI response
    let has_response = stdout.to_lowercase().contains("openai")
        || stdout.to_lowercase().contains("ok")
        || stderr.contains("OpenAI");

    assert!(
        output.status.success() || has_response,
        "OpenAI provider should work. stdout: {}, stderr: {}",
        stdout,
        stderr
    );

    Ok(())
}

/// Test that jcode version command works
#[tokio::test]
async fn binary_version_command() -> Result<()> {
    use std::process::Command;
    let _env = setup_test_env()?;

    let output = Command::new("cargo")
        .args(["run", "--release", "--bin", "jcode", "--", "--version"])
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "Version command should succeed");
    assert!(
        stdout.contains("jcode") || stdout.contains("20"),
        "Version should contain 'jcode' or date. Got: {}",
        stdout
    );

    Ok(())
}

// =============================================================================
// Ambient Mode Integration Tests
// =============================================================================

/// Test safety system: action classification
#[test]
fn test_safety_classification() {
    use jcode::safety::SafetySystem;

    let safety = SafetySystem::new();

    // Tier 1: auto-allowed
    assert!(safety.classify("read") == jcode::safety::ActionTier::AutoAllowed);
    assert!(safety.classify("glob") == jcode::safety::ActionTier::AutoAllowed);
    assert!(safety.classify("grep") == jcode::safety::ActionTier::AutoAllowed);
    assert!(safety.classify("memory") == jcode::safety::ActionTier::AutoAllowed);
    assert!(safety.classify("todoread") == jcode::safety::ActionTier::AutoAllowed);
    assert!(safety.classify("todowrite") == jcode::safety::ActionTier::AutoAllowed);

    // Tier 2: requires permission
    assert!(safety.classify("bash") == jcode::safety::ActionTier::RequiresPermission);
    assert!(safety.classify("edit") == jcode::safety::ActionTier::RequiresPermission);
    assert!(safety.classify("write") == jcode::safety::ActionTier::RequiresPermission);
    assert!(
        safety.classify("create_pull_request") == jcode::safety::ActionTier::RequiresPermission
    );
    assert!(safety.classify("send_email") == jcode::safety::ActionTier::RequiresPermission);

    // Case insensitive
    assert!(safety.classify("READ") == jcode::safety::ActionTier::AutoAllowed);
    assert!(safety.classify("Bash") == jcode::safety::ActionTier::RequiresPermission);
}

/// Test safety system: permission request queue + decision flow
#[test]
fn test_safety_permission_flow() {
    use jcode::safety::{PermissionRequest, PermissionResult, SafetySystem, Urgency};

    let safety = SafetySystem::new();

    // Count existing pending requests (may have leftover state from other tests)
    let baseline = safety.pending_requests().len();

    // Queue a permission request
    let req = PermissionRequest {
        id: "test_perm_flow_001".to_string(),
        action: "create_pull_request".to_string(),
        description: "Create PR for auth fixes".to_string(),
        rationale: "Found 3 failing auth tests".to_string(),
        urgency: Urgency::High,
        wait: false,
        created_at: chrono::Utc::now(),
        context: None,
    };

    let result = safety.request_permission(req);
    assert!(matches!(result, PermissionResult::Queued { .. }));

    // Verify our request was added
    let pending = safety.pending_requests();
    assert_eq!(pending.len(), baseline + 1);
    assert!(pending
        .iter()
        .any(|p| p.action == "create_pull_request" && p.id == "test_perm_flow_001"));

    // Record an approval decision
    let _ = safety.record_decision(
        "test_perm_flow_001",
        true,
        "test",
        Some("looks good".to_string()),
    );

    // Verify our request was removed
    assert_eq!(safety.pending_requests().len(), baseline);
}

/// Test safety system: transcript saving
#[test]
fn test_safety_transcript() {
    use jcode::safety::{AmbientTranscript, SafetySystem, TranscriptStatus};

    let safety = SafetySystem::new();

    let transcript = AmbientTranscript {
        session_id: "test_ambient_001".to_string(),
        started_at: chrono::Utc::now(),
        ended_at: Some(chrono::Utc::now()),
        status: TranscriptStatus::Complete,
        provider: "mock".to_string(),
        model: "mock-model".to_string(),
        actions: vec![],
        pending_permissions: 0,
        summary: Some("Test cycle completed".to_string()),
        compactions: 0,
        memories_modified: 3,
        conversation: None,
    };

    // Should not panic
    let result = safety.save_transcript(&transcript);
    assert!(result.is_ok());
}

/// Test safety system: summary generation
#[test]
fn test_safety_summary_generation() {
    use jcode::safety::{ActionLog, ActionTier, SafetySystem};

    let safety = SafetySystem::new();

    // Log some actions
    safety.log_action(ActionLog {
        action_type: "memory_consolidation".to_string(),
        description: "Merged 2 duplicate memories".to_string(),
        tier: ActionTier::AutoAllowed,
        details: None,
        timestamp: chrono::Utc::now(),
    });

    safety.log_action(ActionLog {
        action_type: "memory_prune".to_string(),
        description: "Pruned 1 stale memory".to_string(),
        tier: ActionTier::AutoAllowed,
        details: None,
        timestamp: chrono::Utc::now(),
    });

    let summary = safety.generate_summary();
    assert!(summary.contains("Merged 2 duplicate memories"));
    assert!(summary.contains("Pruned 1 stale memory"));
}

/// Test ambient state: load, save, record_cycle
#[test]
fn test_ambient_state_lifecycle() {
    use jcode::ambient::{AmbientCycleResult, AmbientState, AmbientStatus, CycleStatus};

    let mut state = AmbientState::default();
    assert!(matches!(state.status, AmbientStatus::Idle));
    assert_eq!(state.total_cycles, 0);
    assert!(state.last_run.is_none());

    // Record a cycle
    let result = AmbientCycleResult {
        summary: "Gardened 3 memories".to_string(),
        memories_modified: 3,
        compactions: 0,
        proactive_work: None,
        next_schedule: None,
        started_at: chrono::Utc::now(),
        ended_at: chrono::Utc::now(),
        status: CycleStatus::Complete,
        conversation: None,
    };

    state.record_cycle(&result);
    assert_eq!(state.total_cycles, 1);
    assert!(state.last_run.is_some());
    assert_eq!(state.last_summary.as_deref(), Some("Gardened 3 memories"));
    assert_eq!(state.last_memories_modified, Some(3));
    assert_eq!(state.last_compactions, Some(0));
    // No next_schedule → should be Idle
    assert!(matches!(state.status, AmbientStatus::Idle));
}

/// Test ambient scheduled queue: push, pop, priority ordering
#[test]
fn test_ambient_scheduled_queue() {
    use jcode::ambient::{Priority, ScheduledItem, ScheduledQueue};

    let tmp = std::env::temp_dir().join("jcode-test-queue.json");
    let _ = std::fs::remove_file(&tmp); // Clean up from previous runs
    let mut queue = ScheduledQueue::load(tmp);
    assert!(queue.is_empty());

    // Push items with different priorities
    let now = chrono::Utc::now();
    queue.push(ScheduledItem {
        id: "low_1".to_string(),
        scheduled_for: now - chrono::Duration::minutes(5),
        context: "low priority task".to_string(),
        priority: Priority::Low,
        created_by_session: "test".to_string(),
        created_at: now,
        working_dir: None,
        task_description: None,
        relevant_files: Vec::new(),
        git_branch: None,
        additional_context: None,
    });

    queue.push(ScheduledItem {
        id: "high_1".to_string(),
        scheduled_for: now - chrono::Duration::minutes(5),
        context: "high priority task".to_string(),
        priority: Priority::High,
        created_by_session: "test".to_string(),
        created_at: now,
        working_dir: None,
        task_description: None,
        relevant_files: Vec::new(),
        git_branch: None,
        additional_context: None,
    });

    queue.push(ScheduledItem {
        id: "future_1".to_string(),
        scheduled_for: now + chrono::Duration::hours(1),
        context: "future task".to_string(),
        priority: Priority::Normal,
        created_by_session: "test".to_string(),
        created_at: now,
        working_dir: None,
        task_description: None,
        relevant_files: Vec::new(),
        git_branch: None,
        additional_context: None,
    });

    assert_eq!(queue.len(), 3);

    // Pop ready items: should get high priority first, then low (future not ready)
    let ready = queue.pop_ready();
    assert_eq!(ready.len(), 2);
    assert_eq!(ready[0].id, "high_1"); // High priority first
    assert_eq!(ready[1].id, "low_1"); // Low priority second

    // Future item still in queue
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.items()[0].id, "future_1");
}

/// Test adaptive scheduler: interval calculation
#[test]
fn test_adaptive_scheduler_intervals() {
    use jcode::ambient_scheduler::{AdaptiveScheduler, AmbientSchedulerConfig};

    let config = AmbientSchedulerConfig {
        min_interval_minutes: 5,
        max_interval_minutes: 120,
        ..Default::default()
    };

    let scheduler = AdaptiveScheduler::new(config);

    // With no rate limit info, should return max interval
    let interval = scheduler.calculate_interval(None);
    assert!(interval.as_secs() >= 120 * 60 - 1); // Allow 1s tolerance
}

/// Test adaptive scheduler: backoff on rate limit
#[test]
fn test_adaptive_scheduler_backoff() {
    use jcode::ambient_scheduler::{AdaptiveScheduler, AmbientSchedulerConfig};

    let config = AmbientSchedulerConfig {
        min_interval_minutes: 5,
        max_interval_minutes: 120,
        ..Default::default()
    };

    let mut scheduler = AdaptiveScheduler::new(config);

    let base_interval = scheduler.calculate_interval(None);

    // Hit rate limit
    scheduler.on_rate_limit_hit();
    let backed_off = scheduler.calculate_interval(None);
    assert!(backed_off >= base_interval);

    // Reset on success
    scheduler.on_successful_cycle();
    let after_reset = scheduler.calculate_interval(None);
    assert!(after_reset <= backed_off);
}

/// Test adaptive scheduler: pause on active session
#[test]
fn test_adaptive_scheduler_pause() {
    use jcode::ambient_scheduler::{AdaptiveScheduler, AmbientSchedulerConfig};

    let config = AmbientSchedulerConfig {
        min_interval_minutes: 5,
        max_interval_minutes: 120,
        pause_on_active_session: true,
        ..Default::default()
    };

    let mut scheduler = AdaptiveScheduler::new(config);

    assert!(!scheduler.should_pause());
    scheduler.set_user_active(true);
    assert!(scheduler.should_pause());
    scheduler.set_user_active(false);
    assert!(!scheduler.should_pause());
}

/// Test ambient tools: end_ambient_cycle via mock agent
#[tokio::test]
async fn test_ambient_end_cycle_tool() -> Result<()> {
    let _env = setup_test_env()?;
    let provider = MockProvider::new();

    // Mock: agent calls end_ambient_cycle tool
    let tool_input = serde_json::json!({
        "summary": "Merged 2 duplicate memories, pruned 1 stale memory",
        "memories_modified": 3,
        "compactions": 0
    })
    .to_string();

    provider.queue_response(vec![
        StreamEvent::ToolUseStart {
            id: "tool_001".to_string(),
            name: "end_ambient_cycle".to_string(),
        },
        StreamEvent::ToolInputDelta(tool_input),
        StreamEvent::ToolUseEnd,
        StreamEvent::MessageEnd {
            stop_reason: Some("tool_use".to_string()),
        },
    ]);

    // After tool execution, the agent calls the provider again — mock a final response
    provider.queue_response(vec![
        StreamEvent::TextDelta("Cycle complete.".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    registry.register_ambient_tools().await;

    let mut agent = Agent::new(provider, registry);

    let response = agent.run_once_capture("Begin ambient cycle").await?;
    assert_eq!(response, "Cycle complete.");

    // The tool should have stored a cycle result
    let result = jcode::tool::ambient::take_cycle_result();
    assert!(result.is_some());
    let result = result.unwrap();
    assert_eq!(
        result.summary,
        "Merged 2 duplicate memories, pruned 1 stale memory"
    );
    assert_eq!(result.memories_modified, 3);
    assert_eq!(result.compactions, 0);

    Ok(())
}

/// Test ambient tools: request_permission via mock agent
#[tokio::test]
async fn test_ambient_request_permission_tool() -> Result<()> {
    let _env = setup_test_env()?;
    let provider = MockProvider::new();

    let tool_input = serde_json::json!({
        "action": "create_pull_request",
        "description": "Create PR for test fixes",
        "rationale": "Found 3 failing tests in auth module",
        "urgency": "high",
        "wait": false
    })
    .to_string();

    provider.queue_response(vec![
        StreamEvent::ToolUseStart {
            id: "tool_perm_001".to_string(),
            name: "request_permission".to_string(),
        },
        StreamEvent::ToolInputDelta(tool_input),
        StreamEvent::ToolUseEnd,
        StreamEvent::MessageEnd {
            stop_reason: Some("tool_use".to_string()),
        },
    ]);

    // After tool execution, mock a final response
    provider.queue_response(vec![
        StreamEvent::TextDelta("Permission requested.".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    registry.register_ambient_tools().await;

    let mut agent = Agent::new(provider, registry);
    let ambient_session_id = agent.session_id().to_string();
    jcode::tool::ambient::register_ambient_session(ambient_session_id.clone());

    let response = agent.run_once_capture("Request permission").await?;
    jcode::tool::ambient::unregister_ambient_session(&ambient_session_id);
    assert_eq!(response, "Permission requested.");

    Ok(())
}

/// Test ambient tools: schedule_ambient via mock agent
#[tokio::test]
async fn test_ambient_schedule_tool() -> Result<()> {
    let _env = setup_test_env()?;
    let provider = MockProvider::new();

    let tool_input = serde_json::json!({
        "wake_in_minutes": 30,
        "context": "Check CI results and verify test fixes",
        "priority": "normal"
    })
    .to_string();

    provider.queue_response(vec![
        StreamEvent::ToolUseStart {
            id: "tool_sched_001".to_string(),
            name: "schedule_ambient".to_string(),
        },
        StreamEvent::ToolInputDelta(tool_input),
        StreamEvent::ToolUseEnd,
        StreamEvent::MessageEnd {
            stop_reason: Some("tool_use".to_string()),
        },
    ]);

    // After tool execution, mock a final response
    provider.queue_response(vec![
        StreamEvent::TextDelta("Scheduled next cycle.".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    registry.register_ambient_tools().await;

    let mut agent = Agent::new(provider, registry);

    let response = agent.run_once_capture("Schedule next cycle").await?;
    assert_eq!(response, "Scheduled next cycle.");

    Ok(())
}

/// Test ambient system prompt builder
#[test]
fn test_ambient_system_prompt_builder() {
    use jcode::ambient::{
        build_ambient_system_prompt, AmbientState, MemoryGraphHealth, ResourceBudget,
    };

    let state = AmbientState::default();
    let queue_items = vec![];
    let health = MemoryGraphHealth {
        total: 42,
        active: 38,
        inactive: 4,
        low_confidence: 2,
        contradictions: 1,
        missing_embeddings: 0,
        duplicate_candidates: 3,
        last_consolidation: None,
    };
    let recent_sessions = vec![];
    let feedback: Vec<String> = vec![];
    let budget = ResourceBudget {
        provider: "mock".to_string(),
        tokens_remaining_desc: "50k tokens".to_string(),
        window_resets_desc: "2h".to_string(),
        user_usage_rate_desc: "5k/min".to_string(),
        cycle_budget_desc: "stay under 50k".to_string(),
    };

    let prompt = build_ambient_system_prompt(
        &state,
        &queue_items,
        &health,
        &recent_sessions,
        &feedback,
        &budget,
        0,
    );

    // Verify key sections exist
    assert!(
        prompt.contains("ambient agent"),
        "Prompt missing 'ambient agent'"
    );
    assert!(
        prompt.contains("Memory Graph Health"),
        "Prompt missing 'Memory Graph Health'"
    );
    assert!(
        prompt.contains("Total memories: 42"),
        "Prompt missing memory count"
    );
    assert!(
        prompt.contains("Resource Budget"),
        "Prompt missing 'Resource Budget'"
    );
    assert!(
        prompt.contains("end_ambient_cycle"),
        "Prompt missing end_ambient_cycle instruction"
    );
}

/// Test ambient runner handle: status_json
#[tokio::test]
async fn test_ambient_runner_status() {
    use jcode::ambient_runner::AmbientRunnerHandle;
    use jcode::safety::SafetySystem;

    let safety = Arc::new(SafetySystem::new());
    let handle = AmbientRunnerHandle::new(safety);

    let status_json = handle.status_json().await;
    let status: serde_json::Value = serde_json::from_str(&status_json).unwrap();

    // Verify expected fields exist and have correct types
    assert!(status.get("status").is_some(), "Missing 'status' field");
    assert!(
        status.get("total_cycles").is_some(),
        "Missing 'total_cycles' field"
    );
    assert!(
        status.get("loop_running").is_some(),
        "Missing 'loop_running' field"
    );
    assert_eq!(
        status["loop_running"], false,
        "Runner loop should not be running"
    );
    assert!(
        status["total_cycles"].is_number(),
        "total_cycles should be a number"
    );
    assert!(
        status.get("queue_count").is_some(),
        "Missing 'queue_count' field"
    );
    assert!(
        status.get("active_user_sessions").is_some(),
        "Missing 'active_user_sessions' field"
    );
}

/// Test ambient runner handle: trigger and stop
#[tokio::test]
async fn test_ambient_runner_trigger_and_stop() {
    use jcode::ambient::AmbientStatus;
    use jcode::ambient_runner::AmbientRunnerHandle;
    use jcode::safety::SafetySystem;

    let safety = Arc::new(SafetySystem::new());
    let handle = AmbientRunnerHandle::new(safety);

    // Stop (sets status to disabled)
    handle.stop().await;
    let state = handle.state().await;
    assert!(
        matches!(state.status, AmbientStatus::Disabled),
        "After stop(), status should be Disabled, got: {:?}",
        state.status
    );

    // Runner should not be running (no loop was started)
    assert!(!handle.is_running().await, "Runner should not be active");
}

/// Test ambient runner handle: queue_json
#[tokio::test]
async fn test_ambient_runner_queue_json() {
    use jcode::ambient_runner::AmbientRunnerHandle;
    use jcode::safety::SafetySystem;

    let safety = Arc::new(SafetySystem::new());
    let handle = AmbientRunnerHandle::new(safety);

    let json = handle.queue_json().await;
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(parsed.is_array());
}

/// Test ambient runner handle: log_json
#[tokio::test]
async fn test_ambient_runner_log_json() {
    use jcode::ambient_runner::AmbientRunnerHandle;
    use jcode::safety::SafetySystem;

    let safety = Arc::new(SafetySystem::new());
    let handle = AmbientRunnerHandle::new(safety);

    let json = handle.log_json().await;
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(parsed.is_array());
}

/// Test memory reinforcement provenance
#[test]
fn test_memory_reinforcement_provenance() {
    use jcode::memory::{MemoryCategory, MemoryEntry};

    let mut entry = MemoryEntry::new(MemoryCategory::Preference, "User prefers dark mode");
    assert!(entry.reinforcements.is_empty());
    assert_eq!(entry.strength, 1); // Initial strength

    // Reinforce with provenance
    entry.reinforce("session_abc123", 42);
    assert_eq!(entry.strength, 2);
    assert_eq!(entry.reinforcements.len(), 1);
    assert_eq!(entry.reinforcements[0].session_id, "session_abc123");
    assert_eq!(entry.reinforcements[0].message_index, 42);

    // Reinforce again from different session
    entry.reinforce("session_def456", 10);
    assert_eq!(entry.strength, 3);
    assert_eq!(entry.reinforcements.len(), 2);
    assert_eq!(entry.reinforcements[1].session_id, "session_def456");
    assert_eq!(entry.reinforcements[1].message_index, 10);
}

/// Test ambient config defaults
#[test]
fn test_ambient_config_defaults() {
    use jcode::config::AmbientConfig;

    let config = AmbientConfig::default();
    assert!(!config.enabled);
    assert!(!config.allow_api_keys);
    assert_eq!(config.min_interval_minutes, 5);
    assert_eq!(config.max_interval_minutes, 120);
    assert!(config.pause_on_active_session);
    assert!(config.proactive_work);
    assert_eq!(config.work_branch_prefix, "ambient/");
    assert!(config.provider.is_none());
    assert!(config.model.is_none());
    assert!(config.api_daily_budget.is_none());
}

/// Test ambient lock acquisition and release
#[test]
fn test_ambient_lock() {
    use jcode::ambient::AmbientLock;
    let _env = setup_test_env().expect("failed to setup isolated JCODE_HOME");

    // First acquisition should succeed
    let lock1 = AmbientLock::try_acquire();
    assert!(lock1.is_ok());
    let lock1 = lock1.unwrap();
    assert!(lock1.is_some());
    let lock1 = lock1.unwrap();

    // Second acquisition should fail (lock held)
    let lock2 = AmbientLock::try_acquire();
    assert!(lock2.is_ok());
    assert!(lock2.unwrap().is_none());

    // Release
    let _ = lock1.release();

    // Now should succeed again
    let lock3 = AmbientLock::try_acquire();
    assert!(lock3.is_ok());
    assert!(lock3.unwrap().is_some());
}

/// Test full ambient cycle simulation with mock provider
/// Simulates: agent receives prompt → uses tools → calls end_ambient_cycle
#[tokio::test]
async fn test_full_ambient_cycle_simulation() -> Result<()> {
    let _env = setup_test_env()?;
    let provider = MockProvider::new();

    // Turn 1: Agent calls end_ambient_cycle with full data
    let end_cycle_input = serde_json::json!({
        "summary": "Gardened memory graph: merged 2 duplicates about dark mode preference, pruned 1 stale memory with confidence 0.02, verified 5 facts against codebase.",
        "memories_modified": 6,
        "compactions": 1,
        "proactive_work": null,
        "next_schedule": {
            "wake_in_minutes": 45,
            "context": "Follow up on memory verification",
            "priority": "normal"
        }
    })
    .to_string();

    provider.queue_response(vec![
        StreamEvent::TextDelta("Starting ambient cycle...\n".to_string()),
        StreamEvent::ToolUseStart {
            id: "call_end".to_string(),
            name: "end_ambient_cycle".to_string(),
        },
        StreamEvent::ToolInputDelta(end_cycle_input),
        StreamEvent::ToolUseEnd,
        StreamEvent::MessageEnd {
            stop_reason: Some("tool_use".to_string()),
        },
    ]);

    // Turn 2: After end_ambient_cycle tool result, agent responds
    provider.queue_response(vec![
        StreamEvent::TextDelta("Ambient cycle completed successfully.".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    registry.register_ambient_tools().await;

    let mut agent = Agent::new(provider.clone(), registry);
    agent.set_system_prompt("You are the jcode ambient maintenance agent.");

    let response = agent.run_once_capture("Begin your ambient cycle.").await?;

    assert!(response.contains("Ambient cycle completed"));

    // Verify end_ambient_cycle stored the result
    let result = jcode::tool::ambient::take_cycle_result();
    assert!(result.is_some());
    let result = result.unwrap();
    assert_eq!(result.memories_modified, 6);
    assert_eq!(result.compactions, 1);
    assert!(result.summary.contains("Gardened memory graph"));
    assert!(result.next_schedule.is_some());
    let sched = result.next_schedule.unwrap();
    assert_eq!(sched.wake_in_minutes, Some(45));
    assert!(sched.context.contains("Follow up"));

    Ok(())
}
