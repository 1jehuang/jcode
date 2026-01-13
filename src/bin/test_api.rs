use jcode::provider::claude::ClaudeProvider;
use jcode::provider::Provider;
use jcode::message::{Message, ContentBlock, ToolDefinition};
use futures::StreamExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("Testing Claude Agent SDK provider...");
    let provider = ClaudeProvider::new();

    let messages = vec![Message {
        role: jcode::message::Role::User,
        content: vec![ContentBlock::Text {
            text: "Say hello in exactly 5 words.".to_string()
        }],
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
