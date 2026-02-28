use super::{EventStream, Provider};
use crate::auth::copilot as copilot_auth;
use crate::message::{
    ContentBlock, Message as ChatMessage, Role, StreamEvent, ToolDefinition,
    TOOL_OUTPUT_MISSING_TEXT,
};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

const COPILOT_API_VERSION: &str = "2025-04-01";

const DEFAULT_MODEL: &str = "claude-sonnet-4-6";

const FALLBACK_MODELS: &[&str] = &[
    "claude-sonnet-4",
    "claude-sonnet-4-6",
    "gpt-4o",
    "gpt-4.1",
    "gpt-5",
    "o3-mini",
    "o4-mini",
    "gemini-2.0-flash-001",
    "gemini-2.5-pro",
];

/// Context window sizes for Copilot models
fn copilot_context_window(model: &str) -> usize {
    match model {
        "claude-sonnet-4" | "claude-sonnet-4-6" | "claude-sonnet-4.6" => 128_000,
        "claude-opus-4-6" | "claude-opus-4.6" | "claude-opus-4.6-fast" => 200_000,
        "claude-opus-4.5" | "claude-opus-4-5" => 200_000,
        "claude-sonnet-4.5" | "claude-sonnet-4-5" => 200_000,
        "claude-haiku-4.5" | "claude-haiku-4-5" => 200_000,
        "gpt-4o" | "gpt-4o-mini" => 128_000,
        m if m.starts_with("gpt-4o") => 128_000,
        m if m.starts_with("gpt-4.1") => 128_000,
        m if m.starts_with("gpt-5") => 128_000,
        "o3-mini" | "o4-mini" => 128_000,
        m if m.starts_with("gemini-2.0-flash") => 1_000_000,
        m if m.starts_with("gemini-2.5") => 1_000_000,
        m if m.starts_with("gemini-3") => 1_000_000,
        _ => 128_000,
    }
}

/// Copilot API provider - uses GitHub Copilot's OpenAI-compatible API.
/// Authenticates via GitHub OAuth token, exchanges for Copilot bearer token,
/// and sends requests to api.githubcopilot.com.
pub struct CopilotApiProvider {
    client: reqwest::Client,
    model: Arc<RwLock<String>>,
    github_token: String,
    bearer_token: Arc<tokio::sync::RwLock<Option<copilot_auth::CopilotApiToken>>>,
    fetched_models: Arc<RwLock<Vec<String>>>,
    session_id: String,
    machine_id: String,
}

impl CopilotApiProvider {
    pub fn new() -> Result<Self> {
        let github_token = copilot_auth::load_github_token()?;
        let model =
            std::env::var("JCODE_COPILOT_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());

        Ok(Self {
            client: crate::provider::shared_http_client(),
            model: Arc::new(RwLock::new(model)),
            github_token,
            bearer_token: Arc::new(tokio::sync::RwLock::new(None)),
            fetched_models: Arc::new(RwLock::new(Vec::new())),
            session_id: Uuid::new_v4().to_string(),
            machine_id: Self::get_or_create_machine_id(),
        })
    }

    pub fn has_credentials() -> bool {
        copilot_auth::has_copilot_credentials()
    }

    pub fn new_with_token(github_token: String) -> Self {
        let model =
            std::env::var("JCODE_COPILOT_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());

        Self {
            client: crate::provider::shared_http_client(),
            model: Arc::new(RwLock::new(model)),
            github_token,
            bearer_token: Arc::new(tokio::sync::RwLock::new(None)),
            fetched_models: Arc::new(RwLock::new(Vec::new())),
            session_id: Uuid::new_v4().to_string(),
            machine_id: Self::get_or_create_machine_id(),
        }
    }

    fn get_or_create_machine_id() -> String {
        let machine_id_path = dirs::home_dir()
            .unwrap_or_default()
            .join(".jcode")
            .join("machine_id");
        if let Ok(id) = std::fs::read_to_string(&machine_id_path) {
            let id = id.trim().to_string();
            if !id.is_empty() {
                return id;
            }
        }
        let id = Uuid::new_v4().to_string().replace('-', "");
        let _ = std::fs::create_dir_all(machine_id_path.parent().unwrap_or(&machine_id_path));
        let _ = std::fs::write(&machine_id_path, &id);
        id
    }

    fn is_user_initiated(messages: &[ChatMessage]) -> bool {
        let Some(last_msg) = messages.last() else {
            return true;
        };
        if last_msg.role != Role::User {
            return true;
        }
        let has_tool_result = last_msg
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult { .. }));
        !has_tool_result
    }

