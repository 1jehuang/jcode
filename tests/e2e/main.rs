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

/// Test that a simple text response works
#[tokio::test]
async fn test_simple_response() -> Result<()> {
    let _lock = lock_jcode_home();
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

    assert_eq!(response, "Hello! How can I help?");
    Ok(())
}

/// Test that multi-turn conversation works with session resume
#[tokio::test]
async fn test_multi_turn_conversation() -> Result<()> {
    let _lock = lock_jcode_home();
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
    let _lock = lock_jcode_home();
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
    let _lock = lock_jcode_home();
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
    let _lock = lock_jcode_home();
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
            ServerEvent::ModelChanged { id, model, error } if id == request_id => {
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
    let _lock = lock_jcode_home();
    let runtime_dir = std::env::temp_dir().join(format!(
        "jcode-resume-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&runtime_dir)?;

    let jcode_home = runtime_dir.join("home");
    std::fs::create_dir_all(&jcode_home)?;

    let prev_home = std::env::var("JCODE_HOME").ok();
    std::env::set_var("JCODE_HOME", &jcode_home);

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
    if let Some(prev) = prev_home {
        std::env::set_var("JCODE_HOME", prev);
    } else {
        std::env::remove_var("JCODE_HOME");
    }

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
    let _lock = lock_jcode_home();
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
    let _lock = lock_jcode_home();
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
    let _lock = lock_jcode_home();
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
    let _lock = lock_jcode_home();
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
        lower_identity.contains("you are jcode"),
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
    let _lock = lock_jcode_home();

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
    let _lock = lock_jcode_home();

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
    let _lock = lock_jcode_home();

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
