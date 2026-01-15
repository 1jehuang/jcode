#![allow(dead_code)]

#![allow(dead_code)]

use super::{EventStream, Provider};
use crate::message::{Message, StreamEvent, ToolDefinition};
use crate::storage;
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

const DEFAULT_MODEL: &str = "claude-opus-4-5-20251101";
const DEFAULT_PERMISSION_MODE: &str = "bypassPermissions";
const BRIDGE_SCRIPT: &str = include_str!("../../scripts/claude_agent_sdk_bridge.py");

/// Available Claude models
const AVAILABLE_MODELS: &[&str] = &[
    "claude-opus-4-5-20251101",
    "claude-sonnet-4-5-20250929",
    "claude-sonnet-4-20250514",
    "claude-haiku-3-5-20241022",
];

#[derive(Clone)]
pub struct ClaudeProvider {
    config: ClaudeSdkConfig,
    model: std::sync::Arc<std::sync::RwLock<String>>,
}

impl ClaudeProvider {
    pub fn new() -> Self {
        let config = ClaudeSdkConfig::from_env();
        let model = config.model.clone();
        Self {
            config,
            model: std::sync::Arc::new(std::sync::RwLock::new(model)),
        }
    }

    fn tool_names_for_sdk(&self, tools: &[ToolDefinition]) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut names = Vec::new();
        for tool in tools {
            let mapped = to_claude_tool_name(&tool.name);
            if seen.insert(mapped.clone()) {
                names.push(mapped);
            }
        }
        names
    }

    fn resolve_bridge_script(&self) -> Result<PathBuf> {
        if let Some(path) = &self.config.bridge_script_path {
            ensure_bridge_script(path)?;
            return Ok(path.clone());
        }

        let base = storage::jcode_dir()?;
        let path = base.join("claude_agent_sdk_bridge.py");
        ensure_bridge_script(&path)?;
        Ok(path)
    }

    async fn spawn_bridge(
        &self,
        script_path: &Path,
        request: &ClaudeSdkRequest<'_>,
    ) -> Result<tokio::process::Child> {
        let payload = serde_json::to_vec(request)?;

        let mut cmd = Command::new(&self.config.python_bin);
        cmd.arg("-u").arg(script_path);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().with_context(|| {
            format!(
                "Failed to spawn Claude Agent SDK bridge using {}",
                self.config.python_bin
            )
        })?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture SDK stdin"))?;
        stdin.write_all(&payload).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
        drop(stdin);

        Ok(child)
    }
}

#[derive(Clone)]
struct ClaudeSdkConfig {
    python_bin: String,
    bridge_script_path: Option<PathBuf>,
    model: String,
    permission_mode: Option<String>,
    cli_path: Option<String>,
    include_partial_messages: bool,
    max_thinking_tokens: Option<u32>,
    use_claude_code_preset: bool,
}

impl ClaudeSdkConfig {
    fn from_env() -> Self {
        let python_bin = std::env::var("JCODE_CLAUDE_SDK_PYTHON")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "python3".to_string());

        let bridge_script_path = std::env::var("JCODE_CLAUDE_SDK_SCRIPT")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from);

        let model = std::env::var("JCODE_CLAUDE_SDK_MODEL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());

        let permission_mode = std::env::var("JCODE_CLAUDE_SDK_PERMISSION_MODE")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| Some(DEFAULT_PERMISSION_MODE.to_string()));

        let cli_path = std::env::var("JCODE_CLAUDE_SDK_CLI_PATH")
            .ok()
            .filter(|value| !value.trim().is_empty());

        let include_partial_messages = std::env::var("JCODE_CLAUDE_SDK_PARTIAL")
            .ok()
            .map(|value| {
                let value = value.to_lowercase();
                !(value == "0" || value == "false" || value == "no")
            })
            .unwrap_or(true);

        // Default to max thinking tokens (128k) for Opus models, can be overridden via env
        let max_thinking_tokens = std::env::var("JCODE_CLAUDE_SDK_THINKING_TOKENS")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .or_else(|| Some(128000)); // Max 128k tokens for extended thinking

        // Use Claude Code preset (default: false to save ~15-20k tokens)
        let use_claude_code_preset = std::env::var("JCODE_USE_CLAUDE_CODE_PRESET")
            .ok()
            .map(|value| {
                let value = value.to_lowercase();
                value == "1" || value == "true" || value == "yes"
            })
            .unwrap_or(false);

        Self {
            python_bin,
            bridge_script_path,
            model,
            permission_mode,
            cli_path,
            include_partial_messages,
            max_thinking_tokens,
            use_claude_code_preset,
        }
    }
}

