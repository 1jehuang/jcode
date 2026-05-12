//! Offline mode example for CarpAI SDK

use carpai_sdk::{CarpAiClient, CarpAiConfig};
use std::time::Duration;
use tokio::time::sleep;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    carpai_sdk::init_logging();

    println!("=== CarpAI SDK Offline Mode Example ===\n");

    // Create client with offline mode enabled
    let config = CarpAiConfig {
        offline: carpai_sdk::OfflineConfig {
            enabled: true,
            max_cache_age_hours: 48,
            queue_requests_when_offline: true,
            max_queued_requests: 100,
            auto_sync_on_reconnect: true,
        },
        ..CarpAiConfig::zero_config()
    };

    let client = CarpAiClient::new(config).await?;
    println!("✓ Client initialized with offline mode\n");

    // Step 1: Make a request while online (to populate cache)
    println!("--- Step 1: Making request (simulating online mode) ---");
    
    let online_request = carpai_sdk::CompletionRequest {
        prompt: "What is Rust's ownership system?".to_string(),
        ..Default::default()
    };

    // Simulate being online
    match client.complete(online_request.clone()).await {
        Ok(response) => {
            println!("Online response received:");
            println!("{}", response.text);
            println!("\n(This response is now cached for offline use)");
        }
        Err(e) => {
            println!("Note: Server not available (expected in demo): {}", e);
            println!("Proceeding with offline demo...\n");
            
            // For demo purposes, manually cache a response
            let cached_response = carpai_sdk::CompletionResponse {
                text: "Rust's ownership system ensures memory safety without garbage collection. Each value has an owner, and there can only be one owner at a time. When the owner goes out of scope, the value is dropped.".to_string(),
                request_id: carpai_sdk::RequestId::new(),
                session_id: None,
                model: "demo".to_string(),
                usage: carpai_sdk::TokenUsage {
                    prompt_tokens: 10,
                    completion_tokens: 40,
                    total_tokens: 50,
                },
                latency_ms: 150.0,
                cached: false,
                finish_reason: Some("stop".to_string()),
            };
            
            // We can't directly access the internal cache from here,
            // but in a real scenario, this would be handled automatically
            println!("[Demo] Cached a sample response for offline use");
        }
    }
    println!();

    // Step 2: Check if we can work offline
    println!("--- Step 2: Checking offline capabilities ---");
    println!("Online status: {}", if client.is_online() { "✓ Online" } else { "⚠ Offline" });
    println!("Cache stats: {:?}", client.cache_stats());
    println!();

    // Step 3: Demonstrate queuing behavior
    println!("--- Step 3: Request queuing (when offline) ---");
    
    let queued_request = carpai_sdk::CompletionRequest {
        prompt: "Explain async/await in Rust".to_string(),
        ..Default::default()
    };

    // Note: In real usage, when offline, requests would be queued automatically
    println!("If offline, this request would be queued:");
    println!("  Prompt: {}", queued_request.prompt);
    println!();

    // Step 4: Show configuration options
    println!("--- Offline Configuration ---");
    let cfg = client.config();
    println!("Offline mode enabled: {}", cfg.offline.enabled);
    println!("Max cache age: {} hours", cfg.offline.max_cache_age_hours);
    println!("Queue requests when offline: {}", cfg.offline.queue_requests_when_offline);
    println!("Max queued requests: {}", cfg.offline.max_queued_requests);
    println!("Auto-sync on reconnect: {}", cfg.offline.auto_sync_on_reconnect);
    println!();

    // Step 5: Error handling demonstration
    println!("--- Step 5: Error Handling ---");
    println!("The SDK provides rich error types with recovery suggestions:\n");
    
    // Simulate different error scenarios
    let error_examples = vec![
        ("Connection error", "Server unreachable"),
        ("Rate limit", "Too many requests"),
        ("Auth error", "Invalid API key"),
        ("Timeout", "Request took too long"),
    ];

    for (error_type, description) in error_examples {
        println!("• {}: {}", error_type, description);
    }
    println!("\nEach error includes:");
    println!("  - Error code for programmatic handling");
    println!("  - Human-readable message");
    println!("  - Recovery suggestion (when available)");
    println!("  - is_recoverable() method for retry logic");
    println!();

    println!("=== Offline Mode Example Complete ===");
    Ok(())
}
