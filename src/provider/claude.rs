#![allow(dead_code)]
#![allow(dead_code)]

use super::{EventStream, Provider};
use crate::message::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, RwLock};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

const DEFAULT_MODEL: &str = "claude-opus-4-5-20251101";
const DEFAULT_PERMISSION_MODE: &str = "bypassPermissions";

/// Available Claude models
const AVAILABLE_MODELS: &[&str] = &["claude-opus-4-5-20251101"];

/// Native tools that jcode handles locally (not Claude Code built-ins)
const NATIVE_TOOL_NAMES: &[&str] = &[
    "selfdev",
    "communicate",
    "memory",
    "remember",
    "session_search",
    "bg",
];

/// Channel for sending native tool results back to the provider (unused for CLI)
pub type NativeToolResultSender = mpsc::Sender<NativeToolResult>;

/// Native tool result to send back to provider (unused for CLI)
#[derive(Debug, Clone, Serialize)]
pub struct NativeToolResult {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub request_id: String,
    pub result: NativeToolResultPayload,
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct NativeToolResultPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl NativeToolResult {
    pub fn success(request_id: String, output: String) -> Self {
        Self {
            msg_type: "native_tool_result",
            request_id,
            result: NativeToolResultPayload {
                output: Some(output),
                error: None,
            },
            is_error: false,
        }
    }

    pub fn error(request_id: String, error: String) -> Self {
        Self {
            msg_type: "native_tool_result",
            request_id,
            result: NativeToolResultPayload {
                output: None,
                error: Some(error),
            },
            is_error: true,
        }
    }
}

#[derive(Clone)]
pub struct ClaudeProvider {
    config: ClaudeCliConfig,
    model: Arc<RwLock<String>>,
}

impl ClaudeProvider {
    pub fn new() -> Self {
        let config = ClaudeCliConfig::from_env();
        let model = config.model.clone();
        Self {
            config,
            model: Arc::new(RwLock::new(model)),
        }
    }

    fn tool_names_for_cli(&self, tools: &[ToolDefinition]) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut names = Vec::new();
        for tool in tools {
            if NATIVE_TOOL_NAMES.contains(&tool.name.as_str()) {
                continue;
            }
            let mapped = to_claude_tool_name(&tool.name);
            if seen.insert(mapped.clone()) {
                names.push(mapped);
            }
        }
        names
    }

    fn extract_user_prompt(&self, messages: &[Message]) -> Result<String> {
        for msg in messages.iter().rev() {
            if msg.role != Role::User {
                continue;
            }
            let mut parts = Vec::new();
            for block in &msg.content {
                match block {
                    ContentBlock::Text { text, .. } => parts.push(text.clone()),
                    ContentBlock::ToolResult { content, .. } => parts.push(content.clone()),
                    ContentBlock::ToolUse { .. } => {}
                }
            }
            if !parts.is_empty() {
                return Ok(parts.join("\n\n"));
            }
        }
        anyhow::bail!("No user prompt found for Claude CLI request");
    }
}

#[derive(Clone)]
struct ClaudeCliConfig {
    cli_path: String,
    model: String,
    permission_mode: Option<String>,
    include_partial_messages: bool,
}

impl ClaudeCliConfig {
    fn from_env() -> Self {
        let cli_path = std::env::var("JCODE_CLAUDE_CLI_PATH")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "claude".to_string());