#[derive(Serialize)]
struct ClaudeSdkRequest<'a> {
    system: &'a str,
    messages: &'a [Message],
    tools: Vec<String>,
    options: ClaudeSdkOptions,
}

#[derive(Serialize)]
struct ClaudeSdkOptions {
    model: String,
    permission_mode: Option<String>,
    cli_path: Option<String>,
    cwd: Option<String>,
    include_partial_messages: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    resume: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_thinking_tokens: Option<u32>,
    use_claude_code_preset: bool,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SdkOutput {
    StreamEvent { event: Value },
    AssistantMessage { content: Vec<SdkContentBlock> },
    UserMessage { content: Vec<SdkContentBlock> },
    ThinkingDone { duration_secs: f64 },
    Compaction {
        trigger: String,
        pre_tokens: Option<u64>,
    },
    Result {
        is_error: bool,
        usage: Option<UsageInfo>,
        session_id: Option<String>,
    },
    Error { message: String },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SdkContentBlock {
    Text { text: String },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[allow(dead_code)]
    ToolResult {
        tool_use_id: String,
        content: Option<Value>,
        #[serde(default)]
        is_error: Option<bool>,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
#[allow(dead_code)]
enum SseEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: Value },
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: usize,
        content_block: ContentBlockInfo,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: usize, delta: DeltaInfo },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: usize },
    #[serde(rename = "message_delta")]
    MessageDelta {
        delta: MessageDeltaInfo,
        #[serde(default)]
        usage: Option<UsageInfo>,
    },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "error")]
    Error { error: ErrorInfo },
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
#[allow(dead_code)]
enum ContentBlockInfo {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(other)]
    Other,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
enum DeltaInfo {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
    #[serde(rename = "signature_delta")]
    SignatureDelta { signature: String },
    #[serde(other)]
    Other,
}

#[derive(Deserialize, Debug)]
struct UsageInfo {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
}

#[derive(Deserialize, Debug)]
struct MessageDeltaInfo {
    stop_reason: Option<String>,
}

#[derive(Deserialize, Debug)]
struct ErrorInfo {
    message: String,
}

struct ClaudeEventTranslator {
    last_stop_reason: Option<String>,
    in_thinking_block: bool,
    in_tool_use_block: bool,
}

impl ClaudeEventTranslator {
    fn new() -> Self {
        Self {
            last_stop_reason: None,
            in_thinking_block: false,
            in_tool_use_block: false,
        }
    }

