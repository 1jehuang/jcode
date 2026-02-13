//! Direct Anthropic API provider
//!
//! Uses the Anthropic Messages API directly without the Python SDK.
//! This provides better control and eliminates the Python dependency.

use super::{EventStream, NativeToolResultSender, Provider};
use crate::auth;
use crate::auth::oauth;
use crate::message::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tokio_stream::wrappers::ReceiverStream;

/// Anthropic Messages API endpoint
const API_URL: &str = "https://api.anthropic.com/v1/messages";

/// OAuth endpoint (with beta=true query param)
const API_URL_OAUTH: &str = "https://api.anthropic.com/v1/messages?beta=true";

/// User-Agent for OAuth requests (must match Claude CLI format)
const CLAUDE_CLI_USER_AGENT: &str = "claude-cli/1.0.0";

/// Beta headers required for OAuth
const OAUTH_BETA_HEADERS: &str = "oauth-2025-04-20,claude-code-20250219,prompt-caching-2024-07-31";

/// Default model
const DEFAULT_MODEL: &str = "claude-opus-4-6";

/// API version header
const API_VERSION: &str = "2023-06-01";

/// Claude Code identity block required for OAuth direct API access
const CLAUDE_CODE_IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
const CLAUDE_CODE_JCODE_NOTICE: &str =
    "You are jcode, powered by Claude Code. You are a third-party CLI, not the official Claude Code CLI.";

fn map_tool_name_for_oauth(name: &str) -> String {
    match name {
        "bash" => "shell_exec",
        "read" => "file_read",
        "write" => "file_write",
        "edit" => "file_edit",
        "glob" => "file_glob",
        "grep" => "file_grep",
        "task" | "subagent" => "task_runner",
        "todoread" => "todo_read",
        "todowrite" => "todo_write",
        _ => name,
    }
    .to_string()
}

fn map_tool_name_from_oauth(name: &str) -> String {
    match name {
        "shell_exec" => "bash",
        "file_read" => "read",
        "file_write" => "write",
        "file_edit" => "edit",
        "file_glob" => "glob",
        "file_grep" => "grep",
        "task_runner" => "subagent",
        "todo_read" => "todoread",
        "todo_write" => "todowrite",
        _ => name,
    }
    .to_string()
}

/// Maximum number of retries for transient errors
const MAX_RETRIES: u32 = 3;

/// Base delay for exponential backoff (in milliseconds)
const RETRY_BASE_DELAY_MS: u64 = 1000;

/// Available models
pub const AVAILABLE_MODELS: &[&str] = &[
    "claude-opus-4-6",
    "claude-opus-4-5-20251101",
    "claude-sonnet-4-20250514",
    "claude-haiku-4-5-20241022",
];

/// Cached OAuth credentials
#[derive(Clone)]
struct CachedCredentials {
    access_token: String,
    refresh_token: String,
    expires_at: i64,
}

/// Direct Anthropic API provider
pub struct AnthropicProvider {
    client: Client,
    model: Arc<std::sync::RwLock<String>>,
    /// Cached OAuth credentials (None if using API key)
    credentials: Arc<RwLock<Option<CachedCredentials>>>,
}

impl AnthropicProvider {
    pub fn new() -> Self {
        let model =
            std::env::var("JCODE_ANTHROPIC_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());

        Self {
            client: Client::new(),
            model: Arc::new(std::sync::RwLock::new(model)),
            credentials: Arc::new(RwLock::new(None)),
        }
    }

    /// Get the access token from credentials
    /// Supports both OAuth tokens and direct API keys
    /// Automatically refreshes OAuth tokens when expired
    async fn get_access_token(&self) -> Result<(String, bool)> {
        // First check for direct API key in environment
        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            return Ok((key, false)); // false = not OAuth
        }

        // Check cached credentials
        {
            let cached = self.credentials.read().await;
            if let Some(ref creds) = *cached {
                let now = chrono::Utc::now().timestamp_millis();
                // Return cached token if not expired (with 5 min buffer)
                if creds.expires_at > now + 300_000 {
                    return Ok((creds.access_token.clone(), true));
                }
            }
        }

        // Load fresh credentials or refresh expired ones
        let fresh_creds =
            auth::claude::load_credentials().context("Failed to load Claude credentials")?;

        let now = chrono::Utc::now().timestamp_millis();

        // Check if token needs refresh (expired or expiring within 5 minutes)
        if fresh_creds.expires_at < now + 300_000 && !fresh_creds.refresh_token.is_empty() {
            crate::logging::info("OAuth token expired or expiring soon, attempting refresh...");

            match oauth::refresh_claude_tokens(&fresh_creds.refresh_token).await {
                Ok(refreshed) => {
                    crate::logging::info("OAuth token refreshed successfully");

                    // Cache the refreshed credentials
                    let mut cached = self.credentials.write().await;
                    *cached = Some(CachedCredentials {
                        access_token: refreshed.access_token.clone(),
                        refresh_token: refreshed.refresh_token,
                        expires_at: refreshed.expires_at,
                    });

                    return Ok((refreshed.access_token, true));
                }
                Err(e) => {
                    crate::logging::error(&format!("OAuth token refresh failed: {}", e));
                    // Fall through to try the possibly-expired token
                }
            }
        }

        // Cache and return the loaded credentials (even if expired, let the API reject it)
        let mut cached = self.credentials.write().await;
        *cached = Some(CachedCredentials {
            access_token: fresh_creds.access_token.clone(),
            refresh_token: fresh_creds.refresh_token,
            expires_at: fresh_creds.expires_at,
        });

        Ok((fresh_creds.access_token, true))
    }