    /// Detect the user's Copilot tier and set the best default model.
    /// Call this after construction. Fetches a bearer token and queries /models.
    /// If JCODE_COPILOT_MODEL is set, this is a no-op (user override).
    pub async fn detect_tier_and_set_default(&self) {
        if std::env::var("JCODE_COPILOT_MODEL").is_ok() {
            crate::logging::info(
                "Copilot model overridden via JCODE_COPILOT_MODEL, skipping tier detection",
            );
            return;
        }

        let bearer = match self.get_bearer_token().await {
            Ok(t) => t,
            Err(e) => {
                crate::logging::info(&format!(
                    "Copilot tier detection: failed to get bearer token: {}",
                    e
                ));
                return;
            }
        };

        match copilot_auth::fetch_available_models(&self.client, &bearer).await {
            Ok(models) => {
                let model_ids: Vec<String> = models.iter().map(|m| m.id.clone()).collect();
                let default = copilot_auth::choose_default_model(&models);
                crate::logging::info(&format!(
                    "Copilot tier detection: {} models available, default -> {}. Models: {}",
                    model_ids.len(),
                    default,
                    model_ids.join(", ")
                ));
                if let Ok(mut m) = self.model.try_write() {
                    *m = default;
                }
                if let Ok(mut fm) = self.fetched_models.try_write() {
                    *fm = model_ids;
                }
            }
            Err(e) => {
                crate::logging::info(&format!(
                    "Copilot tier detection: failed to fetch models: {}",
                    e
                ));
            }
        }
    }

    /// Get a valid Copilot bearer token, refreshing if expired
    async fn get_bearer_token(&self) -> Result<String> {
        {
            let guard = self.bearer_token.read().await;
            if let Some(ref token) = *guard {
                if !token.is_expired() {
                    return Ok(token.token.clone());
                }
            }
        }

        // Need to refresh
        let new_token =
            copilot_auth::exchange_github_token(&self.client, &self.github_token).await?;
        let token_str = new_token.token.clone();
        *self.bearer_token.write().await = Some(new_token);
        Ok(token_str)
    }

    /// Check if an error indicates token expiration
    fn is_auth_error(status: reqwest::StatusCode) -> bool {
        status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN
    }

    /// Build OpenAI-compatible messages array from our message format.
    ///
    /// Properly pairs tool_use blocks (in assistant messages) with their
    /// corresponding tool_result blocks (in user messages), handling
    /// out-of-order results and missing outputs.
    fn build_messages(system: &str, messages: &[ChatMessage]) -> Vec<Value> {
        use std::collections::{HashMap, HashSet};

        let mut result = Vec::new();
        let missing_output = format!("[Error] {}", TOOL_OUTPUT_MISSING_TEXT);

        if !system.is_empty() {
            result.push(json!({
                "role": "system",
                "content": system,
            }));
        }

        let mut tool_result_last_pos: HashMap<String, usize> = HashMap::new();
        for (idx, msg) in messages.iter().enumerate() {
            if let Role::User = msg.role {
                for block in &msg.content {
                    if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                        tool_result_last_pos.insert(tool_use_id.clone(), idx);
                    }
                }
            }
        }