    fn handle_event(&mut self, event: SseEvent) -> Vec<StreamEvent> {
        match event {
            SseEvent::MessageStart { message } => {
                if let Some(usage) = message.get("usage") {
                    let input_tokens = usage.get("input_tokens").and_then(|v| v.as_u64());
                    let output_tokens = usage.get("output_tokens").and_then(|v| v.as_u64());
                    if input_tokens.is_some() || output_tokens.is_some() {
                        return vec![StreamEvent::TokenUsage {
                            input_tokens,
                            output_tokens,
                        }];
                    }
                }
                Vec::new()
            }
            SseEvent::ContentBlockStart { content_block, .. } => match content_block {
                ContentBlockInfo::Text { .. } => Vec::new(),
                ContentBlockInfo::ToolUse { id, name } => {
                    self.in_tool_use_block = true;
                    vec![StreamEvent::ToolUseStart {
                        id,
                        name: to_internal_tool_name(&name),
                    }]
                }
                ContentBlockInfo::Thinking { .. } => {
                    self.in_thinking_block = true;
                    vec![StreamEvent::ThinkingStart]
                }
                ContentBlockInfo::Other => Vec::new(),
            },
            SseEvent::ContentBlockDelta { delta, .. } => match delta {
                DeltaInfo::TextDelta { text } => vec![StreamEvent::TextDelta(text)],
                DeltaInfo::InputJsonDelta { partial_json } => {
                    vec![StreamEvent::ToolInputDelta(partial_json)]
                }
                // Thinking deltas and signatures are internal - silently consume
                DeltaInfo::ThinkingDelta { .. } => Vec::new(),
                DeltaInfo::SignatureDelta { .. } => Vec::new(),
                DeltaInfo::Other => Vec::new(),
            },
            SseEvent::ContentBlockStop { .. } => {
                if self.in_thinking_block {
                    self.in_thinking_block = false;
                    vec![StreamEvent::ThinkingEnd]
                } else if self.in_tool_use_block {
                    self.in_tool_use_block = false;
                    vec![StreamEvent::ToolUseEnd]
                } else {
                    Vec::new()
                }
            }
            SseEvent::MessageDelta { delta, usage } => {
                self.last_stop_reason = delta.stop_reason.clone();
                if let Some(usage) = usage {
                    if usage.input_tokens.is_some() || usage.output_tokens.is_some() {
                        return vec![StreamEvent::TokenUsage {
                            input_tokens: usage.input_tokens,
                            output_tokens: usage.output_tokens,
                        }];
                    }
                }
                Vec::new()
            }
            SseEvent::MessageStop => vec![StreamEvent::MessageEnd {
                stop_reason: self.last_stop_reason.take(),
            }],
            SseEvent::Error { error } => vec![StreamEvent::Error(error.message)],
            _ => Vec::new(),
        }
    }
}

struct OutputParser {
    translator: ClaudeEventTranslator,
    saw_stream_events: bool,
    saw_message_end: bool,
}

impl OutputParser {
    fn new() -> Self {
        Self {
            translator: ClaudeEventTranslator::new(),
            saw_stream_events: false,
            saw_message_end: false,
        }
    }

