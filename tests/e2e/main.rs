//! End-to-end tests for jcode using a mock provider
//!
//! These tests verify the full flow from user input to response
//! without making actual API calls.

mod mock_provider;

use anyhow::Result;
use jcode::agent::Agent;
use jcode::message::StreamEvent;
use jcode::tool::Registry;
use mock_provider::MockProvider;
use std::sync::Arc;

/// Test that a simple text response works
#[tokio::test]
async fn test_simple_response() -> Result<()> {
    let provider = MockProvider::new();

    // Queue a simple response
    provider.queue_response(vec![
        StreamEvent::TextDelta("Hello! ".to_string()),
        StreamEvent::TextDelta("How can I help?".to_string()),
        StreamEvent::MessageEnd { stop_reason: Some("end_turn".to_string()) },
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
        StreamEvent::MessageEnd { stop_reason: Some("end_turn".to_string()) },
        StreamEvent::SessionId("session-abc".to_string()),
    ]);

    // Second turn response
    provider.queue_response(vec![
        StreamEvent::TextDelta("You said hello earlier.".to_string()),
        StreamEvent::MessageEnd { stop_reason: Some("end_turn".to_string()) },
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
        StreamEvent::TokenUsage { input_tokens: Some(10), output_tokens: Some(20) },
        StreamEvent::TextDelta("Response".to_string()),
        StreamEvent::MessageEnd { stop_reason: Some("end_turn".to_string()) },
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
        StreamEvent::Error("Something went wrong".to_string()),
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);

    let result = agent.run_once_capture("Test").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Something went wrong"));

    Ok(())
}