        let mut tool_calls_seen: HashSet<String> = HashSet::new();
        let mut pending_tool_results: HashMap<String, String> = HashMap::new();
        let mut used_tool_results: HashSet<String> = HashSet::new();

        for (idx, msg) in messages.iter().enumerate() {
            match msg.role {
                Role::User => {
                    let mut text_parts: Vec<&str> = Vec::new();
                    for block in &msg.content {
                        match block {
                            ContentBlock::Text { text, .. } => {
                                text_parts.push(text.as_str());
                            }
                            ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                is_error,
                            } => {
                                if used_tool_results.contains(tool_use_id) {
                                    continue;
                                }
                                let output = if is_error == &Some(true) {
                                    format!("[Error] {}", content)
                                } else if content.is_empty() {
                                    TOOL_OUTPUT_MISSING_TEXT.to_string()
                                } else {
                                    content.clone()
                                };
                                if tool_calls_seen.contains(tool_use_id) {
                                    result.push(json!({
                                        "role": "tool",
                                        "tool_call_id": tool_use_id,
                                        "content": output,
                                    }));
                                    used_tool_results.insert(tool_use_id.clone());
                                } else if !pending_tool_results.contains_key(tool_use_id) {
                                    pending_tool_results.insert(tool_use_id.clone(), output);
                                }
                            }
                            _ => {}
                        }
                    }

                    let text = text_parts.join("\n");
                    if !text.is_empty() {
                        result.push(json!({
                            "role": "user",
                            "content": text,
                        }));
                    }
                }
                Role::Assistant => {
                    let mut content_text = String::new();
                    let mut tool_calls = Vec::new();
                    let mut post_tool_outputs: Vec<(String, String)> = Vec::new();
                    let mut missing_tool_outputs: Vec<String> = Vec::new();

                    for block in &msg.content {
                        match block {
                            ContentBlock::Text { text, .. } => {
                                content_text.push_str(text);
                            }
                            ContentBlock::ToolUse { id, name, input } => {
                                tool_calls.push(json!({
                                    "id": id,
                                    "type": "function",
                                    "function": {
                                        "name": name,
                                        "arguments": input.to_string(),
                                    }
                                }));
                                tool_calls_seen.insert(id.clone());
                                if let Some(output) = pending_tool_results.remove(id) {
                                    post_tool_outputs.push((id.clone(), output));
                                    used_tool_results.insert(id.clone());
                                } else {
                                    let has_future_output = tool_result_last_pos
                                        .get(id)
                                        .map(|pos| *pos > idx)
                                        .unwrap_or(false);
                                    if !has_future_output {
                                        missing_tool_outputs.push(id.clone());
                                        used_tool_results.insert(id.clone());
                                    }
                                }
                            }
                            _ => {}
                        }
                    }

                    let mut assistant_msg = json!({
                        "role": "assistant",
                    });

                    if !content_text.is_empty() {
                        assistant_msg["content"] = json!(content_text);
                    }
                    if !tool_calls.is_empty() {
                        assistant_msg["tool_calls"] = json!(tool_calls);
                    }

                    if !content_text.is_empty() || !tool_calls.is_empty() {
                        result.push(assistant_msg);

                        for (tool_call_id, output) in post_tool_outputs {
                            result.push(json!({
                                "role": "tool",
                                "tool_call_id": tool_call_id,
                                "content": output,
                            }));
                        }

                        for missing_id in missing_tool_outputs {
                            result.push(json!({
                                "role": "tool",
                                "tool_call_id": missing_id,
                                "content": missing_output.clone(),
                            }));
                        }
                    }
                }
            }
        }

        result
    }

    /// Build OpenAI-compatible tools array
    fn build_tools(tools: &[ToolDefinition]) -> Vec<Value> {
        tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema,
                    }
                })
            })
            .collect()
    }

    /// Send a streaming request to Copilot API
    async fn stream_request(
        &self,
        messages: Vec<Value>,
        tools: Vec<Value>,
        is_user_initiated: bool,
        tx: mpsc::Sender<Result<StreamEvent>>,
    ) {
        let model = self.model.read().unwrap().clone();
        let max_tokens: u32 = 16_384;
        let initiator = if is_user_initiated { "user" } else { "agent" };

        crate::logging::info(&format!(
            "Copilot request: X-Initiator={} model={}",
            initiator, model
        ));

        // Try up to 2 times (initial + one retry after token refresh)
        for attempt in 0..2 {
            let bearer_token = match self.get_bearer_token().await {
                Ok(t) => t,
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    return;
                }
            };

            let mut body = json!({
                "model": model,
                "messages": messages,
                "max_tokens": max_tokens,
                "stream": true,
            });

            if !tools.is_empty() {
                body["tools"] = json!(tools);
            }

            let request_id = Uuid::new_v4().to_string();

            let resp = self
                .client
                .post(format!(
                    "{}/chat/completions",
                    copilot_auth::COPILOT_API_BASE
                ))
                .header("Authorization", format!("Bearer {}", bearer_token))
                .header("Editor-Version", copilot_auth::EDITOR_VERSION)
                .header("Editor-Plugin-Version", copilot_auth::EDITOR_PLUGIN_VERSION)
                .header(
                    "Copilot-Integration-Id",
                    copilot_auth::COPILOT_INTEGRATION_ID,
                )
                .header("Content-Type", "application/json")
                .header("X-Initiator", initiator)
                .header("X-Request-Id", &request_id)
                .header("Openai-Intent", "conversation-panel")
                .header("Openai-Organization", "github-copilot")
                .header("X-GitHub-Api-Version", COPILOT_API_VERSION)
                .header("Vscode-Sessionid", &self.session_id)
                .header("Vscode-Machineid", &self.machine_id)
                .json(&body)
                .send()
                .await;

            let resp = match resp {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx
                        .send(Err(anyhow::anyhow!("Copilot API request failed: {}", e)))
                        .await;
                    return;
                }
            };

            let status = resp.status();

            // On auth error, invalidate token and retry
            if Self::is_auth_error(status) && attempt == 0 {
                *self.bearer_token.write().await = None;
                crate::logging::info("Copilot bearer token expired, refreshing...");
                continue;
            }

            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                let _ = tx
                    .send(Err(anyhow::anyhow!(
                        "Copilot API error (HTTP {}): {}",
                        status,
                        body
                    )))
                    .await;
                return;
            }

            // Send connection type event
            let _ = tx
                .send(Ok(StreamEvent::ConnectionType {
                    connection: format!("copilot-api ({})", model),
                }))
                .await;

            // Process SSE stream
            self.process_sse_stream(resp, tx).await;
            return;
        }
    }

    async fn process_sse_stream(
        &self,
        resp: reqwest::Response,
        tx: mpsc::Sender<Result<StreamEvent>>,
    ) {
        use futures::StreamExt;

        let mut stream = resp.bytes_stream();
        let mut buffer = String::new();
        let mut current_tool_id = String::new();
        let mut current_tool_name = String::new();
        let mut current_tool_args = String::new();
        let mut input_tokens: u64 = 0;
        let mut output_tokens: u64 = 0;

        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(Err(anyhow::anyhow!("Stream error: {}", e))).await;
                    return;
                }
            };

            buffer.push_str(&String::from_utf8_lossy(&chunk));

            // Process complete SSE lines
            while let Some(line_end) = buffer.find('\n') {
                let line = buffer[..line_end].trim_end_matches('\r').to_string();
                buffer = buffer[line_end + 1..].to_string();

                if line.is_empty() || line.starts_with(':') {
                    continue;
                }

                if let Some(data) = line.strip_prefix("data: ") {
                    if data.trim() == "[DONE]" {
                        // Send usage info before done
                        if input_tokens > 0 || output_tokens > 0 {
                            let _ = tx
                                .send(Ok(StreamEvent::TokenUsage {
                                    input_tokens: Some(input_tokens),
                                    output_tokens: Some(output_tokens),
                                    cache_creation_input_tokens: None,
                                    cache_read_input_tokens: None,
                                }))
                                .await;
                        }
                        let _ = tx
                            .send(Ok(StreamEvent::MessageEnd { stop_reason: None }))
                            .await;
                        return;
                    }

                    let parsed: Value = match serde_json::from_str(data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    // Extract usage if present
                    if let Some(usage) = parsed.get("usage") {
                        input_tokens = usage
                            .get("prompt_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        output_tokens = usage
                            .get("completion_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                    }

                    // Process choices
                    if let Some(choices) = parsed.get("choices").and_then(|c| c.as_array()) {
                        for choice in choices {
                            let delta = match choice.get("delta") {
                                Some(d) => d,
                                None => continue,
                            };

                            // Text content
                            if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                                if !content.is_empty() {
                                    let _ = tx
                                        .send(Ok(StreamEvent::TextDelta(content.to_string())))
                                        .await;
                                }
                            }

                            // Tool calls
                            if let Some(tool_calls) =
                                delta.get("tool_calls").and_then(|t| t.as_array())
                            {
                                for tc in tool_calls {
                                    // New tool call start
                                    if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                                        // Flush previous tool call if any
                                        if !current_tool_id.is_empty() {
                                            let _ = tx.send(Ok(StreamEvent::ToolUseEnd)).await;
                                        }
                                        current_tool_id = id.to_string();
                                        current_tool_name = tc
                                            .get("function")
                                            .and_then(|f| f.get("name"))
                                            .and_then(|n| n.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        current_tool_args.clear();

                                        let _ = tx
                                            .send(Ok(StreamEvent::ToolUseStart {
                                                id: current_tool_id.clone(),
                                                name: current_tool_name.clone(),
                                            }))
                                            .await;
                                    }

                                    // Accumulate arguments
                                    if let Some(args) = tc
                                        .get("function")
                                        .and_then(|f| f.get("arguments"))
                                        .and_then(|a| a.as_str())
                                    {
                                        current_tool_args.push_str(args);
                                        let _ = tx
                                            .send(Ok(StreamEvent::ToolInputDelta(args.to_string())))
                                            .await;
                                    }
                                }
                            }

                            // Finish reason
                            if let Some(finish) =
                                choice.get("finish_reason").and_then(|f| f.as_str())
                            {
                                // Flush last tool call
                                if !current_tool_id.is_empty() {
                                    let _ = tx.send(Ok(StreamEvent::ToolUseEnd)).await;
                                    current_tool_id.clear();
                                    current_tool_name.clear();
                                    current_tool_args.clear();
                                }

                                let stop_reason = match finish {
                                    "stop" => "end_turn",
                                    "tool_calls" => "tool_use",
                                    "length" => "max_tokens",
                                    other => other,
                                };
                                let _ = tx
                                    .send(Ok(StreamEvent::MessageEnd {
                                        stop_reason: Some(stop_reason.to_string()),
                                    }))
                                    .await;
                            }
                        }
                    }
                }
            }
        }

        // Stream ended without [DONE]
        let _ = tx
            .send(Ok(StreamEvent::MessageEnd { stop_reason: None }))
            .await;
    }
}