    fn handle_output(&mut self, output: SdkOutput) -> Vec<StreamEvent> {
        match output {
            SdkOutput::StreamEvent { event } => {
                self.saw_stream_events = true;
                let parsed: SseEvent = match serde_json::from_value(event) {
                    Ok(parsed) => parsed,
                    Err(err) => {
                        return vec![StreamEvent::Error(format!(
                            "Failed to parse SDK stream event: {}",
                            err
                        ))];
                    }
                };

                let events = self.translator.handle_event(parsed);
                if events
                    .iter()
                    .any(|event| matches!(event, StreamEvent::MessageEnd { .. }))
                {
                    self.saw_message_end = true;
                }
                events
            }
            SdkOutput::AssistantMessage { content } => {
                let mut events = Vec::new();
                for block in content {
                    match block {
                        SdkContentBlock::Text { text } => {
                            // Skip text if we already streamed it
                            if !self.saw_stream_events {
                                events.push(StreamEvent::TextDelta(text));
                            }
                        }
                        SdkContentBlock::ToolUse { id, name, input } => {
                            // Skip tool_use if we already streamed it
                            if !self.saw_stream_events {
                                events.push(StreamEvent::ToolUseStart {
                                    id,
                                    name: to_internal_tool_name(&name),
                                });
                                events.push(StreamEvent::ToolInputDelta(
                                    serde_json::to_string(&input).unwrap_or_default(),
                                ));
                                events.push(StreamEvent::ToolUseEnd);
                            }
                        }
                        SdkContentBlock::ToolResult { tool_use_id, content, is_error } => {
                            // Always emit tool results - they contain the actual output/diffs
                            // and only come through AssistantMessage, not stream events
                            let content_str = content
                                .map(|v| {
                                    if let Some(s) = v.as_str() {
                                        s.to_string()
                                    } else {
                                        serde_json::to_string(&v).unwrap_or_default()
                                    }
                                })
                                .unwrap_or_default();
                            events.push(StreamEvent::ToolResult {
                                tool_use_id,
                                content: content_str,
                                is_error: is_error.unwrap_or(false),
                            });
                        }
                        _ => {}
                    }
                }

                if !self.saw_message_end {
                    self.saw_message_end = true;
                    events.push(StreamEvent::MessageEnd { stop_reason: None });
                }

                events
            }
            SdkOutput::UserMessage { content } => {
                // UserMessage contains tool results when SDK executes tools
                let mut events = Vec::new();
                for block in content {
                    if let SdkContentBlock::ToolResult { tool_use_id, content, is_error } = block {
                        let content_str = content
                            .map(|v| {
                                if let Some(s) = v.as_str() {
                                    s.to_string()
                                } else {
                                    serde_json::to_string(&v).unwrap_or_default()
                                }
                            })
                            .unwrap_or_default();
                        events.push(StreamEvent::ToolResult {
                            tool_use_id,
                            content: content_str,
                            is_error: is_error.unwrap_or(false),
                        });
                    }
                }
                events
            }
            SdkOutput::Result { usage, is_error, session_id } => {
                let mut events = Vec::new();
                if let Some(usage) = usage {
                    if usage.input_tokens.is_some() || usage.output_tokens.is_some() {
                        events.push(StreamEvent::TokenUsage {
                            input_tokens: usage.input_tokens,
                            output_tokens: usage.output_tokens,
                        });
                    }
                }
                if let Some(sid) = session_id {
                    events.push(StreamEvent::SessionId(sid));
                }
                if is_error {
                    events.push(StreamEvent::Error(
                        "Claude Agent SDK reported an error".to_string(),
                    ));
                }
                if !self.saw_message_end {
                    self.saw_message_end = true;
                    events.push(StreamEvent::MessageEnd { stop_reason: None });
                }
                events
            }
            SdkOutput::ThinkingDone { duration_secs } => {
                vec![StreamEvent::ThinkingDone { duration_secs }]
            }
            SdkOutput::Compaction { trigger, pre_tokens } => {
                vec![StreamEvent::Compaction { trigger, pre_tokens }]
            }
            SdkOutput::Error { message } => vec![StreamEvent::Error(message)],
            SdkOutput::Other => Vec::new(),
        }
    }
}

