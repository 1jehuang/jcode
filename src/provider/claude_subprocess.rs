use crate::message::{Message, StreamEvent, ToolDefinition, ContentBlock, Role};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use super::{EventStream, Provider};

/// Claude Code subprocess-based provider
///
/// This provider spawns the Claude Code CLI and communicates via stream-json mode.
/// It's the only reliable way to use Claude Max/Pro OAuth tokens, as Anthropic
/// actively detects and blocks unauthorized API usage.
pub struct ClaudeSubprocessProvider {
    model: String,
    bypass_permissions: bool,
}

impl ClaudeSubprocessProvider {
    pub fn new(model: &str, bypass_permissions: bool) -> Self {
        Self {
            model: model.to_string(),
            bypass_permissions,
        }
    }
}

// Input message format for Claude CLI
#[derive(Serialize)]
struct InputMessage {
    #[serde(rename = "type")]
    msg_type: String,
    message: InputContent,
}

#[derive(Serialize)]
struct InputContent {
    role: String,
    content: serde_json::Value,
}

// Output message formats from Claude CLI
#[derive(Deserialize, Debug)]
struct OutputLine {
    #[serde(rename = "type")]
    msg_type: String,
    #[serde(default)]
    subtype: Option<String>,
    #[serde(default)]
    message: Option<AssistantMessage>,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    is_error: Option<bool>,
}

#[derive(Deserialize, Debug)]
struct AssistantMessage {
    #[serde(default)]
    content: Vec<OutputContentBlock>,
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum OutputContentBlock {
    Text { text: String },
    ToolUse { id: String, name: String, input: serde_json::Value },
}

#[async_trait]
impl Provider for ClaudeSubprocessProvider {
    async fn complete(
        &self,
        messages: &[Message],
        _tools: &[ToolDefinition],  // Claude CLI has its own tools
        _system: &str,  // Claude CLI uses its own system prompt
    ) -> Result<EventStream> {
        // Build command
        let mut cmd = Command::new("claude");
        cmd.args([
            "--print",
            "--verbose",
            "--output-format", "stream-json",
            "--input-format", "stream-json",
            "--model", &self.model,
        ]);

        if self.bypass_permissions {
            cmd.arg("--dangerously-skip-permissions");
        }

        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn()
            .map_err(|e| anyhow!("Failed to spawn claude: {}", e))?;

        let mut stdin = child.stdin.take()
            .ok_or_else(|| anyhow!("Failed to get stdin"))?;
        let stdout = child.stdout.take()
            .ok_or_else(|| anyhow!("Failed to get stdout"))?;

        // Convert messages to input format and send
        for msg in messages {
            let input = message_to_input(msg)?;
            let json = serde_json::to_string(&input)?;
            stdin.write_all(json.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
        }
        stdin.flush().await?;
        drop(stdin); // Close stdin to signal end of input

        // Create channel for streaming events
        let (tx, rx) = mpsc::channel(100);

        // Spawn task to read and parse output
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            while let Ok(Some(line)) = lines.next_line().await {
                if line.is_empty() {
                    continue;
                }

                match serde_json::from_str::<OutputLine>(&line) {
                    Ok(output) => {
                        let events = parse_output_line(output);
                        for event in events {
                            if tx.send(Ok(event)).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to parse output line: {} - {}", e, line);
                    }
                }
            }

            // Wait for child to exit
            let _ = child.wait().await;
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn name(&self) -> &str {
        "claude-subprocess"
    }
}

fn message_to_input(msg: &Message) -> Result<InputMessage> {
    let role = match msg.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };

    // Convert content blocks to JSON
    let content: Vec<serde_json::Value> = msg.content.iter().map(|block| {
        match block {
            ContentBlock::Text { text } => serde_json::json!({
                "type": "text",
                "text": text
            }),
            ContentBlock::ToolUse { id, name, input } => serde_json::json!({
                "type": "tool_use",
                "id": id,
                "name": name,
                "input": input
            }),
            ContentBlock::ToolResult { tool_use_id, content, is_error } => serde_json::json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": content,
                "is_error": is_error.unwrap_or(false)
            }),
        }
    }).collect();

    // For simple text-only messages, just use the text directly
    let content_value = if content.len() == 1 {
        if let Some(text) = content[0].get("text") {
            text.clone()
        } else {
            serde_json::Value::Array(content)
        }
    } else {
        serde_json::Value::Array(content)
    };

    Ok(InputMessage {
        msg_type: "user".to_string(),
        message: InputContent {
            role: role.to_string(),
            content: content_value,
        },
    })
}

fn parse_output_line(output: OutputLine) -> Vec<StreamEvent> {
    let mut events = Vec::new();

    match output.msg_type.as_str() {
        "assistant" => {
            if let Some(msg) = output.message {
                for block in msg.content {
                    match block {
                        OutputContentBlock::Text { text } => {
                            events.push(StreamEvent::TextDelta(text));
                        }
                        OutputContentBlock::ToolUse { id, name, input } => {
                            events.push(StreamEvent::ToolUseStart {
                                id: id.clone(),
                                name: name.clone()
                            });
                            events.push(StreamEvent::ToolInputDelta(
                                serde_json::to_string(&input).unwrap_or_default()
                            ));
                            events.push(StreamEvent::ToolUseEnd);
                        }
                    }
                }

                if let Some(reason) = msg.stop_reason {
                    events.push(StreamEvent::MessageEnd {
                        stop_reason: Some(reason)
                    });
                }
            }
        }
        "result" => {
            if output.is_error.unwrap_or(false) {
                if let Some(result) = output.result {
                    events.push(StreamEvent::Error(result));
                }
            }
            events.push(StreamEvent::MessageEnd { stop_reason: Some("end".to_string()) });
        }
        "error" => {
            if let Some(result) = output.result {
                events.push(StreamEvent::Error(result));
            }
        }
        _ => {
            // Ignore other message types (system, etc.)
        }
    }

    events
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_subprocess_provider() {
        let provider = ClaudeSubprocessProvider::new("claude-sonnet-4-20250514", true);
        let messages = vec![Message::user("Say hi in 3 words")];

        match provider.complete(&messages, &[], "").await {
            Ok(mut stream) => {
                use futures::StreamExt;
                while let Some(event) = stream.next().await {
                    println!("Event: {:?}", event);
                }
            }
            Err(e) => {
                eprintln!("Error: {}", e);
            }
        }
    }
}