    /// Convert our Message type to Anthropic API format
    /// Also repairs dangling tool_uses by injecting synthetic tool_results
    fn format_messages(&self, messages: &[Message], is_oauth: bool) -> Vec<ApiMessage> {
        use std::collections::HashSet;

        // First pass: collect all tool_use IDs and tool_result IDs
        let mut tool_use_ids: HashSet<String> = HashSet::new();
        let mut tool_result_ids: HashSet<String> = HashSet::new();

        for msg in messages {
            for block in &msg.content {
                match block {
                    ContentBlock::ToolUse { id, .. } => {
                        tool_use_ids.insert(id.clone());
                    }
                    ContentBlock::ToolResult { tool_use_id, .. } => {
                        tool_result_ids.insert(tool_use_id.clone());
                    }
                    _ => {}
                }
            }
        }

        // Find dangling tool_uses (no matching tool_result)
        let dangling: HashSet<_> = tool_use_ids.difference(&tool_result_ids).cloned().collect();
        if !dangling.is_empty() {
            crate::logging::info(&format!(
                "[anthropic] Repairing {} dangling tool_use(s) by injecting synthetic tool_results",
                dangling.len()
            ));
        }

        // Second pass: build messages, injecting synthetic tool_results after assistant messages
        // that have dangling tool_uses
        let mut result: Vec<ApiMessage> = Vec::new();

        for msg in messages {
            let role = match msg.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            };

            let content = self.format_content_blocks(&msg.content, is_oauth);

            if !content.is_empty() {
                result.push(ApiMessage {
                    role: role.to_string(),
                    content,
                });
            }

            // If this is an assistant message with dangling tool_uses, inject synthetic results
            if matches!(msg.role, Role::Assistant) {
                let mut synthetic_results: Vec<ApiContentBlock> = Vec::new();
                for block in &msg.content {
                    if let ContentBlock::ToolUse { id, .. } = block {
                        if dangling.contains(id) {
                            synthetic_results.push(ApiContentBlock::ToolResult {
                                tool_use_id: id.clone(),
                                content: "[Session interrupted before tool execution completed]"
                                    .to_string(),
                                is_error: true,
                            });
                        }
                    }
                }
                if !synthetic_results.is_empty() {
                    result.push(ApiMessage {
                        role: "user".to_string(),
                        content: synthetic_results,
                    });
                }
            }
        }

        // Third pass: merge consecutive messages of the same role
        // Anthropic API requires strictly alternating user/assistant messages
        let pre_merge_count = result.len();
        let mut merged: Vec<ApiMessage> = Vec::new();
        for msg in result {
            if let Some(last) = merged.last_mut() {
                if last.role == msg.role {
                    // Same role - merge content blocks
                    last.content.extend(msg.content);
                    continue;
                }
            }
            merged.push(msg);
        }

        if merged.len() != pre_merge_count {
            crate::logging::info(&format!(
                "[anthropic] Merged {} consecutive same-role messages",
                pre_merge_count - merged.len()
            ));
        }

        // Validate: check each assistant message with tool_use has matching tool_result in next user message
        for (i, msg) in merged.iter().enumerate() {
            if msg.role == "assistant" {
                let tool_uses: Vec<&String> = msg
                    .content
                    .iter()
                    .filter_map(|b| {
                        if let ApiContentBlock::ToolUse { id, .. } = b {
                            Some(id)
                        } else {
                            None
                        }
                    })
                    .collect();

                if !tool_uses.is_empty() {
                    // Check next message
                    if let Some(next) = merged.get(i + 1) {
                        if next.role != "user" {
                            crate::logging::warn(&format!(
                                "[anthropic] Message {} has tool_use but next message is {} (should be user)",
                                i, next.role
                            ));
                        } else {
                            let tool_results: std::collections::HashSet<&String> = next
                                .content
                                .iter()
                                .filter_map(|b| {
                                    if let ApiContentBlock::ToolResult { tool_use_id, .. } = b {
                                        Some(tool_use_id)
                                    } else {
                                        None
                                    }
                                })
                                .collect();

                            for tu_id in &tool_uses {
                                if !tool_results.contains(*tu_id) {
                                    crate::logging::warn(&format!(
                                        "[anthropic] Message {} has tool_use {} but no matching tool_result in message {}",
                                        i, tu_id, i + 1
                                    ));
                                }
                            }
                        }
                    } else {
                        crate::logging::warn(&format!(
                            "[anthropic] Message {} has tool_use but no next message",
                            i
                        ));
                    }
                }
            }
        }

