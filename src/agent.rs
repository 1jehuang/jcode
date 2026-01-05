use crate::message::{ContentBlock, Message, Role, StreamEvent, ToolCall};
use crate::provider::Provider;
use crate::skill::SkillRegistry;
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
    skills: SkillRegistry,
    messages: Vec<Message>,
    active_skill: Option<String>,
}

impl Agent {
    pub fn new(provider: Box<dyn Provider>, registry: Registry) -> Self {
        let skills = SkillRegistry::load().unwrap_or_default();
        Self {
            provider,
            registry,
            skills,
            messages: Vec::new(),
            active_skill: None,
        }
    }

    /// Run a single turn with the given user message
    pub async fn run_once(&mut self, user_message: &str) -> Result<()> {
        self.messages.push(Message::user(user_message));
        self.run_turn().await
    }

    /// Clear conversation history
    pub fn clear(&mut self) {
        self.messages.clear();
    }

    /// Start an interactive REPL
    pub async fn repl(&mut self) -> Result<()> {
        println!("J-Code - Coding Agent");
        println!("Type your message, or 'quit' to exit.");

        // Show available skills
        let skill_list = self.skills.list();
        if !skill_list.is_empty() {
            println!("Available skills: {}",
                skill_list.iter().map(|s| format!("/{}", s.name)).collect::<Vec<_>>().join(", "));
        }
        println!();

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
                self.active_skill = None;
                println!("Conversation cleared.");
                continue;
            }

            // Check for skill invocation
            if let Some(skill_name) = SkillRegistry::parse_invocation(input) {
                if let Some(skill) = self.skills.get(skill_name) {
                    println!("Activating skill: {}", skill.name);
                    println!("{}\n", skill.description);
                    self.active_skill = Some(skill_name.to_string());
                    continue;
                } else {
                    println!("Unknown skill: /{}", skill_name);
                    println!("Available: {}",
                        self.skills.list().iter().map(|s| format!("/{}", s.name)).collect::<Vec<_>>().join(", "));
                    continue;
                }
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
            let tools = self.registry.definitions().await;

            // Build system prompt with active skill
            let system_prompt = if let Some(ref skill_name) = self.active_skill {
                if let Some(skill) = self.skills.get(skill_name) {
                    format!("{}\n\n{}", SYSTEM_PROMPT, skill.get_prompt())
                } else {
                    SYSTEM_PROMPT.to_string()
                }
            } else {
                SYSTEM_PROMPT.to_string()
            };

            let mut stream = self
                .provider
                .complete(&self.messages, &tools, &system_prompt)
                .await?;

            let mut text_content = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut current_tool: Option<ToolCall> = None;
            let mut current_tool_input = String::new();
            let mut usage_input: Option<u64> = None;
            let mut usage_output: Option<u64> = None;

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
                    StreamEvent::TokenUsage {
                        input_tokens,
                        output_tokens,
                    } => {
                        if let Some(input) = input_tokens {
                            usage_input = Some(input);
                        }
                        if let Some(output) = output_tokens {
                            usage_output = Some(output);
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

            if usage_input.is_some() || usage_output.is_some() {
                let input = usage_input.unwrap_or(0);
                let output = usage_output.unwrap_or(0);
                print!("\n[Tokens] upload: {} download: {}\n", input, output);
                io::stdout().flush()?;
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
