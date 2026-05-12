//! Basic usage example for CarpAI SDK

use carpai_sdk::{CarpAiClient, CarpAiConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    carpai_sdk::init_logging();

    println!("=== CarpAI SDK Basic Usage Example ===\n");

    // Create client with default configuration
    let config = CarpAiConfig::zero_config();
    let client = CarpAiClient::new(config).await?;

    println!("✓ Client initialized successfully\n");

    // Check server health
    match client.health_check().await {
        Ok(health) => {
            println!("Server Health: {:?}", health.status);
            if let Some(ref version) = health.version {
                println!("Server Version: {}", version);
            }
        }
        Err(e) => {
            println!("⚠ Health check failed (this is OK for local development): {}", e);
        }
    }
    println!();

    // Example 1: Simple completion
    println!("--- Example 1: Code Completion ---");
    let completion_request = carpai_sdk::CompletionRequest {
        prompt: "fn fibonacci(n: u64) -> u64 {".to_string(),
        session_id: None,
        model: None,
        max_tokens: Some(100),
        temperature: Some(0.7),
        stop_sequences: vec![],
        top_p: None,
        context: Default::default(),
    };

    match client.complete(completion_request).await {
        Ok(response) => {
            println!("Generated code:");
            println!("{}", response.text);
            println!("\nTokens used: {}", response.usage.total_tokens);
            println!("Latency: {:.1}ms", response.latency_ms);
            println!("Cached: {}", response.cached);
        }
        Err(e) => {
            println!("Error: {} (this is expected if no server is running)", e);
            if let Some(suggestion) = e.recovery_suggestion() {
                println!("Suggestion: {}", suggestion);
            }
        }
    }
    println!();

    // Example 2: Chat completion
    println!("--- Example 2: Chat Completion ---");
    let chat_request = carpai_sdk::ChatCompletionRequest {
        messages: vec![
            carpai_sdk::ChatMessage {
                role: carpai_sdk::MessageRole::System,
                content: "You are a helpful Rust programming assistant.".to_string(),
            },
            carpai_sdk::ChatMessage {
                role: carpai_sdk::MessageRole::User,
                content: "Explain Rust's ownership system in simple terms.".to_string(),
            },
        ],
        model: None,
        max_tokens: Some(200),
        temperature: Some(0.8),
        params: Default::default(),
    };

    match client.chat_complete(chat_request).await {
        Ok(response) => {
            println!("Assistant response:");
            println!("{}", response.message.content);
            println!("\nModel: {}", response.model);
            println!("Latency: {:.1}ms", response.latency_ms);
        }
        Err(e) => {
            println!("Error: {}", e);
        }
    }
    println!();

    // Show cache statistics
    println!("--- Cache Statistics ---");
    let stats = client.cache_stats();
    println!("Total entries: {}", stats.total_entries);
    println!("Valid entries: {}", stats.valid_entries);

    println!("\n=== Example Complete ===");
    Ok(())
}