        merged
    }

    /// Convert our ContentBlock to Anthropic API format
    fn format_content_blocks(
        &self,
        blocks: &[ContentBlock],
        is_oauth: bool,
    ) -> Vec<ApiContentBlock> {
        blocks
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(ApiContentBlock::Text {
                    text: text.clone(),
                    cache_control: None,
                }),
                ContentBlock::ToolUse { id, name, input } => Some(ApiContentBlock::ToolUse {
                    id: id.clone(),
                    name: if is_oauth {
                        map_tool_name_for_oauth(name)
                    } else {
                        name.clone()
                    },
                    // Anthropic API requires input to be an object, not null
                    input: if input.is_null() {
                        serde_json::json!({})
                    } else {
                        input.clone()
                    },
                    cache_control: None,
                }),
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => Some(ApiContentBlock::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: content.clone(),
                    is_error: is_error.unwrap_or(false),
                }),
                _ => None, // Skip other block types (thinking, etc.)
            })
            .collect()
    }

    /// Convert tool definitions to Anthropic API format
    /// Adds cache_control to the last tool for prompt caching
    fn format_tools(&self, tools: &[ToolDefinition], is_oauth: bool) -> Vec<ApiTool> {
        let len = tools.len();
        tools
            .iter()
            .enumerate()
            .map(|(i, tool)| ApiTool {
                name: if is_oauth {
                    map_tool_name_for_oauth(&tool.name)
                } else {
                    tool.name.clone()
                },
                description: tool.description.clone(),
                input_schema: tool.input_schema.clone(),
                // Add cache_control to the last tool to cache all tool definitions
                cache_control: if i == len - 1 {
                    Some(CacheControlParam::ephemeral())
                } else {
                    None
                },
            })
            .collect()
    }
}

