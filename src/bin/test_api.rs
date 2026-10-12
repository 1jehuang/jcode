use futures::StreamExt;
use jcode::message::{ContentBlock, Message, ToolDefinition};
use jcode::provider::Provider;
use jcode_provider_anthropic_runtime::AnthropicProvider;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("Testing direct Anthropic provider (Claude OAuth or ANTHROPIC_API_KEY)...");
    let provider = AnthropicProvider::new();

    let messages = vec![Message {
        role: jcode::message::Role::User,
        content: vec![ContentBlock::Text {
            text: "Say hello in exactly 5 words.".to_string(),
            cache_control: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }];

    let tools: Vec<ToolDefinition> = vec![];
    let system = "You are a helpful assistant.";

    println!("Sending request...");
    let mut stream = provider.complete(&messages, &tools, system, None).await?;

    println!("Response:");
    while let Some(event) = stream.next().await {
        match event {
            Ok(e) => print!("{:?} ", e),
            Err(e) => eprintln!("Error: {}", e),
        }
    }
    println!("\nDone!");

    Ok(())
}
