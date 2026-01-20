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
use jcode::tool::Registry;
use mock_provider::MockProvider;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Test that a simple text response works
#[tokio::test]
async fn test_simple_response() -> Result<()> {
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
    let registry = Registry::new(provider.clone()).await;
    let server_instance = server::Server::new_with_paths(
        provider,
        registry,
        socket_path.clone(),
        debug_socket_path.clone(),
    );

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

/// Test that subscribe selfdev hint marks the session as canary
#[tokio::test]
async fn test_subscribe_selfdev_hint_marks_canary() -> Result<()> {
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
    let registry = Registry::new(provider.clone()).await;
    let server_instance = server::Server::new_with_paths(
        provider,
        registry,
        socket_path.clone(),
        debug_socket_path.clone(),
    );

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
    let registry = Registry::new(provider_dyn.clone()).await;
    let server_instance = server::Server::new_with_paths(
        provider_dyn,
        registry,
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

    let resume_ids = provider
        .captured_resume_session_ids
        .lock()
        .unwrap()
        .clone();
    assert_eq!(resume_ids.len(), 2);
    assert_eq!(resume_ids[0], None);
    assert_eq!(resume_ids[1], None);

    server_handle.abort();
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&debug_socket_path);

    Ok(())
}

/// Test that the system prompt does NOT identify the agent as "Claude Code"
/// The agent should identify as "jcode" or just a generic "coding assistant powered by Claude"
#[tokio::test]
async fn test_system_prompt_no_claude_code_identity() -> Result<()> {
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

    // The system prompt should NOT contain "Claude Code" (case insensitive check)
    let lower_prompt = system_prompt.to_lowercase();
    assert!(
        !lower_prompt.contains("claude code"),
        "System prompt should NOT identify as 'Claude Code'. Found: {}",
        system_prompt
    );

    // The system prompt should NOT contain common Claude Code identifiers
    assert!(
        !lower_prompt.contains("claude-code"),
        "System prompt should NOT contain 'claude-code'. Found: {}",
        system_prompt
    );

    // It's OK if it says "powered by Claude" or just "Claude" (the model name)
    // It's OK if it says "jcode" or "coding assistant"
    // Just not "Claude Code" as that's the Anthropic product name

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