impl Default for AnthropicProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let (token, is_oauth) = self.get_access_token().await?;
        let model = self.model.read().unwrap().clone();

        // Format request
        let api_messages = self.format_messages(messages, is_oauth);
        let api_tools = self.format_tools(tools, is_oauth);

        let request = ApiRequest {
            model: model.clone(),
            max_tokens: 16384,
            system: build_system_param(system, is_oauth),
            messages: format_messages_with_identity(api_messages, is_oauth),
            tools: if api_tools.is_empty() {
                None
            } else {
                Some(api_tools)
            },
            stream: true,
        };

        // Create channel for streaming events
        let (tx, rx) = mpsc::channel::<Result<StreamEvent>>(100);

        // Clone what we need for the async task
        let client = self.client.clone();
        let credentials = Arc::clone(&self.credentials);

        // Spawn task to handle streaming with retry logic.
        // This includes forced OAuth refresh on auth failures.
        tokio::spawn(async move {
            run_stream_with_retries(client, token, is_oauth, request, tx, credentials).await;
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn model(&self) -> String {
        self.model.read().unwrap().clone()
    }

    fn set_model(&self, model: &str) -> Result<()> {
        if !AVAILABLE_MODELS.contains(&model) {
            anyhow::bail!("Model {} not supported by Anthropic provider", model);
        }
        *self.model.write().unwrap() = model.to_string();
        Ok(())
    }

    fn available_models(&self) -> Vec<&'static str> {
        AVAILABLE_MODELS.to_vec()
    }

    fn name(&self) -> &'static str {
        "anthropic"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            client: self.client.clone(),
            model: Arc::new(std::sync::RwLock::new(self.model.read().unwrap().clone())),
            credentials: Arc::new(RwLock::new(None)), // Fresh credentials cache for fork
        })
    }

    fn native_result_sender(&self) -> Option<NativeToolResultSender> {
        None // Direct API doesn't use native tool bridge
    }

    /// Split system prompt completion for better cache efficiency
    /// Static content is cached, dynamic content is not
    async fn complete_split(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system_static: &str,
        system_dynamic: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let (token, is_oauth) = self.get_access_token().await?;
        let model = self.model.read().unwrap().clone();

        // Format request
        let api_messages = self.format_messages(messages, is_oauth);
        let api_tools = self.format_tools(tools, is_oauth);

        let request = ApiRequest {
            model: model.clone(),
            max_tokens: 16384,
            system: build_system_param_split(system_static, system_dynamic, is_oauth),
            messages: format_messages_with_identity(api_messages, is_oauth),
            tools: if api_tools.is_empty() {
                None
            } else {
                Some(api_tools)
            },
            stream: true,
        };

        // Create channel for streaming events
        let (tx, rx) = mpsc::channel::<Result<StreamEvent>>(100);

        // Clone what we need for the async task
        let client = self.client.clone();
        let credentials = Arc::clone(&self.credentials);

        // Spawn task to handle streaming with retry logic
        tokio::spawn(async move {
            run_stream_with_retries(client, token, is_oauth, request, tx, credentials).await;
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }
}

async fn run_stream_with_retries(
    client: Client,
    initial_token: String,
    is_oauth: bool,
    request: ApiRequest,
    tx: mpsc::Sender<Result<StreamEvent>>,
    credentials: Arc<RwLock<Option<CachedCredentials>>>,
) {
    let mut token = initial_token;
    let mut last_error = None;
    let mut attempted_forced_refresh = false;

    for attempt in 0..MAX_RETRIES {
        if attempt > 0 {
            // Exponential backoff: 1s, 2s, 4s
            let delay = RETRY_BASE_DELAY_MS * (1 << (attempt - 1));
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            crate::logging::info(&format!(
                "Retrying Anthropic API request (attempt {}/{})",
                attempt + 1,
                MAX_RETRIES
            ));
        }

        match stream_response(
            client.clone(),
            token.clone(),
            is_oauth,
            request.clone(),
            tx.clone(),
        )
        .await
        {
            Ok(()) => return, // Success
            Err(e) => {
                let error_str = e.to_string().to_lowercase();

                // OAuth auth failures: force refresh and retry once immediately.
                if is_oauth && is_oauth_auth_error(&error_str) && !attempted_forced_refresh {
                    attempted_forced_refresh = true;
                    crate::logging::info(
                        "Anthropic OAuth authentication failed, forcing token refresh...",
                    );
                    match force_refresh_oauth_token(Arc::clone(&credentials)).await {
                        Ok(refreshed_token) => {
                            crate::logging::info(
                                "Forced OAuth token refresh succeeded, retrying request.",
                            );
                            token = refreshed_token;
                            last_error = Some(e);
                            continue;
                        }
                        Err(refresh_err) => {
                            let _ = tx
                                .send(Err(anyhow::anyhow!(
                                    "{}\n\nAutomatic Claude OAuth refresh failed: {}\nRun `jcode login --provider claude` (preferred) or `claude`, then retry.",
                                    e,
                                    refresh_err
                                )))
                                .await;
                            return;
                        }
                    }
                }

                // Check if this is a transient/retryable error
                if is_retryable_error(&error_str) && attempt + 1 < MAX_RETRIES {
                    crate::logging::info(&format!("Transient error, will retry: {}", e));
                    last_error = Some(e);
                    continue;
                }

                // Non-retryable or final attempt
                if is_oauth && is_oauth_auth_error(&error_str) {
                    let _ = tx
                        .send(Err(anyhow::anyhow!(
                            "{}\n\nClaude OAuth authentication failed. Run `jcode login --provider claude` (preferred) or `claude`, then retry.",
                            e
                        )))
                        .await;
                } else {
                    let _ = tx.send(Err(e)).await;
                }
                return;
            }
        }
    }

    // All retries exhausted
    if let Some(e) = last_error {
        let _ = tx
            .send(Err(anyhow::anyhow!(
                "Failed after {} retries: {}",
                MAX_RETRIES,
                e
            )))
            .await;
    }
}

async fn force_refresh_oauth_token(
    credentials: Arc<RwLock<Option<CachedCredentials>>>,
) -> Result<String> {
    let refresh_from_cache = {
        let cached = credentials.read().await;
        cached
            .as_ref()
            .map(|c| c.refresh_token.clone())
            .filter(|t| !t.is_empty())
    };

    let refresh_token = if let Some(token) = refresh_from_cache {
        token
    } else {
        let loaded = auth::claude::load_credentials()
            .context("Failed to load Claude credentials for forced refresh")?;
        if loaded.refresh_token.is_empty() {
            anyhow::bail!("No refresh token available in Claude credentials");
        }
        loaded.refresh_token
    };

    let refreshed = oauth::refresh_claude_tokens(&refresh_token)
        .await
        .context("OAuth refresh endpoint rejected the refresh token")?;

    {
        let mut cached = credentials.write().await;
        *cached = Some(CachedCredentials {
            access_token: refreshed.access_token.clone(),
            refresh_token: refreshed.refresh_token,
            expires_at: refreshed.expires_at,
        });
    }

    Ok(refreshed.access_token)
}

/// Stream the response from Anthropic API
async fn stream_response(
    client: Client,
    token: String,
    is_oauth: bool,
    request: ApiRequest,
    tx: mpsc::Sender<Result<StreamEvent>>,
) -> Result<()> {
    if std::env::var("JCODE_ANTHROPIC_DEBUG")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        if let Ok(json) = serde_json::to_string_pretty(&request) {
            crate::logging::info(&format!("Anthropic request payload:\n{}", json));
        }
    }
    // Build request with appropriate auth headers
    let url = if is_oauth { API_URL_OAUTH } else { API_URL };

    let mut req = client
        .post(url)
        .header("anthropic-version", API_VERSION)
        .header("content-type", "application/json")
        .header("accept", "text/event-stream");

    if is_oauth {
        // OAuth tokens require:
        // 1. Bearer auth (NOT x-api-key)
        // 2. User-Agent matching Claude CLI
        // 3. Multiple beta headers
        // 4. ?beta=true query param (in URL above)
        req = req
            .header("Authorization", format!("Bearer {}", token))
            .header("User-Agent", CLAUDE_CLI_USER_AGENT)
            .header("anthropic-beta", OAUTH_BETA_HEADERS);
    } else {
        // Direct API keys use x-api-key
        // Include prompt-caching beta header for cache_control support
        req = req
            .header("x-api-key", &token)
            .header("anthropic-beta", "prompt-caching-2024-07-31");
    }

    let response = req
        .json(&request)
        .send()
        .await
        .context("Failed to send request to Anthropic API")?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        anyhow::bail!("Anthropic API error ({}): {}", status, error_text);
    }

    // Parse SSE stream
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut current_tool_use: Option<ToolUseAccumulator> = None;
    let mut input_tokens: Option<u64> = None;
    let mut output_tokens: Option<u64> = None;
    let mut cache_read_input_tokens: Option<u64> = None;
    let mut cache_creation_input_tokens: Option<u64> = None;

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result.context("Error reading stream chunk")?;
        let chunk_str = String::from_utf8_lossy(&chunk);
        buffer.push_str(&chunk_str);

        // Process complete SSE events
        while let Some(event) = parse_sse_event(&mut buffer) {
            let events = process_sse_event(
                &event,
                &mut current_tool_use,
                &mut input_tokens,
                &mut output_tokens,
                &mut cache_read_input_tokens,
                &mut cache_creation_input_tokens,
                is_oauth,
            );
            for stream_event in events {
                if tx.send(Ok(stream_event)).await.is_err() {
                    return Ok(()); // Receiver dropped
                }
            }
        }
    }

    // Send final token usage if we have it
    if input_tokens.is_some() || output_tokens.is_some() {
        // Log cache usage for debugging
        if cache_read_input_tokens.is_some() || cache_creation_input_tokens.is_some() {
            crate::logging::info(&format!(
                "Prompt cache: read={:?} created={:?}",
                cache_read_input_tokens, cache_creation_input_tokens
            ));
        }
        let _ = tx
            .send(Ok(StreamEvent::TokenUsage {
                input_tokens,
                output_tokens,
                cache_read_input_tokens,
                cache_creation_input_tokens,
            }))
            .await;
    }

    Ok(())
}