#[async_trait]
impl Provider for CopilotApiProvider {
    async fn complete(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
        system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let is_user_initiated = Self::is_user_initiated(messages);
        let built_messages = Self::build_messages(system, messages);
        let built_tools = Self::build_tools(tools);

        let (tx, rx) = mpsc::channel::<Result<StreamEvent>>(100);

        let provider = CopilotApiProvider {
            client: self.client.clone(),
            model: self.model.clone(),
            github_token: self.github_token.clone(),
            bearer_token: self.bearer_token.clone(),
            fetched_models: self.fetched_models.clone(),
            session_id: self.session_id.clone(),
            machine_id: self.machine_id.clone(),
        };

        tokio::spawn(async move {
            provider
                .stream_request(built_messages, built_tools, is_user_initiated, tx)
                .await;
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn name(&self) -> &str {
        "copilot"
    }

    fn model(&self) -> String {
        self.model
            .try_read()
            .map(|m| m.clone())
            .unwrap_or_else(|_| DEFAULT_MODEL.to_string())
    }

    fn set_model(&self, model: &str) -> Result<()> {
        let trimmed = model.trim();
        if trimmed.is_empty() {
            anyhow::bail!("Copilot model cannot be empty");
        }
        if let Ok(mut current) = self.model.try_write() {
            *current = trimmed.to_string();
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Cannot change model while a request is in progress"
            ))
        }
    }

    fn available_models(&self) -> Vec<&'static str> {
        FALLBACK_MODELS.to_vec()
    }

