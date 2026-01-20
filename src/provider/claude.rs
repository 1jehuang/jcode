#![allow(dead_code)]
#![allow(dead_code)]

use super::{EventStream, Provider};
use crate::message::{CacheControl, ContentBlock, Message, Role, StreamEvent, ToolDefinition};
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
const AVAILABLE_MODELS: &[&str] = &["claude-opus-4-5-20251101"];

/// Native tools that jcode handles locally (not SDK built-ins)
const NATIVE_TOOL_NAMES: &[&str] = &["selfdev", "communicate", "memory", "remember", "session_search", "bg"];

/// Native tool definition for SDK
#[derive(Serialize)]
struct NativeToolDef {
    name: String,
    description: String,
    input_schema: Value,
}

/// Channel for sending native tool results back to the bridge
pub type NativeToolResultSender = mpsc::Sender<NativeToolResult>;

/// Native tool result to send back to the Python bridge
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
    config: ClaudeSdkConfig,
    model: std::sync::Arc<std::sync::RwLock<String>>,
    /// Sender for native tool results - populated during complete()
    native_result_sender: std::sync::Arc<std::sync::Mutex<Option<NativeToolResultSender>>>,
}

impl ClaudeProvider {
    pub fn new() -> Self {
        let config = ClaudeSdkConfig::from_env();
        let model = config.model.clone();
        Self {
            config,
            model: std::sync::Arc::new(std::sync::RwLock::new(model)),
            native_result_sender: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Get a sender for native tool results (if a completion is in progress)
    pub fn native_result_sender(&self) -> Option<NativeToolResultSender> {
        self.native_result_sender
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
    }

    fn tool_names_for_sdk(&self, tools: &[ToolDefinition]) -> Vec<String> {
        // Pass SDK-known tools as names (SDK has built-in implementations)
        // Native tools like selfdev are passed separately with full definitions
        let mut seen = HashSet::new();
        let mut names = Vec::new();
        for tool in tools {
            // Skip native tools - they're handled via native_tools_for_sdk
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

    fn native_tools_for_sdk(&self, tools: &[ToolDefinition]) -> Vec<NativeToolDef> {
        // Pass native tool definitions so the bridge can create MCP tools for them
        tools
            .iter()
            .filter(|t| NATIVE_TOOL_NAMES.contains(&t.name.as_str()))
            .map(|t| NativeToolDef {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.input_schema.clone(),
            })
            .collect()
    }

    fn apply_prompt_cache_control(&self, messages: &[Message]) -> Vec<Message> {
        if !self.config.prompt_cache || messages.is_empty() {
            return messages.to_vec();
        }

        let mut cloned = messages.to_vec();
        let mut applied = false;
        let cache_marker = CacheControl::ephemeral(self.config.prompt_cache_ttl.clone());

        for msg in cloned.iter_mut().rev() {
            if !matches!(msg.role, Role::User | Role::Assistant) {
                continue;
            }
            for block in msg.content.iter_mut().rev() {
                if let ContentBlock::Text { cache_control, .. } = block {
                    *cache_control = Some(cache_marker.clone());
                    applied = true;
                    break;
                }
            }
            if applied {
                break;
            }
        }

        cloned
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
    ) -> Result<(tokio::process::Child, tokio::process::ChildStdin)> {
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
        // Keep stdin open for native tool results
        Ok((child, stdin))
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
    prompt_cache: bool,
    prompt_cache_ttl: Option<String>,
}

impl ClaudeSdkConfig {
    fn from_env() -> Self {
        let python_bin = std::env::var("JCODE_CLAUDE_SDK_PYTHON")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| {
                // Check common venv location first
                if let Some(home) = dirs::home_dir() {
                    let venv_python = home.join(".venv/bin/python3");
                    if venv_python.exists() {
                        return venv_python.to_string_lossy().to_string();
                    }
                }
                "python3".to_string()
            });

        let bridge_script_path = std::env::var("JCODE_CLAUDE_SDK_SCRIPT")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from);

        let mut model = std::env::var("JCODE_CLAUDE_SDK_MODEL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
        if !AVAILABLE_MODELS.contains(&model.as_str()) {
            eprintln!(
                "Warning: '{}' is not supported; falling back to '{}'",
                model, DEFAULT_MODEL
            );
            model = DEFAULT_MODEL.to_string();
        }

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
        let prompt_cache = std::env::var("JCODE_CLAUDE_PROMPT_CACHE")
            .map(|v| v != "0" && v != "false")
            .unwrap_or(true);
        let prompt_cache_ttl = std::env::var("JCODE_CLAUDE_PROMPT_CACHE_TTL")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let prompt_cache_ttl = match prompt_cache_ttl.as_deref() {
            Some("5m") | Some("1h") => prompt_cache_ttl,
            Some(other) => {
                eprintln!(
                    "Warning: Unsupported JCODE_CLAUDE_PROMPT_CACHE_TTL '{}'; expected '5m' or '1h'",
                    other
                );
                None
            }
            None => None,
        };

        Self {
            python_bin,
            bridge_script_path,
            model,
            permission_mode,
            cli_path,
            include_partial_messages,
            max_thinking_tokens,
            prompt_cache,
            prompt_cache_ttl,
        }
    }
}

#[derive(Serialize)]
struct ClaudeSdkRequest<'a> {
    system: &'a str,
    messages: &'a [Message],
    tools: Vec<String>,
    /// Native tool definitions for jcode-specific tools (selfdev, etc.)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    native_tools: Vec<NativeToolDef>,
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
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SdkOutput {
    StreamEvent {
        event: Value,
    },
    AssistantMessage {
        content: Vec<SdkContentBlock>,
    },
    UserMessage {
        content: Vec<SdkContentBlock>,
    },
    ThinkingDone {
        duration_secs: f64,
    },
    Compaction {
        trigger: String,
        pre_tokens: Option<u64>,
    },
    Result {
        is_error: bool,
        usage: Option<UsageInfo>,
        session_id: Option<String>,
    },
    Error {
        message: String,
        #[serde(default)]
        retry_after_secs: Option<u64>,
    },
    /// Native tool call request from the bridge - jcode needs to execute and send result
    NativeToolCall {
        request_id: String,
        tool_name: String,
        input: Value,
    },
    #[serde(other)]
    Other,
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
                        return vec![StreamEvent::Error {
                            message: format!("Failed to parse SDK stream event: {}", err),
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
                        SdkContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => {
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
            SdkOutput::Result {
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
                        message: "Claude Agent SDK reported an error".to_string(),
                        retry_after_secs: None,
                    });
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
            SdkOutput::Compaction {
                trigger,
                pre_tokens,
            } => {
                vec![StreamEvent::Compaction {
                    trigger,
                    pre_tokens,
                }]
            }
            SdkOutput::Error {
                message,
                retry_after_secs,
            } => vec![StreamEvent::Error {
                message,
                retry_after_secs,
            }],
            SdkOutput::NativeToolCall {
                request_id,
                tool_name,
                input,
            } => vec![StreamEvent::NativeToolCall {
                request_id,
                tool_name,
                input,
            }],
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
        let native_tools = self.native_tools_for_sdk(tools);
        let cwd = std::env::current_dir()
            .ok()
            .map(|path| path.display().to_string());

        // Get current model (using runtime value, not config)
        let current_model = self
            .model
            .read()
            .map(|m| m.clone())
            .unwrap_or_else(|_| self.config.model.clone());

        let cached_messages;
        let messages = if self.config.prompt_cache {
            cached_messages = self.apply_prompt_cache_control(messages);
            cached_messages.as_slice()
        } else {
            messages
        };

        // Note: We intentionally don't pass resume_session_id to the bridge.
        // SDK session resume doesn't work across process invocations (each bridge
        // spawn is a new process), so the bridge would send only the last user
        // message thinking the SDK has context - but it doesn't. Instead, we always
        // send full message history and let the bridge format it as context.
        let request = ClaudeSdkRequest {
            system,
            messages,
            tools: tool_names,
            native_tools,
            options: ClaudeSdkOptions {
                model: current_model,
                permission_mode: self.config.permission_mode.clone(),
                cli_path: self.config.cli_path.clone(),
                cwd,
                include_partial_messages: self.config.include_partial_messages,
                resume: None, // Don't use SDK resume - it doesn't persist across processes
                max_thinking_tokens: self.config.max_thinking_tokens,
            },
        };

        let script_path = self.resolve_bridge_script()?;
        let (mut child, stdin) = match self.spawn_bridge(&script_path, &request).await {
            Ok(result) => result,
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

        // Create channel for native tool results
        let (native_result_tx, mut native_result_rx) =
            mpsc::channel::<NativeToolResult>(50);

        // Store sender for external use
        if let Ok(mut guard) = self.native_result_sender.lock() {
            *guard = Some(native_result_tx);
        }

        // Spawn task to write native tool results to bridge stdin
        tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(result) = native_result_rx.recv().await {
                match serde_json::to_string(&result) {
                    Ok(json) => {
                        if stdin.write_all(json.as_bytes()).await.is_err() {
                            break;
                        }
                        if stdin.write_all(b"\n").await.is_err() {
                            break;
                        }
                        if stdin.flush().await.is_err() {
                            break;
                        }
                    }
                    Err(_) => continue,
                }
            }
        });

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
                            .send(Ok(StreamEvent::Error {
                                message: format!("Failed to parse SDK output: {}", err),
                                retry_after_secs: None,
                            }))
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
            anyhow::bail!(
                "Unsupported Claude model '{}'. Only supported model is '{}'.",
                model,
                DEFAULT_MODEL
            );
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
        // Claude Agent SDK executes tools internally - jcode should not re-execute them
        true
    }

    fn fork(&self) -> std::sync::Arc<dyn Provider> {
        let model = self.model();
        let config = self.config.clone();
        std::sync::Arc::new(ClaudeProvider {
            config,
            model: std::sync::Arc::new(std::sync::RwLock::new(model)),
            native_result_sender: std::sync::Arc::new(std::sync::Mutex::new(None)),
        })
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