/// Check if an error is transient and should be retried
fn is_retryable_error(error_str: &str) -> bool {
    // Network/connection errors
    error_str.contains("connection reset")
        || error_str.contains("connection closed")
        || error_str.contains("connection refused")
        || error_str.contains("broken pipe")
        || error_str.contains("timed out")
        || error_str.contains("timeout")
        // Stream/decode errors
        || error_str.contains("error decoding")
        || error_str.contains("error reading")
        || error_str.contains("unexpected eof")
        || error_str.contains("incomplete message")
        // Server errors (5xx)
        || error_str.contains("502 bad gateway")
        || error_str.contains("503 service unavailable")
        || error_str.contains("504 gateway timeout")
        || error_str.contains("overloaded")
}

fn is_oauth_auth_error(error_str: &str) -> bool {
    error_str.contains("oauth token has expired")
        || error_str.contains("token has expired")
        || error_str.contains("authentication_error")
        || error_str.contains("invalid token")
        || error_str.contains("invalid_grant")
        || ((error_str.contains("401 unauthorized") || error_str.contains("403 forbidden"))
            && (error_str.contains("oauth") || error_str.contains("token")))
}

/// Accumulator for tool_use blocks (input comes in chunks)
struct ToolUseAccumulator {
    id: String,
    name: String,
    input_json: String,
}

/// Parse a single SSE event from the buffer
fn parse_sse_event(buffer: &mut String) -> Option<SseEvent> {
    // Look for complete event (ends with double newline)
    let event_end = buffer.find("\n\n")?;
    let event_str = buffer[..event_end].to_string();
    buffer.drain(..event_end + 2);

    let mut event_type = String::new();
    let mut data = String::new();

    for line in event_str.lines() {
        if let Some(rest) = line.strip_prefix("event: ") {
            event_type = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("data: ") {
            data = rest.to_string();
        }
    }

    if event_type.is_empty() && data.is_empty() {
        return None;
    }

    Some(SseEvent { event_type, data })
}

/// SSE event from the stream
struct SseEvent {
    event_type: String,
    data: String,
}

/// Process an SSE event and return StreamEvents if applicable
fn process_sse_event(
    event: &SseEvent,
    current_tool_use: &mut Option<ToolUseAccumulator>,
    input_tokens: &mut Option<u64>,
    output_tokens: &mut Option<u64>,
    cache_read_input_tokens: &mut Option<u64>,
    cache_creation_input_tokens: &mut Option<u64>,
    is_oauth: bool,
) -> Vec<StreamEvent> {
    let mut events = Vec::new();

    match event.event_type.as_str() {
        "message_start" => {
            // Extract usage from message_start (includes cache info)
            if let Ok(parsed) = serde_json::from_str::<MessageStartEvent>(&event.data) {
                if let Some(usage) = parsed.message.usage {
                    *input_tokens = usage.input_tokens.map(|t| t as u64);
                    *cache_read_input_tokens = usage.cache_read_input_tokens.map(|t| t as u64);
                    *cache_creation_input_tokens =
                        usage.cache_creation_input_tokens.map(|t| t as u64);
                }
            }
        }
        "content_block_start" => {
            if let Ok(parsed) = serde_json::from_str::<ContentBlockStartEvent>(&event.data) {
                match parsed.content_block {
                    ApiContentBlockStart::Text { .. } => {
                        // Text block starting - nothing to emit yet
                    }
                    ApiContentBlockStart::ToolUse { id, name } => {
                        let mapped_name = if is_oauth {
                            map_tool_name_from_oauth(&name)
                        } else {
                            name.clone()
                        };
                        // Start accumulating tool use
                        *current_tool_use = Some(ToolUseAccumulator {
                            id: id.clone(),
                            name: mapped_name.clone(),
                            input_json: String::new(),
                        });
                        events.push(StreamEvent::ToolUseStart {
                            id,
                            name: mapped_name,
                        });
                    }
                }
            }
        }
        "content_block_delta" => {
            if let Ok(parsed) = serde_json::from_str::<ContentBlockDeltaEvent>(&event.data) {
                match parsed.delta {
                    ApiDelta::TextDelta { text } => {
                        events.push(StreamEvent::TextDelta(text));
                    }
                    ApiDelta::InputJsonDelta { partial_json } => {
                        if let Some(ref mut tool) = current_tool_use {
                            tool.input_json.push_str(&partial_json);
                        }
                        events.push(StreamEvent::ToolInputDelta(partial_json));
                    }
                }
            }
        }
        "content_block_stop" => {
            // If we were accumulating a tool_use, it's complete now
            if current_tool_use.take().is_some() {
                events.push(StreamEvent::ToolUseEnd);
            }
        }
        "message_delta" => {
            if let Ok(parsed) = serde_json::from_str::<MessageDeltaEvent>(&event.data) {
                if let Some(usage) = parsed.usage {
                    *output_tokens = usage.output_tokens.map(|t| t as u64);
                }
                if let Some(stop_reason) = parsed.delta.stop_reason {
                    events.push(StreamEvent::MessageEnd {
                        stop_reason: Some(stop_reason),
                    });
                }
            }
        }
        "message_stop" => {
            // Final message stop - we may have already sent MessageEnd via message_delta
        }
        "ping" => {
            // Keepalive, ignore
        }
        "error" => {
            crate::logging::error(&format!("Anthropic stream error: {}", event.data));
            events.push(StreamEvent::Error {
                message: event.data.clone(),
                retry_after_secs: None,
            });
        }
        _ => {
            // Unknown event type, ignore
        }
    }

    events
}

// ============================================================================
// API Types
// ============================================================================

#[derive(Serialize, Clone)]
struct ApiRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<ApiSystem>,
    messages: Vec<ApiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ApiTool>>,
    stream: bool,
}

