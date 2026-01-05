use crate::message::{ContentBlock, Message, Role, StreamEvent, ToolCall};
use crate::provider::Provider;
use crate::tool::Registry;
use anyhow::Result;
use futures::StreamExt;
use std::io::{self, Write};

const SYSTEM_PROMPT: &str = r#"You are a coding assistant with access to tools for file operations and shell commands.

## Available Tools
- bash: Execute shell commands
- read: Read file contents
- write: Create or overwrite files
- edit: Edit files by replacing text
- glob: Find files by pattern
- grep: Search file contents with regex
- ls: List directory contents

## Guidelines
1. Use tools to explore and modify the codebase
2. Read files before editing to understand current state
3. Use glob/grep to find relevant files
4. Prefer edit over write for existing files
5. Keep responses concise and action-focused
6. Execute commands to verify changes work

When you need to make changes, use the tools directly. Don't just describe what to do."#;

pub struct Agent {
    provider: Box<dyn Provider>,
    registry: Registry,
    messages: Vec<Message>,
}

impl Agent {
    pub fn new(provider: Box<dyn Provider>, registry: Registry) -> Self {
        Self {
            provider,
            registry,
            messages: Vec::new(),
        }
    }

    /// Run a single turn with the given user message
    pub async fn run_once(&mut self, user_message: &str) -> Result<()> {
        self.messages.push(Message::user(user_message));
        self.run_turn().await
    }

    /// Start an interactive REPL
    pub async fn repl(&mut self) -> Result<()> {
        println!("J-Code - Coding Agent");
        println!("Type your message, or 'quit' to exit.\n");

        loop {
            print!("> ");
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;

            let input = input.trim();
            if input.is_empty() {
                continue;
            }

            if input == "quit" || input == "exit" {
                break;
            }

            if input == "clear" {
                self.messages.clear();
                println!("Conversation cleared.");
                continue;
            }

            self.messages.push(Message::user(input));

            if let Err(e) = self.run_turn().await {
                eprintln!("\nError: {}\n", e);
            }

            println!();
        }

        Ok(())
    }

    /// Run turns until no more tool calls
    async fn run_turn(&mut self) -> Result<()> {
        loop {
            let tools = self.registry.definitions();
            let mut stream = self
                .provider
                .complete(&self.messages, &tools, SYSTEM_PROMPT)
                .await?;

            let mut text_content = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut current_tool: Option<ToolCall> = None;
            let mut current_tool_input = String::new();

            while let Some(event) = stream.next().await {
                match event? {
                    StreamEvent::TextDelta(text) => {
                        print!("{}", text);
                        io::stdout().flush()?;
                        text_content.push_str(&text);
                    }
                    StreamEvent::ToolUseStart { id, name } => {
                        print!("\n[{}] ", name);
                        io::stdout().flush()?;
                        current_tool = Some(ToolCall {
                            id,
                            name,
                            input: serde_json::Value::Null,
                        });
                        current_tool_input.clear();
                    }
                    StreamEvent::ToolInputDelta(delta) => {
                        current_tool_input.push_str(&delta);
                    }
                    StreamEvent::ToolUseEnd => {
                        if let Some(mut tool) = current_tool.take() {
                            // Parse the accumulated JSON
                            tool.input = serde_json::from_str(&current_tool_input)
                                .unwrap_or(serde_json::Value::Null);

                            // Show brief tool info
                            print_tool_summary(&tool);

                            tool_calls.push(tool);
                            current_tool_input.clear();
                        }
                    }
                    StreamEvent::MessageEnd { .. } => {
                        break;
                    }
                    StreamEvent::Error(e) => {
                        return Err(anyhow::anyhow!("Stream error: {}", e));
                    }
                }
            }

            // Add assistant message to history
            let mut content_blocks = Vec::new();
            if !text_content.is_empty() {
                content_blocks.push(ContentBlock::Text { text: text_content });
            }
            for tc in &tool_calls {
                content_blocks.push(ContentBlock::ToolUse {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    input: tc.input.clone(),
                });
            }

            if !content_blocks.is_empty() {
                self.messages.push(Message {
                    role: Role::Assistant,
                    content: content_blocks,
                });
            }

            // If no tool calls, we're done
            if tool_calls.is_empty() {
                println!();
                break;
            }

            // Execute tools and add results
            for tc in tool_calls {
                print!("\n  → ");
                io::stdout().flush()?;

                let result = self.registry.execute(&tc.name, tc.input.clone()).await;

                match result {
                    Ok(output) => {
                        // Show truncated output
                        let preview = if output.len() > 200 {
                            format!("{}...", &output[..200])
                        } else {
                            output.clone()
                        };
                        println!("{}", preview.lines().next().unwrap_or("(done)"));

                        self.messages
                            .push(Message::tool_result(&tc.id, &output, false));
                    }
                    Err(e) => {
                        let error_msg = format!("Error: {}", e);
                        println!("{}", error_msg);
                        self.messages
                            .push(Message::tool_result(&tc.id, &error_msg, true));
                    }
                }
            }

            println!();
        }

        Ok(())
    }
}

fn print_tool_summary(tool: &ToolCall) {
    match tool.name.as_str() {
        "bash" => {
            if let Some(cmd) = tool.input.get("command").and_then(|v| v.as_str()) {
                let short = if cmd.len() > 60 {
                    format!("{}...", &cmd[..60])
                } else {
                    cmd.to_string()
                };
                println!("$ {}", short);
            }
        }
        "read" | "write" | "edit" => {
            if let Some(path) = tool.input.get("file_path").and_then(|v| v.as_str()) {
                println!("{}", path);
            }
        }
        "glob" | "grep" => {
            if let Some(pattern) = tool.input.get("pattern").and_then(|v| v.as_str()) {
                println!("'{}'", pattern);
            }
        }
        "ls" => {
            let path = tool
                .input
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");
            println!("{}", path);
        }
        _ => {}
    }
}