        let mut model = std::env::var("JCODE_CLAUDE_CLI_MODEL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
        if !AVAILABLE_MODELS.contains(&model.as_str()) {
            crate::logging::info(&format!(
                "Warning: '{}' is not supported; falling back to '{}'",
                model, DEFAULT_MODEL
            ));
            model = DEFAULT_MODEL.to_string();
        }

        let permission_mode = std::env::var("JCODE_CLAUDE_CLI_PERMISSION_MODE")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                std::env::var("JCODE_CLAUDE_SDK_PERMISSION_MODE")
                    .ok()
                    .filter(|value| !value.trim().is_empty())
            })
            .or_else(|| Some(DEFAULT_PERMISSION_MODE.to_string()));

        let include_partial_messages = std::env::var("JCODE_CLAUDE_CLI_PARTIAL")
            .ok()
            .or_else(|| std::env::var("JCODE_CLAUDE_SDK_PARTIAL").ok())
            .map(|value| {
                let value = value.to_lowercase();
                !(value == "0" || value == "false" || value == "no")
            })
            .unwrap_or(true);

        Self {
            cli_path,
            model,
            permission_mode,
            include_partial_messages,
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum CliOutput {
    System {
        #[serde(default)]
        session_id: Option<String>,
    },
    StreamEvent {
        event: Value,
        #[serde(default)]
        session_id: Option<String>,
    },
    Assistant {
        message: CliMessage,
        #[serde(default)]
        session_id: Option<String>,
    },
    User {
        message: CliMessage,
        #[serde(default)]
        session_id: Option<String>,
    },
    Result {
        #[serde(default)]
        is_error: bool,
        #[serde(default)]
        usage: Option<UsageInfo>,
        #[serde(default)]
        session_id: Option<String>,
    },
    Error {
        message: String,
        #[serde(default)]
        retry_after_secs: Option<u64>,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct CliMessage {
    content: Value,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SdkContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
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
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
}

#[derive(Deserialize, Debug)]
struct MessageDeltaInfo {
    stop_reason: Option<String>,
}

#[derive(Deserialize, Debug)]
struct ErrorInfo {
    message: String,
    #[serde(default)]
    retry_after_secs: Option<u64>,
    #[serde(default)]
    status_code: Option<u16>,
    #[serde(default)]
    error_type: Option<String>,
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
                    let cache_creation_input_tokens = usage
                        .get("cache_creation_input_tokens")
                        .and_then(|v| v.as_u64());
                    let cache_read_input_tokens = usage
                        .get("cache_read_input_tokens")
                        .and_then(|v| v.as_u64());
                    if input_tokens.is_some()
                        || output_tokens.is_some()
                        || cache_creation_input_tokens.is_some()
                        || cache_read_input_tokens.is_some()
                    {
                        return vec![StreamEvent::TokenUsage {
                            input_tokens,
                            output_tokens,
                            cache_read_input_tokens,
                            cache_creation_input_tokens,
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
                    if usage.input_tokens.is_some()
                        || usage.output_tokens.is_some()
                        || usage.cache_creation_input_tokens.is_some()
                        || usage.cache_read_input_tokens.is_some()
                    {
                        return vec![StreamEvent::TokenUsage {
                            input_tokens: usage.input_tokens,
                            output_tokens: usage.output_tokens,
                            cache_read_input_tokens: usage.cache_read_input_tokens,
                            cache_creation_input_tokens: usage.cache_creation_input_tokens,
                        }];
                    }
                }
                Vec::new()
            }
            SseEvent::MessageStop => vec![StreamEvent::MessageEnd {
                stop_reason: self.last_stop_reason.take(),
            }],
            SseEvent::Error { error } => vec![StreamEvent::Error {
                message: error.message,
                retry_after_secs: error.retry_after_secs,
            }],
            _ => Vec::new(),
        }
    }
}

struct CliOutputParser {
    translator: ClaudeEventTranslator,
    saw_stream_events: bool,
    saw_message_end: bool,
}

impl CliOutputParser {
    fn new() -> Self {
        Self {
            translator: ClaudeEventTranslator::new(),
            saw_stream_events: false,
            saw_message_end: false,
        }
    }

    fn handle_output(&mut self, output: CliOutput) -> Vec<StreamEvent> {
        match output {
            CliOutput::StreamEvent { event, .. } => {
                self.saw_stream_events = true;
                let parsed: SseEvent = match serde_json::from_value(event) {
                    Ok(parsed) => parsed,
                    Err(err) => {
                        return vec![StreamEvent::Error {
                            message: format!("Failed to parse Claude CLI stream event: {}", err),
                            retry_after_secs: None,
                        }];
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
            CliOutput::Assistant { message, .. } => {
                let blocks = parse_content_blocks(&message.content);
                let mut events = Vec::new();
                for block in blocks {
                    match block {
                        SdkContentBlock::Text { text } => {
                            if !self.saw_stream_events {
                                events.push(StreamEvent::TextDelta(text));
                            }
                        }
                        SdkContentBlock::ToolUse { id, name, input } => {
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
                        SdkContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => {
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
            CliOutput::User { message, .. } => {
                let blocks = parse_content_blocks(&message.content);
                let mut events = Vec::new();
                for block in blocks {
                    if let SdkContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } = block
                    {
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
            CliOutput::Result {
                usage,
                is_error,
                session_id,
            } => {
                let mut events = Vec::new();
                if let Some(usage) = usage {
                    if usage.input_tokens.is_some()
                        || usage.output_tokens.is_some()
                        || usage.cache_creation_input_tokens.is_some()
                        || usage.cache_read_input_tokens.is_some()
                    {
                        events.push(StreamEvent::TokenUsage {
                            input_tokens: usage.input_tokens,
                            output_tokens: usage.output_tokens,
                            cache_read_input_tokens: usage.cache_read_input_tokens,
                            cache_creation_input_tokens: usage.cache_creation_input_tokens,
                        });
                    }
                }
                if let Some(sid) = session_id {
                    events.push(StreamEvent::SessionId(sid));
                }
                if is_error {
                    events.push(StreamEvent::Error {
                        message: "Claude CLI reported an error".to_string(),
                        retry_after_secs: None,
                    });
                }
                if !self.saw_message_end {
                    self.saw_message_end = true;
                    events.push(StreamEvent::MessageEnd { stop_reason: None });
                }
                events
            }
            CliOutput::Error {
                message,
                retry_after_secs,
            } => vec![StreamEvent::Error {
                message,
                retry_after_secs,
            }],
            CliOutput::System { session_id } => {
                session_id.map(StreamEvent::SessionId).into_iter().collect()
            }
            CliOutput::Other => Vec::new(),
        }
    }
}

fn parse_content_blocks(content: &Value) -> Vec<SdkContentBlock> {
    match content {
        Value::String(text) => vec![SdkContentBlock::Text { text: text.clone() }],
        Value::Array(items) => items
            .iter()
            .filter_map(|item| serde_json::from_value(item.clone()).ok())
            .collect(),
        _ => Vec::new(),
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
        let tool_names = self.tool_names_for_cli(tools);
        let prompt = self.extract_user_prompt(messages)?;
        let current_model = self
            .model
            .read()
            .map(|m| m.clone())
            .unwrap_or_else(|_| self.config.model.clone());
        let config = self.config.clone();
        let system_prompt = system.to_string();
        let resume = resume_session_id.map(|s| s.to_string());
        let cwd = std::env::current_dir().ok();

        let (tx, rx) = mpsc::channel::<Result<StreamEvent>>(100);

        tokio::spawn(async move {
            if let Err(e) = run_claude_cli(
                config,
                current_model,
                tool_names,
                system_prompt,
                resume,
                prompt,
                cwd,
                tx.clone(),
            )
            .await
            {
                let _ = tx.send(Err(e)).await;
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn model(&self) -> String {
        self.model.read().unwrap().clone()
    }

    fn set_model(&self, model: &str) -> Result<()> {
        if !AVAILABLE_MODELS.contains(&model) {
            anyhow::bail!("Unsupported Claude model '{}'.", model);
        }
        if let Ok(mut current) = self.model.write() {
            *current = model.to_string();
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Cannot change model while a request is in progress"
            ))
        }
    }

    fn available_models(&self) -> Vec<&'static str> {
        AVAILABLE_MODELS.to_vec()
    }

    fn handles_tools_internally(&self) -> bool {
        true
    }

    fn name(&self) -> &'static str {
        "claude"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        let model = self.model();
        let config = self.config.clone();
        Arc::new(ClaudeProvider {
            config,
            model: Arc::new(RwLock::new(model)),
        })
    }

    fn native_result_sender(&self) -> Option<NativeToolResultSender> {
        None
    }
}

async fn run_claude_cli(
    config: ClaudeCliConfig,
    model: String,
    tool_names: Vec<String>,
    system: String,
    resume_session_id: Option<String>,
    prompt: String,
    cwd: Option<PathBuf>,
    tx: mpsc::Sender<Result<StreamEvent>>,
) -> Result<()> {
    let mut cmd = Command::new(&config.cli_path);
    cmd.arg("-p")
        .arg("--verbose")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--input-format")
        .arg("stream-json")
        .arg("--model")
        .arg(&model);

    if config.include_partial_messages {
        cmd.arg("--include-partial-messages");
    }

    if let Some(mode) = &config.permission_mode {
        cmd.arg("--permission-mode").arg(mode);
    }

    if let Some(ref resume) = resume_session_id {
        cmd.arg("--resume").arg(resume);
    } else if !system.trim().is_empty() {
        cmd.arg("--append-system-prompt").arg(system);
    }

    if tool_names.is_empty() {
        cmd.arg("--tools").arg("");
    } else {
        cmd.arg("--tools").arg(tool_names.join(","));
    }

    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .with_context(|| format!("Failed to spawn Claude CLI using {}", config.cli_path))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("Failed to capture Claude CLI stdin"))?;

    let payload = serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": prompt,
        }
    });

    stdin.write_all(payload.to_string().as_bytes()).await?;
    stdin.write_all(b"\n").await?;
    stdin.flush().await?;
    drop(stdin);

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("Failed to capture Claude CLI stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("Failed to capture Claude CLI stderr"))?;

    let tx_stderr = tx.clone();
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            crate::logging::debug(&format!("[claude-cli] {}", line));
        }
        drop(tx_stderr);
    });

    let mut reader = BufReader::new(stdout).lines();
    let mut parser = CliOutputParser::new();

    while let Some(line) = reader.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<CliOutput>(line) {
            Ok(output) => {
                for event in parser.handle_output(output) {
                    if tx.send(Ok(event)).await.is_err() {
                        return Ok(());
                    }
                }
            }
            Err(err) => {
                let event = StreamEvent::Error {
                    message: format!("Failed to parse Claude CLI output: {}", err),
                    retry_after_secs: None,
                };
                if tx.send(Ok(event)).await.is_err() {
                    return Ok(());
                }
            }
        }
    }

    let status = child.wait().await?;
    if !status.success() {
        let event = StreamEvent::Error {
            message: format!("Claude CLI exited with status {}", status),
            retry_after_secs: None,
        };
        let _ = tx.send(Ok(event)).await;
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
        "codesearch" => "ToolSearch",
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
        "ToolSearch" => "codesearch",
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