#[derive(Serialize, Clone)]
#[serde(untagged)]
enum ApiSystem {
    Text(String),
    Blocks(Vec<ApiSystemBlock>),
}

/// Cache control for prompt caching
#[derive(Serialize, Clone)]
struct CacheControlParam {
    #[serde(rename = "type")]
    kind: &'static str,
}

impl CacheControlParam {
    fn ephemeral() -> Self {
        Self { kind: "ephemeral" }
    }
}

#[derive(Serialize, Clone)]
struct ApiSystemBlock {
    #[serde(rename = "type")]
    block_type: &'static str,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControlParam>,
}

fn build_system_param(system: &str, is_oauth: bool) -> Option<ApiSystem> {
    build_system_param_split(system, "", is_oauth)
}

/// Build system param with split static/dynamic content for better caching
fn build_system_param_split(
    static_part: &str,
    dynamic_part: &str,
    is_oauth: bool,
) -> Option<ApiSystem> {
    if is_oauth {
        let mut blocks = Vec::new();
        blocks.push(ApiSystemBlock {
            block_type: "text",
            text: CLAUDE_CODE_IDENTITY.to_string(),
            cache_control: None,
        });
        blocks.push(ApiSystemBlock {
            block_type: "text",
            text: CLAUDE_CODE_JCODE_NOTICE.to_string(),
            cache_control: None,
        });
        // Static content - CACHED (CLAUDE.md, base prompt, skills)
        if !static_part.is_empty() {
            blocks.push(ApiSystemBlock {
                block_type: "text",
                text: static_part.to_string(),
                cache_control: Some(CacheControlParam::ephemeral()),
            });
        }
        // Dynamic content - NOT cached (date, git status, memory)
        if !dynamic_part.is_empty() {
            blocks.push(ApiSystemBlock {
                block_type: "text",
                text: dynamic_part.to_string(),
                cache_control: None,
            });
        }
        return Some(ApiSystem::Blocks(blocks));
    }

    // Non-OAuth: use block format with cache control for static part only
    let has_static = !static_part.is_empty();
    let has_dynamic = !dynamic_part.is_empty();

    if !has_static && !has_dynamic {
        None
    } else {
        let mut blocks = Vec::new();
        if has_static {
            blocks.push(ApiSystemBlock {
                block_type: "text",
                text: static_part.to_string(),
                cache_control: Some(CacheControlParam::ephemeral()),
            });
        }
        if has_dynamic {
            blocks.push(ApiSystemBlock {
                block_type: "text",
                text: dynamic_part.to_string(),
                cache_control: None,
            });
        }
        Some(ApiSystem::Blocks(blocks))
    }
}

fn format_messages_with_identity(messages: Vec<ApiMessage>, is_oauth: bool) -> Vec<ApiMessage> {
    if !is_oauth {
        return messages;
    }

    let mut out = Vec::with_capacity(messages.len() + 1);
    out.push(ApiMessage {
        role: "user".to_string(),
        content: vec![ApiContentBlock::Text {
            text: CLAUDE_CODE_IDENTITY.to_string(),
            cache_control: None,
        }],
    });
    out.extend(messages);

    // Add cache breakpoint to enable conversation caching
    add_message_cache_breakpoint(&mut out);

    out
}

/// Add cache_control to messages for conversation caching.
/// Strategy: Cache everything except the last user message.
/// This way, on subsequent turns, the entire conversation history up to
/// the previous assistant response is cached.
fn add_message_cache_breakpoint(messages: &mut [ApiMessage]) {
    crate::logging::info(&format!(
        "Conversation caching: {} messages to process",
        messages.len()
    ));

    if messages.len() < 3 {
        // Need at least: identity + user + something to cache
        crate::logging::info("Conversation caching: too few messages, skipping");
        return;
    }

    // Find the last assistant message (second-to-last message if last is user)
    // We want to cache up to and including the last complete exchange
    let mut cache_index = None;

    // Walk backwards to find the last assistant message before the final user message
    for (i, msg) in messages.iter().enumerate().rev() {
        if msg.role == "assistant" {
            cache_index = Some(i);
            break;
        }
    }

    // Add cache_control to the last content block of that message
    if let Some(idx) = cache_index {
        if let Some(msg) = messages.get_mut(idx) {
            // Find any Text or ToolUse block to add cache_control to (prefer last, but accept any)
            let mut added_cache = false;
            for block in msg.content.iter_mut().rev() {
                match block {
                    ApiContentBlock::Text { cache_control, .. }
                    | ApiContentBlock::ToolUse { cache_control, .. } => {
                        *cache_control = Some(CacheControlParam::ephemeral());
                        added_cache = true;
                        break;
                    }
                    _ => {}
                }
            }
            if added_cache {
                crate::logging::info(&format!(
                    "Conversation caching: added cache breakpoint at message {}",
                    idx
                ));
            } else {
                crate::logging::info(&format!(
                    "Conversation caching: no text block found in assistant message {}",
                    idx
                ));
            }
        }
    } else {
        crate::logging::info("Conversation caching: no assistant message found");
    }
}