    fn available_models_display(&self) -> Vec<String> {
        if let Ok(models) = self.fetched_models.read() {
            if !models.is_empty() {
                return models.clone();
            }
        }
        FALLBACK_MODELS.iter().map(|m| (*m).to_string()).collect()
    }

    fn supports_compaction(&self) -> bool {
        true
    }

    fn context_window(&self) -> usize {
        copilot_context_window(&self.model())
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(CopilotApiProvider {
            client: self.client.clone(),
            model: Arc::new(RwLock::new(self.model())),
            github_token: self.github_token.clone(),
            bearer_token: self.bearer_token.clone(),
            fetched_models: self.fetched_models.clone(),
            session_id: self.session_id.clone(),
            machine_id: self.machine_id.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_provider(fetched: Vec<String>) -> CopilotApiProvider {
        CopilotApiProvider {
            client: reqwest::Client::new(),
            model: Arc::new(RwLock::new(DEFAULT_MODEL.to_string())),
            github_token: "test-token".to_string(),
            bearer_token: Arc::new(tokio::sync::RwLock::new(None)),
            fetched_models: Arc::new(RwLock::new(fetched)),
            session_id: "test-session".to_string(),
            machine_id: "test-machine".to_string(),
        }
    }

    #[test]
    fn available_models_display_returns_fetched_when_populated() {
        let fetched = vec![
            "claude-opus-4.6".to_string(),
            "claude-sonnet-4.6".to_string(),
            "gpt-5.3-codex".to_string(),
            "gemini-3-pro-preview".to_string(),
        ];
        let provider = make_test_provider(fetched.clone());
        let display = provider.available_models_display();
        assert_eq!(display, fetched);
    }

    #[test]
    fn available_models_display_returns_fallback_when_empty() {
        let provider = make_test_provider(Vec::new());
        let display = provider.available_models_display();
        let expected: Vec<String> = FALLBACK_MODELS.iter().map(|m| m.to_string()).collect();
        assert_eq!(display, expected);
    }

    #[test]
    fn available_models_static_always_returns_fallback() {
        let fetched = vec!["claude-opus-4.6".to_string(), "gpt-5.3-codex".to_string()];
        let provider = make_test_provider(fetched);
        let static_models = provider.available_models();
        let expected: Vec<&str> = FALLBACK_MODELS.to_vec();
        assert_eq!(static_models, expected);
    }

    #[test]
    fn set_model_accepts_any_model_id() {
        let provider = make_test_provider(Vec::new());
        assert!(provider.set_model("claude-opus-4.6").is_ok());
        assert_eq!(provider.model(), "claude-opus-4.6");

        assert!(provider.set_model("some-new-model-2026").is_ok());
        assert_eq!(provider.model(), "some-new-model-2026");
    }

    #[test]
    fn set_model_rejects_empty() {
        let provider = make_test_provider(Vec::new());
        assert!(provider.set_model("").is_err());
        assert!(provider.set_model("   ").is_err());
    }

    #[test]
    fn context_window_handles_dot_and_dash_names() {
        assert_eq!(copilot_context_window("claude-opus-4.6"), 200_000);
        assert_eq!(copilot_context_window("claude-opus-4-6"), 200_000);
        assert_eq!(copilot_context_window("claude-opus-4.6-fast"), 200_000);
        assert_eq!(copilot_context_window("claude-sonnet-4.6"), 128_000);
        assert_eq!(copilot_context_window("claude-sonnet-4-6"), 128_000);
        assert_eq!(copilot_context_window("gpt-5.3-codex"), 128_000);
        assert_eq!(copilot_context_window("gemini-3-pro-preview"), 1_000_000);
        assert_eq!(copilot_context_window("gemini-2.5-pro"), 1_000_000);
        assert_eq!(copilot_context_window("unknown-model"), 128_000);
    }

    #[test]
    fn has_credentials_returns_bool() {
        let _ = CopilotApiProvider::has_credentials();
    }

    #[test]
    fn fork_preserves_fetched_models() {
        let fetched = vec!["model-a".to_string(), "model-b".to_string()];
        let provider = make_test_provider(fetched.clone());
        let forked = provider.fork();
        assert_eq!(forked.available_models_display(), fetched);
    }

    fn make_msg(role: Role, blocks: Vec<ContentBlock>) -> ChatMessage {
        ChatMessage {
            role,
            content: blocks,
            timestamp: None,
        }
    }

    #[test]
    fn build_messages_pairs_tool_use_with_tool_result() {
        let messages = vec![
            make_msg(
                Role::User,
                vec![ContentBlock::Text {
                    text: "hello".into(),
                    cache_control: None,
                }],
            ),
            make_msg(
                Role::Assistant,
                vec![
                    ContentBlock::Text {
                        text: "let me check".into(),
                        cache_control: None,
                    },
                    ContentBlock::ToolUse {
                        id: "call_1".into(),
                        name: "bash".into(),
                        input: serde_json::json!({"command": "echo hi"}),
                    },
                ],
            ),
            make_msg(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "call_1".into(),
                    content: "hi\n".into(),
                    is_error: None,
                }],
            ),
        ];

        let built = CopilotApiProvider::build_messages("system prompt", &messages);

        assert_eq!(built.len(), 4);
        assert_eq!(built[0]["role"], "system");
        assert_eq!(built[1]["role"], "user");
        assert_eq!(built[1]["content"], "hello");
        assert_eq!(built[2]["role"], "assistant");
        assert!(built[2]["tool_calls"].is_array());
        assert_eq!(built[2]["tool_calls"][0]["id"], "call_1");
        assert_eq!(built[3]["role"], "tool");
        assert_eq!(built[3]["tool_call_id"], "call_1");
        assert_eq!(built[3]["content"], "hi\n");
    }

    #[test]
    fn build_messages_injects_missing_tool_output() {
        let messages = vec![
            make_msg(
                Role::User,
                vec![ContentBlock::Text {
                    text: "go".into(),
                    cache_control: None,
                }],
            ),
            make_msg(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "call_orphan".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"command": "crash"}),
                }],
            ),
        ];

        let built = CopilotApiProvider::build_messages("", &messages);

        assert_eq!(built.len(), 3);
        assert_eq!(built[1]["role"], "assistant");
        assert_eq!(built[2]["role"], "tool");
        assert_eq!(built[2]["tool_call_id"], "call_orphan");
        assert!(built[2]["content"].as_str().unwrap().contains("missing"));
    }

    #[test]
    fn build_messages_handles_batch_multiple_tool_calls() {
        let messages = vec![
            make_msg(
                Role::User,
                vec![ContentBlock::Text {
                    text: "do things".into(),
                    cache_control: None,
                }],
            ),
            make_msg(
                Role::Assistant,
                vec![
                    ContentBlock::ToolUse {
                        id: "call_a".into(),
                        name: "bash".into(),
                        input: serde_json::json!({"command": "a"}),
                    },
                    ContentBlock::ToolUse {
                        id: "call_b".into(),
                        name: "bash".into(),
                        input: serde_json::json!({"command": "b"}),
                    },
                    ContentBlock::ToolUse {
                        id: "call_c".into(),
                        name: "bash".into(),
                        input: serde_json::json!({"command": "c"}),
                    },
                ],
            ),
            make_msg(
                Role::User,
                vec![
                    ContentBlock::ToolResult {
                        tool_use_id: "call_a".into(),
                        content: "result_a".into(),
                        is_error: None,
                    },
                    ContentBlock::ToolResult {
                        tool_use_id: "call_b".into(),
                        content: "result_b".into(),
                        is_error: None,
                    },
                    ContentBlock::ToolResult {
                        tool_use_id: "call_c".into(),
                        content: "result_c".into(),
                        is_error: None,
                    },
                ],
            ),
        ];

        let built = CopilotApiProvider::build_messages("", &messages);

        assert_eq!(built[0]["role"], "user");
        assert_eq!(built[1]["role"], "assistant");
        let tc = built[1]["tool_calls"].as_array().unwrap();
        assert_eq!(tc.len(), 3);

        assert_eq!(built[2]["role"], "tool");
        assert_eq!(built[2]["tool_call_id"], "call_a");
        assert_eq!(built[2]["content"], "result_a");
        assert_eq!(built[3]["role"], "tool");
        assert_eq!(built[3]["tool_call_id"], "call_b");
        assert_eq!(built[3]["content"], "result_b");
        assert_eq!(built[4]["role"], "tool");
        assert_eq!(built[4]["tool_call_id"], "call_c");
        assert_eq!(built[4]["content"], "result_c");
    }

    #[test]
    fn build_messages_skips_empty_user_text() {
        let messages = vec![
            make_msg(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "call_1".into(),
                    name: "read".into(),
                    input: serde_json::json!({"file": "x"}),
                }],
            ),
            make_msg(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "call_1".into(),
                    content: "file content".into(),
                    is_error: None,
                }],
            ),
        ];

        let built = CopilotApiProvider::build_messages("", &messages);

        assert_eq!(built.len(), 2);
        assert_eq!(built[0]["role"], "assistant");
        assert_eq!(built[1]["role"], "tool");
        assert_eq!(built[1]["content"], "file content");
    }

    #[test]
    fn is_user_initiated_empty_messages() {
        let messages: Vec<ChatMessage> = vec![];
        assert!(CopilotApiProvider::is_user_initiated(&messages));
    }

    #[test]
    fn is_user_initiated_user_text_message() {
        let messages = vec![make_msg(
            Role::User,
            vec![ContentBlock::Text {
                text: "Hello".into(),
                cache_control: None,
            }],
        )];
        assert!(CopilotApiProvider::is_user_initiated(&messages));
    }

    #[test]
    fn is_user_initiated_tool_result_is_agent() {
        let messages = vec![
            make_msg(
                Role::User,
                vec![ContentBlock::Text {
                    text: "Hello".into(),
                    cache_control: None,
                }],
            ),
            make_msg(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "call_1".into(),
                    name: "file_read".into(),
                    input: json!({}),
                }],
            ),
            make_msg(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "call_1".into(),
                    content: "file content".into(),
                    is_error: None,
                }],
            ),
        ];
        assert!(!CopilotApiProvider::is_user_initiated(&messages));
    }

    #[test]
    fn is_user_initiated_assistant_last_is_user_initiated() {
        let messages = vec![
            make_msg(
                Role::User,
                vec![ContentBlock::Text {
                    text: "Hello".into(),
                    cache_control: None,
                }],
            ),
            make_msg(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: "Hi there".into(),
                    cache_control: None,
                }],
            ),
        ];
        assert!(CopilotApiProvider::is_user_initiated(&messages));
    }
}
