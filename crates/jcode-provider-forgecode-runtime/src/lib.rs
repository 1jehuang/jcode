//! ForgeCode CLI provider runtime (subprocess transport), moved out of
//! `jcode-base` so provider edits compile only this crate plus a binary
//! relink instead of rebuilding the base -> app-core -> tui spine. The
//! binary's composition root registers [`ForgeCodeProvider`] with
//! `jcode_base::provider::external` at startup.

use anyhow::{Context, Result};
use async_trait::async_trait;
use jcode_message_types::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use jcode_provider_core::{EventStream, Provider};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, LazyLock, RwLock};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{Mutex, mpsc};
use tokio_stream::wrappers::ReceiverStream;

/// Global mutex to serialize ForgeCode CLI requests.
/// Prevents concurrent subprocess writes from racing.
static FORGECODE_CLI_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

const DEFAULT_MODEL: &str = "default";

/// Maximum number of retries for transient errors.
const MAX_RETRIES: u32 = 5;

/// Base delay for exponential backoff (in milliseconds).
const RETRY_BASE_DELAY_MS: u64 = 1000;

/// Extra delay for subprocess transport errors.
const TRANSPORT_ERROR_DELAY_MS: u64 = 2000;

/// Native tools that jcode handles locally (not ForgeCode built-ins).
const NATIVE_TOOL_NAMES: &[&str] = &[
    "selfdev",
    "communicate",
    "memory",
    "session_search",
    "bg",
];

#[derive(Clone)]
pub struct ForgeCodeProvider {
    config: ForgeCodeCliConfig,
    model: Arc<RwLock<String>>,
}

impl ForgeCodeProvider {
    pub fn new() -> Self {
        let config = ForgeCodeCliConfig::from_env();
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
            let mapped = to_forgecode_tool_name(&tool.name);
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
                    ContentBlock::ToolResult { content, .. } => {
                        parts.push(content.clone());
                    }
                    ContentBlock::ToolUse { .. } => {}
                    ContentBlock::Reasoning { .. }
                    | ContentBlock::ReasoningTrace { .. }
                    | ContentBlock::AnthropicThinking { .. }
                    | ContentBlock::OpenAIReasoning { .. } => {}
                    ContentBlock::Image { .. } => {}
                    ContentBlock::OpenAICompaction { .. } => {}
                }
            }
            if !parts.is_empty() {
                return Ok(parts.join("\n\n"));
            }
        }
        anyhow::bail!("No user prompt found for ForgeCode CLI request");
    }
}

impl Default for ForgeCodeProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
struct ForgeCodeCliConfig {
    cli_path: String,
    model: String,
    permission_mode: Option<String>,
}

impl ForgeCodeCliConfig {
    fn from_env() -> Self {
        let cli_path = std::env::var("JCODE_FORGECODE_CLI_PATH")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "forge".to_string());

        let model = std::env::var("JCODE_FORGECODE_CLI_MODEL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());

        let permission_mode = std::env::var("JCODE_FORGECODE_CLI_PERMISSION_MODE")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| Some("bypassPermissions".to_string()));

        Self {
            cli_path,
            model,
            permission_mode,
        }
    }
}

// ---------------------------------------------------------------------------
// CLI output parsing types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum CliOutput {
    System {
        #[serde(default)]
        session_id: Option<String>,
    },
    StreamEvent {
        event: Value,
        #[serde(default, rename = "session_id")]
        _session_id: Option<String>,
    },
    Assistant {
        message: CliMessage,
        #[serde(default, rename = "session_id")]
        _session_id: Option<String>,
    },
    User {
        message: CliMessage,
        #[serde(default, rename = "session_id")]
        _session_id: Option<String>,
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
enum SseEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: Value },
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        #[serde(rename = "index")]
        _index: usize,
        content_block: ContentBlockInfo,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta {
        #[serde(rename = "index")]
        _index: usize,
        delta: DeltaInfo,
    },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop {
        #[serde(rename = "index")]
        _index: usize,
    },
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
enum ContentBlockInfo {
    #[serde(rename = "text")]
    Text {
        #[serde(rename = "text")]
        _text: String,
    },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
    #[serde(rename = "thinking")]
    Thinking {
        #[serde(rename = "thinking")]
        _thinking: String,
    },
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
    ThinkingDelta {
        #[serde(rename = "thinking")]
        _thinking: String,
    },
    #[serde(rename = "signature_delta")]
    SignatureDelta {
        #[serde(rename = "signature")]
        _signature: String,
    },
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
    #[serde(default, rename = "status_code")]
    _status_code: Option<u16>,
    #[serde(default, rename = "error_type")]
    _error_type: Option<String>,
}