#[async_trait]
impl Provider for ClaudeProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let tool_names = self.tool_names_for_sdk(tools);
        let cwd = std::env::current_dir()
            .ok()
            .map(|path| path.display().to_string());

        // Get current model (using runtime value, not config)
        let current_model = self.model.read()
            .map(|m| m.clone())
            .unwrap_or_else(|_| self.config.model.clone());

        let request = ClaudeSdkRequest {
            system,
            messages,
            tools: tool_names,
            options: ClaudeSdkOptions {
                model: current_model,
                permission_mode: self.config.permission_mode.clone(),
                cli_path: self.config.cli_path.clone(),
                cwd,
                include_partial_messages: self.config.include_partial_messages,
                resume: resume_session_id.map(|s| s.to_string()),
                max_thinking_tokens: self.config.max_thinking_tokens,
                use_claude_code_preset: self.config.use_claude_code_preset,
            },
        };

        let script_path = self.resolve_bridge_script()?;
        let mut child = match self.spawn_bridge(&script_path, &request).await {
            Ok(child) => child,
            Err(err) => {
                if self.config.python_bin == "python3"
                    && err
                        .root_cause()
                        .downcast_ref::<std::io::Error>()
                        .map(|e| e.kind() == std::io::ErrorKind::NotFound)
                        .unwrap_or(false)
                {
                    let mut fallback = self.clone();
                    fallback.config.python_bin = "python".to_string();
                    fallback.spawn_bridge(&script_path, &request).await?
                } else {
                    return Err(err);
                }
            }
        };

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture SDK stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture SDK stderr"))?;

        let (tx, rx) = mpsc::channel(200);

        tokio::spawn(async move {
            let mut stderr_reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = stderr_reader.next_line().await {
                if !line.trim().is_empty() {
                    eprintln!("[claude-sdk] {}", line);
                }
            }
        });

        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            let mut parser = OutputParser::new();

            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }

                let output = match serde_json::from_str::<SdkOutput>(&line) {
                    Ok(parsed) => parsed,
                    Err(err) => {
                        let _ = tx
                            .send(Ok(StreamEvent::Error(format!(
                                "Failed to parse SDK output: {}",
                                err
                            ))))
                            .await;
                        continue;
                    }
                };

                for event in parser.handle_output(output) {
                    if tx.send(Ok(event)).await.is_err() {
                        break;
                    }
                }
            }

            let _ = child.wait().await;
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn name(&self) -> &str {
        "claude"
    }

    fn model(&self) -> String {
        self.model
            .read()
            .map(|m| m.clone())
            .unwrap_or_else(|_| DEFAULT_MODEL.to_string())
    }

    fn set_model(&self, model: &str) -> Result<()> {
        if !AVAILABLE_MODELS.contains(&model) {
            eprintln!("Warning: '{}' is not in the known model list, but will try anyway", model);
        }
        if let Ok(mut current) = self.model.write() {
            *current = model.to_string();
            Ok(())
        } else {
            Err(anyhow::anyhow!("Cannot change model while a request is in progress"))
        }
    }

    fn available_models(&self) -> Vec<&'static str> {
        AVAILABLE_MODELS.to_vec()
    }

    fn handles_tools_internally(&self) -> bool {
        // Claude Agent SDK executes tools internally - jcode should not re-execute them
        true
    }
}

fn ensure_bridge_script(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        storage::ensure_dir(parent)?;
    }

    let should_write = match std::fs::read_to_string(path) {
        Ok(existing) => existing != BRIDGE_SCRIPT,
        Err(_) => true,
    };

    if should_write {
        std::fs::write(path, BRIDGE_SCRIPT)?;
    }

    Ok(())
}

fn to_claude_tool_name(name: &str) -> String {
    match name {
        "bash" => "Bash",
        "read" => "Read",
        "write" => "Write",
        "edit" => "Edit",
        "multiedit" => "MultiEdit",
        "patch" => "Patch",
        "apply_patch" => "ApplyPatch",
        "glob" => "Glob",
        "grep" => "Grep",
        "ls" => "Ls",
        "webfetch" => "WebFetch",
        "websearch" => "WebSearch",
        "codesearch" => "CodeSearch",
        "invalid" => "Invalid",
        "skill" => "Skill",
        "skill_manage" => "SkillManage",
        "conversation_search" => "ConversationSearch",
        "lsp" => "Lsp",
        "task" => "Task",
        "todowrite" => "TodoWrite",
        "todoread" => "TodoRead",
        "batch" => "Batch",
        _ => name,
    }
    .to_string()
}

fn to_internal_tool_name(name: &str) -> String {
    match name {
        "Bash" => "bash",
        "Read" => "read",
        "Write" => "write",
        "Edit" => "edit",
        "MultiEdit" => "multiedit",
        "Patch" => "patch",
        "ApplyPatch" => "apply_patch",
        "Glob" => "glob",
        "Grep" => "grep",
        "Ls" => "ls",
        "WebFetch" => "webfetch",
        "WebSearch" => "websearch",
        "CodeSearch" => "codesearch",
        "Invalid" => "invalid",
        "Skill" => "skill",
        "SkillManage" => "skill_manage",
        "ConversationSearch" => "conversation_search",
        "Lsp" => "lsp",
        "Task" => "task",
        "TodoWrite" => "todowrite",
        "TodoRead" => "todoread",
        "Batch" => "batch",
        _ => name,
    }
    .to_string()
}