#[derive(Serialize, Clone)]
struct ApiMessage {
    role: String,
    content: Vec<ApiContentBlock>,
}

#[derive(Serialize, Clone)]
#[serde(tag = "type")]
enum ApiContentBlock {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControlParam>,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControlParam>,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
}

#[derive(Serialize, Clone)]
struct ApiTool {
    name: String,
    description: String,
    input_schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControlParam>,
}

// Response types for SSE parsing

#[derive(Deserialize)]
struct MessageStartEvent {
    message: MessageStartMessage,
}

#[derive(Deserialize)]
struct MessageStartMessage {
    usage: Option<UsageInfo>,
}

#[derive(Deserialize)]
struct ContentBlockStartEvent {
    #[allow(dead_code)]
    index: u32,
    content_block: ApiContentBlockStart,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ApiContentBlockStart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
}

#[derive(Deserialize)]
struct ContentBlockDeltaEvent {
    #[allow(dead_code)]
    index: u32,
    delta: ApiDelta,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ApiDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
}

#[derive(Deserialize)]
struct MessageDeltaEvent {
    delta: MessageDeltaDelta,
    usage: Option<UsageInfo>,
}

#[derive(Deserialize)]
struct MessageDeltaDelta {
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct UsageInfo {
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
    cache_read_input_tokens: Option<u32>,
    cache_creation_input_tokens: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sse_event() {
        let mut buffer = "event: message_start\ndata: {\"type\":\"message_start\"}\n\n".to_string();
        let event = parse_sse_event(&mut buffer).unwrap();
        assert_eq!(event.event_type, "message_start");
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_available_models() {
        let provider = AnthropicProvider::new();
        let models = provider.available_models();
        assert!(models.contains(&"claude-opus-4-5-20251101"));
    }

    #[test]
    fn test_dangling_tool_use_repair() {
        let provider = AnthropicProvider::new();

        // Create messages with a dangling tool_use (no corresponding tool_result)
        let messages = vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "Hello".to_string(),
                    cache_control: None,
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Text {
                        text: "Let me check".to_string(),
                        cache_control: None,
                    },
                    ContentBlock::ToolUse {
                        id: "tool_123".to_string(),
                        name: "bash".to_string(),
                        input: serde_json::json!({"command": "ls"}),
                    },
                    ContentBlock::ToolUse {
                        id: "tool_456".to_string(),
                        name: "read".to_string(),
                        input: serde_json::json!({"file_path": "/tmp/test"}),
                    },
                ],
            },
            // Missing tool_results for tool_123 and tool_456!
        ];

        let formatted = provider.format_messages(&messages, false);

        // Should have 3 messages:
        // 1. User: "Hello"
        // 2. Assistant: text + tool_uses
        // 3. User: synthetic tool_results for the dangling tool_uses
        assert_eq!(formatted.len(), 3);

        // Check the synthetic tool_result message
        let synthetic_msg = &formatted[2];
        assert_eq!(synthetic_msg.role, "user");
        assert_eq!(synthetic_msg.content.len(), 2);

