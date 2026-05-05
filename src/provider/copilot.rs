use super::{EventStream, Provider};
use crate::auth::copilot as copilot_auth;
use crate::message::{
    ContentBlock, Message as ChatMessage, Role, StreamEvent, TOOL_OUTPUT_MISSING_TEXT,
    ToolDefinition,
};
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

const COPILOT_API_VERSION: &str = "2025-04-01";

const DEFAULT_MODEL: &str = "claude-sonnet-4-6";

const FALLBACK_MODELS: &[&str] = &[
    "claude-sonnet-4.6",
    "claude-sonnet-4.5",
    "claude-haiku-4.5",
    "claude-opus-4.6",
    "claude-opus-4.6-fast",
    "claude-opus-4.5",
    "claude-sonnet-4",
    "gemini-3-pro-preview",
    "gpt-5.4",
    "gpt-5.4-pro",
    "gpt-5.3-codex",
    "gpt-5.2-codex",
    "gpt-5.2",
    "gpt-5.1-codex-max",
    "gpt-5.1-codex",
    "gpt-5.1",
    "gpt-5.1-codex-mini",
    "gpt-5-mini",
    "gpt-4.1",
];

pub(crate) fn is_known_display_model(model: &str) -> bool {
    FALLBACK_MODELS.contains(&model)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CatalogSource {
    None,
    Cached,
    Live,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedCatalog {
    models: Vec<String>,
    fetched_at_rfc3339: String,
}

/// Copilot API provider - uses GitHub Copilot's OpenAI-compatible API.
/// Authenticates via GitHub OAuth token, exchanges for Copilot bearer token,
/// and sends requests to api.githubcopilot.com.
/// Premium request conservation mode.
/// 0 = normal (every user message is premium)
/// 1 = one premium per session (first user message only, rest are agent)
/// 2 = zero premium (all requests sent as agent)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PremiumMode {
    Normal = 0,
    OnePerSession = 1,
    Zero = 2,
}

pub struct CopilotApiProvider {
    client: reqwest::Client,
    model: Arc<RwLock<String>>,
    github_token: String,
    bearer_token: Arc<tokio::sync::RwLock<Option<copilot_auth::CopilotApiToken>>>,
    fetched_models: Arc<RwLock<Vec<String>>>,
    catalog_source: Arc<RwLock<CatalogSource>>,
    session_id: String,
    machine_id: String,
    init_ready: Arc<tokio::sync::Notify>,
    init_done: Arc<std::sync::atomic::AtomicBool>,
    premium_mode: Arc<std::sync::atomic::AtomicU8>,
    user_turn_count: Arc<std::sync::atomic::AtomicU64>,
    created_at: std::time::Instant,
}

impl CopilotApiProvider {
    fn persisted_catalog_path() -> Result<std::path::PathBuf> {
        Ok(crate::storage::app_config_dir()?.join("copilot_models_cache.json"))
    }

    fn load_persisted_catalog() -> Option<PersistedCatalog> {
        let path = Self::persisted_catalog_path().ok()?;
        crate::storage::read_json(&path)
            .ok()
            .filter(|catalog: &PersistedCatalog| !catalog.models.is_empty())
    }

    fn persist_catalog(models: &[String]) {
        if models.is_empty() {
            return;
        }
        let Ok(path) = Self::persisted_catalog_path() else {
            return;
        };
        let payload = PersistedCatalog {
            models: models.to_vec(),
            fetched_at_rfc3339: Utc::now().to_rfc3339(),
        };
        if let Err(error) = crate::storage::write_json(&path, &payload) {
            crate::logging::warn(&format!(
                "Failed to persist Copilot model catalog {}: {}",
                path.display(),
                error
            ));
        }
    }

    fn seed_cached_catalog(&self) {
        if let Some(catalog) = Self::load_persisted_catalog() {
            if let Ok(mut models) = self.fetched_models.try_write() {
                *models = catalog.models;
            }
            if let Ok(mut source) = self.catalog_source.try_write() {
                *source = CatalogSource::Cached;
            }
        }
    }

    pub(crate) fn model_catalog_detail(&self) -> String {
        match self
            .catalog_source
            .try_read()
            .map(|g| *g)
            .unwrap_or(CatalogSource::None)
        {
            CatalogSource::Live => String::new(),
            CatalogSource::Cached => "cached live catalog".to_string(),
            CatalogSource::None => "catalog still loading".to_string(),
        }
    }

    pub fn new() -> Result<Self> {
        let github_token = copilot_auth::load_github_token()?;
        let model =
            std::env::var("JCODE_COPILOT_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());

        let provider = Self {
            client: crate::provider::shared_http_client(),
            model: Arc::new(RwLock::new(model)),
            github_token,
            bearer_token: Arc::new(tokio::sync::RwLock::new(None)),
            fetched_models: Arc::new(RwLock::new(Vec::new())),
            catalog_source: Arc::new(RwLock::new(CatalogSource::None)),
            session_id: Uuid::new_v4().to_string(),
            machine_id: Self::get_or_create_machine_id(),
            init_ready: Arc::new(tokio::sync::Notify::new()),
            init_done: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            premium_mode: Arc::new(std::sync::atomic::AtomicU8::new(Self::env_premium_mode())),
            user_turn_count: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            created_at: std::time::Instant::now(),
        };
        provider.seed_cached_catalog();
        Ok(provider)
    }

    pub fn has_credentials() -> bool {
        copilot_auth::has_copilot_credentials()
    }

    fn env_premium_mode() -> u8 {
        match std::env::var("JCODE_COPILOT_PREMIUM").ok().as_deref() {
            Some("0") => PremiumMode::Zero as u8,
            Some("1") => PremiumMode::OnePerSession as u8,
            _ => PremiumMode::Normal as u8,
        }
    }

    pub fn new_with_token(github_token: String) -> Self {
        let model =
            std::env::var("JCODE_COPILOT_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());

        let provider = Self {
            client: crate::provider::shared_http_client(),
            model: Arc::new(RwLock::new(model)),
            github_token,
            bearer_token: Arc::new(tokio::sync::RwLock::new(None)),
            fetched_models: Arc::new(RwLock::new(Vec::new())),
            catalog_source: Arc::new(RwLock::new(CatalogSource::None)),
            session_id: Uuid::new_v4().to_string(),
            machine_id: Self::get_or_create_machine_id(),
            init_ready: Arc::new(tokio::sync::Notify::new()),
            init_done: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            premium_mode: Arc::new(std::sync::atomic::AtomicU8::new(Self::env_premium_mode())),
            user_turn_count: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            created_at: std::time::Instant::now(),
        };
        provider.seed_cached_catalog();
        provider
    }

    fn startup_prefetch_grace_ms() -> u64 {
        std::env::var("JCODE_COPILOT_PREFETCH_STARTUP_GRACE_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(2000)
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

    fn is_user_initiated_raw(messages: &[ChatMessage]) -> bool {
        for msg in messages.iter().rev() {
            if msg.role != Role::User {
                return true;
            }
            let has_tool_result = msg
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolResult { .. }));
            if has_tool_result {
                return false;
            }
            let is_text_only = msg
                .content
                .iter()
                .all(|block| matches!(block, ContentBlock::Text { .. }));
            if !is_text_only || msg.content.is_empty() {
                return true;
            }
            let is_system_reminder = msg.content.iter().any(|block| {
                if let ContentBlock::Text { text, .. } = block {
                    text.contains("<system-reminder>")
                } else {
                    false
                }
            });
            if is_system_reminder {
                continue;
            }
            return true;
        }
        true
    }

    fn is_user_initiated(&self, messages: &[ChatMessage]) -> bool {
        let raw = Self::is_user_initiated_raw(messages);
        if !raw {
            return false;
        }
        let mode = self.premium_mode.load(std::sync::atomic::Ordering::Relaxed);
        match mode {
            2 => false,
            1 => {
                let count = self
                    .user_turn_count
                    .load(std::sync::atomic::Ordering::Relaxed);
                count == 0
            }
            _ => true,
        }
    }

    pub fn set_premium_mode(&self, mode: PremiumMode) {
        self.premium_mode
            .store(mode as u8, std::sync::atomic::Ordering::Relaxed);
        if mode != PremiumMode::Normal {
            crate::logging::info(&format!("Copilot premium mode set to {:?}", mode));
        }
    }

    pub fn get_premium_mode(&self) -> PremiumMode {
        match self.premium_mode.load(std::sync::atomic::Ordering::Relaxed) {
            1 => PremiumMode::OnePerSession,
            2 => PremiumMode::Zero,
            _ => PremiumMode::Normal,
        }
    }

    /// Detect the user's Copilot tier and set the best default model.
    /// Call this after construction. Fetches a bearer token and queries /models.
    /// If JCODE_COPILOT_MODEL is set, this is a no-op (user override).
    pub async fn detect_tier_and_set_default(&self) {
        let detect_start = std::time::Instant::now();
        if std::env::var("JCODE_COPILOT_MODEL").is_ok() {
            crate::logging::info(
                "Copilot model overridden via JCODE_COPILOT_MODEL, skipping tier detection",
            );
            self.mark_init_done();
            return;
        }

        let bearer_start = std::time::Instant::now();
        let bearer = match self.get_bearer_token().await {
            Ok(t) => t,
            Err(e) => {
                crate::logging::info(&format!(
                    "Copilot tier detection: failed to get bearer token after {}ms: {}",
                    bearer_start.elapsed().as_millis(),
                    e
                ));
                self.mark_init_done();
                return;
            }
        };

        let fetch_start = std::time::Instant::now();
        match copilot_auth::fetch_available_models(&self.client, &bearer).await {
            Ok(models) => {
                let picker_models: Vec<String> = models
                    .iter()
                    .filter(|m| m.model_picker_enabled)
                    .map(|m| m.id.clone())
                    .collect();
                let all_ids: Vec<String> = models.iter().map(|m| m.id.clone()).collect();
                let default = copilot_auth::choose_default_model(&models);
                crate::logging::info(&format!(
                    "Copilot tier detection: bearer={}ms, fetch_models={}ms, total={}ms, {} total, {} picker-enabled, default -> {}. Picker: [{}]. All: [{}]",
                    bearer_start.elapsed().as_millis(),
                    fetch_start.elapsed().as_millis(),
                    detect_start.elapsed().as_millis(),
                    all_ids.len(),
                    picker_models.len(),
                    default,
                    picker_models.join(", "),
                    all_ids.join(", ")
                ));
                if let Ok(mut m) = self.model.try_write() {
                    *m = default;
                }
                let display_models = if picker_models.is_empty() {
                    all_ids
                } else {
                    picker_models
                };
                if let Ok(mut fm) = self.fetched_models.try_write() {
                    *fm = display_models;
                }
                if let Ok(mut source) = self.catalog_source.try_write() {
                    *source = CatalogSource::Live;
                }
                Self::persist_catalog(
                    &self
                        .fetched_models
                        .try_read()
                        .map(|models| models.clone())
                        .unwrap_or_default(),
                );
            }
            Err(e) => {
                crate::logging::info(&format!(
                    "Copilot tier detection: bearer={}ms, fetch_models={}ms, total={}ms, failed to fetch models: {}",
                    bearer_start.elapsed().as_millis(),
                    fetch_start.elapsed().as_millis(),
                    detect_start.elapsed().as_millis(),
                    e
                ));
            }
        }
        self.mark_init_done();
    }

    fn mark_init_done(&self) {
        self.init_done
            .store(true, std::sync::atomic::Ordering::Release);
        self.init_ready.notify_waiters();
        crate::bus::Bus::global().publish_models_updated();
    }

    pub(crate) fn complete_init_without_tier_detection(&self) {
        self.mark_init_done();
    }

    async fn wait_for_init(&self) {
        if self.init_done.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        let notified = self.init_ready.notified();
        if self.init_done.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        notified.await;
    }

    /// Get a valid Copilot bearer token, refreshing if expired
    async fn get_bearer_token(&self) -> Result<String> {
        {
            let guard = self.bearer_token.read().await;
            if let Some(ref token) = *guard
                && !token.is_expired()
            {
                return Ok(token.token.clone());
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
                                        "tool_call_id": crate::message::sanitize_tool_id(tool_use_id),
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
                                let args = if input.is_object() {
                                    input.to_string()
                                } else {
                                    "{}".to_string()
                                };
                                tool_calls.push(json!({
                                    "id": crate::message::sanitize_tool_id(id),
                                    "type": "function",
                                    "function": {
                                        "name": name,
                                        "arguments": args,
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
                                "tool_call_id": crate::message::sanitize_tool_id(&tool_call_id),
                                "content": output,
                            }));
                        }

                        for missing_id in missing_tool_outputs {
                            result.push(json!({
                                "role": "tool",
                                "tool_call_id": crate::message::sanitize_tool_id(&missing_id),
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
                        // Prompt-visible. Approximate token cost for this field:
                        // t.description_token_estimate().
                        "description": t.description,
                        "parameters": t.input_schema,
                    }
                })
            })
            .collect()
    }

    /// Send a streaming request to Copilot API with retry logic
    async fn stream_request(
        &self,
        messages: Vec<Value>,
        tools: Vec<Value>,
        is_user_initiated: bool,
        tx: mpsc::Sender<Result<StreamEvent>>,
    ) {
        use crate::message::ConnectionPhase;

        self.wait_for_init().await;
        let model = self
            .model
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let max_tokens: u32 = 32_768;
        let initiator = if is_user_initiated { "user" } else { "agent" };

        const MAX_RETRIES: u32 = 3;
        const RETRY_BASE_DELAY_MS: u64 = 1000;
        let mut last_error: Option<anyhow::Error> = None;
        let mut attempted_auth_refresh = false;

        for attempt in 0..MAX_RETRIES {
            if attempt > 0 {
                let delay = RETRY_BASE_DELAY_MS * (1 << (attempt - 1));
                crate::logging::info(&format!(
                    "Retrying Copilot API request (attempt {}/{}) after {}ms",
                    attempt + 1,
                    MAX_RETRIES,
                    delay
                ));
                let _ = tx
                    .send(Ok(StreamEvent::ConnectionPhase {
                        phase: ConnectionPhase::Retrying {
                            attempt: attempt + 1,
                            max: MAX_RETRIES,
                        },
                    }))
                    .await;
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            }

            crate::logging::info(&format!(
                "Copilot request: X-Initiator={} model={}",
                initiator, model
            ));

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
                    let error_str = e.to_string().to_lowercase();
                    if is_retryable_error(&error_str) && attempt + 1 < MAX_RETRIES {
                        crate::logging::info(&format!(
                            "Transient Copilot error, will retry: {}",
                            e
                        ));
                        last_error = Some(anyhow::anyhow!("Copilot API request failed: {}", e));
                        continue;
                    }
                    let _ = tx
                        .send(Err(anyhow::anyhow!("Copilot API request failed: {}", e)))
                        .await;
                    return;
                }
            };

            let status = resp.status();

            // On auth error, invalidate token and retry once
            if Self::is_auth_error(status) && !attempted_auth_refresh {
                attempted_auth_refresh = true;
                *self.bearer_token.write().await = None;
                crate::logging::info("Copilot bearer token expired, refreshing...");
                last_error = Some(anyhow::anyhow!("Copilot auth error (HTTP {})", status));
                continue;
            }

            if !status.is_success() {
                let body_text = crate::util::http_error_body(resp, "HTTP error").await;
                let error_str =
                    format!("Copilot API error (HTTP {}): {}", status, body_text).to_lowercase();
                if is_retryable_error(&error_str) && attempt + 1 < MAX_RETRIES {
                    crate::logging::info(&format!("Retryable Copilot HTTP error: {}", error_str));
                    last_error = Some(anyhow::anyhow!(
                        "Copilot API error (HTTP {}): {}",
                        status,
                        body_text
                    ));
                    continue;
                }
                let _ = tx
                    .send(Err(anyhow::anyhow!(
                        "Copilot API error (HTTP {}): {}",
                        status,
                        body_text
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

            // Process SSE stream - returns Err on timeout/stream errors
            match self.process_sse_stream(resp, tx.clone()).await {
                Ok(()) => return,
                Err(e) => {
                    let error_str = e.to_string().to_lowercase();
                    if is_retryable_error(&error_str) && attempt + 1 < MAX_RETRIES {
                        crate::logging::info(&format!(
                            "Copilot stream failed (attempt {}/{}), will retry: {}",
                            attempt + 1,
                            MAX_RETRIES,
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

        // All retries exhausted
        if let Some(e) = last_error {
            let _ = tx
                .send(Err(anyhow::anyhow!(
                    "Copilot: failed after {} retries: {}",
                    MAX_RETRIES,
                    e
                )))
                .await;
        }
    }

    async fn process_sse_stream(
        &self,
        resp: reqwest::Response,
        tx: mpsc::Sender<Result<StreamEvent>>,
    ) -> Result<()> {
        use futures::StreamExt;

        const SSE_CHUNK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

        let mut stream = resp.bytes_stream();
        let mut buffer = String::new();
        let mut current_tool_id = String::new();
        let mut current_tool_name = String::new();
        let mut current_tool_args = String::new();
        let mut input_tokens: u64 = 0;
        let mut output_tokens: u64 = 0;
        let mut saw_any_data = false;

        loop {
            let chunk = match tokio::time::timeout(SSE_CHUNK_TIMEOUT, stream.next()).await {
                Ok(Some(Ok(c))) => c,
                Ok(Some(Err(e))) => {
                    anyhow::bail!("Stream error: {}", e);
                }
                Ok(None) => break, // stream ended normally
                Err(_) => {
                    crate::logging::warn(&format!(
                        "Copilot SSE stream timed out (no data for {}s, saw_data={})",
                        SSE_CHUNK_TIMEOUT.as_secs(),
                        saw_any_data
                    ));
                    anyhow::bail!(
                        "Stream read timeout: no data received for {} seconds",
                        SSE_CHUNK_TIMEOUT.as_secs()
                    );
                }
            };
            saw_any_data = true;

            buffer.push_str(&String::from_utf8_lossy(&chunk));

            // Prevent unbounded buffer growth from malformed SSE streams
            const MAX_SSE_BUFFER_SIZE: usize = 10 * 1024 * 1024; // 10 MB
            if buffer.len() > MAX_SSE_BUFFER_SIZE {
                if let Some(last_boundary) = buffer.rfind("\n\n") {
                    buffer = buffer[last_boundary + 2..].to_string();
                } else {
                    anyhow::bail!("SSE buffer exceeded {} bytes without complete event boundary", MAX_SSE_BUFFER_SIZE);
                }
            }

            // Process complete SSE lines
            while let Some(line_end) = buffer.find('\n') {
                let line = buffer[..line_end].trim_end_matches('\r').to_string();
                buffer = buffer[line_end + 1..].to_string();

                if line.is_empty() || line.starts_with(':') {
                    continue;
                }

                if let Some(data) = crate::util::sse_data_line(&line) {
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
                        crate::copilot_usage::record_request(input_tokens, output_tokens, true);
                        let _ = tx
                            .send(Ok(StreamEvent::MessageEnd { stop_reason: None }))
                            .await;
                        return Ok(());
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
                            if let Some(content) = delta.get("content").and_then(|c| c.as_str())
                                && !content.is_empty()
                            {
                                let _ = tx
                                    .send(Ok(StreamEvent::TextDelta(content.to_string())))
                                    .await;
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
        Ok(())
    }
}

fn is_retryable_error(error_str: &str) -> bool {
    crate::provider::is_transient_transport_error(error_str)
        || error_str.contains("500 internal server error")
        || error_str.contains("502 bad gateway")
        || error_str.contains("503 service unavailable")
        || error_str.contains("504 gateway timeout")
        || error_str.contains("overloaded")
        || error_str.contains("429 too many requests")
        || error_str.contains("rate limit")
        || error_str.contains("rate_limit")
        || error_str.contains("stream error")
        || error_str.contains("stream read timeout")
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
        self.wait_for_init().await;

        self.get_bearer_token().await.map_err(|e| {
            crate::logging::warn(&format!(
                "Copilot bearer token acquisition failed (will trigger fallback): {}",
                e
            ));
            e
        })?;

        let is_user_initiated = self.is_user_initiated(messages);
        if is_user_initiated {
            self.user_turn_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        let built_messages = Self::build_messages(system, messages);
        let built_tools = Self::build_tools(tools);

        let (tx, rx) = mpsc::channel::<Result<StreamEvent>>(100);

        let provider = CopilotApiProvider {
            client: self.client.clone(),
            model: self.model.clone(),
            github_token: self.github_token.clone(),
            bearer_token: self.bearer_token.clone(),
            fetched_models: self.fetched_models.clone(),
            catalog_source: self.catalog_source.clone(),
            session_id: self.session_id.clone(),
            machine_id: self.machine_id.clone(),
            init_ready: self.init_ready.clone(),
            init_done: self.init_done.clone(),
            premium_mode: self.premium_mode.clone(),
            user_turn_count: self.user_turn_count.clone(),
            created_at: self.created_at,
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
        if trimmed.contains("[1m]") {
            anyhow::bail!(
                "1M context window models are not supported via Copilot. Use the Anthropic API directly."
            );
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
        if let Ok(models) = self.fetched_models.read()
            && !models.is_empty()
        {
            return models.clone();
        }
        FALLBACK_MODELS
            .iter()
            .map(|model| model.to_string())
            .collect()
    }

    fn available_models_for_switching(&self) -> Vec<String> {
        self.available_models_display()
    }

    async fn prefetch_models(&self) -> Result<()> {
        let grace_ms = Self::startup_prefetch_grace_ms();
        if self.created_at.elapsed().as_millis() < u128::from(grace_ms) {
            crate::logging::info(&format!(
                "Skipping Copilot model prefetch during startup grace window ({}ms)",
                grace_ms
            ));
            return Ok(());
        }
        self.detect_tier_and_set_default().await;
        Ok(())
    }

    fn supports_compaction(&self) -> bool {
        true
    }

    fn set_premium_mode(&self, mode: PremiumMode) {
        CopilotApiProvider::set_premium_mode(self, mode);
    }

    fn premium_mode(&self) -> PremiumMode {
        CopilotApiProvider::get_premium_mode(self)
    }

    fn context_window(&self) -> usize {
        crate::provider::context_limit_for_model_with_provider(&self.model(), Some(self.name()))
            .unwrap_or(128_000)
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(CopilotApiProvider {
            client: self.client.clone(),
            model: Arc::new(RwLock::new(self.model())),
            github_token: self.github_token.clone(),
            bearer_token: self.bearer_token.clone(),
            fetched_models: self.fetched_models.clone(),
            catalog_source: self.catalog_source.clone(),
            session_id: self.session_id.clone(),
            machine_id: self.machine_id.clone(),
            init_ready: self.init_ready.clone(),
            init_done: self.init_done.clone(),
            // premium_mode is shared since it's a global setting
            premium_mode: self.premium_mode.clone(),
            // Fresh turn count for the fork - each fork is an independent session
            user_turn_count: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            created_at: self.created_at,
        })
    }
}

#[cfg(test)]
#[path = "copilot_tests.rs"]
mod tests;
