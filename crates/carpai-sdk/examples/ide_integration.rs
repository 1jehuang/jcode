//! IDE integration example for CarpAI SDK

use carpai_sdk::{CarpAiClient, CarpAiConfig, IdeAdapter, IdeType, GenericIdeAdapter};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    carpai_sdk::init_logging();

    println!("=== CarpAI SDK IDE Integration Example ===\n");

    // Detect current IDE (if any)
    let detected_ide = IdeType::detect();
    match &detected_ide {
        Some(ide) => println!("Detected IDE: {}", ide.as_str()),
        None => println!("No IDE detected, using generic adapter"),
    }
    println!();

    // Create client
    let config = CarpAiConfig::zero_config();
    let mut client = CarpAiClient::new(config).await?;

    // Set up IDE adapter
    let ide_type = detected_ide.unwrap_or(IdeType::VSCode);
    let ide_adapter: Arc<dyn IdeAdapter> = Arc::new(GenericIdeAdapter::new(ide_type));
    client = client.with_ide_adapter(ide_adapter);

    println!("✓ Client with IDE adapter initialized\n");

    // Simulate getting file context from IDE
    println!("--- Simulating IDE Integration ---");
    
    // Example: Get active file info (would come from real IDE)
    println!("Simulating: Getting current file context...");
    
    // Example completion with file context
    let request = carpai_sdk::CompletionRequest {
        prompt: "// TODO: Implement error handling\nfn parse_config(".to_string(),
        session_id: None,
        model: Some("default".to_string()),
        max_tokens: Some(150),
        temperature: Some(0.7),
        stop_sequences: vec![],
        top_p: None,
        context: carpai_sdk::CompletionContext {
            file_path: Some("src/config.rs".to_string()),
            language: Some("rust".to_string()),
            cursor_position: Some((42, 16)),
            surrounding_code: Some("struct Config { ... }\n\nfn load_config(path: &str) -> Result<Config> {\n    // TODO: Implement error handling\n    fn parse_config(".to_string()),
            project_root: Some("/home/user/my-project".to_string()),
            metadata: Default::default(),
        },
    };

    match client.complete(request).await {
        Ok(response) => {
            println!("Completion result:");
            println!("{}", response.text);
            
            // In a real IDE plugin, you would now insert this text at cursor position
            println!("\n[IDE would insert this code at cursor position]");
        }
        Err(e) => {
            println!("Error: {}", e);
        }
    }
    println!();

    // Example: Code action (explain selected code)
    println!("--- Code Action Example ---");
    let action_request = carpai_sdk::CodeActionRequest {
        action_type: carpai_sdk::CodeActionType::Explain,
        code: "async fn fetch_data(url: &str) -> Result<String> {\n    let response = reqwest::get(url).await?;\n    Ok(response.text().await?)\n}".to_string(),
        file_path: Some("src/api.rs".to_string()),
        language: Some("rust".to_string()),
        selection: Some((10, 0, 13, 1)),
        instruction: None,
    };

    match client.code_action(action_request).await {
        Ok(response) => {
            println!("Explanation:");
            println!("{}", response.result);
            println!("Confidence: {:.1}%", response.confidence * 100.0);
        }
        Err(e) => {
            println!("Error: {}", e);
        }
    }

    println!("\n=== IDE Integration Example Complete ===");
    Ok(())
}