        // Verify both tool_results are present
        let mut found_ids = std::collections::HashSet::new();
        for block in &synthetic_msg.content {
            if let ApiContentBlock::ToolResult {
                tool_use_id,
                is_error,
                content,
            } = block
            {
                found_ids.insert(tool_use_id.clone());
                assert!(is_error);
                assert!(content.contains("interrupted"));
            } else {
                panic!("Expected ToolResult block");
            }
        }
        assert!(found_ids.contains("tool_123"));
        assert!(found_ids.contains("tool_456"));
    }

    #[test]
    fn test_no_repair_when_tool_results_present() {
        let provider = AnthropicProvider::new();

        // Create messages where tool_use has a corresponding tool_result
        let messages = vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "Hello".to_string(),
                    cache_control: None,
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "tool_123".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "ls"}),
                }],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tool_123".to_string(),
                    content: "file1.txt\nfile2.txt".to_string(),
                    is_error: Some(false),
                }],
            },
        ];

        let formatted = provider.format_messages(&messages, false);

        // Should have exactly 3 messages (no synthetic ones added)
        assert_eq!(formatted.len(), 3);

        // The last message should be the actual tool_result, not synthetic
        let last_msg = &formatted[2];
        if let ApiContentBlock::ToolResult { content, .. } = &last_msg.content[0] {
            assert!(content.contains("file1.txt"));
        } else {
            panic!("Expected ToolResult block");
        }
    }

    #[test]
    fn test_cache_breakpoint_no_messages() {
        let mut messages: Vec<ApiMessage> = vec![];
        add_message_cache_breakpoint(&mut messages);
        // Should not panic, just return early
        assert!(messages.is_empty());
    }

    #[test]
    fn test_cache_breakpoint_too_few_messages() {
        let mut messages = vec![
            ApiMessage {
                role: "user".to_string(),
                content: vec![ApiContentBlock::Text {
                    text: "Hello".to_string(),
                    cache_control: None,
                }],
            },
            ApiMessage {
                role: "user".to_string(),
                content: vec![ApiContentBlock::Text {
                    text: "World".to_string(),
                    cache_control: None,
                }],
            },
        ];
        add_message_cache_breakpoint(&mut messages);
        // With only 2 messages, should not add cache control
        for msg in &messages {
            for block in &msg.content {
                if let ApiContentBlock::Text { cache_control, .. } = block {
                    assert!(cache_control.is_none());
                }
            }
        }
    }

    #[test]
    fn test_cache_breakpoint_adds_to_assistant_message() {
        let mut messages = vec![
            ApiMessage {
                role: "user".to_string(),
                content: vec![ApiContentBlock::Text {
                    text: "Identity".to_string(),
                    cache_control: None,
                }],
            },
            ApiMessage {
                role: "user".to_string(),
                content: vec![ApiContentBlock::Text {
                    text: "Hello".to_string(),
                    cache_control: None,
                }],
            },
            ApiMessage {
                role: "assistant".to_string(),
                content: vec![ApiContentBlock::Text {
                    text: "Hi there!".to_string(),
                    cache_control: None,
                }],
            },
            ApiMessage {
                role: "user".to_string(),
                content: vec![ApiContentBlock::Text {
                    text: "How are you?".to_string(),
                    cache_control: None,
                }],
            },
        ];

        add_message_cache_breakpoint(&mut messages);

        // Assistant message (index 2) should have cache_control
        if let ApiContentBlock::Text { cache_control, .. } = &messages[2].content[0] {
            assert!(cache_control.is_some());
        } else {
            panic!("Expected Text block");
        }

        // Other messages should NOT have cache_control
        for (i, msg) in messages.iter().enumerate() {
            if i == 2 {
                continue; // Skip the assistant message we just checked
            }
            for block in &msg.content {
                if let ApiContentBlock::Text { cache_control, .. } = block {
                    assert!(
                        cache_control.is_none(),
                        "Message {} should not have cache_control",
                        i
                    );
                }
            }
        }
    }

    #[test]
    fn test_cache_breakpoint_finds_text_in_mixed_content() {
        // Assistant message with tool_use followed by text
        let mut messages = vec![
            ApiMessage {
                role: "user".to_string(),
                content: vec![ApiContentBlock::Text {
                    text: "Identity".to_string(),
                    cache_control: None,
                }],
            },
            ApiMessage {
                role: "user".to_string(),
                content: vec![ApiContentBlock::Text {
                    text: "Run a command".to_string(),
                    cache_control: None,
                }],
            },
            ApiMessage {
                role: "assistant".to_string(),
                content: vec![
                    ApiContentBlock::Text {
                        text: "Running command...".to_string(),
                        cache_control: None,
                    },
                    ApiContentBlock::ToolUse {
                        id: "tool_1".to_string(),
                        name: "bash".to_string(),
                        input: serde_json::json!({"command": "ls"}),
                        cache_control: None,
                    },
                ],
            },
            ApiMessage {
                role: "user".to_string(),
                content: vec![ApiContentBlock::Text {
                    text: "Thanks".to_string(),
                    cache_control: None,
                }],
            },
        ];

        add_message_cache_breakpoint(&mut messages);

        // The last block (ToolUse) in the assistant message should have cache_control
        // (we prefer the last block for maximum cache coverage)
        let assistant_msg = &messages[2];
        let has_cached_block = assistant_msg.content.iter().any(|block| {
            matches!(
                block,
                ApiContentBlock::ToolUse {
                    cache_control: Some(_),
                    ..
                }
            )
        });
        assert!(
            has_cached_block,
            "Should have added cache_control to last block (ToolUse) in assistant message"
        );
    }

    #[test]
    fn test_system_param_split_oauth() {
        let static_content = "This is static content";
        let dynamic_content = "This is dynamic content";

        let result = build_system_param_split(static_content, dynamic_content, true);

        if let Some(ApiSystem::Blocks(blocks)) = result {
            // Should have 4 blocks: identity, notice, static (cached), dynamic (not cached)
            assert_eq!(blocks.len(), 4);

            // Block 0: identity (no cache)
            assert!(blocks[0].cache_control.is_none());

            // Block 1: notice (no cache)
            assert!(blocks[1].cache_control.is_none());

            // Block 2: static (cached)
            assert!(blocks[2].cache_control.is_some());
            assert!(blocks[2].text.contains("static"));

            // Block 3: dynamic (not cached)
            assert!(blocks[3].cache_control.is_none());
            assert!(blocks[3].text.contains("dynamic"));
        } else {
            panic!("Expected Blocks variant");
        }
    }

    #[test]
    fn test_system_param_split_non_oauth() {
        let static_content = "This is static content";
        let dynamic_content = "This is dynamic content";

        let result = build_system_param_split(static_content, dynamic_content, false);

        if let Some(ApiSystem::Blocks(blocks)) = result {
            // Should have 2 blocks: static (cached), dynamic (not cached)
            assert_eq!(blocks.len(), 2);

            // Block 0: static (cached)
            assert!(blocks[0].cache_control.is_some());

            // Block 1: dynamic (not cached)
            assert!(blocks[1].cache_control.is_none());
        } else {
            panic!("Expected Blocks variant");
        }
    }
}