// ---------------------------------------------------------------------------
// Event translator
// ---------------------------------------------------------------------------

struct ForgeCodeEventTranslator {
    last_stop_reason: Option<String>,
    in_thinking_block: bool,
    in_tool_use_block: bool,
}

impl ForgeCodeEventTranslator {
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
                    let output_tokens =
                        usage.get("output_tokens").and_then(|v| v.as_u64());
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
            SseEvent::ContentBlockStart {
                content_block, ..
            } => match content_block {
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
                DeltaInfo::TextDelta { text } => {
                    vec![StreamEvent::TextDelta(text)]
                }
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
                if let Some(usage) = usage
                    && (usage.input_tokens.is_some()
                        || usage.output_tokens.is_some()
                        || usage
                            .cache_creation_input_tokens
                            .is_some()
                        || usage.cache_read_input_tokens.is_some())
                {
                    return vec![StreamEvent::TokenUsage {
                        input_tokens: usage.input_tokens,
                        output_tokens: usage.output_tokens,
                        cache_read_input_tokens: usage.cache_read_input_tokens,
                        cache_creation_input_tokens: usage
                            .cache_creation_input_tokens,
                    }];
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

// ---------------------------------------------------------------------------
// CLI output parser
// ---------------------------------------------------------------------------

struct CliOutputParser {
    translator: ForgeCodeEventTranslator,
    saw_stream_events: bool,
    saw_message_end: bool,
}

impl CliOutputParser {
    fn new() -> Self {
        Self {
            translator: ForgeCodeEventTranslator::new(),
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
                            message: format!(
                                "Failed to parse ForgeCode CLI stream event: {}",
                                err
                            ),
                            retry_after_secs: None,
                        }];
                    }
                };

                let events = self.translator.handle_event(parsed);
                if events.iter().any(|event| {
                    matches!(event, StreamEvent::MessageEnd { .. })
                }) {
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
                        SdkContentBlock::ToolUse {
                            id, name, input,
                        } => {
                            if !self.saw_stream_events {
                                events.push(StreamEvent::ToolUseStart {
                                    id,
                                    name: to_internal_tool_name(&name),
                                });
                                events.push(StreamEvent::ToolInputDelta(
                                    serde_json::to_string(&input)
                                        .unwrap_or_default(),
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
                                        serde_json::to_string(&v)
                                            .unwrap_or_default()
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
                    events.push(StreamEvent::MessageEnd {
                        stop_reason: None,
                    });
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
                                    serde_json::to_string(&v)
                                        .unwrap_or_default()
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
                if let Some(usage) = usage
                    && (usage.input_tokens.is_some()
                        || usage.output_tokens.is_some()
                        || usage
                            .cache_creation_input_tokens
                            .is_some()
                        || usage.cache_read_input_tokens.is_some())
                {
                    events.push(StreamEvent::TokenUsage {
                        input_tokens: usage.input_tokens,
                        output_tokens: usage.output_tokens,
                        cache_read_input_tokens: usage.cache_read_input_tokens,
                        cache_creation_input_tokens: usage
                            .cache_creation_input_tokens,
                    });
                }
                if let Some(sid) = session_id {
                    events.push(StreamEvent::SessionId(sid));
                }
                if is_error {
                    events.push(StreamEvent::Error {
                        message: "ForgeCode CLI reported an error".to_string(),
                        retry_after_secs: None,
                    });
                }
                if !self.saw_message_end {
                    self.saw_message_end = true;
                    events.push(StreamEvent::MessageEnd {
                        stop_reason: None,
                    });
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
                session_id
                    .map(StreamEvent::SessionId)
                    .into_iter()
                    .collect()
            }
            CliOutput::Other => Vec::new(),
        }
    }
}

fn parse_content_blocks(content: &Value) -> Vec<SdkContentBlock> {
    match content {
        Value::String(text) => {
            vec![SdkContentBlock::Text {
                text: text.clone(),
            }]
        }
        Value::Array(items) => items
            .iter()
            .filter_map(|item| serde_json::from_value(item.clone()).ok())
            .collect(),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Provider trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Provider for ForgeCodeProvider {
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

        let payload = json!({
            "model": &current_model,
            "tool_names": &tool_names,
            "resume_present": resume.is_some(),
        });
        jcode_provider_core::fingerprint::log_provider_canonical_input(
            "forgecode-cli",
            &current_model,
            "forgecode_cli_prompt",
            &payload,
            &[Value::String(prompt.clone())],
            (!system_prompt.trim().is_empty())
                .then(|| Value::String(system_prompt.clone()))
                .as_ref(),
            Some(&Value::Array(
                tool_names
                    .iter()
                    .map(|name| Value::String(name.clone()))
                    .collect(),
            )),
            Some(tool_names.len()),
            &[
                (
                    "resume_present",
                    resume.is_some().to_string(),
                ),
                (
                    "logical_message_count",
                    messages.len().to_string(),
                ),
            ],
        );

        let (tx, rx) = mpsc::channel::<Result<StreamEvent>>(100);

        tokio::spawn(async move {
            if tx
                .send(Ok(StreamEvent::ConnectionType {
                    connection: "cli subprocess".to_string(),
                }))
                .await
                .is_err()
            {
                return;
            }
            let mut last_error: Option<anyhow::Error> = None;

            for attempt in 0..MAX_RETRIES {
                if attempt > 0 {
                    let base_delay =
                        jcode_provider_core::attempt_tracker::retry_backoff_delay(
                            attempt,
                            RETRY_BASE_DELAY_MS,
                        );
                    let extra_delay =
                        if let Some(ref e) = last_error {
                            let err_str =
                                e.to_string().to_lowercase();
                            if err_str.contains("not ready") {
                                TRANSPORT_ERROR_DELAY_MS
                            } else {
                                0
                            }
                        } else {
                            0
                        };
                    let delay = base_delay
                        + std::time::Duration::from_millis(extra_delay);
                    tokio::time::sleep(delay).await;
                    jcode_base::logging::info(&format!(
                        "Retrying ForgeCode CLI request (attempt \
                         {}/{}, delay {}ms)",
                        attempt + 1,
                        MAX_RETRIES,
                        delay.as_millis()
                    ));
                }

                let _guard = FORGECODE_CLI_LOCK.lock().await;

                match run_forgecode_cli(
                    config.clone(),
                    current_model.clone(),
                    tool_names.clone(),
                    system_prompt.clone(),
                    resume.clone(),
                    prompt.clone(),
                    cwd.clone(),
                    tx.clone(),
                )
                .await
                {
                    Ok(()) => return,
                    Err(e) => {
                        let error_str =
                            format!("{e:#}").to_lowercase();
                        if is_retryable_error(&error_str)
                            && attempt + 1 < MAX_RETRIES
                        {
                            jcode_base::logging::info(&format!(
                                "Transient error, will retry: {}",
                                e
                            ));
                            last_error = Some(e);
                            continue;
                        }
                        let _ = tx.send(Err(e)).await;
                        return;
                    }
                }
            }

            if let Some(e) = last_error {
                let _ = tx
                    .send(Err(anyhow::anyhow!(
                        "Failed after {} retries: {}",
                        MAX_RETRIES,
                        e
                    )))
                    .await;
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn model(&self) -> String {
        self.model
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn set_model(&self, model: &str) -> Result<()> {
        if let Ok(mut current) = self.model.write() {
            *current = model.to_string();
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Cannot change model while a request is in \
                 progress"
            ))
        }
    }

    fn handles_tools_internally(&self) -> bool {
        true
    }

    fn name(&self) -> &'static str {
        "forgecode"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        let model = self.model();
        let config = self.config.clone();
        Arc::new(ForgeCodeProvider {
            config,
            model: Arc::new(RwLock::new(model)),
        })
    }
}

// ---------------------------------------------------------------------------
// Subprocess execution
// ---------------------------------------------------------------------------

#[expect(
    clippy::too_many_arguments,
    reason = "ForgeCode CLI launch threads model, tools, system prompt, cwd, \
              resume state, and stream channel explicitly"
)]
async fn run_forgecode_cli(
    config: ForgeCodeCliConfig,
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

    cmd.kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .with_context(|| {
            format!(
                "Failed to spawn ForgeCode CLI using {}",
                config.cli_path
            )
        })?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Failed to capture ForgeCode CLI stdin"
            )
        })?;

    let payload = serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": prompt,
        }
    });

    async fn terminate_child(
        child: &mut tokio::process::Child,
    ) {
        let _ = child.kill().await;
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            child.wait(),
        )
        .await;
    }

    if let Err(err) = async {
        stdin
            .write_all(payload.to_string().as_bytes())
            .await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
        Ok::<(), std::io::Error>(())
    }
    .await
    {
        terminate_child(&mut child).await;
        return Err(err.into());
    }
    drop(stdin);

    let stdout = child.stdout.take().ok_or_else(|| {
        anyhow::anyhow!("Failed to capture ForgeCode CLI stdout")
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        anyhow::anyhow!("Failed to capture ForgeCode CLI stderr")
    })?;

    let tx_stderr = tx.clone();
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            jcode_base::logging::debug(&format!(
                "[forgecode-cli] {}",
                line
            ));
        }
        drop(tx_stderr);
    });

    let mut reader = BufReader::new(stdout).lines();
    let mut parser = CliOutputParser::new();
    let mut saw_output = false;

    loop {
        tokio::select! {
            _ = tx.closed() => {
                terminate_child(&mut child).await;
                return Ok(());
            }
            line = reader.next_line() => {
                let line = match line? {
                    Some(line) => line,
                    None => break,
                };
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                match serde_json::from_str::<CliOutput>(line) {
                    Ok(output) => {
                        for event in parser.handle_output(output) {
                            if let StreamEvent::Error {
                                message, ..
                            } = &event
                            {
                                let err_lower =
                                    message.to_lowercase();
                                if !saw_output
                                    && is_retryable_error(&err_lower)
                                {
                                    terminate_child(&mut child)
                                        .await;
                                    return Err(anyhow::anyhow!(
                                        message.clone()
                                    ));
                                }
                            }

                            if matches!(
                                event,
                                StreamEvent::TextDelta(_)
                                    | StreamEvent::ToolUseStart {
                                        ..
                                    }
                                    | StreamEvent::ToolInputDelta(
                                        _
                                    )
                                    | StreamEvent::ToolUseEnd
                                    | StreamEvent::ToolResult {
                                        ..
                                    }
                                    | StreamEvent::MessageEnd {
                                        ..
                                    }
                                    | StreamEvent::ThinkingStart
                                    | StreamEvent::ThinkingDelta(
                                        _
                                    )
                                    | StreamEvent::ThinkingEnd
                                    | StreamEvent::ThinkingDone {
                                        ..
                                    }
                            ) {
                                saw_output = true;
                            }

                            if tx.send(Ok(event)).await.is_err()
                            {
                                terminate_child(&mut child)
                                    .await;
                                return Ok(());
                            }
                        }
                    }
                    Err(err) => {
                        let event = StreamEvent::Error {
                            message: format!(
                                "Failed to parse ForgeCode CLI \
                                 output: {}",
                                err
                            ),
                            retry_after_secs: None,
                        };
                        if tx.send(Ok(event)).await.is_err() {
                            terminate_child(&mut child).await;
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    let status = child.wait().await?;
    if !status.success() {
        let event = StreamEvent::Error {
            message: format!(
                "ForgeCode CLI exited with status {}",
                status
            ),
            retry_after_secs: None,
        };
        let _ = tx.send(Ok(event)).await;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn is_retryable_error(error_str: &str) -> bool {
    jcode_provider_core::is_transient_transport_error(error_str)
        || error_str.contains("not ready for writing")
        || error_str.contains("502 bad gateway")
        || error_str.contains("503 service unavailable")
        || error_str.contains("504 gateway timeout")
        || error_str.contains("overloaded")
}

fn to_forgecode_tool_name(name: &str) -> String {
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
        "open" => "Open",
        "codesearch" => "CodeSearch",
        "invalid" => "Invalid",
        "skill" => "Skill",
        "skill_manage" => "SkillManage",
        "conversation_search" => "ConversationSearch",
        "lsp" => "Lsp",
        "task" | "subagent" => "Task",
        "todo" | "todowrite" | "todoread" => "Todo",
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
        "Open" => "open",
        "Launch" => "open",
        "CodeSearch" => "codesearch",
        "Invalid" => "invalid",
        "Skill" => "skill",
        "SkillManage" => "skill_manage",
        "ConversationSearch" => "conversation_search",
        "Lsp" => "lsp",
        "Task" => "subagent",
        "Todo" | "TodoWrite" | "TodoRead" => "todo",
        "Batch" => "batch",
        _ => name,
    }
    .to_string()
}
